//! External sources (`src/inc/routes/external.mjs`): the 4chan JSON/media proxy used by the
//! scroller, rehosting 4chan media onto the board and editing rehosted items.
//!
//! Differences to the old code:
//! * Responses are `application/json` (flummpress ignored the `headers` passed to `res.reply`, so
//!   4chan JSON — including user-written comments — went out as `text/html` on this origin).
//! * The media proxy decides the content type from the file's first bytes and only passes
//!   jpeg/png/gif/webp/webm/mp4; upstream failures answer 404/502 instead of an empty 200.
//! * Rehosting only ever downloads from the 4chan CDN; the old unanchored URL check let any URL
//!   containing `/api/v2/scroller/external/4chan/<b>/media/` through to curl.
//! * Users who are neither owner nor admin may only re-rate rehosted 4chan items (the old route
//!   let any 4chan-group user change the rating of every item on the site).

use super::chan::{self as chanr, Rehosts, js_str, num, truthy};
use crate::chan_http::{self, ChanUrl, ProxyStart};
use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, json_resp, with_header};
use crate::media::{self, TempPath, mime, queue};
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Map, Value, json};
use std::time::{Duration, Instant};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/api\/v2\/scroller\/external\/4chan\/(?<board>[a-z0-9]+)\/(?<tid>\d+)\/?$", Guard::ChanAuth, thread_json);
    r.post_g(r"^\/api\/v2\/scroller\/external\/rehost-meta\/?$", Guard::LoggedIn, rehost_meta);
    r.get_g(r"^\/api\/v2\/scroller\/external\/4chan\/(?<board>[a-z0-9]+)\/catalog\/?$", Guard::ChanAuth, catalog_json);
    r.get_g(r"^\/api\/v2\/scroller\/external\/4chan\/(?<board>[a-z0-9]+)\/find\/(?<postno>\d+)\/?$", Guard::ChanAuth, find_post);
    r.get_g(r"^\/api\/v2\/scroller\/external\/4chan\/(?<board>[a-z0-9]+)\/media\/(?<file>[^/]+)$", Guard::ChanAuth, media_proxy);
    r.post_g(r"^\/api\/v2\/scroller\/rehost\/?$", Guard::ChanAuth, rehost);
    r.get_g(r"^\/api\/v2\/scroller\/rehost\/details\/(?<id>\d+)\/?$", Guard::ChanAuth, rehost_details_get);
    r.post_g(r"^\/api\/v2\/scroller\/rehost\/details\/?$", Guard::ChanAuth, rehost_details_post);
}

fn json(v: Value, code: u16) -> Response {
    json_resp(&v, code)
}

fn fail(code: u16, msg: &str) -> Response {
    json(json!({ "success": false, "msg": msg }), code)
}

fn cache(r: Response, v: &'static str) -> Response {
    with_header(r, "cache-control", v)
}

// ── Per-user rate limit for the proxy routes (F-001) ────────────────────────

const PROXY_RATE_LIMIT: u32 = 5000;
const PROXY_RATE_WINDOW: Duration = Duration::from_secs(600);

static PROXY_RATE: once_cell::sync::Lazy<dashmap::DashMap<i64, (Instant, u32)>> = once_cell::sync::Lazy::new(dashmap::DashMap::new);

/// `None` when the request may proceed, else the 429 answer.
fn proxy_rate_limit(req: &Req) -> Option<Response> {
    let key = req.session()?.id;
    let now = Instant::now();
    // Periodic cleanup so the map can't grow without bound
    if PROXY_RATE.len() > 10_000 {
        PROXY_RATE.retain(|_, (start, _)| now.duration_since(*start) <= PROXY_RATE_WINDOW * 2);
    }
    let mut e = PROXY_RATE.entry(key).or_insert((now, 0));
    if now.duration_since(e.0) > PROXY_RATE_WINDOW {
        *e = (now, 0);
    }
    e.1 += 1;
    (e.1 > PROXY_RATE_LIMIT).then(|| fail(429, "Rate limit exceeded"))
}

fn board_param(req: &Req) -> String {
    req.param("board").unwrap_or("").to_string()
}

