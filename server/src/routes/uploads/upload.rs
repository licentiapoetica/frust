//! `POST /api/v2/upload` (`src/upload_handler.mjs`): file, URL, YouTube and album uploads.

use super::{api_key_session, apply_word_filter, assign_tag, csrf_header_ok, declared_len, ensure_tag, fail, log_anon_activity, send};
use crate::config::Config;
use crate::db::RowExt;
use crate::http::{Req, Res};
use crate::media::multipart::{self, FilePart, Form, Limits, MpError};
use crate::media::{self, TempPath, mime, queue};
use crate::session::Session;
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

static YT_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?\?(?:\S*?&?v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").unwrap()
});
static HTTP_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)^https?://").unwrap());

/// Domain-based tags for URL uploads (currently only `youtube`).
fn auto_tags_from_url(url: &str) -> Vec<String> {
    let Ok(u) = url::Url::parse(url) else { return vec![] };
    let host = u.host_str().unwrap_or("").to_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    let short_slds = ["co", "com", "net", "org", "gov", "edu", "ac", "or", "ne"];
    let domain = if parts.len() >= 3 && short_slds.contains(&parts[parts.len() - 2]) { parts[parts.len() - 3..].join(".") } else { parts[parts.len().saturating_sub(2)..].join(".") };
    let yt = |d: &str| d.ends_with("youtube.com") || d.ends_with("youtu.be");
    if yt(&domain) || yt(&host) { vec!["youtube".into()] } else { vec![] }
}

/// URL uploads must be http(s) and must not point at this machine or a private network
/// (the old code handed any string to yt-dlp/curl, including option-like strings and
/// `file://`/`localhost` targets).
pub fn validate_remote_url(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    let host = u.host()?;
    let blocked = match host {
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") || d.ends_with(".internal")
        }
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_unspecified() || ip.is_broadcast() || ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64,
        url::Host::Ipv6(ip) => {
            ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80 || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private())
        }
    };
    (!blocked).then_some(u)
}

fn rating_tag_id(cfg: &Config, rating: Option<&str>) -> i64 {
    match rating {
        Some("sfw") => 1,
        Some("nsfw") => 2,
        Some("nsfl") => cfg.nsfl_tag_id(),
        _ => 0,
    }
}

fn plural(n: i64) -> &'static str {
    if n != 1 { "s" } else { "" }
}

/// Everything the request settled before processing.
struct Ctx {
    owner: String,
    session: Session,
    manual: bool,
    visibility: i64,
    expires_at: Option<i64>,
    slug: String,
    now: i64,
    title: Option<String>,
    comment: String,
    is_oc: bool,
    is_album: bool,
    total_album_items: usize,
    rating: Option<String>,
    tags: Vec<String>,
    audit_ip: Option<String>,
    max_bytes: f64,
}

impl Ctx {
    fn user_id(&self) -> i32 {
        self.session.id as i32
    }

    fn display_name(&self) -> Value {
        self.session.display_name.clone().filter(|d| !d.is_empty()).map(Value::String).unwrap_or(Value::Null)
    }
}

pub async fn handle_upload(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if req.session.is_none() && cfg.enabled("websrv.enable_user_api_keys") {
        if let Some(key) = req.header("x-api-key").map(str::to_string) {
            match api_key_session(&st, &key).await {
                Ok(Some(sess)) => {
                    if sess.banned {
                        return fail(403, "Account banned");
                    }
                    req.session = Some(sess);
                }
                Ok(None) => {}
                Err(e) => tracing::error!("[UPLOAD] API key lookup error: {e}"),
            }
        }
    }
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    if s::is_anon_session(Some(&session)) && !s::can_anon_do(&cfg, "upload") {
        return fail(403, "Uploading requires a registered account or anonymous upload permission");
    }
    if !session.api_key_auth && !csrf_header_ok(&req, &session) {
        return fail(403, "Invalid CSRF token");
    }

    let mut max = cfg.f64("main.maxfilesize").filter(|m| *m > 0.0).unwrap_or(150.0 * 1024.0 * 1024.0);
    if session.admin {
        max = (max * cfg.f64("main.adminmultiplier").filter(|m| *m != 0.0).unwrap_or(10.0)).floor();
    }

    let ct = req.content_type().to_string();
    let form = if ct.contains("multipart/form-data") {
        let Some(boundary) = multipart::boundary(&ct) else { return fail(400, "Invalid multipart boundary") };
        let body = req.take_body().unwrap_or_else(Body::empty);
        match multipart::parse(body, &boundary, declared_len(&req), &cfg.paths.tmp, Limits::total(max as u64)).await {
            Ok(f) => f,
            Err(MpError::TooLarge) => return too_large(&cfg, &session),
            Err(MpError::Invalid(e)) => {
                tracing::warn!("[UPLOAD] Malformed multipart body from {}: {e}", session.user);
                Form::default()
            }
            Err(MpError::Io(e)) => return Err(e.into()),
        }
    } else if ct.contains("application/json") || ct.contains("application/x-www-form-urlencoded") {
        if declared_len(&req).is_some_and(|l| l as f64 > max) {
            return too_large(&cfg, &session);
        }
        let Ok(bytes) = req.body_bytes(max as usize).await else { return too_large(&cfg, &session) };
        if ct.contains("application/json") {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(v) => Form::from_json(&v),
                Err(_) => return fail(400, "Invalid JSON body"),
            }
        } else {
            Form::from_urlencoded(&bytes)
        }
    } else {
        return fail(400, "Invalid content type");
    };

    match process(&st, &req, &cfg, session, &form, max).await {
        Ok(r) => Ok(r),
        Err(e) => send(json!({ "success": false, "msg": crate::util::log_error(&format!("{:#}", e.0), "Upload failed") }), 500),
    }
}

fn too_large(cfg: &Config, session: &Session) -> Res {
    let boosted = session.admin || session.is_moderator;
    let limit = cfg.f64("main.maxfilesize").unwrap_or(0.0) * if boosted { cfg.f64("main.adminmultiplier").unwrap_or(1.0) } else { 1.0 };
    tracing::error!("[UPLOAD HANDLER ERROR] [BODY_TOO_LARGE] User: {}. Limit: {}", session.user, crate::util::format_size_f(limit));
    fail(413, "File too large")
}

