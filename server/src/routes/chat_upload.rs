//! Temporary public file hosting for external chat clients (port of src/chat_upload_handler.mjs).
//!
//! Auth is a dedicated upload-only key from `upload_api_keys` (not a user API key), managed with
//! `f0ckm chat-upload-key …` (replaces scripts/chat-upload-key.mjs). Files live in `paths.cu`
//! and always expire.
//!
//! Routes (bypass routes like in index.mjs, exempt from the session CSRF check):
//!   GET    /api/chat/upload            — allowed types and limits, X-API-Key required
//!   POST   /api/chat/upload            — raw file as request body, X-API-Key required
//!   DELETE /api/chat/upload/:slug      — X-Delete-Token (returned by the upload) required
//!   GET    /cu/:slug[/:name]           — public download, Range supported
//!
//! Changes from the old handler: the per-key rate limit window is pruned (the old map kept every
//! key forever, harmless), and a body that ends early (client abort) is discarded instead of
//! being stored truncated.

use crate::db::RowExt;
use crate::http::{Req, Res, Router, json_raw, raw};
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use base64::Engine;
use futures::StreamExt;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/chat\/upload$", None, upload);
    r.pre(Method::GET, r"^\/api\/chat\/upload$", None, info);
    r.pre(Method::DELETE, r"^\/api\/chat\/upload\/(?<slug>[A-Za-z0-9_-]{12})$", None, delete);
    r.pre(Method::GET, r"^\/cu\/(?<slug>[A-Za-z0-9_-]{12})(?:\/[^/]*)?$", None, serve);
    r.pre(Method::HEAD, r"^\/cu\/(?<slug>[A-Za-z0-9_-]{12})(?:\/[^/]*)?$", None, serve);
}

// ── Config ──────────────────────────────────────────────────────────────────

fn is_enabled(st: &AppState) -> bool {
    st.cfg().enabled("websrv.chat_uploads")
}

/// `parseInt(x, 10) || dflt`
fn cfg_int(st: &AppState, path: &str, dflt: i64) -> i64 {
    let cfg = st.cfg();
    let v = cfg.get(path);
    let n = match v {
        Value::String(s) => crate::social::helpers::js_parse_int(s),
        other => crate::config::as_int(other),
    };
    n.filter(|n| *n != 0).unwrap_or(dflt)
}

fn max_bytes(st: &AppState) -> i64 {
    cfg_int(st, "websrv.chat_upload_max_bytes", 100 * 1024 * 1024)
}

fn default_expiry(st: &AppState) -> i64 {
    cfg_int(st, "websrv.chat_upload_expiry_days", 30) * 86400
}

fn max_expiry(st: &AppState) -> i64 {
    cfg_int(st, "websrv.chat_upload_max_expiry_days", 30) * 86400
}

fn rate_per_minute(st: &AppState) -> usize {
    cfg_int(st, "websrv.chat_upload_rate_per_minute", 30).max(0) as usize
}

fn upload_dir(st: &AppState) -> PathBuf {
    st.cfg().paths.cu.clone()
}

/// `websrv.chat_upload_mimes` (or `allowedMimes`) categories resolved against `cfg.mimes`.
pub fn allowed_mimes(st: &AppState) -> Vec<String> {
    let cfg = st.cfg();
    let list = |p: &str| cfg.get(p).as_array().map(|a| a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()).to_lowercase()).collect::<Vec<_>>());
    let src = list("websrv.chat_upload_mimes").or_else(|| list("allowedMimes"));
    let all: Vec<String> = cfg.mimes().keys().cloned().collect();
    crate::media::mime::allowed_from(&all, src)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