/// Insert `v` unless it is `undefined` in the old code (missing → key left out by JSON.stringify).
fn put(m: &mut Map<String, Value>, k: &str, v: &Value) {
    if !v.is_null() {
        m.insert(k.into(), v.clone());
    }
}

// ── 4chan JSON proxy ────────────────────────────────────────────────────────

async fn thread_json(st: AppState, req: Req) -> Res {
    if let Some(r) = proxy_rate_limit(&req) {
        return Ok(r);
    }
    let board = board_param(&req);
    let tid = req.param("tid").unwrap_or("").to_string();
    let result: anyhow::Result<Value> = async {
        let url = tid.parse::<u64>().ok().and_then(|t| ChanUrl::api_thread(&board, t)).ok_or_else(|| anyhow::anyhow!("invalid thread"))?;
        tracing::info!("[EXTERNAL] Fetching 4chan thread: {}", url.as_str());
        let data = chan_http::fetch_json(&st.cfg(), &url).await?;
        let posts = chanr::posts_of(&data);
        let rehosts = match chanr::lookup_rehosts(&st, &board, &posts).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[EXTERNAL] DB src check error: {e}");
                Rehosts::default()
            }
        };
        Ok(json!({ "success": true, "posts": posts, "board": board, "tid": tid, "rehosts": rehosts.ids_json(), "rehost_paths": rehosts.paths_json() }))
    }
    .await;
    match result {
        Ok(v) => Ok(cache(json(v, 200), "no-cache")),
        Err(e) => {
            tracing::error!("[EXTERNAL] 4chan fetch error: {e:#}");
            Ok(fail(500, "fetch_failed"))
        }
    }
}

async fn catalog_json(st: AppState, req: Req) -> Res {
    if let Some(r) = proxy_rate_limit(&req) {
        return Ok(r);
    }
    let board = board_param(&req);
    let result: anyhow::Result<Vec<Value>> = async {
        let url = ChanUrl::api_catalog(&board).ok_or_else(|| anyhow::anyhow!("invalid board"))?;
        let pages = chan_http::fetch_json(&st.cfg(), &url).await?;
        Ok(chanr::catalog_threads(&pages)?
            .iter()
            .map(|t| {
                let mut m = Map::new();
                put(&mut m, "no", &t["no"]);
                m.insert("sub".into(), if truthy(&t["sub"]) { t["sub"].clone() } else { json!("") });
                let com = t["com"].as_str().map(|c| chanr::slice_chars(&chanr::strip_tags(c), 120)).unwrap_or_default();
                m.insert("com".into(), json!(com));
                m.insert("replies".into(), if truthy(&t["replies"]) { t["replies"].clone() } else { json!(0) });
                m.insert("images".into(), if truthy(&t["images"]) { t["images"].clone() } else { json!(0) });
                put(&mut m, "tim", &t["tim"]);
                put(&mut m, "ext", &t["ext"]);
                m.insert("sticky".into(), if truthy(&t["sticky"]) { t["sticky"].clone() } else { json!(0) });
                Value::Object(m)
            })
            .collect())
    }
    .await;
    match result {
        Ok(threads) => Ok(cache(json(json!({ "success": true, "board": board, "threads": threads }), 200), "private, max-age=120")),
        Err(e) => {
            tracing::error!("[EXTERNAL] Catalog fetch error: {e:#}");
            Ok(fail(500, "catalog_fetch_failed"))
        }
    }
}

/// Thread id of the post `postno` in a catalog (OP or one of the last replies).
pub(crate) fn find_in_catalog(threads: &[Value], postno: i64) -> Option<Value> {
    for t in threads {
        if num(t, "no") == Some(postno) && t["no"].is_number() {
            return Some(t["no"].clone());
        }
        if let Some(replies) = t["last_replies"].as_array() {
            if replies.iter().any(|r| r["no"].is_number() && num(r, "no") == Some(postno)) {
                return Some(t["no"].clone());
            }
        }
    }
    None
}