async fn process(st: &AppState, req: &Req, cfg: &Config, session: Session, form: &Form, max: f64) -> Res {
    let count = |n: &str| form.entries.iter().filter(|(k, _)| k == n).count();
    let mut raw_files: Vec<FilePart> = if count("files") > 1 {
        form.files("files").into_iter().cloned().collect()
    } else if count("file") > 1 {
        form.files("file").into_iter().cloned().collect()
    } else if let Some(f) = form.file("files") {
        vec![f.clone()]
    } else if let Some(f) = form.file("file") {
        vec![f.clone()]
    } else {
        vec![]
    };
    let mut input_url: Option<String> = form.text("url").map(str::trim).filter(|u| !u.is_empty()).map(str::to_string);
    if input_url.is_none() {
        if let Some(f) = form.text("file").map(str::trim).filter(|f| HTTP_RE.is_match(f)) {
            input_url = Some(f.to_string());
        }
    }
    if let Some(u) = &input_url {
        raw_files.clear();
        if let Ok(mut parsed) = url::Url::parse(u) {
            if parsed.query_pairs().any(|(k, _)| k == "igsh") {
                let kept: Vec<(String, String)> = parsed.query_pairs().filter(|(k, _)| k != "igsh").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
                if kept.is_empty() {
                    parsed.set_query(None);
                } else {
                    parsed.query_pairs_mut().clear().extend_pairs(kept);
                }
                input_url = Some(parsed.to_string());
            }
        }
    }
    let file: Option<FilePart> = raw_files.first().cloned();

    let rating = form.text("rating").map(str::to_string);
    let comment = form.joined("comment").map(|c| c.trim().to_string()).unwrap_or_default();
    let mut title: Option<String> = form.joined("title").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).map(|t| t.chars().take(500).collect());
    let is_oc = form.flag("is_oc");

    let album_urls: Vec<(usize, String)> = (0..101)
        .filter_map(|i| form.text(&format!("subf0ck_url_{i}")).map(str::trim).filter(|u| HTTP_RE.is_match(u)).map(|u| (i, u.to_string())))
        .collect();
    let total_album_items = raw_files.len() + album_urls.len();
    let album_requested = form.flag("is_album") || (total_album_items > 1 && !form.truthy("is_shitpost"));
    let is_album = album_requested && total_album_items > 1;
    let is_shitpost = !is_album && (form.flag("is_shitpost") || cfg.get("websrv.shitpost_mode") == &Value::Bool(true));

    let max_album = [cfg.int("websrv.max_album_items"), cfg.int("websrv.max_album_images")].into_iter().flatten().find(|v| *v != 0).unwrap_or(100);
    if is_album && total_album_items as i64 > max_album {
        return fail(400, &format!("Album exceeds maximum limit of {max_album} items"));
    }
    if file.is_none() && input_url.is_none() && album_urls.is_empty() {
        return fail(400, "No file or URL provided");
    }
    if file.is_none() && input_url.is_none() {
        input_url = Some(album_urls[0].1.clone());
    }
    if input_url.is_some() && cfg.is_false("websrv.web_url_upload") {
        return fail(403, "URL uploads are disabled");
    }
    if let Some(u) = &input_url {
        if validate_remote_url(u).is_none() {
            return fail(400, "Invalid URL");
        }
    }

    // Visibility: header, body field, user default, system default
    let mut visibility = 0i64;
    if cfg.enabled("enable_private_uploads") {
        let num = |p: &str| cfg.get(p).as_f64().map(|f| f as i64);
        let sys_default = num("default_upload_visibility").or_else(|| num("websrv.default_upload_visibility")).unwrap_or(0);
        if !(cfg.enabled("allow_user_upload_visibility") && cfg.enabled("websrv.allow_user_upload_visibility")) {
            visibility = sys_default;
        } else {
            let raw = req.header("x-upload-visibility").filter(|h| !h.is_empty()).map(str::to_string).or_else(|| form.joined("visibility").filter(|v| !v.is_empty())).unwrap_or_default();
            visibility = match raw.trim().to_lowercase().as_str() {
                "private" | "2" => 2,
                "unlisted" | "1" => 1,
                "public" | "0" => 0,
                _ => session.default_upload_visibility.unwrap_or(sys_default),
            };
        }
    }

    let raw_expiry = req
        .header("x-upload-expiry")
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .or_else(|| form.joined("expiry").filter(|v| !v.is_empty()))
        .or_else(|| form.joined("expires_at").filter(|v| !v.is_empty()));
    let now = crate::util::now_secs();
    let expires_at = media::calculate_expires_at(cfg, raw_expiry.as_deref(), now);
    let slug = crate::util::generate_slug(11);

    if let Some(max_len) = cfg.get("main.comment_max_length").as_f64() {
        if !comment.is_empty() && comment.chars().count() as f64 > max_len {
            return fail(400, &format!("Comment too long (max {} characters)", crate::util::js_num(max_len)));
        }
    }

    let effective_rating = rating.filter(|r| ["sfw", "nsfw", "nsfl"].contains(&r.as_str()));
    let admin_api = session.api_key_auth && session.admin;
    if !is_shitpost && !admin_api && effective_rating.is_none() {
        return fail(400, "Rating (sfw/nsfw/nsfl) is required");
    }
    if is_shitpost && !admin_api && cfg.get("websrv.shitpost_require_rating") == &Value::Bool(true) && effective_rating.is_none() {
        return fail(400, "Rating (sfw/nsfw/nsfl) is required for each item");
    }
    if effective_rating.as_deref() == Some("nsfl") && !cfg.enable_nsfl() {
        return fail(400, "NSFL mode is currently disabled");
    }

    let is_rating = |t: &str| ["sfw", "nsfw", "nsfl"].contains(&t.to_lowercase().as_str());
    let user_tags: Vec<String> = form.joined("tags").map(|t| t.split(',').map(str::trim).filter(|t| !t.is_empty() && !is_rating(t)).map(str::to_string).collect()).unwrap_or_default();
    let auto_tags = match (&input_url, &file) {
        (Some(u), None) => auto_tags_from_url(u),
        _ => vec![],
    };
    let mut tags: Vec<String> = Vec::new();
    for t in user_tags.iter().chain(auto_tags.iter()) {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }

    let min_tags = st.settings.min_tags();
    let shitpost_min = if is_shitpost { crate::config::as_int(cfg.get("websrv.shitpost_min_tags")).unwrap_or(0) } else { 0 };
    if !is_shitpost && min_tags > 0 && (user_tags.len() as i64) < min_tags {
        return fail(400, &format!("At least {min_tags} tag{} required", plural(min_tags)));
    }
    if is_shitpost && shitpost_min > 0 && (user_tags.len() as i64) < shitpost_min {
        return fail(400, &format!("At least {shitpost_min} tag{} required", plural(shitpost_min)));
    }

    let allowed = mime::allowed_mimes(cfg);
    let square_clicker = cfg.enabled("websrv.square_clicker_enabled");
    if let (Some(f), None) = (&file, &input_url) {
        let mut m = f.content_type.clone();
        if (m == "application/octet-stream" || m.is_empty() || m == "application/x-www-form-urlencoded") && f.filename.to_lowercase().ends_with(".swf") {
            m = "application/x-shockwave-flash".into();
        }
        let beatmap = square_clicker && f.filename.to_lowercase().ends_with(".osz");
        if !allowed.contains(&m) && !beatmap {
            return fail(400, &format!("Invalid file type: {m}"));
        }
    }
    if is_album {
        for (i, f) in raw_files.iter().enumerate() {
            if f.size == 0 {
                return fail(400, &format!("Album item #{} is empty", i + 1));
            }
        }
    }

    let owner = s::session_owner_name(Some(&session)).unwrap_or_else(|| session.user.clone());
    let staff = session.admin || session.is_moderator;
    let mut manual = st.settings.manual_approval();
    let trusted = st.settings.trusted_uploads();
    if trusted > 0 && !staff {
        match sql!("SELECT count(*) as count FROM items WHERE username = ", owner.clone(), " AND is_deleted = false").fetch_scalar::<i64, _>(&st.db).await {
            Ok(c) if c.unwrap_or(0) >= trusted => {}
            _ => manual = true,
        }
    }
    if !manual && !staff {
        let since = now - 12 * 3600;
        let c = sql!("SELECT count(*) as count FROM items WHERE username = ", owner.clone(), " AND stamp > ", since as i32, " AND is_deleted = false")
            .fetch_scalar::<i64, _>(&st.db)
            .await?
            .unwrap_or(0);
        let limit = cfg.int("main.upload_limit").unwrap_or(69);
        if c >= limit {
            return fail(429, &format!("Rate limit exceeded. You can only upload {limit} files every 12 hours."));
        }
    }

    let ctx = Ctx {
        owner,
        session,
        manual,
        visibility,
        expires_at,
        slug,
        now,
        title: title.take(),
        comment,
        is_oc,
        is_album,
        total_album_items,
        rating: effective_rating,
        tags,
        audit_ip: crate::security::storable_ip(cfg, &req.real_ip()),
        max_bytes: max,
    };

    if file.is_none() {
        if let Some(u) = &input_url {
            if let Some(c) = YT_RE.captures(u) {
                if cfg.enabled("websrv.enable_youtube_upload") {
                    return youtube_upload(st, req, cfg, &ctx, c[1].to_string(), &album_urls).await;
                }
            }
            if !st.settings.bypass_duplicate_check() {
                if let Some(id) = queue::check_repost_link(st, u).await? {
                    return send(json!({ "success": false, "msg": "This URL has already been uploaded", "repost": id }), 409);
                }
            }
        }
    }

    file_upload(st, req, cfg, &ctx, form, file, raw_files, input_url, album_urls).await
}

