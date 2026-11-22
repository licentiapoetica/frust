//! Burn after read (`items.burn_after_read`): only the first visitor gets to see the item.
//!
//! - A burn upload is at least unlisted, so no listing, feed or random pick shows it: it's only
//!   reached through its link (the item page, or the file's direct URL).
//! - The first visit that isn't the uploader or staff claims it: the item page (also its ajax
//!   version) or the file itself under `/b/`. Bots and link previews (Discord, Telegram, ...),
//!   prefetches and HEAD requests never claim, they get a 404 like everyone after.
//! - Claiming stores the sha256 of the visitor's [`COOKIE`] in `burn_claim`. Only that visitor
//!   gets the files, and only until they have them: every byte sent to them is counted
//!   ([`track`]), and once each file of the item went out completely the item is purged at once
//!   (a video's range requests are pieces of one viewing, so they add up instead of each one
//!   burning it). A file being sent keeps streaming after that (the open file outlives its
//!   deletion), and the browser keeps it in its own cache to loop or replay it.
//! - Without anything going out for [`IDLE_SECS`] (tab closed, a YouTube embed, never pressed
//!   play) `expires_at` is reached and the expiry purger (media::delete) deletes it.
//! - Thumbnails and cover art never claim; only who may see the item gets them.
//! - The uploader, admins and moderators see the item without claiming it.
//!
//! The file names of burn items live in memory (like private_items), reloaded every 30 s; the
//! database decides who claimed first.

use crate::db::RowExt;
use crate::http::{CookieExpiry, Req, cookie_options, raw};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;

/// The visitor's claim token (48 hex characters).
pub const COOKIE: &str = "f0ck_burn";

#[derive(Clone, Debug, Default)]
struct Entry {
    /// Uploader, lowercase.
    owner: String,
    /// sha256 of the claimant's token.
    claim: Option<String>,
}

#[derive(Default)]
struct Index {
    items: HashMap<i64, Entry>,
    /// File name (item and album entries) -> item.
    files: HashMap<String, i64>,
    /// Album entry thumbnail name (its file name without extension) -> item.
    thumbs: HashMap<String, i64>,
}

static INDEX: Lazy<RwLock<Index>> = Lazy::new(|| RwLock::new(Index::default()));

/// An opened item goes after this many seconds without a request (or bytes going out) for it.
pub const IDLE_SECS: i64 = 60;
/// `expires_at` is moved on at most this often while the first viewer is busy with the item.
const BUMP_EVERY: i64 = 10;
/// Lifetime of the claim cookie (the item is gone long before).
const COOKIE_SECS: i64 = 86400;

/// What the first viewer has received of an opened item.
#[derive(Default)]
struct Delivery {
    /// File name -> (size, byte ranges sent, merged).
    files: HashMap<String, (u64, Vec<(u64, u64)>)>,
    /// Last time `expires_at` was moved on.
    bumped: i64,
    done: bool,
}

static DELIVERY: Lazy<Mutex<HashMap<i64, Delivery>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn stem(f: &str) -> &str {
    crate::media::mime::file_stem(f)
}

fn load(rows: &[crate::db::Row], files: &[crate::db::Row], ix: &mut Index) {
    for r in rows {
        let Some(id) = r.i64("id") else { continue };
        ix.items.insert(id, Entry { owner: r.string("username").to_lowercase(), claim: r.str("burn_claim").map(str::to_string) });
        let dest = r.string("dest");
        if !dest.is_empty() {
            ix.files.insert(dest, id);
        }
    }
    for f in files {
        let (Some(id), dest) = (f.i64("item_id"), f.string("dest")) else { continue };
        if !dest.is_empty() {
            ix.thumbs.insert(stem(&dest).to_string(), id);
            ix.files.insert(dest, id);
        }
    }
}

async fn query(st: &AppState, only: Option<i64>) -> anyhow::Result<(Vec<crate::db::Row>, Vec<crate::db::Row>)> {
    let mut q = sql!("SELECT id, dest, username, burn_claim FROM items WHERE burn_after_read AND is_purged = false AND is_deleted = false");
    let mut f = sql!("SELECT a.item_id, a.dest FROM album_items a JOIN items i ON i.id = a.item_id WHERE i.burn_after_read AND i.is_purged = false AND i.is_deleted = false");
    if let Some(id) = only {
        q.push(" AND id = ").bind(id as i32);
        f.push(" AND i.id = ").bind(id as i32);
    }
    Ok((q.fetch_all(&st.db).await?, f.fetch_all(&st.db).await?))
}

