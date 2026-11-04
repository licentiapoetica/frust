//! Item/feed core (port of src/inc/routeinc/f0cklib.mjs, everything before `getComments`).
//!
//! The public functions return `serde_json::Value`s with the same keys as the old JavaScript
//! objects, because templates and the frontend consume them unchanged.
//!
//! * [`get_item_page`] — `getItemPage`
//! * [`getf0cks`] — `getf0cks` (feed grids)
//! * [`getf0ck`] — `getf0ck` (single item page)
//! * [`get_random`], [`get_random_pool`] — `getRandom`, `getRandomPool`
//! * [`search::search`] — `src/inc/routeinc/search.mjs`

mod feed;
mod item;
mod random;
pub mod recommend;
pub mod search;

pub use feed::{get_item_page, getf0cks};
pub use item::getf0ck;
#[allow(unused_imports)] // get_random_pool: used by the API routes
pub use random::{get_random, get_random_pool};

use crate::config::Config;
use crate::db::{Row, RowExt, Sql};
use crate::session::Session;
use crate::settings::{anon_allowed_mimes, anon_allowed_modes, can_anon_do, is_anon_session, session_owner_name};
use crate::sql;
use crate::state::AppState;
use crate::util::{escape_like, get_mode, get_multi_rating_mode, slugify};
use serde_json::{Value, json};

/// The option object every feed function takes (`{ user, tag, hall, mime, ... }`).
/// Fields left at their default behave like `undefined` in the old code.
#[derive(Default, Clone)]
pub struct FeedOpts<'a> {
    pub user: Option<String>,
    pub tag: Option<String>,
    pub hall: Option<String>,
    pub mime: Option<String>,
    /// `page` (`+(page ?? 1)`)
    pub page: Option<i64>,
    pub mode: Option<i64>,
    pub ratings: Option<Vec<String>>,
    pub fav: bool,
    pub session: Option<&'a Session>,
    pub limit: Option<i64>,
    pub strict: bool,
    pub newer: Option<String>,
    pub exclude: Vec<i64>,
    pub user_id: Option<i64>,
    pub is_admin: Option<bool>,
    pub random: bool,
    pub user_hall: Option<String>,
    pub user_hall_owner: Option<String>,
    pub tagger: Option<String>,
    /// `ids` (array or comma list); `None` when not given.
    pub ids: Option<Vec<i64>>,
    /// `total` (explicit total, skips the count query)
    pub total: Option<i64>,
    /// getf0ck: item id or slug
    pub itemid: Option<String>,
    pub lang: Option<String>,
    pub subf0ck: Option<String>,
    pub bypass_filter: bool,
    /// getItemPage
    pub target_item_id: Option<String>,
    pub target_item_pinned: Option<bool>,
}

impl FeedOpts<'_> {
    fn user_id(&self) -> Option<i64> {
        self.user_id.filter(|&u| u != 0)
    }
}

/// `ids` → `cleanIds`: positive integers from an array or a comma separated string.
pub fn clean_ids(v: &Value) -> Option<Vec<i64>> {
    let parse = |s: &str| -> Option<i64> {
        let t = s.trim();
        if t.is_empty() {
            return Some(0); // Number('') === 0, filtered below
        }
        let f: f64 = t.parse().ok()?;
        (f.fract() == 0.0 && f.is_finite()).then_some(f as i64)
    };
    let list: Vec<i64> = match v {
        Value::Array(a) => a
            .iter()
            .filter_map(|x| match x {
                Value::Number(n) => n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64),
                Value::String(s) => parse(s),
                _ => None,
            })
            .collect(),
        Value::String(s) => s.split(',').filter_map(parse).collect(),
        _ => return None,
    };
    Some(list.into_iter().filter(|&n| n > 0).collect())
}

pub(crate) fn nsfl_id(cfg: &Config) -> i64 {
    cfg.nsfl_tag_id()
}