/// Insert an `album_items` row; returns its id.
#[allow(clippy::too_many_arguments)]
async fn insert_album_item(st: &AppState, item_id: i64, dest: &str, mime: &str, size: i64, checksum: &str, phash: Option<String>, dims: Option<(i32, i32)>, order: i64, slug: &str) -> anyhow::Result<Option<i64>> {
    let row = sql!(
        "INSERT INTO album_items (item_id, dest, mime, size, checksum, phash, width, height, order_index, slug) VALUES (",
        item_id as i32, ", ", dest, ", ", mime, ", ", size.min(i32::MAX as i64) as i32, ", ", checksum, ", ", phash, ", ",
        dims.map(|d| d.0), ", ", dims.map(|d| d.1), ", ", order as i32, ", ", slug, ") RETURNING id"
    )
    .fetch_optional(&st.db)
    .await?;
    Ok(row.and_then(|r| r.i64("id")))
}

async fn add_common_tags(st: &AppState, ctx: &Ctx, item_id: i64, extra: &[&str]) -> anyhow::Result<()> {
    let uid = ctx.session.id;
    for t in ctx.tags.iter().map(String::as_str).chain(if ctx.is_oc { vec!["oc", "original content"] } else { vec![] }).chain(extra.iter().copied()) {
        if let Some(tag_id) = ensure_tag(st, t).await? {
            assign_tag(st, item_id, tag_id, uid).await?;
        }
    }
    Ok(())
}

async fn insert_comment(st: &AppState, ctx: &Ctx, item_id: i64) {
    if ctx.comment.is_empty() {
        return;
    }
    let content = apply_word_filter(st, &ctx.comment).await;
    if let Err(e) = sql!("INSERT INTO comments (item_id, user_id, content) VALUES (", item_id as i32, ", ", ctx.user_id(), ", ", content, ")").execute(&st.db).await {
        tracing::error!("[UPLOAD HANDLER] Failed to insert comment: {e}");
    }
}

async fn subscribe_uploader(st: &AppState, cfg: &Config, ctx: &Ctx, item_id: i64) {
    if !cfg.enabled("enable_comments") {
        return;
    }
    if let Err(e) = sql!("INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", ctx.user_id(), ", ", item_id as i32, ") ON CONFLICT DO NOTHING").execute(&st.db).await {
        tracing::error!("[UPLOAD HANDLER] Failed to auto-subscribe uploader: {e}");
    }
}

fn new_item_payload(cfg: &Config, ctx: &Ctx, item_id: i64, dest: &str, mime: &str) -> Value {
    json!({
        "id": item_id,
        "dest": dest,
        "mime": mime,
        "username": ctx.owner,
        "display_name": ctx.display_name(),
        "tag_id": rating_tag_id(cfg, ctx.rating.as_deref()),
        "is_oc": ctx.is_oc,
        "slug": ctx.slug,
        "visibility": ctx.visibility,
        "is_album": ctx.is_album,
        "album_count": if ctx.is_album { ctx.total_album_items } else { 0 },
    })
}

async fn notify_new_item(st: &AppState, payload: &Value) {
    if let Err(e) = sql!("SELECT pg_notify('new_item', ", payload.to_string(), ")").execute(&st.db).await {
        tracing::error!("[UPLOAD] new_item notify failed: {e}");
    }
}