async fn find_post(st: AppState, req: Req) -> Res {
    if let Some(r) = proxy_rate_limit(&req) {
        return Ok(r);
    }
    let board = board_param(&req);
    let postno_raw = req.param("postno").unwrap_or("").to_string();
    let cfg = st.cfg();
    let found = |tid: Value| cache(json(json!({ "success": true, "tid": tid, "board": board }), 200), "private, max-age=300");
    let Some(postno) = postno_raw.parse::<u64>().ok() else { return Ok(fail(500, "find_failed")) };

    // 1) The post may be the thread OP itself
    if let Some(url) = ChanUrl::api_thread(&board, postno) {
        if let Ok(thread) = chan_http::fetch_json(&cfg, &url).await {
            if truthy(&thread["posts"]) {
                return Ok(found(json!(postno)));
            }
        }
    }
    // 2) Search the catalog's last replies
    let result: anyhow::Result<Option<Value>> = async {
        let url = ChanUrl::api_catalog(&board).ok_or_else(|| anyhow::anyhow!("invalid board"))?;
        let pages = chan_http::fetch_json(&cfg, &url).await?;
        Ok(find_in_catalog(&chanr::catalog_threads(&pages)?, postno as i64))
    }
    .await;
    match result {
        Ok(Some(tid)) => Ok(found(tid)),
        Ok(None) => Ok(json(json!({ "success": false, "msg": "post_not_found" }), 200)),
        Err(e) => {
            tracing::error!("[EXTERNAL] Find post error: {e:#}");
            Ok(fail(500, "find_failed"))
        }
    }
}

// ── Media proxy ─────────────────────────────────────────────────────────────

/// Allowed file extensions (prevents use as a generic proxy).
const ALLOWED_MEDIA_EXTS: [&str; 7] = ["jpg", "jpeg", "png", "gif", "webp", "webm", "mp4"];

async fn media_proxy(st: AppState, req: Req) -> Res {
    if let Some(r) = proxy_rate_limit(&req) {
        return Ok(r);
    }
    let board = board_param(&req);
    let file = req.param("file").unwrap_or("").to_string();
    let ext = file.rsplit('.').next().unwrap_or("").to_lowercase();
    if !ALLOWED_MEDIA_EXTS.contains(&ext.as_str()) {
        return Ok(fail(400, "Disallowed file type"));
    }
    let Some(url) = ChanUrl::media(&board, &file) else { return Ok(fail(400, "Invalid filename")) };
    match chan_http::proxy_media(&st.cfg(), &url).await {
        Ok(ProxyStart::Ok { content_type, body }) => {
            let mut r = Response::new(body);
            let h = r.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
            h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
            h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
            Ok(r)
        }
        Ok(ProxyStart::Failed { http_error: true }) => Ok(fail(404, "Upstream media not found")),
        Ok(ProxyStart::Failed { http_error: false }) => Ok(fail(502, "Upstream fetch failed")),
        Ok(ProxyStart::BadType) => {
            tracing::warn!("[EXTERNAL] Media proxy: unexpected content from {}", url.as_str());
            Ok(fail(502, "Unsupported upstream content type"))
        }
        Err(e) => {
            tracing::error!("[EXTERNAL] Media proxy error: {e:#}");
            Ok(fail(502, "Upstream fetch failed"))
        }
    }
}

// ── rehost-meta ─────────────────────────────────────────────────────────────

/// `(req.post.ids || '').split(',').map(Number).filter(n => n > 0)` (integers only).
fn parse_ids(v: &Value) -> Vec<i32> {
    let parts: Vec<String> = match v {
        Value::String(s) => s.split(',').map(str::to_string).collect(),
        Value::Array(a) => a.iter().map(js_str).collect(),
        Value::Number(n) => vec![n.to_string()],
        _ => vec![],
    };
    parts.iter().filter_map(|p| p.trim().parse::<i32>().ok()).filter(|&n| n > 0).collect()
}

fn rating_of(cfg: &crate::config::Config, rating_tag_id: Option<i64>) -> (&'static str, &'static str) {
    match rating_tag_id {
        Some(1) => ("sfw", "SFW"),
        Some(2) => ("nsfw", "NSFW"),
        Some(n) if n == cfg.nsfl_tag_id() => ("nsfl", "NSFL"),
        _ => ("untagged", "?"),
    }
}

