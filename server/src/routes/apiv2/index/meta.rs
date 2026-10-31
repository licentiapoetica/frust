//! Metadata endpoints of apiv2/index.mjs: `/meta/extract/item/:id`, `/meta/cache`,
//! `/audio-metadata`, `/meta/fetch`.

use super::j;
use crate::db::RowExt;
use crate::http::{Req, Res};
use crate::proc::{Opts, spawn_with};
use crate::social::helpers::js_str;
use crate::sql;
use crate::state::AppState;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_META_CACHE: usize = 2000;
const MAX_AUDIO_CACHE: usize = 1000;
const UA_124: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";
const UA_126: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";
const UA_120: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// url → metadata (insertion ordered; the oldest entry is dropped past the limit)
static META_CACHE: Lazy<Mutex<Map<String, Value>>> = Lazy::new(|| Mutex::new(Map::new()));
/// file name / `item_<id>` → audio-metadata response
static AUDIO_CACHE: Lazy<Mutex<Map<String, Value>>> = Lazy::new(|| Mutex::new(Map::new()));

fn cache_put(cache: &Mutex<Map<String, Value>>, key: &str, v: Value, max: usize) {
    let mut c = cache.lock();
    c.insert(key.to_string(), v);
    if c.len() > max {
        if let Some(first) = c.keys().next().cloned() {
            c.shift_remove(&first);
        }
    }
}

fn opts(timeout_secs: u64, ignore_exit: bool) -> Opts {
    Opts { timeout: Duration::from_secs(timeout_secs), ignore_exit_code: ignore_exit, quiet: true, ..Default::default() }
}

/// `cfg.main.socks` as `socks5h://` (DNS through the proxy), else the SSRF guard proxy.
fn socks_h(st: &AppState) -> Option<String> {
    crate::media::socks(&st.cfg())
        .map(|s| match s.strip_prefix("socks5://") {
            Some(rest) => format!("socks5h://{rest}"),
            None => s,
        })
        .or_else(crate::net_guard::proxy_url)
}

// ── GET /meta/extract/item/:id ──────────────────────────────────────────────

struct Fields {
    out: Vec<String>,
    seen: std::collections::HashSet<String>,
}

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());

impl Fields {
    fn add(&mut self, val: &str) {
        let v = val.trim();
        if v.is_empty() {
            return;
        }
        // strip HTML tags (e.g. <script> in EXIF data) and control characters
        let v = TAG_RE.replace_all(v, "");
        let v: String = v.trim().chars().filter(|c| !(*c as u32 <= 0x1f || *c as u32 == 0x7f)).collect();
        let v = v.trim();
        if v.is_empty() || !self.seen.insert(v.to_lowercase()) {
            return;
        }
        self.out.push(utf16_prefix(v, 255));
    }
}

fn value_text(v: &Value) -> String {
    js_str(Some(v)).unwrap_or_else(|| "null".into())
}

pub(super) async fn extract_item(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let id: i32 = match req.param("id").and_then(|i| i.parse().ok()) {
        Some(i) => i,
        None => return j(json!({ "success": false, "msg": "Item not found" }), 404),
    };
    let r: anyhow::Result<Res> = async {
        let Some(item) = sql!("SELECT dest, mime, original_filename FROM items WHERE id = ", id).fetch_optional(&st.db).await? else {
            return Ok(j(json!({ "success": false, "msg": "Item not found" }), 404));
        };
        let mime = item.string("mime");
        let is_media = (mime.starts_with("video/") || mime.starts_with("audio/") || mime.starts_with("image/")) && !mime.contains("flash") && !mime.contains("youtube");
        if !is_media {
            return Ok(j(json!({ "success": false, "msg": "Metadata extraction not available for this file type" }), 400));
        }
        let dest = item.string("dest");
        let full = if crate::media::is_plain_name(&dest) { cfg.paths.b.join(&dest) } else { PathBuf::from("/nonexistent") };
        let mut f = Fields { out: vec![], seen: Default::default() };
        if let Some(orig) = item.str("original_filename").filter(|o| !o.is_empty()) {
            let base = match orig.rfind('.') {
                Some(i) if i > 0 => &orig[..i],
                _ => orig,
            };
            f.add(base.trim());
        }
        if mime.starts_with("image/") {
            exif_fields(&st, &full, &mut f).await;
        } else {
            let meta = crate::media::queue::get_video_metadata(&full).await.unwrap_or_default();
            for k in ["title", "TITLE", "comment", "COMMENT", "artist", "ARTIST", "album_artist", "ALBUM_ARTIST", "author", "AUTHOR", "genre", "GENRE", "description", "DESCRIPTION"] {
                if let Some(Value::String(v)) = meta.get(k) {
                    f.add(v.trim());
                }
            }
        }
        Ok(j(json!({ "success": true, "fields": f.out }), 200))
    }
    .await;
    match r {
        Ok(res) => res,
        Err(e) => {
            tracing::error!("[API-V2-EXTRACT-RETR] Error: {e}");
            j(json!({ "success": false, "msg": "Extraction failed" }), 500)
        }
    }
}