#[allow(clippy::too_many_arguments)]
fn success(cfg: &Config, ctx: &Ctx, item_id: i64, dest: &str, mime: &str, file_url: Value) -> Res {
    let base = cfg.url_full();
    let route = format!("/{}", ctx.slug);
    let msg = if ctx.manual { "Upload successful! Your upload is pending admin approval." } else { "Upload successful! Your upload is now live." };
    let mut v = json!({
        "success": true,
        "msg": msg,
        "itemid": item_id,
        "slug": ctx.slug,
        "visibility": ctx.visibility,
        "manual_approval": ctx.manual,
        "redirect": if ctx.manual { Value::Null } else { Value::String(route.clone()) },
        "url": if ctx.manual { format!("{base}/pending#i{item_id}") } else { format!("{base}{route}") },
        "status": if ctx.manual { "pending" } else { "live" },
        "status_url": format!("{base}/pending#i{item_id}"),
        "status_api": format!("{base}/api/v2/uploads/{item_id}/status"),
        "file_url": file_url,
        "dest": dest,
        "mime": mime,
        "tag_id": rating_tag_id(cfg, ctx.rating.as_deref()),
        "is_oc": ctx.is_oc,
    });
    let o = v.as_object_mut().expect("object");
    let album_count = if ctx.is_album { ctx.total_album_items } else { 0 };
    // Key order of the old responses (YouTube vs. file uploads differ slightly)
    if mime == "video/youtube" {
        o.insert("display_name".into(), ctx.display_name());
        o.insert("username".into(), json!(ctx.owner));
        o.insert("is_album".into(), json!(ctx.is_album));
        o.insert("album_count".into(), json!(album_count));
    } else {
        o.insert("is_album".into(), json!(ctx.is_album));
        o.insert("album_count".into(), json!(album_count));
        o.insert("display_name".into(), ctx.display_name());
        o.insert("username".into(), json!(ctx.owner));
    }
    send(v, 200)
}

// ── YouTube embeds ──────────────────────────────────────────────────────────

async fn youtube_upload(st: &AppState, req: &Req, cfg: &Config, ctx: &Ctx, video_id: String, album_urls: &[(usize, String)]) -> Res {
    let yt_url = format!("https://www.youtube.com/watch?v={video_id}");
    let filename = format!("yt:{video_id}");
    let checksum = format!("yt_{video_id}_{}", crate::util::now_ms());
    let item_id = sql!(
        "insert into items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, is_oc, title, visibility, slug, expires_at, uploader_ip) values (",
        yt_url.clone(), ", ", filename.clone(), ", 'video/youtube', 0, ", checksum, ", NULL, ", ctx.owner.clone(), ", 'web', 'web', ", ctx.now as i32, ", ",
        !ctx.manual, ", ", ctx.is_oc, ", ", ctx.title.clone(), ", ", ctx.visibility as i32, ", ", ctx.slug.clone(), ", ", ctx.expires_at, ", ", ctx.audit_ip.clone(),
        ") RETURNING id"
    )
    .fetch_optional(&st.db)
    .await?
    .and_then(|r| r.i64("id"))
    .ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
    st.invalidate_upload_count_cache(&ctx.owner);

    if ctx.session.is_anon {
        log_anon_activity(st, req, "upload", Some(item_id), json!({ "type": "youtube", "videoId": video_id, "slug": ctx.slug })).await;
    }
    if ctx.visibility == 2 {
        crate::private_items::add_private(st, item_id, &filename, &ctx.owner);
    }
    subscribe_uploader(st, cfg, ctx, item_id).await;

    let t = queue::gen_thumbnail(cfg, &filename, "video/youtube", item_id, &yt_url, ctx.manual, 512).await;
    let _ = t;

    if ctx.rating.is_some() {
        assign_tag(st, item_id, rating_tag_id(cfg, ctx.rating.as_deref()), ctx.session.id).await?;
        queue::gen_blurred_thumbnail(cfg, item_id, ctx.manual).await;
    }
    for tag in &ctx.tags {
        if let Some(id) = ensure_tag(st, tag).await? {
            assign_tag(st, item_id, id, ctx.session.id).await?;
        }
    }
    insert_comment(st, ctx, item_id).await;
    if ctx.is_oc {
        for name in ["oc", "original content"] {
            if let Some(id) = ensure_tag(st, name).await? {
                assign_tag(st, item_id, id, ctx.session.id).await?;
            }
        }
    }
    if !ctx.manual {
        st.caches.feed_count.clear();
        notify_new_item(st, &new_item_payload(cfg, ctx, item_id, &filename, "video/youtube")).await;
    }

    // URL-only album: the first video is the cover, further YouTube links become sub-items
    if ctx.is_album && !album_urls.is_empty() {
        let cover_checksum = format!("yt_{video_id}_cover_{}", crate::util::now_ms());
        insert_album_item(st, item_id, &filename, "video/youtube", 0, &cover_checksum, None, None, 0, &crate::util::generate_slug(11)).await?;
        sql!("UPDATE items SET is_album = true, album_count = ", ctx.total_album_items as i32, " WHERE id = ", item_id as i32).execute(&st.db).await?;
        for (ui, (_, u)) in album_urls.iter().enumerate().skip(1) {
            let Some(c) = YT_RE.captures(u) else { continue };
            let sub_id = &c[1];
            let sub_checksum = format!("yt_{sub_id}_{}_{ui}", crate::util::now_ms());
            insert_album_item(st, item_id, &format!("yt:{sub_id}"), "video/youtube", 0, &sub_checksum, None, None, ui as i64, &crate::util::generate_slug(11)).await?;
            // The old code also rendered t/<album_item_id>.webp here, which overwrote the
            // thumbnail of the unrelated item with that id; album YouTube thumbs come from
            // img.youtube.com, so nothing is generated.
        }
    }
    success(cfg, ctx, item_id, &filename, "video/youtube", Value::Null)
}

// ── Files and downloaded URLs ───────────────────────────────────────────────

/// Removes leftovers named `<uuid>.*` in the temp dir (partial yt-dlp downloads etc.).
struct PrefixCleanup {
    dir: PathBuf,
    prefix: String,
}