async fn rehost_meta(st: AppState, mut req: Req) -> Res {
    let ids = parse_ids(&req.post().await?.get("ids").cloned().unwrap_or(Value::Null));
    if ids.is_empty() {
        return Ok(json(json!({}), 200));
    }
    let cfg = st.cfg();
    let rating_ids: Vec<i32> = vec![1, 2, cfg.nsfl_tag_id() as i32];
    // Only items the caller may see: staff see everything; others only live public/unlisted
    // items and their own (the old code answered for any id, incl. pending and private ones).
    let visible = match req.session() {
        Some(s) if s.is_admin_or_mod() => crate::db::Sql::empty(),
        session => {
            let owner = crate::settings::session_owner_name(session).unwrap_or_default().to_lowercase();
            sql!(" AND ((i.active = true AND i.is_deleted = false AND COALESCE(i.visibility, 0) IN (0, 1)) OR lower(i.username) = ", owner, ")")
        }
    };
    let rows = sql!(
        r#"SELECT i.id, i.username, i.stamp,
          COALESCE(uo.display_name, i.username) as display_name,
          uo.avatar_file, uo.avatar,
          (SELECT ta.tag_id FROM tags_assign ta
           WHERE ta.item_id = i.id AND ta.tag_id = ANY("#,
        rating_ids,
        r#"::int[])
           ORDER BY ta.tag_id LIMIT 1) AS rating_tag_id,
          (SELECT COUNT(*) FROM comments WHERE comments.item_id = i.id AND comments.is_deleted = false) AS comment_count
        FROM items i
        LEFT JOIN "user" u ON u."user" = i.username
        LEFT JOIN user_options uo ON uo.user_id = u.id
        WHERE i.id = ANY("#,
        ids,
        "::int[])",
        @visible,
        " ORDER BY i.id"
    )
    .fetch_all(&st.db)
    .await;
    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[EXTERNAL] rehost-meta error: {e}");
            return Ok(json(json!({}), 500));
        }
    };
    let mut meta = Map::new();
    for r in rows {
        let (class, label) = rating_of(&cfg, r.i64("rating_tag_id"));
        let avatar = if r.bool("avatar_file") {
            format!("/a/{}", r.string("avatar_file"))
        } else if r.bool("avatar") {
            format!("/t/{}.webp", js_str(r.val("avatar")))
        } else {
            "/a/default.png".to_string()
        };
        meta.insert(
            r.i64("id").unwrap_or(0).to_string(),
            json!({
                "username": r.val("username"),
                "display_name": r.val("display_name"),
                "avatar": avatar,
                "stamp": r.val("stamp"),
                "rating_class": class,
                "rating_label": label,
                "comment_count": r.i64("comment_count").unwrap_or(0),
            }),
        );
    }
    Ok(json(Value::Object(meta), 200))
}

// ── Rehost ──────────────────────────────────────────────────────────────────

/// Link path for an item: its slug when slugs are enabled, else the id.
async fn item_path(st: &AppState, id: i64) -> Value {
    if !s::enable_item_slugs(&st.cfg()) {
        return json!(id);
    }
    let slug = sql!("SELECT slug FROM items WHERE id = ", id as i32, " LIMIT 1").fetch_optional(&st.db).await.ok().flatten().map(|r| r.string("slug"));
    match slug.filter(|s| !s.is_empty()) {
        Some(s) => json!(s),
        None => json!(id),
    }
}

/// `INSERT INTO comment_subscriptions ... DO UPDATE SET is_subscribed = true` (when comments are on).
async fn auto_subscribe(st: &AppState, user_id: i64, item_id: i64, what: &str) {
    if !st.cfg().enabled("enable_comments") {
        return;
    }
    let r = sql!(
        "INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", user_id as i32, ", ", item_id as i32,
        ") ON CONFLICT (user_id, item_id) DO UPDATE SET is_subscribed = true"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        tracing::error!("[REHOST] Auto-subscribe ({what}) error: {e}");
    }
}

fn rating_tag_id(cfg: &crate::config::Config, rating: &str) -> i64 {
    match rating {
        "sfw" => 1,
        "nsfw" => 2,
        _ => cfg.nsfl_tag_id(),
    }
}