async fn exif_fields(st: &AppState, full: &Path, f: &mut Fields) {
    let p = full.to_string_lossy().into_owned();
    let out = match spawn_with("exiftool", &["-j", "-s", "-n", "--", &p], opts(60, true)).await {
        Ok(o) => o.stdout_str(),
        Err(e) => {
            tracing::error!("[API-V2-EXTRACT-EXIF] exiftool error: {e}");
            return;
        }
    };
    if out.trim().is_empty() {
        return;
    }
    let tags = match serde_json::from_str::<Value>(&out) {
        Ok(Value::Array(a)) => a.into_iter().next().unwrap_or(Value::Null),
        Ok(_) => Value::Null,
        Err(e) => {
            tracing::error!("[API-V2-EXTRACT-EXIF] exiftool error: {e}");
            return;
        }
    };
    let truthy = crate::config::truthy;
    for field in [
        "Title", "ObjectName", "Headline", "Caption", "CaptionAbstract", "Description", "ImageDescription", "UserComment", "Comment", "Artist", "Creator",
        "By-line", "Credit", "Source", "Copyright", "CopyrightNotice", "Rights", "Keywords", "Subject", "Category", "SupplementalCategories", "Software",
        "Make", "Model", "LensModel",
    ] {
        match tags.get(field) {
            Some(Value::Array(a)) => a.iter().for_each(|v| f.add(&value_text(v))),
            Some(v) if truthy(v) => {
                let s = value_text(v);
                if field == "Keywords" || field == "Subject" {
                    s.split([',', ';']).for_each(|x| f.add(x.trim()));
                } else {
                    f.add(&s);
                }
            }
            _ => {}
        }
    }
    let mut has_text_location = false;
    for field in ["City", "State", "Province-State", "Country", "CountryCode", "Sub-location", "Location"] {
        if let Some(v) = tags.get(field).filter(|v| truthy(v)) {
            let s = value_text(v);
            if !s.trim().is_empty() {
                f.add(&s);
                has_text_location = true;
            }
        }
    }
    let num = |k: &str| match tags.get(k) {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => crate::social::helpers::js_parse_float(s),
        _ => None,
    };
    let (Some(lat), Some(lon)) = (num("GPSLatitude"), num("GPSLongitude")) else { return };
    if !lat.is_finite() || !lon.is_finite() {
        return;
    }
    f.add(&format!("{lat:.5},{lon:.5}"));
    if has_text_location {
        return;
    }
    // reverse geocode (best effort) through the proxy, DNS included
    let url = format!(
        "https://nominatim.openstreetmap.org/reverse?lat={}&lon={}&format=json&zoom=10&addressdetails=1",
        crate::util::js_num(lat),
        crate::util::js_num(lon)
    );
    let proxy = socks_h(st);
    let mut args: Vec<&str> = vec![];
    if let Some(p) = &proxy {
        args.extend(["--proxy", p.as_str()]);
    }
    args.extend(["-s", "--max-time", "10", "--user-agent", UA_124, &url]);
    let geo = match spawn_with("curl", &args, opts(20, false)).await {
        Ok(o) => o.stdout_str(),
        Err(e) => {
            if st.cfg().truthy("websrv.debug") {
                tracing::warn!("[META-EXTRACT] Reverse geocode failed: {e}");
            }
            return;
        }
    };
    let Ok(geo) = serde_json::from_str::<Value>(geo.trim()) else { return };
    let addr = &geo["address"];
    let s = |k: &str| addr.get(k).filter(|v| truthy(v)).map(value_text);
    if let Some(city) = s("city").or_else(|| s("town")).or_else(|| s("village")).or_else(|| s("municipality")) {
        f.add(&city);
    }
    for k in ["state", "country"] {
        if let Some(v) = s(k) {
            f.add(&v);
        }
    }
    if let Some(cc) = s("country_code") {
        f.add(&cc.to_uppercase());
    }
}