impl Drop for PrefixCleanup {
    fn drop(&mut self) {
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(&self.prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

fn last_line(out: &crate::proc::Output) -> Option<String> {
    out.stdout_str().lines().map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_string)
}

enum Download {
    Ok(PathBuf),
    Failed(&'static str),
}

/// yt-dlp (preferred format, then any), then plain curl.
async fn download(cfg: &Config, url: &str, uuid: &str, max: f64) -> Download {
    let tmp = &cfg.paths.tmp;
    let out_tpl = tmp.join(format!("{uuid}.%(ext)s")).to_string_lossy().into_owned();
    let max_k = format!("{}k", crate::util::js_num(max / 1024.0));
    let socks = media::socks(cfg);
    let mut base: Vec<&str> = Vec::new();
    if let Some(p) = &socks {
        base.extend(["--proxy", p.as_str()]);
    }
    base.extend(["--js-runtimes", "node", "--geo-bypass", "--extractor-args", "youtube:player-client=ios,web", "--newline", "--no-colors"]);
    let quiet = || crate::proc::Opts { quiet: true, ..Default::default() };

    let mut a1 = base.clone();
    a1.extend([
        "-f", "bv*[height<=1080]+ba/b[height<=1080] / wv*+ba/w",
        "--max-filesize", &max_k,
        "--postprocessor-args", "ffmpeg:-bitexact",
        "-o", &out_tpl,
        "--print", "after_move:filepath",
        "--merge-output-format", "mp4",
        "--", url,
    ]);
    let mut downloaded = match crate::proc::spawn_with("yt-dlp", &a1, quiet()).await {
        Ok(o) => last_line(&o),
        Err(_) if url.to_lowercase().contains("instagram.com") => return Download::Failed("Failed to download Instagram URL"),
        Err(_) => {
            let mut a2 = base.clone();
            a2.extend(["--max-filesize", &max_k, "-o", &out_tpl, "--print", "after_move:filepath", "--", url]);
            match crate::proc::spawn_with("yt-dlp", &a2, quiet()).await {
                Ok(o) => last_line(&o),
                Err(_) => {
                    let fallback = tmp.join(format!("{uuid}.curl_tmp"));
                    let fb = fallback.to_string_lossy().into_owned();
                    let referer = url::Url::parse(url)
                        .ok()
                        .map(|u| {
                            let mut host = u.host_str().unwrap_or("").to_string();
                            if host.contains("imgur.com") {
                                host = "imgur.com".into();
                            }
                            format!("{}://{host}/", u.scheme())
                        })
                        .unwrap_or_else(|| url.to_string());
                    let max_b = crate::util::js_num(max);
                    let mut c: Vec<&str> = vec![
                        "-s", "-S", "-f", "-L", "--proto", "=http,https", "--proto-redir", "=http,https", url, "-o", &fb,
                        "--max-filesize", &max_b, "--connect-timeout", "30", "--max-time", "300",
                        "--user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
                        "--referer", &referer,
                        "-H", "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
                        "-H", "Accept-Language: en-US,en;q=0.9",
                    ];
                    let proxy_host = socks.as_deref().map(|p| p.split_once("://").map(|(_, h)| h).unwrap_or(p).to_string());
                    if let Some(h) = &proxy_host {
                        c.extend(["--socks5-hostname", h.as_str()]);
                    }
                    if crate::proc::spawn("curl", &c).await.is_err() {
                        return Download::Failed("Failed to download file from URL");
                    }
                    Some(fb)
                }
            }
        }
    };
    // yt-dlp reports the final path: accept it only inside our temp dir and with our prefix
    let tmp_real = tokio::fs::canonicalize(tmp).await.unwrap_or(tmp.clone());
    if let Some(p) = downloaded.take() {
        let pb = PathBuf::from(&p);
        if let Ok(real) = tokio::fs::canonicalize(&pb).await {
            let inside = real.parent() == Some(tmp_real.as_path()) && real.file_name().map(|n| n.to_string_lossy().starts_with(&format!("{uuid}."))).unwrap_or(false);
            if inside && real.is_file() {
                return Download::Ok(real);
            }
        }
    }
    Download::Failed("Failed to download file from URL")
}

async fn analyse_sub(path: &Path, mime: &str) -> (anyhow::Result<String>, Option<String>, Option<(i32, i32)>) {
    tokio::join!(queue::sha256_file(path), queue::generate_phash(path), queue::dimensions(path, mime))
}

#[allow(clippy::too_many_arguments)]
async fn file_upload(
    st: &AppState,
    req: &Req,
    cfg: &Config,
    ctx: &Ctx,
    form: &Form,
    file: Option<FilePart>,
    raw_files: Vec<FilePart>,
    input_url: Option<String>,
    album_urls: Vec<(usize, String)>,
) -> Res {
    let uuid = queue::genuuid();
    let _cleanup = PrefixCleanup { dir: cfg.paths.tmp.clone(), prefix: format!("{uuid}.") };
    let tmp = TempPath::new(cfg.paths.tmp.join(format!("{uuid}.tmp")));
    for d in [cfg.paths.tmp.clone(), cfg.paths.pending.join("b"), cfg.paths.pending.join("t"), cfg.paths.pending.join("ca")] {
        tokio::fs::create_dir_all(d).await?;
    }

    if let Some(f) = &file {
        media::move_file(&f.path, &tmp.path).await?;
    } else if let Some(u) = &input_url {
        match download(cfg, u, &uuid, ctx.max_bytes).await {
            Download::Ok(p) => {
                if p != tmp.path {
                    media::move_file(&p, &tmp.path).await?;
                }
            }
            Download::Failed(msg) => return fail(400, msg),
        }
    }

    let mut actual = mime::detect(&tmp.path).await?;
    let allowed = mime::allowed_mimes(cfg);
    // TODO(port): beatmap sets (.osz → audio item + Square Clicker maps, src/inc/beatmap.mjs) are
    // not ported yet; zip uploads are handled like any other archive meanwhile.
    if !allowed.contains(&actual) {
        return fail(400, &format!("Invalid file type detected: {actual}"));
    }
    if actual == "application/pdf" && !st.settings.enable_pdf() {
        return fail(403, "PDF uploads are currently disabled.");
    }
    let archive = mime::is_archive_mime(cfg, &actual);
    if archive && cfg.is_false("websrv.enable_archive") {
        return fail(403, "Archive uploads are currently disabled.");
    }
    if mime::needs_transcode(&actual) {
        let converted = TempPath::new(PathBuf::from(format!("{}.mp4", tmp.path.display())));
        match media::transcode::transcode_to_mp4(&tmp.path, &converted.path).await {
            Ok(()) => {
                tokio::fs::rename(&converted.path, &tmp.path).await?;
                actual = "video/mp4".into();
            }
            Err(e) => {
                tracing::error!("[UPLOAD] Transcode to mp4 failed: {e}");
                return fail(400, "Could not convert this video to a playable format.");
            }
        }
    }

    // Phase A: checksum, perceptual hash, dimensions, audio-only MP4 detection
    let needs_audio_probe = actual == "video/mp4" || actual == "video/quicktime";
    let orig_ext = match (&file, &input_url) {
        (Some(f), _) => f.filename.rsplit('.').next().unwrap_or("").to_lowercase(),
        (None, Some(u)) => u.split('?').next().unwrap_or("").rsplit('.').next().unwrap_or("").to_lowercase(),
        _ => String::new(),
    };
    let tp = tmp.path.clone();
    let (checksum, phash, dims, audio_mp4) = tokio::join!(
        queue::sha256_file(&tp),
        async {
            if archive { None } else { queue::generate_phash(&tp).await }
        },
        async {
            if archive { None } else { queue::dimensions(&tp, &actual).await }
        },
        async {
            if !needs_audio_probe {
                return None;
            }
            if orig_ext == "m4a" || orig_ext == "aac" {
                return Some("audio/mp4");
            }
            let p = tp.to_string_lossy().into_owned();
            match crate::proc::spawn_with("ffprobe", &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_type", "-of", "csv=p=0", &p], crate::proc::Opts { quiet: true, ..Default::default() }).await {
                Ok(o) if o.stdout_str().trim().is_empty() => Some("audio/mp4"),
                _ => None,
            }
        }
    );
    let checksum = checksum?;
    if let Some(m) = audio_mp4 {
        actual = m.to_string();
        tracing::info!("[UPLOAD] Reclassified to {m}");
    }
    let ext = mime::ext_for(cfg, &actual).unwrap_or_else(|| "bin".into());
    let filename = format!("{uuid}.{ext}");
    let dest_path = cfg.paths.pending.join("b").join(&filename);

    // Phase B: duplicates
    let bypass = st.settings.bypass_duplicate_check();
    if !bypass {
        let (by_sum, by_phash) = tokio::join!(queue::check_repost_sum(st, &checksum), queue::check_repost_phash(st, phash.as_deref()));
        if let Some(id) = by_sum? {
            return send(json!({ "success": false, "msg": "This file already exists", "repost": id }), 409);
        }
        if let Some(id) = by_phash? {
            return send(json!({ "success": false, "msg": "This file is a visual duplicate", "repost": id }), 409);
        }
    }

    // Bypass mode: link to an existing copy of the same file instead of storing it twice
    let mut linked = false;
    if bypass {
        let existing = sql!("SELECT dest, checksum FROM items WHERE checksum = ", checksum.clone(), " OR checksum LIKE ", format!("{checksum}_bypass_%"), " ORDER BY id DESC LIMIT 1")
            .fetch_optional(&st.db)
            .await?;
        if let Some(e) = existing {
            let dest = e.string("dest");
            if media::is_plain_name(&dest) {
                let r = async {
                    let real = tokio::fs::canonicalize(cfg.paths.b.join(&dest)).await?;
                    let link = if ctx.manual { dest_path.clone() } else { cfg.paths.b.join(&filename) };
                    let dir = link.parent().map(Path::to_path_buf).unwrap_or_default();
                    let dir = tokio::fs::canonicalize(&dir).await.unwrap_or(dir);
                    let rel = media::relative_path(&dir, &real);
                    tokio::fs::symlink(&rel, &link).await?;
                    tracing::info!("[UPLOAD] bypass: symlinked {} → {}", link.display(), rel.display());
                    std::io::Result::Ok(())
                }
                .await;
                match r {
                    Ok(()) => linked = true,
                    Err(e) => tracing::error!("[UPLOAD ERROR] bypass symlink failed: {e}"),
                }
            }
        }
    }
    let size = tokio::fs::metadata(&tmp.path).await.map(|m| m.len() as i64).unwrap_or(0);
    if !linked {
        media::move_file(&tmp.path, &dest_path).await?;
        media::make_public(&dest_path).await;
    }
    drop(tmp);
    let insert_checksum = if bypass { format!("{checksum}_bypass_{}", crate::util::now_ms()) } else { checksum.clone() };

    let original_filename: Option<String> = match (&file, &input_url) {
        (Some(f), _) => Some(multipart::sanitize_filename(&f.filename)).filter(|n| !n.is_empty()),
        (None, Some(u)) => url::Url::parse(u).ok().and_then(|p| p.path_segments().and_then(|mut s| s.next_back().map(str::to_string))).map(|n| multipart::sanitize_filename(&n)).filter(|n| !n.is_empty()),
        _ => None,
    };

    let item_id = sql!(
        "insert into items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, is_oc, original_filename, title, width, height, visibility, slug, expires_at, uploader_ip, is_album, album_count) values (",
        input_url.clone().unwrap_or_default(), ", ", filename.clone(), ", ", actual.clone(), ", ", size.min(i32::MAX as i64) as i32, ", ", insert_checksum.clone(), ", ", phash.clone(), ", ",
        ctx.owner.clone(), ", 'web', 'web', ", ctx.now as i32, ", ", !ctx.manual, ", ", ctx.is_oc, ", ", original_filename, ", ", ctx.title.clone(), ", ",
        dims.map(|d| d.0), ", ", dims.map(|d| d.1), ", ", ctx.visibility as i32, ", ", ctx.slug.clone(), ", ", ctx.expires_at, ", ", ctx.audit_ip.clone(), ", ",
        ctx.is_album, ", ", if ctx.is_album { ctx.total_album_items as i32 } else { 0 },
        ") RETURNING id"
    )
    .fetch_optional(&st.db)
    .await?
    .and_then(|r| r.i64("id"))
    .ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
    st.invalidate_upload_count_cache(&ctx.owner);

    if ctx.is_album {
        album_items(st, cfg, ctx, form, item_id, &filename, &actual, size, &insert_checksum, phash.clone(), dims, &raw_files, &album_urls, file.is_none(), input_url.as_deref()).await?;
    }

    if ctx.session.is_anon {
        log_anon_activity(st, req, "upload", Some(item_id), json!({ "filename": filename, "mime": actual, "slug": ctx.slug })).await;
    }
    if ctx.visibility == 2 {
        crate::private_items::add_private(st, item_id, &filename, &ctx.owner);
    }
    subscribe_uploader(st, cfg, ctx, item_id).await;

    // Thumbnail & cover art
    let is_pending = if linked { ctx.manual } else { true };
    let mut thumb_done = false;
    if mime::is_flash(&actual) {
        if let Some(thumb) = form.file("thumbnail").filter(|t| t.size > 0) {
            match custom_flash_thumb(cfg, thumb, item_id, is_pending).await {
                Ok(()) => {
                    thumb_done = true;
                    tracing::info!("[UPLOAD] Custom thumbnail processed for Flash item {item_id}");
                }
                Err(e) => tracing::error!("[UPLOAD] Custom thumbnail processing failed for item {item_id}: {e:#}"),
            }
        }
    }
    if !thumb_done {
        let r = queue::gen_thumbnail(cfg, &filename, &actual, item_id, "", is_pending, 512).await;
        if actual.starts_with("audio/") && r.cover_extracted {
            sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", item_id as i32).execute(&st.db).await?;
        }
    }
    queue::gen_blurred_thumbnail(cfg, item_id, is_pending).await;

    insert_comment(st, ctx, item_id).await;

    if ctx.rating.is_some() {
        let rid = rating_tag_id(cfg, ctx.rating.as_deref());
        assign_tag(st, item_id, rid, ctx.session.id).await?;
        if ctx.is_album {
            let _ = sql!(
                "INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) SELECT id, ",
                rid as i32, ", ", ctx.user_id(), " FROM album_items WHERE item_id = ", item_id as i32, " ON CONFLICT DO NOTHING"
            )
            .execute(&st.db)
            .await;
        }
    }
    let swf_tags: &[&str] = if mime::is_flash(&actual) { &["Flash", "SWF"] } else { &[] };
    add_common_tags(st, ctx, item_id, swf_tags).await?;

    if !ctx.manual {
        st.caches.feed_count.clear();
        if !linked {
            let p = &cfg.paths.pending;
            media::move_safe(&dest_path, &cfg.paths.b.join(&filename)).await;
            media::move_safe(&p.join("t").join(format!("{item_id}.webp")), &cfg.paths.t.join(format!("{item_id}.webp"))).await;
            if actual.starts_with("audio") {
                media::move_safe(&p.join("ca").join(format!("{item_id}.webp")), &cfg.paths.ca.join(format!("{item_id}.webp"))).await;
            }
            media::move_safe(&p.join("t").join(format!("{item_id}_blur.webp")), &cfg.paths.t.join(format!("{item_id}_blur.webp"))).await;
        }
    }

    // Notifications run after the response (the old code regenerated the thumbnail here a
    // second time; it was already generated above from the same file, so that step is gone).
    {
        let st = st.clone();
        let cfg = st.cfg();
        let payload = new_item_payload(&cfg, ctx, item_id, &filename, &actual);
        let (manual, visibility, owner, mime_type) = (ctx.manual, ctx.visibility, ctx.owner.clone(), actual.clone());
        tokio::spawn(async move {
            let base = cfg.url_full().to_string();
            if visibility == 0 {
                if let Some(hook) = cfg.client("discord").and_then(|c| c["webhook_url"].as_str()).filter(|h| !h.is_empty()) {
                    let kind = mime_type.split('/').next().unwrap_or("");
                    let msg = format!("{owner} uploaded a new {kind}: {base}/{item_id}");
                    if let Err(e) = st.http.post(hook).json(&json!({ "content": msg })).send().await {
                        tracing::error!("[UPLOAD] Discord Webhook failed: {e}");
                    }
                }
            }
            if !manual {
                notify_new_item(&st, &payload).await;
                if visibility == 0 {
                    st.bot.notify_matrix(&cfg, &format!("{owner} uploaded a new item {base}/{item_id}")).await;
                }
            } else {
                queue::notify_admins(&st, item_id).await;
            }
        });
    }

    let images = cfg.str("websrv.paths.images").filter(|p| !p.is_empty()).unwrap_or("/b");
    let file_url = if ctx.manual { Value::Null } else { Value::String(format!("{}{images}/{filename}", cfg.url_full())) };
    success(cfg, ctx, item_id, &filename, &actual, file_url)
}

/// Custom thumbnail for Flash items: must be a raster image (checked by content).
async fn custom_flash_thumb(cfg: &Config, thumb: &FilePart, item_id: i64, pending: bool) -> anyhow::Result<()> {
    let m = mime::detect(&thumb.path).await?;
    if !m.starts_with("image/") || m.contains("svg") {
        anyhow::bail!("thumbnail is not a raster image ({m})");
    }
    let (_, t_dir, _) = queue::dirs(cfg, pending).await;
    let dest = t_dir.join(format!("{item_id}.webp"));
    let src = format!("{}[0]", thumb.path.display());
    crate::proc::spawn("magick", &[&src, "-resize", "512x512^", "-gravity", "center", "-crop", "512x512+0+0", "+repage", &dest.to_string_lossy()]).await?;
    Ok(())
}

/// Detected MIME of an album sub-file, or `None` if it isn't allowed. A client-declared type is
/// only trusted when libmagic agrees on the top-level type (the old code took any declared type
/// that appeared in `cfg.mimes`, so the content check could be bypassed).
fn sub_mime(allowed: &[String], detected: &str, declared: &str) -> Option<String> {
    if allowed.iter().any(|a| a == detected) {
        return Some(detected.to_string());
    }
    let top = |m: &str| m.split('/').next().unwrap_or("").to_string();
    (allowed.iter().any(|a| a == declared) && top(detected) == top(declared) && !detected.is_empty()).then(|| declared.to_string())
}

#[allow(clippy::too_many_arguments)]
async fn album_items(
    st: &AppState,
    cfg: &Config,
    ctx: &Ctx,
    form: &Form,
    item_id: i64,
    filename: &str,
    actual: &str,
    size: i64,
    checksum: &str,
    phash: Option<String>,
    dims: Option<(i32, i32)>,
    raw_files: &[FilePart],
    album_urls: &[(usize, String)],
    url_only: bool,
    input_url: Option<&str>,
) -> anyhow::Result<()> {
    insert_album_item(st, item_id, filename, actual, size, checksum, phash, dims, 0, &crate::util::generate_slug(11)).await?;
    let allowed = mime::allowed_mimes(cfg);
    let (b_dir, t_dir, c_dir) = if ctx.manual {
        (cfg.paths.pending.join("b"), cfg.paths.pending.join("t"), cfg.paths.pending.join("ca"))
    } else {
        (cfg.paths.b.clone(), cfg.paths.t.clone(), cfg.paths.ca.clone())
    };
    let uid = ctx.session.id;

    for (i, sub) in raw_files.iter().enumerate().skip(1) {
        let sub_uuid = queue::genuuid();
        let sub_tmp = TempPath::new(cfg.paths.tmp.join(format!("{sub_uuid}.tmp")));
        media::move_file(&sub.path, &sub_tmp.path).await?;
        let detected = mime::detect(&sub_tmp.path).await.unwrap_or_default();
        let Some(sub_mime) = sub_mime(&allowed, &detected, &sub.content_type) else {
            tracing::warn!("[UPLOAD] Skipping album item #{} ({detected})", i + 1);
            continue;
        };
        let (sum, sub_phash, sub_dims) = analyse_sub(&sub_tmp.path, &sub_mime).await;
        let sum = sum?;
        let sub_ext = mime::ext_for(cfg, &sub_mime).unwrap_or_else(|| "webp".into());
        let sub_filename = format!("{sub_uuid}.{sub_ext}");
        let sub_dest = b_dir.join(&sub_filename);
        let sub_size = tokio::fs::metadata(&sub_tmp.path).await.map(|m| m.len() as i64).unwrap_or(sub.size as i64);
        media::move_file(&sub_tmp.path, &sub_dest).await?;
        media::make_public(&sub_dest).await;
        drop(sub_tmp);
        let sub_id = insert_album_item(st, item_id, &sub_filename, &sub_mime, sub_size, &sum, sub_phash, sub_dims, i as i64, &crate::util::generate_slug(11)).await?;

        let raw_sub_tags = form.joined(&format!("subf0ck_tags_{i}")).filter(|t| !t.is_empty()).or_else(|| form.joined(&format!("subtags_{i}")).filter(|t| !t.is_empty()));
        if let (Some(sub_id), Some(raw)) = (sub_id, &raw_sub_tags) {
            let parsed: Vec<&str> = raw
                .split([',', ';', '\n'])
                .map(str::trim)
                .filter(|t| !t.is_empty() && t.chars().count() <= 85 && !["sfw", "nsfw", "nsfl"].contains(&t.to_lowercase().as_str()))
                .collect();
            for t in parsed {
                let r = async {
                    if let Some(tag_id) = ensure_tag(st, t).await? {
                        sql!("INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", sub_id as i32, ", ", tag_id as i32, ", ", uid as i32, ") ON CONFLICT DO NOTHING")
                            .execute(&st.db)
                            .await?;
                        assign_tag(st, item_id, tag_id, uid).await?;
                    }
                    anyhow::Ok(())
                }
                .await;
                if let Err(e) = r {
                    tracing::warn!("[UPLOAD] Error assigning tag \"{t}\" to subf0ck {sub_id}: {e}");
                }
            }
        } else if let Some(sub_id) = sub_id {
            for t in &ctx.tags {
                if let Ok(Some(r)) = sql!("SELECT id FROM tags WHERE normalized = slugify(", t.clone(), ") LIMIT 1").fetch_optional(&st.db).await {
                    if let Some(tag_id) = r.i64("id") {
                        let _ = sql!("INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", sub_id as i32, ", ", tag_id as i32, ", ", uid as i32, ") ON CONFLICT DO NOTHING")
                            .execute(&st.db)
                            .await;
                    }
                }
            }
        }

        // Filmstrip thumbnail t/<subUuid>.webp
        let thumb = t_dir.join(format!("{sub_uuid}.webp"));
        let (sd, th) = (sub_dest.to_string_lossy().into_owned(), thumb.to_string_lossy().into_owned());
        let r: anyhow::Result<()> = async {
            if sub_mime.starts_with("image/") {
                crate::proc::spawn("magick", &[&format!("{sd}[0]"), "-resize", "256x256^", "-gravity", "center", "-crop", "256x256+0+0", "+repage", &th]).await?;
            } else if sub_mime.starts_with("video/") {
                if crate::proc::spawn("ffmpegthumbnailer", &["-i", &sd, "-s", "256", "-o", &th]).await.is_err() {
                    crate::proc::spawn("ffmpeg", &["-y", "-ss", "00:00:01", "-i", &sd, "-vframes", "1", "-vf", "scale=256:256:force_original_aspect_ratio=increase,crop=256:256", &th]).await?;
                }
            } else if sub_mime.starts_with("audio/") {
                let ca = c_dir.join(format!("{sub_uuid}.webp")).to_string_lossy().into_owned();
                let quiet = crate::proc::Opts { quiet: true, ..Default::default() };
                if crate::proc::spawn_with("ffmpeg", &["-y", "-i", &sd, "-an", "-vcodec", "webp", "-frames:v", "1", &ca], quiet).await.is_ok()
                    && tokio::fs::metadata(&ca).await.map(|m| m.len() > 0).unwrap_or(false)
                {
                    let _ = crate::proc::spawn("magick", &[&format!("{ca}[0]"), "-resize", "256x256^", "-gravity", "center", "-crop", "256x256+0+0", "+repage", &th]).await;
                }
            }
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::warn!("[UPLOAD] Failed to generate thumbnail for album item {sub_filename}: {e}");
        }
    }

    // YouTube links added to the album
    for (pos, (index, u)) in album_urls.iter().enumerate() {
        if url_only && pos == 0 && Some(u.as_str()) == input_url {
            continue; // the cover, already stored as the item itself
        }
        let Some(c) = YT_RE.captures(u) else { continue };
        let vid = &c[1];
        let sum = format!("yt_{vid}_{}_{index}", crate::util::now_ms());
        insert_album_item(st, item_id, &format!("yt:{vid}"), "video/youtube", 0, &sum, None, None, (raw_files.len() + pos) as i64, &crate::util::generate_slug(11)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_ids() {
        let id = |u: &str| YT_RE.captures(u).map(|c| c[1].to_string());
        assert_eq!(id("https://www.youtube.com/watch?v=dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id("https://youtu.be/dQw4w9WgXcQ?t=1").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id("https://www.youtube.com/embed/dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id("https://www.youtube.com/watch?feature=x&v=dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(id("https://vimeo.com/123456"), None);
    }

    #[test]
    fn auto_tags() {
        assert_eq!(auto_tags_from_url("https://m.youtube.com/watch?v=x"), vec!["youtube"]);
        assert_eq!(auto_tags_from_url("https://youtu.be/x"), vec!["youtube"]);
        assert!(auto_tags_from_url("https://example.co.uk/a.png").is_empty());
        assert!(auto_tags_from_url("not a url").is_empty());
    }

    #[test]
    fn remote_urls() {
        assert!(validate_remote_url("https://example.com/a.png").is_some());
        assert!(validate_remote_url("--exec=id").is_none());
        assert!(validate_remote_url("file:///etc/passwd").is_none());
        assert!(validate_remote_url("http://localhost:5454/").is_none());
        assert!(validate_remote_url("http://127.0.0.1/").is_none());
        assert!(validate_remote_url("http://10.1.2.3/").is_none());
        assert!(validate_remote_url("http://192.168.0.1/").is_none());
        assert!(validate_remote_url("http://[::1]/").is_none());
        assert!(validate_remote_url("http://[::ffff:127.0.0.1]/").is_none());
        assert!(validate_remote_url("http://169.254.169.254/latest").is_none());
        assert!(validate_remote_url("http://100.64.0.1/").is_none());
        assert!(validate_remote_url("ftp://example.com/").is_none());
    }

    #[test]
    fn sub_mime_decisions() {
        let allowed: Vec<String> = ["image/png", "image/jpeg", "audio/mpeg", "audio/aac"].iter().map(|s| s.to_string()).collect();
        assert_eq!(sub_mime(&allowed, "image/png", "image/jpeg").as_deref(), Some("image/png"));
        assert_eq!(sub_mime(&allowed, "audio/x-hx-aac-adts", "audio/aac").as_deref(), Some("audio/aac"));
        assert_eq!(sub_mime(&allowed, "text/html", "image/png"), None, "declared type can't override content");
        assert_eq!(sub_mime(&allowed, "image/svg+xml", "image/svg+xml"), None);
    }

    #[test]
    fn rating_ids() {
        let c = crate::media::test_cfg("nsfl_tag_id: 7");
        assert_eq!(rating_tag_id(&c, Some("sfw")), 1);
        assert_eq!(rating_tag_id(&c, Some("nsfl")), 7);
        assert_eq!(rating_tag_id(&c, None), 0);
    }
}