/// Tag names from an array or a comma-separated string, without rating names.
fn tag_list(v: &Value) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::Array(a) => a.iter().map(js_str).collect(),
        Value::String(s) => s.split(',').map(str::to_string).collect(),
        _ => vec![],
    };
    raw.into_iter().map(|t| t.trim().to_string()).filter(|t| !t.is_empty() && !matches!(t.to_lowercase().as_str(), "sfw" | "nsfw" | "nsfl")).collect()
}

/// A positive dimension from the request (`Number(x) || null`), sanity-capped.
fn dim(v: &Value) -> Option<i32> {
    let f = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (f.is_finite() && f >= 1.0 && f <= 100_000.0).then_some(f as i32)
}

async fn rehost(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let url_raw = match &body["url"] {
        Value::String(s) if !s.is_empty() => s.clone(),
        Value::Null => String::new(),
        v if truthy(v) => js_str(v),
        _ => String::new(),
    };
    if url_raw.is_empty() {
        return Ok(fail(400, "URL is required"));
    }
    let Some(source) = chan_http::rehost_source(&url_raw) else {
        return Ok(fail(400, "Only 4chan media URLs are supported for rehosting"));
    };
    let Some(session) = req.session().cloned() else { return Ok(fail(401, "Unauthorized")) };
    match rehost_inner(&st, &body, &source, &session).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[REHOST] Error: {e:#}");
            Ok(fail(500, "Rehost failed"))
        }
    }
}