// ── POST /meta/cache ────────────────────────────────────────────────────────

static YT_HOST_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(youtube\.com|youtu\.be)").unwrap());

use crate::util::utf16_prefix;

async fn persist(st: &AppState, url: &str, meta: &Value) {
    if let Err(e) = sql!(
        "INSERT INTO meta_cache (url, data) VALUES (",
        url,
        ", ",
        meta.clone(),
        ") ON CONFLICT (url) DO UPDATE SET data = EXCLUDED.data, created_at = CURRENT_TIMESTAMP"
    )
    .execute(&st.db)
    .await
    {
        tracing::error!("[META-CACHE] DB save failed: {e}");
    }
}

pub(super) async fn cache(st: AppState, mut req: Req) -> Res {
    if !st.cfg().truthy("websrv.web_meta_extraction") {
        return j(json!({ "success": false, "msg": "Metadata extraction is disabled" }), 403);
    }
    let body = req.post_value().await;
    let url = body.get("url").filter(|v| crate::config::truthy(v));
    let meta = body.get("meta").filter(|v| crate::config::truthy(v));
    let title = meta.and_then(|m| m.get("title")).filter(|v| crate::config::truthy(v));
    let (Some(url), Some(meta), Some(title)) = (url, meta, title) else {
        return j(json!({ "success": false, "msg": "url and meta.title required" }), 400);
    };
    let url = value_text(url);
    if !YT_HOST_RE.is_match(&url) {
        return j(json!({ "success": false, "msg": "Only YouTube URLs accepted" }), 400);
    }
    // keep the shared cache table from being stuffed with huge keys
    if url.len() > 2048 {
        return j(json!({ "success": false, "msg": "Invalid request body" }), 400);
    }
    let author = meta.get("author").filter(|v| crate::config::truthy(v)).map(|a| utf16_prefix(&value_text(a), 200)).unwrap_or_else(|| "Unknown".into());
    let safe = json!({ "title": utf16_prefix(&value_text(title), 500), "site_name": "youtube.com", "author": author });
    cache_put(&META_CACHE, &url, safe.clone(), MAX_META_CACHE);
    persist(&st, &url, &safe).await;
    j(json!({ "success": true }), 200)
}

// ── GET /audio-metadata ─────────────────────────────────────────────────────

/// Private (or unavailable) media only for its owner and staff — the old endpoint read the tags of
/// any file.
fn may_see(st: &AppState, req: &Req, path: &str) -> bool {
    let Some(hit) = crate::private_items::from_path(st, path) else { return true };
    let Some(s) = req.session() else { return false };
    s.admin || s.is_moderator || crate::settings::session_owner_name(Some(s)).map(|o| o.to_lowercase() == hit.owner).unwrap_or(false)
}

pub(super) async fn audio_metadata(st: AppState, req: Req) -> Res {
    let r = audio_metadata_inner(&st, &req).await;
    match r {
        Ok(res) => res,
        Err(e) => {
            tracing::error!("[API-V2-AUDIO-METADATA] Error: {e}");
            j(json!({ "success": false, "msg": "Failed to extract audio metadata" }), 500)
        }
    }
}

static ARTIST_TITLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(.+?)\s*[-–—_]\s*(.+)$").unwrap());
static TRACK_NO_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[0-9]+[\s._-]+").unwrap());