fn fail(code: u16, msg: &str) -> Res {
    send(json!({ "success": false, "msg": msg }), code)
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Sliding one-minute window per key id.
static RATE: Lazy<Mutex<HashMap<i64, Vec<Instant>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn rate_limited(st: &AppState, key_id: i64) -> bool {
    let now = Instant::now();
    let max = rate_per_minute(st);
    let mut map = RATE.lock();
    map.retain(|_, hits| hits.iter().any(|t| now.duration_since(*t) < Duration::from_secs(60)));
    let hits = map.entry(key_id).or_default();
    hits.retain(|t| now.duration_since(*t) < Duration::from_secs(60));
    let limited = hits.len() >= max;
    if !limited {
        hits.push(now);
    }
    limited
}

#[derive(Debug, Clone)]
pub struct UploadKey {
    pub id: i64,
    pub name: String,
    pub max_bytes: Option<i64>,
}

async fn resolve_key(st: &AppState, req: &Req) -> anyhow::Result<Option<UploadKey>> {
    let Some(key) = req.header("x-api-key").filter(|k| !k.is_empty() && k.len() <= 200) else { return Ok(None) };
    let row = sql!("SELECT id, name, max_bytes FROM upload_api_keys WHERE key_hash = ", crate::util::sha256(key), " AND revoked = false LIMIT 1")
        .fetch_optional(&st.db)
        .await?;
    Ok(row.map(|r| UploadKey { id: r.i64("id").unwrap_or(0), name: r.string("name"), max_bytes: r.i64("max_bytes").filter(|m| *m > 0) }))
}

/// Feature check + key lookup; `Err` is the response to send (404 / 401 / 500).
pub async fn auth_key(st: &AppState, req: &Req) -> Result<UploadKey, Res> {
    if !is_enabled(st) {
        return Err(fail(404, "Not found"));
    }
    match resolve_key(st, req).await {
        Ok(Some(k)) => Ok(k),
        Ok(None) => Err(fail(401, "Invalid API key")),
        Err(e) => {
            tracing::error!("[CHAT_UP] Key lookup error: {e}");
            Err(fail(500, "Server error"))
        }
    }
}

/// Lifetime in seconds from `X-Upload-Expiry`: seconds, or 30m / 1h / 24h / 7d / 2w; clamped.
pub fn parse_expiry(val: Option<&str>, default: i64, max: i64) -> i64 {
    static RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^(\d+)\s*([smhdw]?)$").unwrap());
    let Some(v) = val.map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty()) else { return default };
    let Some(c) = RE.captures(&v) else { return default };
    let mult: i64 = match &c[2] {
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => 1,
    };
    let secs = c[1].parse::<i64>().ok().and_then(|n| n.checked_mul(mult)).unwrap_or(i64::MAX);
    if secs == 0 {
        return default;
    }
    secs.clamp(60, max.max(60))
}

/// Media plays in the browser; everything else (pdf, archives, flash) is a download.
fn is_inline_mime(mime: &str) -> bool {
    (mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/")) && mime != "image/svg+xml"
}

/// `sanitizeName`: decoded, last path component, no control characters/quotes/slashes, ≤120 chars.
pub fn sanitize_name(raw: Option<&str>) -> String {
    let raw = raw.unwrap_or("");
    let name = crate::util::decode_uri_component(raw).unwrap_or_else(|| raw.to_string());
    let base = name.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !(c.is_control() || matches!(c, '"' | '\\' | '/'))).collect();
    let clean: String = clean.trim().chars().take(120).collect();
    if clean.is_empty() { "file".into() } else { clean }
}

fn key_limit(st: &AppState, key: &UploadKey) -> i64 {
    let m = max_bytes(st);
    key.max_bytes.map(|k| k.min(m)).unwrap_or(m)
}

fn site_base(st: &AppState) -> String {
    st.cfg().str_or("main.url.full", "").trim_end_matches('/').to_string()
}

// ── Info ────────────────────────────────────────────────────────────────────

async fn info(st: AppState, req: Req) -> Res {
    let key = match auth_key(&st, &req).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    let mimes = allowed_mimes(&st);
    let cfg = st.cfg();
    let all = cfg.mimes();
    let mut exts: Vec<String> = vec![];
    for m in &mimes {
        if let Some(e) = all.get(m).and_then(Value::as_str).filter(|e| !e.is_empty()) {
            if !exts.iter().any(|x| x == e) {
                exts.push(e.to_string());
            }
        }
    }
    send(
        json!({
            "success": true,
            "allowed_mimes": mimes,
            "allowed_extensions": exts,
            "max_bytes": key_limit(&st, &key),
            "default_expiry": default_expiry(&st),
            "max_expiry": max_expiry(&st),
        }),
        200,
    )
}