/// `getGlobalfilter(session)` — SQL condition over `tag_id` for the nsfp tags that apply.
pub(crate) fn global_filter(st: &AppState, cfg: &Config, session: Option<&Session>) -> Option<String> {
    let nsfp = st.settings.nsfp_ids();
    if nsfp.is_empty() {
        return None;
    }
    let nsfl = nsfl_id(cfg);
    let mut tags = nsfp;
    match session {
        Some(_) if is_anon_session(session) => {
            let allowed = anon_allowed_modes(cfg);
            if allowed.iter().any(|m| m == "nsfw") {
                tags.retain(|&id| id != 2);
            }
            if allowed.iter().any(|m| m == "nsfl") {
                tags.retain(|&id| id != nsfl);
            }
        }
        None => {
            if cfg.truthy("websrv.public_nsfw") {
                tags.retain(|&id| id != 2);
            }
        }
        Some(_) => tags.retain(|&id| id != 2 && id != nsfl),
    }
    if tags.is_empty() {
        return None;
    }
    Some(tags.iter().map(|n| format!("tag_id = {n}")).collect::<Vec<_>>().join(" or "))
}

/// `and not exists (... globalfilter)` for guests and anonymous sessions, empty otherwise.
pub(crate) fn global_filter_sql(st: &AppState, cfg: &Config, session: Option<&Session>) -> Sql {
    if !is_anon_session(session) {
        return Sql::empty();
    }
    match global_filter(st, cfg, session) {
        Some(gf) => {
            let mut q = Sql::new("and not exists (select 1 from tags_assign where item_id = items.id and (");
            q.push(&gf).push("))");
            q
        }
        None => Sql::empty(),
    }
}

pub(crate) fn exclude_sql(excluded: &[i64]) -> Sql {
    if excluded.is_empty() {
        return Sql::empty();
    }
    let ids: Vec<i32> = excluded.iter().map(|&n| n as i32).collect();
    sql!("and not exists (select 1 from tags_assign where item_id = items.id and tag_id = any(", ids, "::int[]))")
}

fn is_guest(session: Option<&Session>) -> bool {
    session.map(|s| s.user.is_empty()).unwrap_or(true)
}

const MODE_NAMES: [&str; 5] = ["sfw", "nsfw", "untagged", "all", "nsfl"];

/// The anonymous-user part of the ratings sanitising shared by computeBaseMode/getf0cks/getf0ck.
/// `check_filter`: getf0cks/computeBaseMode drop the ratings when the anon may not filter.
pub(crate) fn safe_ratings(cfg: &Config, ratings: Option<&Vec<String>>, session: Option<&Session>, check_filter: bool) -> Option<Vec<String>> {
    let ratings = ratings.filter(|r| !r.is_empty()).cloned();
    if is_guest(session) {
        return Some(vec!["sfw".into()]);
    }
    if !is_anon_session(session) {
        return ratings;
    }
    let allowed = anon_allowed_modes(cfg);
    if check_filter && !can_anon_do(cfg, "filter") {
        return None;
    }
    let mut safe = ratings?;
    safe.retain(|r| allowed.contains(r));
    if safe.is_empty() {
        return if allowed.len() == 1 && !allowed.iter().any(|m| m == "all") {
            Some(vec![allowed[0].clone()])
        } else if allowed.len() < 5 {
            Some(allowed.clone())
        } else {
            None
        };
    }
    Some(safe)
}