async fn audio_metadata_inner(st: &AppState, req: &Req) -> anyhow::Result<Res> {
    let cfg = st.cfg();
    let raw_src = req.query("src").filter(|s| !s.is_empty()).or_else(|| req.query("file")).unwrap_or("");
    let item_id = req.query("id").unwrap_or("");
    if raw_src.is_empty() && item_id.is_empty() {
        return Ok(j(json!({ "success": false, "msg": "Missing src or id" }), 400));
    }
    let filename = if raw_src.is_empty() {
        String::new()
    } else {
        let p = raw_src.split('?').next().unwrap_or("");
        p.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
    };
    if !filename.is_empty() && (filename.contains('/') || filename.contains('\\') || filename.contains("..") || filename.contains('\0')) {
        return Ok(j(json!({ "success": false, "msg": "Invalid filename" }), 400));
    }
    let not_found = || Ok(j(json!({ "success": false, "msg": "File not found" }), 404));
    let numeric = !item_id.is_empty() && item_id.bytes().all(|b| b.is_ascii_digit());
    if (!filename.is_empty() && !may_see(st, req, &format!("/b/{filename}"))) || (numeric && !may_see(st, req, &format!("/t/{item_id}"))) {
        return not_found();
    }
    let cache_key = if filename.is_empty() { format!("item_{item_id}") } else { filename.clone() };
    if let Some(v) = AUDIO_CACHE.lock().get(&cache_key).cloned() {
        return Ok(j(v, 200));
    }

    let mut full: Option<PathBuf> = (!filename.is_empty()).then(|| cfg.paths.b.join(&filename));
    let mut original = String::new();
    if !item_id.is_empty() {
        let row = if numeric {
            match item_id.parse::<i32>() {
                Ok(n) => sql!("SELECT dest, original_filename, title FROM items WHERE id = ", n).fetch_optional(&st.db).await?,
                Err(_) => None,
            }
        } else {
            sql!("SELECT dest, original_filename, title FROM items WHERE slug = ", item_id).fetch_optional(&st.db).await?
        };
        match row {
            Some(r) => {
                let dest = r.string("dest");
                if !may_see(st, req, &format!("/b/{dest}")) {
                    return not_found();
                }
                if full.is_none() && crate::media::is_plain_name(&dest) {
                    full = Some(cfg.paths.b.join(&dest));
                }
                original = r.string("original_filename");
            }
            None => {
                let sub = if numeric {
                    match item_id.parse::<i32>() {
                        Ok(n) => sql!("SELECT dest FROM album_items WHERE id = ", n).fetch_optional(&st.db).await?,
                        Err(_) => None,
                    }
                } else {
                    sql!("SELECT dest FROM album_items WHERE slug = ", item_id).fetch_optional(&st.db).await?
                };
                if let Some(r) = sub {
                    let dest = r.string("dest");
                    if full.is_none() && crate::media::is_plain_name(&dest) {
                        full = Some(cfg.paths.b.join(&dest));
                    }
                }
            }
        }
    }
    let Some(full) = full else { return not_found() };
    if tokio::fs::metadata(&full).await.is_err() {
        return Ok(j(json!({ "success": false, "msg": "File not found on disk" }), 404));
    }

    let tags = crate::media::queue::get_video_metadata(&full).await.unwrap_or_default();
    let mut lower: Map<String, Value> = Map::new();
    for (k, v) in &tags {
        if v.is_null() || v.as_str() == Some("") {
            continue;
        }
        let key: String = k.to_lowercase().chars().filter(|c| !matches!(c, '-' | '_') && !c.is_whitespace()).collect();
        lower.insert(key, json!(value_text(v).trim()));
    }
    let pick = |keys: &[&str]| keys.iter().find_map(|k| lower.get(*k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string)).unwrap_or_default();
    let mut title = pick(&["title", "tracktitle", "song", "songname", "name"]);
    let mut artist = pick(&["artist", "albumartist", "author", "performer", "creator", "composer", "byline"]);
    let album = pick(&["album", "albumtitle"]);
    let genre = pick(&["genre"]);
    let date = pick(&["date", "year"]);
    if title.is_empty() && artist.is_empty() {
        let name = if original.is_empty() {
            full.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            original.clone()
        };
        if let Some(c) = ARTIST_TITLE_RE.captures(&name) {
            artist = TRACK_NO_RE.replace(&c[1], "").trim().to_string();
            title = c[2].trim().to_string();
        } else if !name.is_empty() && !name.starts_with("subf0ck") && name.encode_utf16().count() < 60 {
            title = name.trim().to_string();
        }
    }
    let result = json!({
        "success": true,
        "artist": artist.trim(),
        "title": title.trim(),
        "album": album.trim(),
        "genre": genre.trim(),
        "date": date.trim(),
        "has_meta": !artist.is_empty() || !title.is_empty(),
    });
    cache_put(&AUDIO_CACHE, &cache_key, result.clone(), MAX_AUDIO_CACHE);
    Ok(j(result, 200))
}

// ── GET /meta/fetch ─────────────────────────────────────────────────────────

static MEDIA_URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\.(mp4|webm|mp3|ogg|opus|flac|m4a|mkv|jpg|jpeg|png|gif|webp|swf)$").unwrap());
static LARGE_SITE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(instagram\.com|reels|twitter\.com|x\.com)").unwrap());
const REDDIT_WALL: &str = "Reddit - Please wait for verification";

async fn set_cache(st: &AppState, url: &str, meta: &Value) {
    if !meta.get("title").map(crate::config::truthy).unwrap_or(false) {
        return;
    }
    cache_put(&META_CACHE, url, meta.clone(), MAX_META_CACHE);
    persist(st, url, meta).await;
}