/// Load every burn item (boot, then every 30 s).
pub async fn init(st: &AppState) {
    match query(st, None).await {
        Ok((rows, files)) => {
            let mut ix = Index::default();
            load(&rows, &files, &mut ix);
            *INDEX.write() = ix;
        }
        Err(e) => tracing::error!("[BURN] Failed to load burn-after-read items: {e}"),
    }
}

/// (Re)load one item, e.g. right after its upload. True when it's a burn item.
pub async fn reload_item(st: &AppState, id: i64) -> bool {
    match query(st, Some(id)).await {
        Ok((rows, files)) => {
            forget(id);
            load(&rows, &files, &mut INDEX.write());
            !rows.is_empty()
        }
        Err(e) => {
            tracing::error!("[BURN] Failed to load item #{id}: {e}");
            false
        }
    }
}

/// Make sure burn item `id` is in the index (it may have been uploaded a moment ago). False when it
/// can't be loaded.
pub async fn reload_if_missing(st: &AppState, id: i64) -> bool {
    INDEX.read().items.contains_key(&id) || reload_item(st, id).await
}

/// Drop a purged item.
pub fn forget(id: i64) {
    DELIVERY.lock().remove(&id);
    let mut ix = INDEX.write();
    ix.items.remove(&id);
    ix.files.retain(|_, v| *v != id);
    ix.thumbs.retain(|_, v| *v != id);
}

/// The burn item behind a media path: `/b/<file>`, `/t/<id>.webp`, `/t/<id>_blur.webp`,
/// `/t/<album entry>.webp`, `/ca/<id>.webp`.
pub fn item_for_path(path: &str) -> Option<i64> {
    let ix = INDEX.read();
    if ix.items.is_empty() {
        return None;
    }
    let clean = |s: &str| crate::util::decode_lossy(s).split(['?', '#']).next().unwrap_or("").to_string();
    if let Some(rest) = path.strip_prefix("/b/") {
        return ix.files.get(&clean(rest)).copied();
    }
    let (rest, thumb) = match path.strip_prefix("/t/") {
        Some(r) => (r, true),
        None => (path.strip_prefix("/ca/")?, false),
    };
    let name = clean(rest);
    let base = stem(&name);
    let base = base.strip_suffix("_blur").unwrap_or(base);
    match base.parse::<i64>() {
        Ok(id) if ix.items.contains_key(&id) => Some(id),
        _ if thumb => ix.thumbs.get(base).copied(),
        _ => None,
    }
}

/// Is `user` the uploader of burn item `id`?
pub fn is_owner(id: i64, user: &str) -> bool {
    !user.is_empty() && INDEX.read().items.get(&id).is_some_and(|e| e.owner == user.to_lowercase())
}

/// May this request claim an unclaimed item? A real visit: GET, no prefetch, no bot.
fn may_claim_request(req: &Req) -> bool {
    req.method == Method::GET && !req.is_prefetch() && !crate::util::is_bot_ua(req.header("user-agent").unwrap_or(""))
}

fn token(req: &Req) -> Option<String> {
    req.cookie(COOKIE).filter(|t| t.len() == 48 && t.bytes().all(|b| b.is_ascii_hexdigit())).map(str::to_string)
}

fn no_store(req: &Req) {
    req.set_header("cache-control", "private, no-cache, no-store, must-revalidate");
}

fn set_claim(id: i64, claim: Option<String>) {
    if let Some(e) = INDEX.write().items.get_mut(&id) {
        e.claim = claim;
    }
}