async fn rehost_inner(st: &AppState, body: &Value, source: &chan_http::RehostSource, session: &crate::session::Session) -> anyhow::Result<Response> {
    let cfg = st.cfg();
    let rating = body["rating"].as_str().map(str::to_lowercase).filter(|r| matches!(r.as_str(), "sfw" | "nsfw" | "nsfl"));
    let owner = s::session_owner_name(Some(session)).unwrap_or_default();
    let max_bytes = cfg.int("main.maxfilesize").filter(|&n| n > 0).map(|n| n as u64).unwrap_or(100 * 1024 * 1024);

    let uuid = queue::genuuid();
    tokio::fs::create_dir_all(&cfg.paths.tmp).await?;
    let tmp = TempPath::new(cfg.paths.tmp.join(format!("{uuid}.tmp")));
    chan_http::download(&cfg, &source.download, &tmp.path, max_bytes).await?;

    let mime_type = mime::detect(&tmp.path).await?;
    let Some(ext) = mime::ext_for(&cfg, &mime_type) else { anyhow::bail!("Unsupported file type: {mime_type}") };
    if media::sound_required(&cfg, &mime_type) && media::video_has_no_sound(&tmp.path).await {
        return Ok(fail(400, media::NO_SOUND_MSG));
    }
    let final_tmp = TempPath::new(cfg.paths.tmp.join(format!("{uuid}.{ext}")));
    tokio::fs::rename(&tmp.path, &final_tmp.path).await?;
    drop(tmp);

    let checksum = queue::sha256_file(&final_tmp.path).await?;
    let bypass = st.settings.bypass_duplicate_check();
    if !bypass {
        if let Some(repost) = queue::check_repost_sum(st, &checksum).await? {
            drop(final_tmp);
            auto_subscribe(st, session.id, repost, "repost").await;
            let path = item_path(st, repost).await;
            return Ok(json(json!({ "success": true, "repost": true, "item_id": repost, "item_path": path, "msg": "Already on site" }), 200));
        }
    }
    let phash = queue::generate_phash(&final_tmp.path).await;
    if !bypass && phash.is_some() {
        if let Some(m) = queue::check_repost_phash(st, phash.as_deref()).await? {
            drop(final_tmp);
            auto_subscribe(st, session.id, m, "phash repost").await;
            let path = item_path(st, m).await;
            return Ok(json(json!({ "success": true, "repost": true, "item_id": m, "item_path": path, "msg": "Already on site (visual match)" }), 200));
        }
    }

    let mut dims = dim(&body["width"]).zip(dim(&body["height"]));
    if dims.is_none() && (mime_type.starts_with("image/") || mime_type.starts_with("video/")) {
        dims = queue::dimensions(&final_tmp.path, &mime_type).await;
    }

    let filename = format!("{uuid}.{ext}");
    let manual = st.settings.manual_approval();
    let dest_dir = if manual { cfg.paths.pending.join("b") } else { cfg.paths.b.clone() };
    tokio::fs::create_dir_all(&dest_dir).await?;
    let dest_path = dest_dir.join(&filename);
    media::move_file(&final_tmp.path, &dest_path).await?;
    drop(final_tmp);
    media::make_public(&dest_path).await;
    let size = tokio::fs::metadata(&dest_path).await.map(|m| m.len() as i64).unwrap_or(0);
    let insert_checksum = if bypass { format!("{checksum}_bypass_{}", crate::util::now_ms()) } else { checksum };
    let original_filename = body["original_filename"]
        .as_str()
        .map(crate::media::multipart::sanitize_filename)
        .map(|f| f.chars().take(255).collect::<String>())
        .filter(|f| !f.is_empty());

    let inserted = sql!(
        "insert into items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, is_oc, original_filename, width, height, slug) values (",
        source.src.clone(), ", ", filename.clone(), ", ", mime_type.clone(), ", ", size.min(i32::MAX as i64) as i32, ", ", insert_checksum, ", ", phash, ", ",
        owner.clone(), ", 'web', 'web', ", crate::util::now_secs() as i32, ", ", !manual, ", ", truthy(&body["is_oc"]), ", ", original_filename, ", ",
        dims.map(|d| d.0), ", ", dims.map(|d| d.1), ", ", crate::util::generate_slug(11),
        ") RETURNING id"
    )
    .fetch_optional(&st.db)
    .await;
    let item_id = match inserted.map(|r| r.and_then(|r| r.i64("id"))) {
        Ok(Some(id)) => id,
        Ok(None) => {
            let _ = tokio::fs::remove_file(&dest_path).await;
            anyhow::bail!("insert returned no id");
        }
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest_path).await;
            return Err(e.into());
        }
    };
    st.invalidate_upload_count_cache(&owner);
    auto_subscribe(st, session.id, item_id, "new item").await;

    let t = queue::gen_thumbnail(&cfg, &filename, &mime_type, item_id, &source.src, manual, queue::thumb_size(0)).await;
    if !t.ok {
        tracing::error!("[REHOST] Thumbnail error for item {item_id}");
    }
    queue::gen_blurred_thumbnail(&cfg, item_id, manual).await;

    if let Some(r) = &rating {
        super::uploads::assign_tag(st, item_id, rating_tag_id(&cfg, r), session.id).await?;
    }
    for tag in tag_list(&body["tags"]) {
        if let Some(id) = super::uploads::ensure_tag(st, &tag).await? {
            super::uploads::assign_tag(st, item_id, id, session.id).await?;
        }
    }
    if let Some(c) = body["comment"].as_str().map(str::trim).filter(|c| !c.is_empty()) {
        let content = crate::wordfilter::apply_word_filter(st, c).await;
        if let Err(e) = sql!("INSERT INTO comments (item_id, user_id, content) VALUES (", item_id as i32, ", ", session.id as i32, ", ", content, ")").execute(&st.db).await {
            tracing::error!("[REHOST] Comment insert error: {e}");
        }
    }
    sql!("INSERT INTO notifications (user_id, type, reference_id, item_id) VALUES (", session.id as i32, ", 'upload_success', 0, ", item_id as i32, ")")
        .execute(&st.db)
        .await?;

    if !manual {
        st.caches.feed_count.clear();
        let payload = json!({
            "id": item_id,
            "dest": filename,
            "mime": mime_type,
            "username": owner,
            "display_name": session.display_name.clone().filter(|d| !d.is_empty()),
            "tag_id": rating.as_deref().map(|r| rating_tag_id(&cfg, r)).unwrap_or(0),
            "is_oc": false,
            "is_album": false,
            "album_count": 0,
        });
        if let Err(e) = sql!("SELECT pg_notify('new_item', ", payload.to_string(), ")").execute(&st.db).await {
            tracing::error!("[REHOST] new_item notify failed: {e}");
        }
        let msg = format!("{} uploaded a new item {}", session.user, crate::bot::item_url(st, item_id).await);
        let bst = st.clone();
        tokio::spawn(async move {
            let cfg = bst.cfg();
            bst.bot.notify_matrix(&cfg, &msg).await;
        });
    }

    let path = item_path(st, item_id).await;
    Ok(json(json!({ "success": true, "item_id": item_id, "item_path": path }), 200))
}