/// `computeBaseMode(mode, ratings, session)` — SQL condition for the rating mode. Built only from
/// integers and fixed strings (plus the anon owner name, restricted to `[a-z0-9_]`).
pub(crate) fn compute_base_mode(cfg: &Config, mode: Option<i64>, ratings: Option<&Vec<String>>, session: Option<&Session>) -> String {
    let mut eff_mode = mode.unwrap_or(0);
    let ratings_arr = ratings.filter(|r| !r.is_empty()).cloned();
    let mut safe = ratings_arr.clone();
    let nsfl = nsfl_id(cfg);

    if is_guest(session) {
        eff_mode = 0;
        safe = Some(vec!["sfw".into()]);
    } else if is_anon_session(session) {
        let allowed = anon_allowed_modes(cfg);
        let can_filter = can_anon_do(cfg, "filter");
        if !can_filter {
            safe = None;
            eff_mode = 0;
        } else if let Some(s) = safe.as_mut() {
            s.retain(|r| allowed.contains(r));
            if s.is_empty() {
                safe = if allowed.len() == 1 && !allowed.iter().any(|m| m == "all") {
                    Some(vec![allowed[0].clone()])
                } else if allowed.len() < 5 {
                    Some(allowed.clone())
                } else {
                    None
                };
            }
        }
        let current = usize::try_from(eff_mode).ok().and_then(|i| MODE_NAMES.get(i)).copied().unwrap_or("sfw");
        if eff_mode == 3 {
            if allowed.len() < 5 {
                safe = Some(allowed.clone());
            }
        } else if !allowed.iter().any(|m| m == current) {
            let fallback = allowed.first().map(String::as_str).unwrap_or("sfw");
            eff_mode = MODE_NAMES.iter().position(|m| *m == fallback).unwrap_or(0) as i64;
        }
    }

    let multi = |s: &Option<Vec<String>>| s.as_ref().and_then(|s| get_multi_rating_mode(cfg, s));
    let mut base = if eff_mode == 2 {
        match &safe {
            Some(s) if s.iter().any(|r| r == "untagged") && s.len() > 1 => multi(&safe).unwrap_or_else(|| get_mode(cfg, 2)),
            _ => get_mode(cfg, 2),
        }
    } else if eff_mode == 3 {
        match &safe {
            Some(s) if s.len() > 1 => multi(&safe).unwrap_or_else(|| get_mode(cfg, 3)),
            _ => get_mode(cfg, 3),
        }
    } else {
        multi(&safe).unwrap_or_else(|| get_mode(cfg, eff_mode))
    };

    let public_nsfw = cfg.truthy("websrv.public_nsfw");
    let public_untagged = cfg.truthy("websrv.public_untagged");
    if session.is_none() {
        let few = safe.as_ref().map(|s| s.len() <= 1).unwrap_or(true);
        if (eff_mode == 0 || eff_mode == 3 || mode.is_none()) && few {
            base = if public_nsfw {
                if public_untagged {
                    format!("(items.id in (select item_id from tags_assign where tag_id in (1, 2)) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))")
                } else {
                    "items.id in (select item_id from tags_assign where tag_id in (1, 2))".into()
                }
            } else if public_untagged {
                format!("(items.id in (select item_id from tags_assign where tag_id = 1) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))")
            } else {
                "items.id in (select item_id from tags_assign where tag_id = 1)".into()
            };
        } else if eff_mode == 2 {
            if !public_untagged {
                base = "items.id in (select item_id from tags_assign where tag_id = 1)".into();
            }
        } else if eff_mode == 1 {
            if !public_nsfw {
                base = "items.id in (select item_id from tags_assign where tag_id = 1)".into();
            }
        } else if eff_mode == 4 {
            base = "1 = 0".into();
        }
    } else if is_anon_session(session) {
        let allowed = anon_allowed_modes(cfg);
        let has = |m: &str| allowed.iter().any(|x| x == m);
        if !has("nsfl") {
            base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = {nsfl})");
        }
        if !has("nsfw") {
            base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = 2)");
        }
        if !has("untagged") {
            base = format!("({base}) and exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl}))");
        }
        if !has("sfw") {
            base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = 1)");
        }
        // Uploaders always see their own untagged items. The name goes into raw SQL, so only the
        // plain shadow-login charset is accepted.
        if let Some(owner) = session_owner_name(session) {
            if !has("untagged") && !owner.is_empty() && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                base = format!(
                    "(({base}) or items.id in (select own.id from items own where lower(own.username) = '{}' and not exists (select 1 from tags_assign ota where ota.item_id = own.id and ota.tag_id in (1, 2, {nsfl}))))",
                    owner.to_lowercase()
                );
            }
        }
    }
    base
}

/// `resolveNumericItemId(idOrSlug)`
pub async fn resolve_numeric_item_id(st: &AppState, id_or_slug: &str) -> Option<i64> {
    if id_or_slug.is_empty() {
        return None;
    }
    if id_or_slug.bytes().all(|b| b.is_ascii_digit()) {
        return id_or_slug.parse::<i64>().ok().filter(|&n| n != 0);
    }
    let id: Option<i32> = sql!("SELECT id FROM items WHERE slug = ", id_or_slug, " LIMIT 1").fetch_scalar(&st.db).await.ok().flatten();
    if let Some(id) = id.filter(|&i| i != 0) {
        return Some(id as i64);
    }
    let sub: Option<i32> = sql!("SELECT item_id FROM album_items WHERE slug = ", id_or_slug, " LIMIT 1").fetch_scalar(&st.db).await.ok().flatten();
    sub.filter(|&i| i != 0).map(i64::from)
}