// ── Upload ──────────────────────────────────────────────────────────────────

enum WriteErr {
    TooLarge,
    Other(anyhow::Error),
}

/// Stream the request body into `path` (created new, 0600), at most `limit` bytes.
async fn write_body(body: Body, path: &PathBuf, limit: u64) -> Result<u64, WriteErr> {
    let mut opts = tokio::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    opts.mode(0o600);
    let mut out = opts.open(path).await.map_err(|e| WriteErr::Other(e.into()))?;
    let mut stream = body.into_data_stream();
    let mut size = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| WriteErr::Other(anyhow::anyhow!("body read failed: {e}")))?;
        size += chunk.len() as u64;
        if size > limit {
            return Err(WriteErr::TooLarge);
        }
        out.write_all(&chunk).await.map_err(|e| WriteErr::Other(e.into()))?;
    }
    out.flush().await.map_err(|e| WriteErr::Other(e.into()))?;
    Ok(size)
}

async fn upload(st: AppState, mut req: Req) -> Res {
    let key = match auth_key(&st, &req).await {
        Ok(k) => k,
        Err(r) => return r,
    };
    if rate_limited(&st, key.id) {
        return fail(429, "Rate limit exceeded");
    }
    let limit = key_limit(&st, &key);
    let declared = req.header("content-length").and_then(crate::social::helpers::js_parse_int).unwrap_or(0);
    if declared > limit {
        return send(json!({ "success": false, "msg": "File too large", "max_bytes": limit }), 413);
    }
    let allowed = allowed_mimes(&st);
    let hint: String = req.content_type().split(';').next().unwrap_or("").trim().to_lowercase().chars().take(100).collect();
    if !hint.is_empty() && hint != "application/octet-stream" && !allowed.contains(&hint) {
        return fail(415, &format!("File type not allowed: {hint}"));
    }

    let dir = upload_dir(&st);
    let _ = tokio::fs::create_dir_all(&dir).await;
    let slug = b64url(&crate::util::random_bytes(9));
    let path = dir.join(&slug);
    let name = sanitize_name(req.header("x-filename"));
    let body = req.take_body().unwrap_or_else(Body::empty);
    // Removed on every early return below.
    let guard = crate::media::TempPath::new(path.clone());

    let size = match write_body(body, &path, limit.max(0) as u64).await {
        Ok(s) => s,
        Err(WriteErr::TooLarge) => return send(json!({ "success": false, "msg": "File too large", "max_bytes": limit }), 413),
        Err(WriteErr::Other(e)) => {
            tracing::error!("[CHAT_UP] Write error: {e}");
            return fail(500, "Upload failed");
        }
    };
    if size == 0 {
        return fail(400, "Empty body");
    }
    let mime = crate::media::mime::detect(&path).await.unwrap_or_default();
    if !allowed.contains(&mime) {
        let shown = if mime.is_empty() { "unknown" } else { mime.as_str() };
        return fail(415, &format!("File type not allowed: {shown}"));
    }
    let lifetime = parse_expiry(req.header("x-upload-expiry"), default_expiry(&st), max_expiry(&st));
    let expires_at = crate::util::now_secs() + lifetime;
    let delete_token = b64url(&crate::util::random_bytes(24));

    let r: anyhow::Result<()> = async {
        sql!(
            "INSERT INTO chat_uploads (slug, key_id, original_name, mime, mime_hint, size_bytes, delete_token_hash, expires_at) VALUES (",
            slug.as_str(), ", ", key.id as i32, ", ", name.as_str(), ", ", mime.as_str(), ", ", hint.as_str(), ", ", size as i64, ", ",
            crate::util::sha256(&delete_token), ", to_timestamp(", expires_at, "))"
        )
        .execute(&st.db)
        .await?;
        sql!("UPDATE upload_api_keys SET last_used_at = now() WHERE id = ", key.id as i32).execute(&st.db).await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[CHAT_UP] DB error: {e}");
        return fail(500, "Server error");
    }
    guard.keep();
    tracing::info!("[CHAT_UP] {} uploaded {slug} ({mime}, {size} bytes) with key \"{}\"", key.id, key.name);
    send(
        json!({
            "success": true,
            "slug": slug,
            "url": format!("{}/cu/{slug}/{}", site_base(&st), crate::util::encode_uri_component(&name)),
            "name": name,
            "mime": mime,
            "inline": is_inline_mime(&mime),
            "size": size,
            "expires_at": expires_at,
            "delete_token": delete_token,
        }),
        200,
    )
}