// ── Rehost details ──────────────────────────────────────────────────────────

fn is_rating(t: &Value) -> bool {
    matches!(t["normalized"].as_str(), Some("sfw" | "nsfw" | "nsfl"))
}

async fn rehost_details_get(st: AppState, req: Req) -> Res {
    let Some(item_id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return Ok(fail(500, "Invalid item id")) };
    match crate::util::get_tags(&st, item_id as i64, req.session(), None).await {
        Ok(tags) => {
            let rating = tags.iter().find(|t| is_rating(t)).and_then(|t| t["normalized"].as_str()).unwrap_or("untagged").to_string();
            let user_tags: Vec<Value> = tags.iter().filter(|t| !is_rating(t)).map(|t| t["tag"].clone()).collect();
            Ok(json(json!({ "success": true, "item_id": item_id, "rating": rating, "tags": user_tags }), 200))
        }
        Err(e) => {
            tracing::error!("[REHOST-DETAILS] Error: {e}");
            Ok(fail(500, "Failed to load details"))
        }
    }
}

async fn rehost_details_post(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned().filter(|s| !s.user.is_empty()) else { return Ok(fail(401, "Unauthorized")) };
    let body = req.post().await?.clone();
    // parseInt(item_id, 10)
    let item_id = crate::config::as_int(&body["item_id"]).filter(|&n| n != 0 && n > i32::MIN as i64 && n <= i32::MAX as i64);
    let Some(item_id) = item_id else { return Ok(fail(400, "Valid item_id is required")) };
    match rehost_details_inner(&st, &req, &session, &body, item_id).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[REHOST-DETAILS] Error: {e:#}");
            Ok(fail(500, "Failed to save details"))
        }
    }
}