/// All MIME types mapped to `swf` in the config.
fn flash_mimes(cfg: &Config) -> Vec<String> {
    cfg.mimes().into_iter().filter(|(_, ext)| ext.as_str() == Some("swf")).map(|(m, _)| m).collect()
}

/// `resolveMimeSQL(rawMime, session, itemAlias)` → (mimeParts, `and (...)` fragment)
pub(crate) fn resolve_mime_sql(cfg: &Config, raw_mime: Option<&str>, session: Option<&Session>, alias: &str) -> (Vec<String>, Sql) {
    const KNOWN: [&str; 5] = ["video", "audio", "image", "flash", "pdf"];
    let mut parts: Vec<String> = raw_mime.unwrap_or("").split(',').filter(|m| KNOWN.contains(m)).map(str::to_string).collect();
    if is_anon_session(session) {
        let allowed = anon_allowed_mimes(cfg);
        let can_filter = can_anon_do(cfg, "filter");
        if !can_filter || allowed.len() == 1 {
            parts = if allowed.len() < 5 { allowed.clone() } else { vec![] };
        } else if !parts.is_empty() {
            parts.retain(|m| allowed.contains(m));
            if parts.is_empty() {
                parts = if allowed.len() < 5 { allowed.clone() } else { vec![] };
            }
        } else if allowed.len() < 5 {
            parts = allowed.clone();
        }
    }
    if parts.is_empty() {
        return (parts, Sql::empty());
    }
    let col = if alias == "i" { "i.mime" } else { "items.mime" };
    let flash = flash_mimes(cfg);
    let mut q = Sql::new("and (");
    for (i, m) in parts.iter().enumerate() {
        if i > 0 {
            q.push(" or ");
        }
        match m.as_str() {
            "flash" if flash.is_empty() => {
                q.push("false");
            }
            "flash" => {
                for (j, fm) in flash.iter().enumerate() {
                    if j > 0 {
                        q.push(" or ");
                    }
                    q.push(col).push(" = ").bind(fm.as_str());
                }
            }
            "pdf" => {
                q.push(col).push(" = 'application/pdf'");
            }
            _ => {
                q.push(col).push(" ilike ").bind(format!("{m}/%"));
            }
        }
    }
    q.push(")");
    (parts, q)
}

// ── Count cache ─────────────────────────────────────────────────────────────
// COUNT(DISTINCT items.id) in getf0cks is expensive; cache it per filter combination for 90 s
// (`st.caches.feed_count`).

pub(crate) fn count_cache_key(f: &Filters, fav: bool, session: Option<&Session>) -> String {
    let mut excluded = f.excluded_tags.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    excluded.sort(); // JS default sort compares as strings
    json!([
        f.modequery,
        f.tag.clone().unwrap_or_default(),
        f.user.clone().unwrap_or_default(),
        f.hall.clone().unwrap_or_default(),
        f.mime.clone().unwrap_or_default(),
        if fav { 1 } else { 0 },
        if session.is_some() && !is_anon_session(session) { 1 } else { 0 },
        excluded.join(","),
        f.newer_than.map(|n| n.to_string()).unwrap_or_default(),
        f.user_hall_obj.as_ref().map(|u| u.string("id")).unwrap_or_default(),
        f.tagger.clone().unwrap_or_default(),
    ])
    .to_string()
}

/// Clear the feed count cache (`f0cklib.clearCountCache`).
pub fn clear_count_cache(st: &AppState) {
    st.caches.feed_count.clear();
}

