//! Direct media URLs of rated items: guests (and anonymous users) get the same 502 as private
//! items for files whose rating they may not see (nsfl always, nsfw without `websrv.public_nsfw`,
//! untagged without `websrv.public_untagged`; anon users: `anonymous_permissions.allowed_modes`).
//!
//! Gated: `/b/<dest>` (items and album entries), `/t/<id>.webp`, `/t/<album entry>.webp`,
//! `/ca/<id>.webp`. `/t/<id>_blur.webp` stays public. Ratings are looked up per file and cached
//! for a few seconds; a failed lookup counts as nsfl (fail closed).

use crate::config::Config;
use crate::db::RowExt;
use crate::sql;
use crate::state::{AppState, TtlCache};
use once_cell::sync::Lazy;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub sfw: bool,
    pub nsfw: bool,
    pub nsfl: bool,
}

impl Flags {
    fn untagged(self) -> bool {
        !self.sfw && !self.nsfw && !self.nsfl
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Viewer {
    /// No (valid) session, or banned.
    Guest,
    /// Anonymous-access user (passkey shadow / legacy `anon_` account).
    Anon,
    Member,
}

/// Which item (or album entry) a media path belongs to.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Dest(String),
    /// Thumbnail: item id and/or album entry base name (dest without extension).
    Thumb(Option<i64>, String),
    Item(i64),
}

static CACHE: Lazy<TtlCache<Flags>> = Lazy::new(|| TtlCache::new(Duration::from_secs(10), 20000));

fn target(path: &str) -> Option<Target> {
    let clean = |s: &str| crate::util::decode_lossy(s).split(['?', '#']).next().unwrap_or("").to_string();
    let stem = |f: &str| f.rsplit_once('.').map(|(b, _)| b.to_string()).unwrap_or_else(|| f.to_string());
    if let Some(rest) = path.strip_prefix("/b/") {
        let dest = clean(rest);
        return (!dest.is_empty() && !dest.contains('/')).then_some(Target::Dest(dest));
    }
    if let Some(rest) = path.strip_prefix("/t/") {
        let base = stem(&clean(rest));
        if base.is_empty() || base.contains('/') || base.ends_with("_blur") {
            return None;
        }
        return Some(Target::Thumb(base.parse().ok(), base));
    }
    let rest = path.strip_prefix("/ca/")?;
    stem(&clean(rest)).parse().ok().map(Target::Item)
}

/// Rating of the item behind `path`, `None` when the path is no item media.
async fn lookup(st: &AppState, path: &str) -> Option<Flags> {
    let t = target(path)?;
    let key = format!("{t:?}");
    if let Some(f) = CACHE.get(&key) {
        return Some(f);
    }
    let cfg = st.cfg();
    let mut q = sql!("WITH hit AS (");
    match &t {
        Target::Dest(d) => {
            q.append(&sql!(
                "SELECT id AS item_id, NULL::int AS album_item_id FROM items WHERE dest = ",
                d.clone(),
                " UNION ALL SELECT item_id, id FROM album_items WHERE dest = ",
                d.clone()
            ));
        }
        Target::Item(id) => {
            q.append(&sql!("SELECT id AS item_id, NULL::int AS album_item_id FROM items WHERE id = ", *id as i32));
        }
        Target::Thumb(id, base) => {
            if let Some(id) = id.and_then(|i| i32::try_from(i).ok()) {
                q.append(&sql!("SELECT id AS item_id, NULL::int AS album_item_id FROM items WHERE id = ", id, " UNION ALL "));
            }
            let like = format!("{}.%", crate::util::escape_like(base));
            q.append(&sql!("(SELECT item_id, id FROM album_items WHERE dest LIKE ", like, " LIMIT 1)"));
        }
    }
    // An album entry counts with its own rating tags and its album's.
    q.append(&sql!(
        "), rt AS (
            SELECT t.id, t.normalized FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id
              WHERE ta.item_id IN (SELECT item_id FROM hit)
            UNION ALL
            SELECT t.id, t.normalized FROM album_items_tags_assign ata JOIN tags t ON t.id = ata.tag_id
              WHERE ata.album_item_id IN (SELECT album_item_id FROM hit WHERE album_item_id IS NOT NULL)
        )
        SELECT EXISTS (SELECT 1 FROM hit) AS found,
            EXISTS (SELECT 1 FROM rt WHERE id = 1) AS sfw,
            EXISTS (SELECT 1 FROM rt WHERE id = 2) AS nsfw,
            EXISTS (SELECT 1 FROM rt WHERE id = ",
        cfg.nsfl_tag_id() as i32,
        " OR normalized = 'nsfl') AS nsfl"
    ));
    match q.fetch_optional(&st.db).await {
        Ok(Some(r)) if r.bool("found") => {
            let f = Flags { sfw: r.bool("sfw"), nsfw: r.bool("nsfw"), nsfl: r.bool("nsfl") };
            CACHE.set(&key, f);
            Some(f)
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("[MEDIA GATE] rating lookup for {path} failed: {e}");
            Some(Flags { nsfl: true, ..Flags::default() })
        }
    }
}

/// May `viewer` load media of an item rated `f`? (Same rules as the item listings.)
fn allowed(cfg: &Config, viewer: Viewer, f: Flags) -> bool {
    match viewer {
        Viewer::Member => true,
        Viewer::Guest => {
            !(f.nsfl || (f.nsfw && !cfg.truthy("websrv.public_nsfw")) || (f.untagged() && !cfg.truthy("websrv.public_untagged")))
        }
        Viewer::Anon => {
            let modes = crate::settings::anon_allowed_modes(cfg);
            let has = |m: &str| modes.iter().any(|x| x == m);
            !((f.nsfl && !has("nsfl")) || (f.nsfw && !has("nsfw")) || (f.untagged() && !has("untagged")) || (f.sfw && !has("sfw")))
        }
    }
}

pub enum Decision {
    /// No item media, or visible to everyone: serve as usual.
    Public,
    /// Visible to this viewer only: serve, but never from a shared cache.
    Restricted,
    Deny,
}

/// Gate `path` for the viewer `viewer()` resolves (only called for restricted files).
pub async fn check<F, Fut>(st: &AppState, path: &str, viewer: F) -> Decision
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Viewer>,
{
    let Some(f) = lookup(st, path).await else { return Decision::Public };
    let cfg = st.cfg();
    if allowed(&cfg, Viewer::Guest, f) && allowed(&cfg, Viewer::Anon, f) {
        return Decision::Public;
    }
    if allowed(&cfg, viewer().await, f) { Decision::Restricted } else { Decision::Deny }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets() {
        assert_eq!(target("/b/abc.mp4"), Some(Target::Dest("abc.mp4".into())));
        assert_eq!(target("/b/a%20b.png"), Some(Target::Dest("a b.png".into())));
        assert_eq!(target("/b/"), None);
        assert_eq!(target("/b/x/y.png"), None);
        assert_eq!(target("/t/123.webp"), Some(Target::Thumb(Some(123), "123".into())));
        assert_eq!(target("/t/abcdef.webp"), Some(Target::Thumb(None, "abcdef".into())));
        assert_eq!(target("/t/123_blur.webp"), None);
        assert_eq!(target("/ca/42.webp"), Some(Target::Item(42)));
        assert_eq!(target("/ca/nope.webp"), None);
        assert_eq!(target("/c/1.png"), None);
    }

    fn cfg(yaml: &str) -> Config {
        crate::media::test_cfg(yaml)
    }

    #[test]
    fn guest_rules() {
        let off = cfg("websrv: {public_nsfw: false, public_untagged: false}");
        let on = cfg("websrv: {public_nsfw: true, public_untagged: true}");
        let sfw = Flags { sfw: true, ..Flags::default() };
        let nsfw = Flags { nsfw: true, ..Flags::default() };
        let nsfl = Flags { nsfl: true, ..Flags::default() };
        let untagged = Flags::default();
        assert!(allowed(&off, Viewer::Guest, sfw));
        assert!(!allowed(&off, Viewer::Guest, nsfw));
        assert!(!allowed(&off, Viewer::Guest, nsfl));
        assert!(!allowed(&off, Viewer::Guest, untagged));
        assert!(allowed(&on, Viewer::Guest, nsfw));
        assert!(allowed(&on, Viewer::Guest, untagged));
        assert!(!allowed(&on, Viewer::Guest, nsfl));
        assert!(allowed(&off, Viewer::Member, nsfl));
    }
}