async fn rehost_details_inner(st: &AppState, req: &Req, session: &crate::session::Session, body: &Value, item_id: i64) -> anyhow::Result<Response> {
    let cfg = st.cfg();
    let id32 = item_id as i32;
    let Some(row) = sql!("SELECT id, username, src FROM items WHERE id = ", id32, " AND active = true AND is_deleted = false LIMIT 1").fetch_optional(&st.db).await? else {
        return Ok(fail(404, "Item not found"));
    };
    let owner = s::session_owner_name(Some(session)).map(|o| o.to_lowercase());
    let uname = row.string("username").to_lowercase();
    let is_owner = !uname.is_empty() && owner.as_deref() == Some(uname.as_str());
    let is_admin = session.admin || session.is_moderator;
    let is_chan_user = s::can_use_chan(&cfg, Some(session));

    let tags = &body["tags"];
    let comment = body["comment"].as_str().map(str::trim).filter(|c| !c.is_empty());
    let has_tags = match tags {
        Value::Array(a) => !a.is_empty(),
        Value::String(s) => !s.trim().is_empty(),
        _ => false,
    };
    if (has_tags || comment.is_some()) && !is_owner && !is_admin {
        return Ok(fail(403, "Only owner can edit tags and comment"));
    }
    if !is_owner && !is_admin && !is_chan_user {
        return Ok(fail(403, "Unauthorized"));
    }
    // Other 4chan-group users may only rate items that were rehosted from 4chan
    if !is_owner && !is_admin && chan_http::rehost_source(&row.string("src")).is_none() {
        return Ok(fail(403, "Unauthorized"));
    }

    let nsfl_id: i64 = sql!("SELECT id FROM tags WHERE normalized = 'nsfl' LIMIT 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")).unwrap_or_else(|| cfg.nsfl_tag_id());
    let uid = session.id as i32;

    // 1. Rating
    let rating = match &body["rating"] {
        Value::Null => None,
        Value::String(s) if s.is_empty() => None,
        v => Some(js_str(v).trim().to_lowercase()),
    };
    if let Some(r) = rating {
        let tag = match r.as_str() {
            "sfw" => Some(1),
            "nsfw" => Some(2),
            "nsfl" => Some(nsfl_id),
            _ => None, // 'untagged': no rating tag
        };
        // One transaction, so a failing insert doesn't leave the item without its old rating
        let mut tx = st.db.begin().await?;
        sql!(
            "DELETE FROM tags_assign WHERE item_id = ", id32,
            " AND (tag_id IN (1, 2, ", nsfl_id as i32, ") OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl')))"
        )
        .execute(&mut *tx)
        .await?;
        if let Some(t) = tag {
            sql!("INSERT INTO tags_assign (item_id, tag_id, user_id) VALUES (", id32, ", ", t as i32, ", ", uid, ") ON CONFLICT DO NOTHING").execute(&mut *tx).await?;
        }
        tx.commit().await?;
    }

    // 2. Tags
    if !tags.is_null() {
        let clean = tag_list(tags);
        let mut q = sql!(
            "DELETE FROM tags_assign WHERE item_id = ", id32,
            " AND tag_id NOT IN (1, 2, ", nsfl_id as i32, ") AND tag_id NOT IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl'))"
        );
        if !is_owner {
            q.push(" AND user_id = ").bind(uid);
        }
        q.execute(&st.db).await?;
        for name in clean {
            if let Some(tid) = super::uploads::ensure_tag(st, &name).await? {
                super::uploads::assign_tag(st, item_id, tid, session.id).await?;
            }
        }
    }

    // 3. Comment
    if let Some(c) = comment {
        let content = crate::wordfilter::apply_word_filter(st, c).await;
        sql!("INSERT INTO comments (item_id, user_id, content) VALUES (", id32, ", ", uid, ", ", content, ")").execute(&st.db).await?;
    }

    let fresh = crate::util::get_tags(st, item_id, req.session(), None).await?;
    let payload = json!({ "item_id": item_id, "fresh": true, "tags": fresh });
    let _ = sql!("SELECT pg_notify('tags', ", payload.to_string(), ")").execute(&st.db).await;

    let rating = fresh.iter().find(|t| is_rating(t)).and_then(|t| t["normalized"].as_str()).unwrap_or("untagged").to_string();
    let clean: Vec<Value> = fresh
        .iter()
        .filter(|t| !is_rating(t))
        .map(|t| json!({ "id": t["id"], "tag": t["tag"], "normalized": t["normalized"], "badge": t["badge"] }))
        .collect();
    let path = item_path(st, item_id).await;
    Ok(json(json!({ "success": true, "item_id": item_id, "item_path": path, "rating": rating, "tags": clean }), 200))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids() {
        assert_eq!(parse_ids(&json!("1,2, 3,,-4,x,0,5")), vec![1, 2, 3, 5]);
        assert_eq!(parse_ids(&json!([7, "8"])), vec![7, 8]);
        assert_eq!(parse_ids(&Value::Null), Vec::<i32>::new());
        assert_eq!(parse_ids(&json!("99999999999")), Vec::<i32>::new());
    }

    #[test]
    fn tags_and_dims() {
        assert_eq!(tag_list(&json!(" cat, SFW ,dog,,nsfl")), vec!["cat", "dog"]);
        assert_eq!(tag_list(&json!(["a", " b "])), vec!["a", "b"]);
        assert!(tag_list(&json!(5)).is_empty());
        assert_eq!(dim(&json!("640")), Some(640));
        assert_eq!(dim(&json!(360)), Some(360));
        assert_eq!(dim(&json!("")), None);
        assert_eq!(dim(&json!("-1")), None);
        assert_eq!(dim(&json!("1e9")), None);
    }

    #[test]
    fn find_post_in_catalog() {
        // Shape of https://a.4cdn.org/<board>/catalog.json (flattened)
        let threads: Vec<Value> = serde_json::from_str(
            r#"[{"no":100,"last_replies":[{"no":101},{"no":102}]},{"no":200},{"no":300,"last_replies":[{"no":305}]}]"#,
        )
        .unwrap();
        assert_eq!(find_in_catalog(&threads, 200), Some(json!(200)));
        assert_eq!(find_in_catalog(&threads, 102), Some(json!(100)));
        assert_eq!(find_in_catalog(&threads, 305), Some(json!(300)));
        assert_eq!(find_in_catalog(&threads, 999), None);
    }
}