/// `checkFavoritesAccess(rawUser, { session, user_id, is_admin })` → (is_private, is_allowed)
pub(crate) async fn check_favorites_access(st: &AppState, raw_user: &str, o: &FeedOpts<'_>) -> anyhow::Result<(bool, bool)> {
    if raw_user.is_empty() {
        return Ok((false, true));
    }
    let decoded = crate::http::decode_uri(raw_user);
    let target = sql!(
        r#"select u.id, u."user", u.admin, uo.favorites_private
    from "user" u
    left join user_options uo on uo.user_id = u.id
    where u."user" ilike "#,
        decoded.as_str(),
        " or u.login ilike ",
        decoded.as_str(),
        " limit 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(target) = target.filter(|t| t.bool("favorites_private")) else {
        return Ok((false, true));
    };

    let mut req_user_id = o.user_id;
    let mut req_is_admin = o.is_admin;
    if let Some(s) = o.session {
        if req_user_id.is_none() {
            req_user_id = Some(s.id);
        }
        if req_is_admin.is_none() {
            req_is_admin = Some(s.admin);
        }
    }
    let mut allowed = false;
    if let Some(uid) = req_user_id.filter(|&u| u != 0) {
        if Some(uid) == target.i64("id") || req_is_admin == Some(true) {
            allowed = true;
        } else if req_is_admin.is_none() {
            let admin: Option<bool> = sql!(r#"select admin from "user" where id = "#, uid, " limit 1").fetch_scalar(&st.db).await?;
            allowed = admin.unwrap_or(false);
        }
    }
    Ok((true, allowed))
}

/// Result of `buildFeedFilters`.
pub(crate) struct Filters {
    pub user: Option<String>,
    pub user_like: Option<String>,
    pub decoded_tag: String,
    pub is_title_search: bool,
    pub title_query: Option<String>,
    pub tag: Option<String>,
    pub hall: Option<String>,
    pub hall_obj: Option<Value>,
    pub user_hall_slug: Option<String>,
    pub user_hall_owner: Option<String>,
    pub user_hall_obj: Option<Row>,
    pub mime: Option<String>,
    pub mime_sql: Sql,
    pub excluded_tags: Vec<i64>,
    pub newer_than: Option<i64>,

    pub strict_params: Vec<String>,
    pub is_strict: bool,
    /// Escaped for ILIKE.
    pub tagger: Option<String>,
    pub modequery: String,
    pub tag_filter: Sql,
    pub title_filter: Sql,
    pub hall_filter: Sql,
    pub user_hall_filter: Sql,
    pub visibility_filter: Sql,
}

/// `parseInt(x)` for strings: leading integer, `None` for NaN.
pub(crate) fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, digits) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let end = digits.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(digits.len());
    if end == 0 {
        return None;
    }
    let n: i64 = digits[..end].parse().ok()?;
    Some(if neg { -n } else { n })
}

/// `tag.split(',').map(trim).filter(Boolean)`
pub(crate) fn tag_terms(tag: &str) -> Vec<String> {
    tag.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect()
}

/// strict params: slugified tag list when strict (or a comma list) is requested.
pub(crate) fn strict_params(strict: bool, tag: Option<&str>) -> Vec<String> {
    match tag {
        Some(t) if !t.is_empty() && (strict || t.contains(',')) => t.split(',').map(slugify).filter(|t| !t.is_empty()).collect(),
        _ => vec![],
    }
}