// ── Delete ──────────────────────────────────────────────────────────────────

async fn delete(st: AppState, req: Req) -> Res {
    if !is_enabled(&st) {
        return fail(404, "Not found");
    }
    let Some(token) = req.header("x-delete-token").filter(|t| !t.is_empty()) else { return fail(401, "Delete token required") };
    let slug = req.param("slug").unwrap_or("").to_string();
    let rows = sql!("DELETE FROM chat_uploads WHERE slug = ", slug.as_str(), " AND delete_token_hash = ", crate::util::sha256(token), " RETURNING id")
        .fetch_all(&st.db)
        .await?;
    if rows.is_empty() {
        return fail(404, "Not found");
    }
    let _ = tokio::fs::remove_file(upload_dir(&st).join(&slug)).await;
    send(json!({ "success": true }), 200)
}

// ── Download ────────────────────────────────────────────────────────────────

fn not_found() -> Res {
    Ok(crate::http::reply(404, "text/plain", "404 - file not found."))
}

/// Byte range from a `Range` header (`bytes=a-b`, `bytes=a-`, `bytes=-n`); `Err(())` = 416.
fn parse_range(h: Option<&str>, size: u64) -> Result<Option<(u64, u64)>, ()> {
    static RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^bytes=(\d*)-(\d*)$").unwrap());
    let Some(c) = h.and_then(|h| RE.captures(h)) else { return Ok(None) };
    let (a, b) = (&c[1], &c[2]);
    if size == 0 || (a.is_empty() && b.is_empty()) {
        return Ok(None);
    }
    let num = |s: &str| s.parse::<u64>().unwrap_or(u64::MAX);
    let mut end = size - 1;
    let start = if a.is_empty() {
        size.saturating_sub(num(b))
    } else {
        if !b.is_empty() {
            end = num(b).min(end);
        }
        num(a)
    };
    if start > end || start >= size {
        return Err(());
    }
    Ok(Some((start, end)))
}

async fn serve(st: AppState, req: Req) -> Res {
    if !is_enabled(&st) {
        return not_found();
    }
    let slug = req.param("slug").unwrap_or("").to_string();
    let Some(row) = sql!(
        "SELECT original_name, mime, extract(epoch from expires_at)::bigint AS expires FROM chat_uploads WHERE slug = ", slug.as_str(),
        " AND expires_at > now() LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?
    else {
        return not_found();
    };
    let path = upload_dir(&st).join(&slug);
    let Ok(mut file) = tokio::fs::File::open(&path).await else { return not_found() };
    let size = match file.metadata().await {
        Ok(m) if m.is_file() => m.len(),
        _ => return not_found(),
    };
    let mime = row.string("mime");
    let inline = is_inline_mime(&mime);
    let ttl = (row.i64("expires").unwrap_or(0) - crate::util::now_secs()).max(0);

    let range = match parse_range(req.header("range"), size) {
        Ok(r) => r,
        Err(()) => {
            let mut r = raw(416, "");
            if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                r.headers_mut().insert(header::CONTENT_RANGE, v);
            }
            return Ok(r);
        }
    };
    let (start, end, code) = match range {
        Some((s, e)) => (s, e, 206),
        None => (0, size.saturating_sub(1), 200),
    };
    let len = if size == 0 { 0 } else { end - start + 1 };

    let body = if req.method == Method::HEAD || size == 0 {
        Body::empty()
    } else {
        if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return not_found();
        }
        use tokio::io::AsyncReadExt;
        Body::from_stream(tokio_util::io::ReaderStream::new(file.take(len)))
    };
    let mut r = raw(code, body);
    let h = r.headers_mut();
    let mut set = |k: &'static str, v: String| {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    };
    set("content-type", mime.clone());
    set("accept-ranges", "bytes".into());
    set("cache-control", format!("public, max-age={}", ttl.min(86400)));
    set(
        "content-disposition",
        format!("{}; filename*=UTF-8''{}", if inline { "inline" } else { "attachment" }, crate::util::encode_uri_component(&row.string("original_name"))),
    );
    set("x-content-type-options", "nosniff".into());
    set("content-security-policy", "default-src 'none'; sandbox".into());
    set("access-control-allow-origin", "*".into());
    set("access-control-expose-headers", "Content-Length, Content-Range, Content-Disposition".into());
    set("cross-origin-resource-policy", "cross-origin".into());
    if code == 206 {
        set("content-range", format!("bytes {start}-{end}/{size}"));
    }
    set("content-length", len.to_string());
    Ok(r)
}