/// May this request see burn item `id`? `privileged`: the uploader or staff (never claims).
/// `claim`: this request may claim an unclaimed item (the page or `/b/`, not a thumbnail).
/// `known_burn`: the caller knows it's a burn item, so a missing index entry means "no".
/// Not a burn item: true.
pub async fn may_view(st: &AppState, req: &Req, id: i64, privileged: bool, claim: bool, known_burn: bool) -> bool {
    let entry = INDEX.read().items.get(&id).cloned();
    let Some(entry) = entry else { return !known_burn };
    no_store(req);
    if privileged {
        return true;
    }
    let mine = token(req);
    let mine_hash = mine.as_deref().map(crate::util::sha256);
    if entry.claim.is_some() && entry.claim == mine_hash {
        bump(st, id);
        return true;
    }
    if !claim || !may_claim_request(req) {
        return false;
    }
    // Claim it (or find out that someone else just did)
    let tok = mine.unwrap_or_else(|| crate::util::random_hex(24));
    let hash = crate::util::sha256(&tok);
    let cfg = st.cfg();
    let now = crate::util::now_secs();
    let until = now + IDLE_SECS;
    let won = sql!(
        "UPDATE items SET burn_claim = ", hash.clone(), ", expires_at = LEAST(COALESCE(expires_at, ", until, "), ", until,
        ") WHERE id = ", id as i32, " AND burn_after_read AND burn_claim IS NULL AND is_purged = false RETURNING id"
    )
    .fetch_optional(&st.db)
    .await;
    match won {
        Ok(Some(_)) => {
            set_claim(id, Some(hash));
            DELIVERY.lock().insert(id, Delivery { bumped: now, ..Default::default() });
            crate::media::delete::expiry_changed();
            req.set_cookie(format!("{COOKIE}={tok}; {}", cookie_options(&cfg, CookieExpiry::MaxAge(COOKIE_SECS), true)));
            tracing::info!("[BURN] Item #{id} opened by its first viewer");
            true
        }
        Ok(None) => {
            let db = sql!("SELECT burn_claim FROM items WHERE id = ", id as i32, " LIMIT 1").fetch_optional(&st.db).await.ok().flatten();
            let claim = db.and_then(|r| r.str("burn_claim").map(str::to_string));
            let ok = claim.as_deref() == Some(hash.as_str());
            set_claim(id, claim);
            ok
        }
        Err(e) => {
            tracing::error!("[BURN] Claiming item #{id} failed: {e}");
            false
        }
    }
}

/// The first viewer is still busy with item `id`: move `expires_at` on to [`IDLE_SECS`] from now
/// (at most every [`BUMP_EVERY`] seconds).
fn bump(st: &AppState, id: i64) {
    let now = crate::util::now_secs();
    {
        let mut d = DELIVERY.lock();
        let e = d.entry(id).or_default();
        if e.done || now - e.bumped < BUMP_EVERY {
            return;
        }
        e.bumped = now;
    }
    let st = st.clone();
    tokio::spawn(async move {
        let until = now + IDLE_SECS;
        let r = sql!("UPDATE items SET expires_at = ", until, " WHERE id = ", id as i32,
            " AND burn_claim IS NOT NULL AND is_purged = false AND expires_at < ", until, " AND expires_at >= ", now).execute(&st.db).await;
        if let Err(e) = r {
            tracing::warn!("[BURN] Keeping item #{id} alive failed: {e}");
        }
    });
}