/// The per-term tag condition (items + album sub-items), `with_albums` false for getf0ck/getRandom.
pub(crate) fn tag_filter_sql(tag: &str, is_strict: bool, tagger: Option<&str>, with_albums: bool) -> Sql {
    let terms = tag_terms(tag);
    let mut q = Sql::empty();
    if terms.is_empty() {
        return q;
    }
    if let Some(tagger) = tagger {
        for (i, term) in terms.iter().enumerate() {
            if i > 0 {
                q.push(" ");
            }
            q.append(&sql!(
                r#"and (items.id in (
          select ta.item_id from tags_assign ta
          join tags t on t.id = ta.tag_id
          join "user" u on u.id = ta.user_id
          where t.normalized like '%' || slugify("#,
                term.as_str(),
                r#") || '%'
          and u.user ilike "#,
                tagger,
                r#"
        ) or items.id in (
          select ai.item_id from album_items ai
          join album_items_tags_assign aita on aita.album_item_id = ai.id
          join tags t on t.id = aita.tag_id
          join "user" u on u.id = aita.user_id
          where t.normalized like '%' || slugify("#,
                term.as_str(),
                r#") || '%'
          and u.user ilike "#,
                tagger,
                "\n        ))"
            ));
        }
        return q;
    }
    let n = terms.len() as i64;
    if is_strict {
        let main = sql!(
            r#"items.id in (
          select ta.item_id
          from tags_assign ta
          join tags t on t.id = ta.tag_id
          where t.normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest("#,
            terms.clone(),
            r#"::text[]) AS x))
          group by ta.item_id
          having count(distinct t.normalized) = "#,
            n,
            "\n        )"
        );
        if with_albums {
            q.push("and (").append(&main).append(&sql!(
                r#" or items.id in (
          select ai.item_id
          from album_items ai
          join album_items_tags_assign aita on aita.album_item_id = ai.id
          join tags t on t.id = aita.tag_id
          where t.normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest("#,
                terms.clone(),
                r#"::text[]) AS x))
          group by ai.item_id
          having count(distinct t.normalized) = "#,
                n,
                "\n        ))"
            ));
        } else {
            q.push("and ").append(&main);
        }
        return q;
    }
    for (i, term) in terms.iter().enumerate() {
        if i > 0 {
            q.push(" ");
        }
        if with_albums {
            q.append(&sql!(
                "and (items.id in (\n            select ta.item_id from tags_assign ta join tags t on t.id = ta.tag_id where t.normalized like '%' || slugify(",
                term.as_str(),
                ") || '%'\n          ) or items.id in (\n            select ai.item_id from album_items ai join album_items_tags_assign aita on aita.album_item_id = ai.id join tags t on t.id = aita.tag_id where t.normalized like '%' || slugify(",
                term.as_str(),
                ") || '%'\n          ))"
            ));
        } else {
            q.append(&sql!(
                "and items.id in (select ta.item_id from tags_assign ta join tags t on t.id = ta.tag_id where t.normalized like '%' || slugify(",
                term.as_str(),
                ") || '%')"
            ));
        }
    }
    q
}

pub(crate) fn visibility_filter(session: Option<&Session>) -> Sql {
    if session.map(|s| s.admin).unwrap_or(false) {
        return Sql::empty();
    }
    match session_owner_name(session) {
        Some(owner) => sql!(
            "and (coalesce(items.visibility, 0) = 0 or (lower(items.username) = ",
            owner.to_lowercase(),
            " and items.visibility != 3))"
        ),
        None => Sql::new("and coalesce(items.visibility, 0) = 0"),
    }
}

/// `decodeURIComponent` that keeps the input when it is malformed (the old code threw a 500).
pub(crate) fn decode_component(s: &str) -> String {
    crate::util::decode_lossy(s)
}

/// `title:` prefix handling shared by the feed functions → (decoded tag, is title search, title query, tag)
pub(crate) fn split_title_tag(raw_tag: Option<&str>) -> (String, bool, Option<String>, Option<String>) {
    let decoded = raw_tag.filter(|t| !t.is_empty()).map(decode_component).unwrap_or_default();
    let is_title = decoded.starts_with("title:");
    let title_query = is_title.then(|| decoded[6..].trim().to_string());
    // lib.parseTag decodes (again) the raw value
    let tag = if is_title { None } else { raw_tag.filter(|t| !t.is_empty()).map(decode_component) };
    (decoded, is_title, title_query, tag)
}

pub(crate) async fn user_hall_row(st: &AppState, owner: &str, slug: &str) -> anyhow::Result<Option<Row>> {
    sql!(
        r#"SELECT uh.id, uh.name, uh.slug, uh.description, uh.is_private, u."user" as owner_name
      FROM user_halls uh
      JOIN "user" u ON u.id = uh.user_id
      WHERE u."user" ILIKE "#,
        owner,
        " AND uh.slug = ",
        slug,
        "\n      LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await
}

pub(crate) async fn hall_obj(st: &AppState, hall: &str) -> anyhow::Result<Option<Value>> {
    let row = sql!("SELECT name, slug, description FROM halls WHERE slug = ", hall, " LIMIT 1").fetch_optional(&st.db).await?;
    Ok(row.map(|r| json!({ "name": r.val("name"), "slug": r.val("slug"), "description": r.val("description") })))
}

/// `buildFeedFilters({...})`
pub(crate) async fn build_feed_filters(st: &AppState, cfg: &Config, o: &FeedOpts<'_>) -> anyhow::Result<Filters> {
    let user = o.user.as_deref().filter(|u| !u.is_empty()).map(crate::http::decode_uri);
    let user_like = user.as_deref().map(escape_like);
    let (decoded_tag, is_title_search, title_query, tag) = split_title_tag(o.tag.as_deref());

    let hall = o.hall.clone();
    let hall_obj = match hall.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => hall_obj(st, h).await?,
        None => None,
    };
    let user_hall_slug = o.user_hall.clone();
    let user_hall_owner = o.user_hall_owner.clone();
    let user_hall_obj = match (user_hall_slug.as_deref().filter(|s| !s.is_empty()), user_hall_owner.as_deref().filter(|s| !s.is_empty())) {
        (Some(slug), Some(owner)) => user_hall_row(st, owner, slug).await?,
        _ => None,
    };
    let mime = o.mime.clone();
    let (_, mime_sql) = resolve_mime_sql(cfg, mime.as_deref(), o.session, "items");

    let excluded_tags = if o.session.is_some() { o.exclude.clone() } else { vec![] };
    let newer_than = o.newer.as_deref().filter(|n| !n.is_empty()).and_then(parse_int).filter(|&n| n != 0);


    let strict_params = strict_params(o.strict, tag.as_deref());
    let is_strict = !strict_params.is_empty();
    let tagger = o.tagger.as_deref().filter(|t| !t.is_empty()).map(escape_like);
    let modequery = compute_base_mode(cfg, o.mode, o.ratings.as_ref(), o.session);

    let mut tag_filter = Sql::empty();
    let mut title_filter = Sql::empty();
    if is_title_search && title_query.as_deref().map(|q| !q.is_empty()).unwrap_or(false) {
        title_filter = sql!("and items.title ILIKE ", format!("%{}%", title_query.as_deref().unwrap_or("")), " and items.title IS NOT NULL");
    } else if let Some(t) = tag.as_deref().filter(|t| !t.is_empty()) {
        tag_filter = tag_filter_sql(t, is_strict, tagger.as_deref(), true);
    }

    let hall_filter = match hall.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => sql!(
            "and items.id in (select item_id from halls_assign join halls on halls.id = halls_assign.hall_id where halls.slug = ",
            h,
            ")"
        ),
        None => Sql::empty(),
    };
    let user_hall_filter = match &user_hall_obj {
        Some(u) => sql!(
            "and items.id in (select uha.item_id from user_halls_assign uha where uha.hall_id = ",
            u.i64("id").unwrap_or(0) as i32,
            ")"
        ),
        None => Sql::empty(),
    };

    Ok(Filters {
        user,
        user_like,
        decoded_tag,
        is_title_search,
        title_query,
        tag,
        hall,
        hall_obj,
        user_hall_slug,
        user_hall_owner,
        user_hall_obj,
        mime,
        mime_sql,
        excluded_tags,
        newer_than,

        strict_params,
        is_strict,
        tagger,
        modequery,
        tag_filter,
        title_filter,
        hall_filter,
        user_hall_filter,
        visibility_filter: visibility_filter(o.session),
    })
}

// ── Rating helpers for tag lists ────────────────────────────────────────────

/// Rating flags of a tag list (`t.id == 1`, `t.id == 2`, nsfl by id or name).
pub(crate) struct Rating {
    pub nsfl: bool,
    pub nsfw: bool,
    pub sfw: bool,
}

impl Rating {
    pub fn of(cfg: &Config, tags: &[Value]) -> Rating {
        let nsfl_id = cfg.get("nsfl_tag_id");
        let nsfl_id = if crate::config::truthy(nsfl_id) { crate::config::as_int(nsfl_id) } else { Some(3) };
        let id = |t: &Value| crate::config::as_int(&t["id"]);
        Rating {
            nsfl: cfg.enable_nsfl() && tags.iter().any(|t| id(t) == nsfl_id || t["normalized"].as_str() == Some("nsfl")),
            nsfw: tags.iter().any(|t| id(t) == Some(2)),
            sfw: tags.iter().any(|t| id(t) == Some(1)),
        }
    }
    pub fn untagged(&self) -> bool {
        !self.sfw && !self.nsfw && !self.nsfl
    }
    pub fn name(&self) -> &'static str {
        if self.nsfl {
            "nsfl"
        } else if self.nsfw {
            "nsfw"
        } else if self.sfw {
            "sfw"
        } else {
            "untagged"
        }
    }
}

/// `t.id == 1 || t.id == 2 || (enable_nsfl && (t.id == nsfl || t.normalized === 'nsfl'))`
pub(crate) fn is_rating_tag(cfg: &Config, t: &Value) -> bool {
    let id = crate::config::as_int(&t["id"]);
    let nsfl_id = cfg.get("nsfl_tag_id");
    let nsfl_id = if crate::config::truthy(nsfl_id) { crate::config::as_int(nsfl_id) } else { Some(3) };
    id == Some(1) || id == Some(2) || (cfg.enable_nsfl() && (id == nsfl_id || t["normalized"].as_str() == Some("nsfl")))
}

/// The album sub-item visibility rules shared by getf0cks and getf0ck (`isSubVisible`).
pub(crate) struct SubVisibility<'a> {
    pub cfg: &'a Config,
    pub session: Option<&'a Session>,
    pub eff_mode: i64,
    pub safe_ratings: Option<Vec<String>>,
    pub excluded_tags: &'a [i64],
}

impl SubVisibility<'_> {
    /// Rating allowed for this visitor at all (guest: `websrv.public_*`, anon: `allowed_modes`).
    /// Unlike the filter, `bypass` never overrides this.
    pub fn permitted(&self, tags: &[Value]) -> bool {
        let cfg = self.cfg;
        let r = Rating::of(cfg, tags);
        let untagged = r.untagged();
        if is_guest(self.session) {
            return !(r.nsfl || (r.nsfw && !cfg.truthy("websrv.public_nsfw")) || (untagged && !cfg.truthy("websrv.public_untagged")));
        }
        if is_anon_session(self.session) {
            let allowed = anon_allowed_modes(cfg);
            let has = |x: &str| allowed.iter().any(|a| a == x);
            return !((r.nsfl && !has("nsfl")) || (r.nsfw && !has("nsfw")) || (untagged && !has("untagged")) || (r.sfw && !has("sfw")));
        }
        true
    }

    pub fn visible(&self, tags: &[Value]) -> bool {
        let cfg = self.cfg;
        let r = Rating::of(cfg, tags);
        let untagged = r.untagged();
        // The old code compared the excluded tag *ids* against tag names, so this never matched;
        // compare ids as intended.
        if !self.excluded_tags.is_empty()
            && tags.iter().any(|t| crate::config::as_int(&t["id"]).map(|id| self.excluded_tags.contains(&id)).unwrap_or(false))
        {
            return false;
        }
        let m = self.eff_mode;
        if is_guest(self.session) {
            let pn = cfg.truthy("websrv.public_nsfw");
            let pu = cfg.truthy("websrv.public_untagged");
            if r.nsfl || (r.nsfw && !pn) || (untagged && !pu) {
                return false;
            }
            if m == 0 && (r.nsfw || r.nsfl || (untagged && !pu)) {
                return false;
            }
            if m == 1 && !r.nsfw {
                return false;
            }
            if m == 2 && (!untagged || !pu) {
                return false;
            }
            return true;
        }
        let safe = self.safe_ratings.as_ref().filter(|s| !s.is_empty());
        if is_anon_session(self.session) {
            let allowed = anon_allowed_modes(cfg);
            let has = |x: &str| allowed.iter().any(|a| a == x);
            if (r.nsfl && !has("nsfl")) || (r.nsfw && !has("nsfw")) || (untagged && !has("untagged")) || (r.sfw && !has("sfw")) {
                return false;
            }
            if let Some(s) = safe {
                if !s.iter().any(|x| x == r.name()) {
                    return false;
                }
            } else if m != 3 {
                if m == 0 && (r.nsfw || r.nsfl || (untagged && !has("untagged"))) {
                    return false;
                }
                if m == 1 && !r.nsfw {
                    return false;
                }
                if m == 2 && (!untagged || !has("untagged")) {
                    return false;
                }
                if m == 4 && !r.nsfl {
                    return false;
                }
            }
            return true;
        }
        if let Some(s) = safe {
            if !s.iter().any(|x| x == r.name()) {
                return false;
            }
        } else if m != 3 {
            if (m == 0 && (r.nsfw || r.nsfl)) || (m == 1 && !r.nsfw) || (m == 2 && !untagged) || (m == 4 && !r.nsfl) {
                return false;
            }
        }
        true
    }
}

/// Sub-item tag matching for tag feeds: every strict param, or any term as substring.
pub(crate) fn sub_matches_tag(tags: &[Value], terms: &[String], strict: &[String]) -> bool {
    let norm = |t: &Value| t["normalized"].as_str().map(str::to_string);
    if !strict.is_empty() {
        return strict.iter().all(|sp| tags.iter().any(|t| norm(t).as_deref() == Some(sp.as_str())));
    }
    terms.iter().any(|term| {
        let slug = slugify(term);
        tags.iter().any(|t| norm(t).map(|n| !n.is_empty() && n.contains(&slug)).unwrap_or(false))
    })
}