// ── Expiry ──────────────────────────────────────────────────────────────────

pub async fn cleanup_expired(st: &AppState) {
    let r: anyhow::Result<usize> = async {
        let rows = sql!("SELECT id, slug FROM chat_uploads WHERE expires_at < now()").fetch_all(&st.db).await?;
        if rows.is_empty() {
            return Ok(0);
        }
        let dir = upload_dir(st);
        let mut ids: Vec<i64> = vec![];
        for r in &rows {
            let slug = r.string("slug");
            if crate::media::is_plain_name(&slug) {
                let _ = tokio::fs::remove_file(dir.join(&slug)).await;
            }
            ids.extend(r.i64("id"));
        }
        sql!("DELETE FROM chat_uploads WHERE id = ANY(", ids, ")").execute(&st.db).await?;
        Ok(rows.len())
    }
    .await;
    match r {
        Ok(0) => {}
        Ok(n) => tracing::info!("[CHAT_UP] Cleanup: removed {n} expired upload(s)"),
        Err(e) => tracing::error!("[CHAT_UP] Cleanup error: {e}"),
    }
}

/// At startup, then hourly (the old setInterval).
pub fn spawn_cleanup(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        let _ = tokio::fs::create_dir_all(upload_dir(&st)).await;
        loop {
            cleanup_expired(&st).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry() {
        let (d, m) = (30 * 86400, 30 * 86400);
        assert_eq!(parse_expiry(None, d, m), d);
        assert_eq!(parse_expiry(Some("1h"), d, m), 3600);
        assert_eq!(parse_expiry(Some("30M"), d, m), 1800);
        assert_eq!(parse_expiry(Some("5"), d, m), 60);
        assert_eq!(parse_expiry(Some("90d"), d, m), m);
        assert_eq!(parse_expiry(Some("0"), d, m), d);
        assert_eq!(parse_expiry(Some("abc"), d, m), d);
        assert_eq!(parse_expiry(Some("99999999999999999999w"), d, m), m);
    }

    #[test]
    fn names() {
        assert_eq!(sanitize_name(Some("a%2Fb%2Fc.png")), "c.png");
        assert_eq!(sanitize_name(Some("../../etc/passwd")), "passwd");
        assert_eq!(sanitize_name(Some("x\"y\\z\u{7}.txt")), "xyz.txt");
        assert_eq!(sanitize_name(Some("%E0%A4%A")), "%E0%A4%A");
        assert_eq!(sanitize_name(None), "file");
        assert_eq!(sanitize_name(Some("dir/")), "dir");
    }

    #[test]
    fn ranges() {
        assert_eq!(parse_range(None, 100), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-9"), 100), Ok(Some((0, 9))));
        assert_eq!(parse_range(Some("bytes=90-"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_range(Some("bytes=-10"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_range(Some("bytes=50-500"), 100), Ok(Some((50, 99))));
        assert_eq!(parse_range(Some("bytes=100-"), 100), Err(()));
        assert_eq!(parse_range(Some("bytes=-"), 100), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-1,5-6"), 100), Ok(None));
    }
}