fn lines(out: &str) -> Vec<String> {
    out.trim().split('\n').map(|l| l.trim().to_string()).collect()
}

pub(super) async fn fetch(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !cfg.truthy("websrv.web_meta_extraction") {
        return j(json!({ "success": false, "msg": "Metadata extraction is disabled" }), 403);
    }
    let Some(url) = req.query("url").filter(|u| !u.is_empty()).map(str::to_string) else {
        return j(json!({ "success": false, "msg": "URL required" }), 400);
    };
    if let Some(m) = META_CACHE.lock().get(&url).cloned() {
        return j(json!({ "success": true, "meta": m }), 200);
    }
    match sql!("SELECT data FROM meta_cache WHERE url = ", url.as_str(), " LIMIT 1").fetch_optional(&st.db).await {
        Ok(Some(r)) => {
            let meta = r.val("data").clone();
            cache_put(&META_CACHE, &url, meta.clone(), MAX_META_CACHE);
            return j(json!({ "success": true, "meta": meta }), 200);
        }
        Ok(None) => {}
        Err(e) => tracing::error!("[META-CACHE] DB lookup failed: {e}"),
    }
    // Cache miss: only sessions may trigger outbound fetches (SSRF)
    if req.session().is_none() {
        return j(json!({ "success": false, "msg": "Not cached" }), 401);
    }
    if MEDIA_URL_RE.is_match(&url) {
        return j(json!({ "success": false, "msg": "Metadata extraction skipped for direct media URLs" }), 400);
    }
    // Only public http(s) targets, never option-like strings or local/private hosts (the old code
    // handed any string to yt-dlp and curl).
    if crate::routes::uploads::upload::validate_remote_url(&url).is_none() {
        return j(json!({ "success": false, "msg": "Invalid URL" }), 400);
    }
    let is_large = LARGE_SITE_RE.is_match(&url);
    let is_yt = YT_HOST_RE.is_match(&url);
    let socks = crate::media::proxy_url(&cfg);
    let socks_h = socks_h(&st);
    let js_runtime = crate::media::ytdlp_js_runtime(&cfg);
    let proxy: Vec<&str> = match &socks {
        Some(p) => vec!["--proxy", p.as_str()],
        None => vec![],
    };

    if is_yt {
        let oembed = format!("https://www.youtube.com/oembed?url={}&format=json", crate::util::encode_uri_component(&url));
        let mut a: Vec<&str> = vec!["-s", "--max-time", "15", "--user-agent", UA_126];
        if let Some(p) = &socks_h {
            a.extend(["--proxy", p.as_str()]);
        }
        a.push(&oembed);
        if let Ok(o) = spawn_with("curl", &a, opts(30, true)).await {
            let t = o.stdout_str();
            let t = t.trim();
            if !t.is_empty() {
                if !t.starts_with('{') && !t.starts_with('[') {
                    tracing::warn!("[META-FETCH] YouTube oEmbed returned non-JSON (likely HTML wall), falling back to yt-dlp");
                } else if let Ok(d) = serde_json::from_str::<Value>(t) {
                    if let Some(title) = d.get("title").filter(|v| crate::config::truthy(v)) {
                        let author = d.get("author_name").filter(|v| crate::config::truthy(v)).cloned().unwrap_or(json!("Unknown"));
                        let meta = json!({ "title": title, "site_name": "youtube.com", "author": author });
                        set_cache(&st, &url, &meta).await;
                        return j(json!({ "success": true, "meta": meta }), 200);
                    }
                }
            }
        }
        for attempt in 1..=2 {
            let mut a = proxy.clone();
            a.extend(["--quiet", "--no-warnings", "--js-runtimes", js_runtime.as_str(), "--print", "%(title)s", "--print", "%(uploader)s", "--skip-download", "--", &url]);
            match spawn_with("yt-dlp", &a, opts(120, false)).await {
                Ok(o) => {
                    let l = lines(&o.stdout_str());
                    let title = l.first().cloned().unwrap_or_default();
                    if !title.is_empty() {
                        let author = l.get(1).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Unknown".into());
                        let meta = json!({ "title": title, "site_name": "youtube.com", "author": author });
                        set_cache(&st, &url, &meta).await;
                        return j(json!({ "success": true, "meta": meta }), 200);
                    }
                }
                Err(e) => {
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    tracing::error!("[META-FETCH] YouTube yt-dlp fallback failed: {e}");
                }
            }
        }
        return j(json!({ "success": false, "msg": "Failed to extract YouTube metadata" }), 500);
    }

    // yt-dlp for Instagram, Twitter and other complex sites
    for attempt in 1..=3 {
        let mut a = proxy.clone();
        a.extend([
            "--quiet", "--no-warnings", "--js-runtimes", js_runtime.as_str(), "--geo-bypass", "--print", "%(title)s", "--print", "%(uploader)s", "--print",
            "%(webpage_url_domain)s", "--skip-download", "--", &url,
        ]);
        let r = match spawn_with("yt-dlp", &a, opts(120, false)).await {
            Ok(o) => {
                let l = lines(&o.stdout_str());
                let title = l.first().cloned().unwrap_or_default();
                if title.contains(REDDIT_WALL) { Err(anyhow::anyhow!("Reddit bot protection detected")) } else { Ok((title, l)) }
            }
            Err(e) => Err(e),
        };
        match r {
            Ok((title, l)) => {
                if !title.is_empty() {
                    let site = l.get(2).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Media Site".into());
                    let author = l.get(1).filter(|s| !s.is_empty()).cloned().unwrap_or_else(|| "Unknown".into());
                    let meta = json!({ "title": title, "site_name": site, "author": author });
                    set_cache(&st, &url, &meta).await;
                    return j(json!({ "success": true, "meta": meta }), 200);
                }
            }
            Err(e) => {
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                tracing::error!("[META-FETCH] yt-dlp failed after 3 attempts for {url}: {e}");
                if is_large {
                    return j(json!({ "success": false, "msg": "Failed to extract metadata via yt-dlp after 3 attempts" }), 500);
                }
            }
        }
    }

    // curl for smaller/generic sites
    for attempt in 1..=3 {
        let mut a = proxy.clone();
        a.extend([
            "-s", "-L", "--proto", "=http,https", "--proto-redir", "=http,https", "--max-time", "5", "--user-agent", UA_120, "--max-filesize", "2097152",
            "--fail", "--", &url,
        ]);
        match spawn_with("curl", &a, opts(30, true)).await {
            Ok(o) if o.code == Some(0) => {
                let html = o.stdout_str();
                if !html.is_empty() {
                    let meta = Value::Object(extract_meta(&html));
                    if meta.get("title").and_then(Value::as_str).is_some_and(|t| t.contains(REDDIT_WALL)) {
                        return j(json!({ "success": false, "msg": "Reddit bot protection encountered" }), 403);
                    }
                    set_cache(&st, &url, &meta).await;
                    return j(json!({ "success": true, "meta": meta }), 200);
                }
            }
            Ok(o) => {
                let code = o.code.unwrap_or(-1);
                if attempt < 3 && code != 63 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                tracing::error!("[META-FETCH FALLBACK ERROR] {url} (Attempt {attempt}): curl exited with {code}");
                if code == 63 {
                    return j(json!({ "success": false, "msg": "Page too large for metadata extraction" }), 400);
                }
            }
            Err(e) => {
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
                tracing::error!("[META-FETCH FALLBACK ERROR] {url} (Attempt {attempt}): {e}");
            }
        }
    }
    j(json!({ "success": false, "msg": "Failed to extract metadata from this URL after multiple attempts" }), 500)
}