/// Add `[from, to)` to the merged ranges.
fn add_range(ranges: &mut Vec<(u64, u64)>, from: u64, to: u64) {
    if to <= from {
        return;
    }
    ranges.push((from, to));
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for &(a, b) in ranges.iter() {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    *ranges = merged;
}

fn complete(size: u64, ranges: &[(u64, u64)]) -> bool {
    size == 0 || ranges.first().is_some_and(|&(a, b)| a == 0 && b >= size)
}

/// The item's own files (item and album entries; YouTube embeds have none).
fn item_files(id: i64) -> Vec<String> {
    INDEX.read().files.iter().filter(|(f, i)| **i == id && !f.starts_with("yt:")).map(|(f, _)| f.clone()).collect()
}

/// Bytes `[from, to)` of `file` (`size` bytes) went out to the first viewer of item `id`. True
/// when every file of the item is now complete (the caller purges it).
fn record(id: i64, file: &str, size: u64, from: u64, to: u64) -> bool {
    let files = item_files(id);
    let mut d = DELIVERY.lock();
    let e = d.entry(id).or_default();
    if e.done {
        return false;
    }
    let f = e.files.entry(file.to_string()).or_insert_with(|| (size, Vec::new()));
    add_range(&mut f.1, from, to);
    let all = !files.is_empty() && files.iter().all(|n| e.files.get(n).is_some_and(|(s, r)| complete(*s, r)));
    if all {
        e.done = true;
    }
    all
}

/// Every file is out: delete the item now.
fn burn_now(st: &AppState, id: i64) {
    let st = st.clone();
    tokio::spawn(async move {
        tracing::info!("[BURN] Item #{id} was read by its first viewer, burning it");
        let now = crate::util::now_secs();
        if let Err(e) = sql!("UPDATE items SET expires_at = ", now, " WHERE id = ", id as i32).execute(&st.db).await {
            return tracing::error!("[BURN] Burning item #{id} failed: {e}");
        }
        crate::media::delete::purge_expired_uploads(&st).await;
    });
}

/// Start offset and total size of a file response (200: the whole file, 206: its Content-Range).
fn span(resp: &Response) -> Option<(u64, u64)> {
    let h = resp.headers();
    match resp.status().as_u16() {
        200 => h.get(header::CONTENT_LENGTH)?.to_str().ok()?.parse().ok().map(|n| (0, n)),
        206 => {
            // bytes <start>-<end>/<size>
            let v = h.get(header::CONTENT_RANGE)?.to_str().ok()?.strip_prefix("bytes ")?;
            let (range, size) = v.split_once('/')?;
            Some((range.split_once('-')?.0.trim().parse().ok()?, size.trim().parse().ok()?))
        }
        _ => None,
    }
}

/// A file response to the first viewer (app.rs, `OutExtras::burn`): count what actually goes out;
/// when the item's files are all complete, burn it.
pub fn track(st: &AppState, id: i64, file: String, resp: Response) -> Response {
    use futures::StreamExt;
    let Some((start, size)) = span(&resp) else { return resp };
    let (parts, body) = resp.into_parts();
    let st = st.clone();
    let mut pos = start;
    let stream = http_body_util::BodyExt::into_data_stream(body).map(move |chunk| {
        if let Ok(b) = &chunk {
            let to = pos + b.len() as u64;
            if record(id, &file, size, pos, to) {
                burn_now(&st, id);
            } else {
                bump(&st, id);
            }
            pos = to;
        }
        chunk
    });
    Response::from_parts(parts, axum::body::Body::from_stream(stream))
}

/// May this session load the item's file at all (ratings, NSFP, `protect_files`; the same rules
/// as the media URLs in app.rs)? A visitor who'd be refused the file mustn't burn the item.
pub async fn file_allowed(st: &AppState, session: Option<&crate::session::Session>, dest: &str) -> bool {
    use crate::media_gate::{Decision, Viewer};
    let viewer = match session {
        None => Viewer::Guest,
        Some(s) if s.user.is_empty() => Viewer::Guest,
        Some(s) if s.is_anonymous_user() => {
            if crate::settings::enable_anonymous_access(&st.cfg()) { Viewer::Anon } else { Viewer::Guest }
        }
        Some(_) => Viewer::Member,
    };
    if viewer == Viewer::Guest && st.settings.protect_files() {
        return false;
    }
    !matches!(crate::media_gate::check(st, &format!("/b/{dest}"), || async move { viewer }).await, Decision::Deny { .. })
}

/// The answer for everyone who may not see a burn item's file.
pub fn gone() -> Response {
    let body = "<html>\r\n<head><title>404 Not Found</title></head>\r\n<body>\r\n<center><h1>404 Not Found</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n";
    let mut r = raw(404, body);
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-store, must-revalidate"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_find_their_item() {
        {
            let mut ix = INDEX.write();
            ix.items.insert(77, Entry { owner: "al".into(), claim: None });
            ix.files.insert("abc.mp4".into(), 77);
            ix.files.insert("def.png".into(), 77);
            ix.thumbs.insert("def".into(), 77);
        }
        assert_eq!(item_for_path("/b/abc.mp4"), Some(77));
        assert_eq!(item_for_path("/b/abc.mp4?x=1"), Some(77));
        assert_eq!(item_for_path("/b/def.png"), Some(77));
        assert_eq!(item_for_path("/t/77.webp"), Some(77));
        assert_eq!(item_for_path("/t/77_blur.webp"), Some(77));
        assert_eq!(item_for_path("/t/def.webp"), Some(77));
        assert_eq!(item_for_path("/ca/77.webp"), Some(77));
        assert_eq!(item_for_path("/b/other.mp4"), None);
        assert_eq!(item_for_path("/t/78.webp"), None);
        assert!(is_owner(77, "AL"));
        assert!(!is_owner(77, "bob"));
        forget(77);
        assert_eq!(item_for_path("/b/abc.mp4"), None);
    }

    #[test]
    fn ranges_add_up() {
        let mut r = Vec::new();
        add_range(&mut r, 100, 200);
        add_range(&mut r, 0, 50);
        assert!(!complete(200, &r));
        add_range(&mut r, 40, 120);
        assert_eq!(r, vec![(0, 200)]);
        assert!(complete(200, &r));
        add_range(&mut r, 300, 300);
        assert_eq!(r, vec![(0, 200)]);
        assert!(complete(0, &[]));
    }

    #[test]
    fn every_file_must_be_complete() {
        {
            let mut ix = INDEX.write();
            ix.items.insert(91, Entry { owner: "al".into(), claim: Some("x".into()) });
            ix.files.insert("one.png".into(), 91);
            ix.files.insert("two.png".into(), 91);
            ix.files.insert("yt:abc".into(), 91);
        }
        assert!(!record(91, "one.png", 10, 0, 10));
        assert!(!record(91, "two.png", 20, 10, 20));
        assert!(record(91, "two.png", 20, 0, 10), "both files out: burn");
        assert!(!record(91, "two.png", 20, 0, 20), "only once");
        forget(91);
    }
}