// ── extractMeta(html) ───────────────────────────────────────────────────────

static TITLE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<title[^>]*>(.*?)</title>").unwrap());
static LANG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)<html[^>]+lang=["']([^"']+)["']"#).unwrap());
static LD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)<script[^>]+type=["']application/ld\+json["'][^>]*>(.*?)</script>"#).unwrap());
static HTML_COMMENT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static SPLIT_KW_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[,;]+").unwrap());

fn split_keywords(s: &str) -> Vec<Value> {
    SPLIT_KW_RE.split(s).map(str::trim).filter(|k| !k.is_empty()).map(|k| json!(k)).collect()
}

/// Every `property|name="<val>" content="..."` (either attribute order) for repeated tags.
fn all_props(html: &str, val: &str, both_orders: bool) -> Vec<Value> {
    let v = regex::escape(val);
    let a = Regex::new(&format!(r#"(?i)property=["']{v}["']\s+content=["']([^"']*)["']"#)).unwrap();
    let mut out: Vec<Value> = a.captures_iter(html).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty()).map(Value::String).collect();
    if both_orders {
        let b = Regex::new(&format!(r#"(?i)content=["']([^"']*)["']\s+property=["']{v}["']"#)).unwrap();
        out.extend(b.captures_iter(html).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty()).map(Value::String));
    }
    out
}

pub(crate) fn extract_meta(html: &str) -> Map<String, Value> {
    let clean = html.replace("\r\n", " ").replace(['\n', '\r'], " ");
    let get = |attr: &str, val: &str| -> Option<String> {
        let v = regex::escape(val);
        let r1 = Regex::new(&format!(r#"(?i)<meta\s+{attr}=["']{v}["']\s+content=["']([^"']*)["']"#)).ok()?;
        let r2 = Regex::new(&format!(r#"(?i)<meta\s+content=["']([^"']*)["']\s+{attr}=["']{v}["']"#)).ok()?;
        let c = r1.captures(&clean).or_else(|| r2.captures(&clean))?;
        Some(c[1].trim().to_string()).filter(|s| !s.is_empty())
    };
    let prop = |k: &str| get("property", k);
    let named = |k: &str| get("name", k);
    let opt = |v: Option<String>| v.map(Value::String).unwrap_or(Value::Null);
    let mut m = Map::new();
    if let Some(c) = TITLE_RE.captures(&clean) {
        m.insert("title".into(), json!(c[1].trim()));
    }
    if let Some(d) = named("description") {
        m.insert("description".into(), json!(d));
    }
    if let Some(k) = named("keywords") {
        m.insert("keywords".into(), Value::Array(split_keywords(&k)));
    }
    if let Some(a) = named("author") {
        m.insert("author".into(), json!(a));
    }
    if let Some(l) = LANG_RE.captures(&clean).map(|c| c[1].trim().to_string()).filter(|l| !l.is_empty()) {
        m.insert("language".into(), json!(l));
    }
    m.insert("og_title".into(), opt(prop("og:title")));
    m.insert("site_name".into(), opt(prop("og:site_name")));
    m.insert("og_desc".into(), opt(prop("og:description")));
    m.insert("og_type".into(), opt(prop("og:type")));
    m.insert("og_locale".into(), opt(prop("og:locale")));
    m.insert("article_tags".into(), Value::Array(all_props(&clean, "og:article:tag", true)));
    m.insert("article_section".into(), opt(prop("og:article:section")));
    m.insert("article_author".into(), opt(prop("og:article:author").or_else(|| prop("article:author"))));
    m.insert("music_album".into(), opt(prop("og:music:album")));
    m.insert("music_musician".into(), opt(prop("og:music:musician")));
    m.insert("music_song".into(), opt(prop("og:music:song")));
    m.insert("video_tags".into(), Value::Array(all_props(&clean, "og:video:tag", true)));
    m.insert("book_author".into(), opt(prop("og:book:author").or_else(|| prop("book:author"))));
    m.insert("book_tags".into(), Value::Array(all_props(&clean, "og:book:tag", false)));
    let first = prop("og:profile:first_name").or_else(|| prop("profile:first_name"));
    let last = prop("og:profile:last_name").or_else(|| prop("profile:last_name"));
    if first.is_some() || last.is_some() {
        m.insert("profile_name".into(), json!([first, last].into_iter().flatten().collect::<Vec<_>>().join(" ")));
    }
    m.insert("profile_username".into(), opt(prop("og:profile:username").or_else(|| prop("profile:username"))));
    m.insert("twitter_title".into(), opt(named("twitter:title")));
    m.insert("twitter_desc".into(), opt(named("twitter:description")));
    m.insert("twitter_creator".into(), opt(named("twitter:creator")));
    m.insert("twitter_site".into(), opt(named("twitter:site")));
    for i in 1..=4 {
        if let (Some(l), Some(d)) = (named(&format!("twitter:label{i}")), named(&format!("twitter:data{i}"))) {
            let e = m.entry("twitter_labels").or_insert_with(|| json!([]));
            if let Value::Array(a) = e {
                a.push(json!(format!("{l}: {d}")));
            }
        }
    }
    m.insert("price_amount".into(), opt(prop("product:price:amount").or_else(|| prop("og:price:amount"))));
    m.insert("price_currency".into(), opt(prop("product:price:currency").or_else(|| prop("og:price:currency"))));
    if let Some(k) = named("news_keywords") {
        m.insert("news_keywords".into(), Value::Array(split_keywords(&k)));
    }
    m.insert("category".into(), opt(named("category").or_else(|| prop("article:section"))));
    m.insert("genre".into(), opt(named("genre")));

    for c in LD_RE.captures_iter(&clean) {
        let text = HTML_COMMENT_RE.replace_all(&c[1], "");
        let Ok(ld) = serde_json::from_str::<Value>(&text) else { continue };
        let entries = match ld {
            Value::Array(a) => a,
            other => vec![other],
        };
        for e in entries {
            if e.is_null() {
                break; // JS: property access on null threw and ended this script block
            }
            json_ld_entry(&mut m, &e);
        }
    }
    m
}

fn json_ld_entry(m: &mut Map<String, Value>, e: &Value) {
    let t = crate::config::truthy;
    let has = |m: &Map<String, Value>, k: &str| m.get(k).map(t).unwrap_or(false);
    let field = |k: &str| e.get(k).filter(|v| t(v));
    let names = |v: &Value| -> Vec<String> {
        let list = match v {
            Value::Array(a) => a.clone(),
            other => vec![other.clone()],
        };
        list.iter()
            .map(|x| x.get("name").filter(|n| t(n)).cloned().unwrap_or_else(|| x.clone()))
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    };
    if let Some(v) = field("name").filter(|_| !has(m, "ld_name")) {
        m.insert("ld_name".into(), json!(value_text(v).trim()));
    }
    if let Some(v) = field("description").filter(|_| !has(m, "ld_desc")) {
        m.insert("ld_desc".into(), json!(value_text(v).trim()));
    }
    if let Some(v) = field("author") {
        let n: Vec<String> = names(v).into_iter().map(|s| s.trim().to_string()).collect();
        if !n.is_empty() && !has(m, "ld_author") {
            m.insert("ld_author".into(), json!(n.join(", ")));
        }
    }
    if let Some(v) = field("keywords").filter(|_| !has(m, "ld_keywords")) {
        let kw: Vec<Value> = match v {
            Value::String(s) => split_keywords(s),
            Value::Array(a) => a.iter().map(|x| json!(value_text(x))).collect(),
            _ => vec![],
        };
        if !kw.is_empty() {
            m.insert("ld_keywords".into(), Value::Array(kw));
        }
    }
    if let Some(v) = field("genre").filter(|_| !has(m, "ld_genre")) {
        m.insert("ld_genre".into(), json!(value_text(v).trim()));
    }
    if let Some(v) = field("headline").filter(|_| !has(m, "ld_headline")) {
        m.insert("ld_headline".into(), json!(value_text(v).trim()));
    }
    if let Some(v) = field("articleSection").filter(|_| !has(m, "category")) {
        m.insert("category".into(), json!(value_text(v).trim()));
    }
    if let Some(v) = field("creator").filter(|_| !has(m, "ld_creator")) {
        m.insert("ld_creator".into(), json!(names(v).join(", ")));
    }
    if let Some(v) = field("about").filter(|_| !has(m, "ld_about")) {
        let sub = v.get("name").filter(|n| t(n)).cloned().or_else(|| v.as_str().map(|s| json!(s)));
        if let Some(s) = sub.filter(|s| t(s)) {
            m.insert("ld_about".into(), json!(value_text(&s).trim()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_meta() {
        let html = r#"<html lang="de"><head><title> Hi
 there </title><meta name="description" content="desc"><meta property="og:title" content="OG">
<meta content="t1" property="og:article:tag"><meta property="og:article:tag" content="t2">
<meta name="keywords" content="a, b;;c"><meta name="twitter:label1" content="Runtime"><meta name="twitter:data1" content="90 min">
<script type="application/ld+json">{"name":"LD","author":[{"name":"A"},"B",{"x":1}],"keywords":["k",1],"articleSection":"Sec","creator":{"x":1}}</script></head></html>"#;
        let m = extract_meta(html);
        assert_eq!(m["title"], "Hi  there");
        assert_eq!(m["description"], "desc");
        assert_eq!(m["keywords"], json!(["a", "b", "c"]));
        assert_eq!(m["language"], "de");
        assert_eq!(m["og_title"], "OG");
        assert_eq!(m["article_tags"], json!(["t2", "t1"]));
        assert_eq!(m["twitter_labels"], json!(["Runtime: 90 min"]));
        assert_eq!(m["ld_name"], "LD");
        assert_eq!(m["ld_author"], "A, B");
        assert_eq!(m["ld_keywords"], json!(["k", "1"]));
        assert_eq!(m["category"], "Sec");
        assert_eq!(m["ld_creator"], "");
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        assert_eq!(&keys[..6], &["title", "description", "keywords", "language", "og_title", "site_name"]);
    }
}
