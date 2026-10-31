//! `POST /api/v2/meta/extract-file` and `POST /api/v2/meta/strip-gps`
//! (`src/meta_extract_handler.mjs`, `src/meta_strip_handler.mjs`).

use super::{csrf_header_ok, declared_len, fail, send};
use crate::http::{Req, Res, raw};
use crate::media::multipart::{self, MpError};
use crate::media::{TempPath, mime, queue};
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderValue, header};
use serde_json::{Value, json};

const EXTRACT_MAX: u64 = 5 * 1024 * 1024;
const STRIP_MAX: u64 = 20 * 1024 * 1024;

/// Collects unique, cleaned metadata strings.
#[derive(Default)]
pub struct Results {
    pub items: Vec<String>,
    seen: std::collections::HashSet<String>,
}

impl Results {
    pub fn add(&mut self, v: &Value) {
        let s = match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Array(a) => {
                for x in a {
                    self.add(x);
                }
                return;
            }
            _ => return,
        };
        self.add_str(&s);
    }

    pub fn add_str(&mut self, s: &str) {
        static TAGS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<[^>]*>").unwrap());
        let clean: String = TAGS.replace_all(s, "").chars().filter(|c| !matches!(*c as u32, 0..=0x1f | 0x7f)).collect();
        let clean = clean.trim();
        let n = clean.encode_utf16().count();
        if n > 1 && n <= 255 && self.seen.insert(clean.to_lowercase()) {
            self.items.push(clean.to_string());
        }
    }
}

const TEXT_FIELDS: &[&str] = &[
    "Title", "Description", "Comment", "Artist", "Author", "Creator", "Copyright", "CopyrightNotice", "Rights", "Make", "Model", "LensModel", "Software",
    "ObjectName", "Headline", "Caption", "CaptionAbstract", "Subject", "Keywords", "By-line", "ByLine", "Credit", "Source",
];
const LOCATION_FIELDS: &[&str] = &[
    "City", "Sub-location", "Province-State", "Country-PrimaryLocationName", "Country", "State", "Location", "Sublocation", "CountryCode",
    "XMP-iptcExt:LocationName", "XMP-photoshop:City", "XMP-photoshop:Country",
];
const AV_KEYS: &[&str] = &["title", "TITLE", "comment", "COMMENT", "artist", "ARTIST", "album_artist", "ALBUM_ARTIST", "author", "AUTHOR", "genre", "GENRE", "description", "DESCRIPTION"];

fn coord(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        // parseFloat: leading number of the string
        Value::String(s) => {
            let t = s.trim();
            let end = t.char_indices().find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+')))).map(|(i, _)| i).unwrap_or(t.len());
            t[..end].parse().ok()
        }
        _ => None,
    }
}

/// Fields from exiftool's JSON for one file.
pub fn exif_fields(tags: &serde_json::Map<String, Value>, r: &mut Results) -> (bool, Option<(f64, f64)>) {
    for k in TEXT_FIELDS {
        if let Some(v) = tags.get(*k).filter(|v| crate::config::truthy(v)) {
            r.add(v);
        }
    }
    let mut has_text_location = false;
    for k in LOCATION_FIELDS {
        if let Some(v) = tags.get(*k).filter(|v| crate::config::truthy(v)) {
            r.add(v);
            has_text_location = true;
        }
    }
    let gps = match (tags.get("GPSLatitude").and_then(coord), tags.get("GPSLongitude").and_then(coord)) {
        (Some(lat), Some(lon)) => {
            r.add_str(&format!("{lat:.6},{lon:.6}"));
            Some((lat, lon))
        }
        _ => None,
    };
    (has_text_location, gps)
}

pub async fn handle_meta_extract(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        tracing::error!("[META-EXTRACT] CSRF mismatch for {}", session.user);
        return fail(403, "Invalid CSRF token");
    }
    let cfg = st.cfg();
    let ct = req.content_type().to_string();
    if !ct.contains("multipart/form-data") || multipart::boundary(&ct).is_none() {
        return fail(400, "Invalid content type");
    }
    let body = req.take_body().unwrap_or_else(Body::empty);
    let form = match multipart::parse_small(body, &ct, declared_len(&req), &cfg.paths.tmp, EXTRACT_MAX).await {
        Ok(f) => f,
        Err(MpError::TooLarge) => return fail(500, "Error extracting metadata"),
        Err(e) => {
            tracing::error!("[META-BYPASS-EXTRACT ERROR] {e}");
            return fail(500, "Error extracting metadata");
        }
    };
    let Some(file) = form.file("file") else { return fail(400, "No file chunk provided") };

    let magic = {
        use tokio::io::AsyncReadExt;
        let mut buf = [0u8; 12];
        let mut n = 0;
        if let Ok(mut f) = tokio::fs::File::open(&file.path).await {
            while n < 12 {
                match f.read(&mut buf[n..]).await {
                    Ok(0) | Err(_) => break,
                    Ok(k) => n += k,
                }
            }
        }
        buf[..n].to_vec()
    };
    let is_image = mime::sniff_image(&magic);
    let path = file.path.to_string_lossy().into_owned();
    let mut results = Results::default();

    if is_image {
        match crate::proc::spawn_with("exiftool", &["-json", "-charset", "utf8", "--", &path], crate::proc::Opts { quiet: true, ..Default::default() }).await {
            Ok(o) => {
                let parsed: Value = serde_json::from_slice(&o.stdout).unwrap_or(Value::Null);
                if let Some(tags) = parsed.get(0).and_then(Value::as_object) {
                    let (has_text_location, gps) = exif_fields(tags, &mut results);
                    if let (Some((lat, lon)), false, Some(socks)) = (gps, has_text_location, crate::media::socks(&cfg)) {
                        reverse_geocode(&socks, lat, lon, &mut results).await;
                    }
                }
            }
            Err(e) => tracing::warn!("[META-EXTRACT] exiftool failed, skipping EXIF: {e}"),
        }
    } else if let Some(meta) = queue::get_video_metadata(&file.path).await {
        for k in AV_KEYS {
            if let Some(Value::String(v)) = meta.get(*k) {
                if !v.is_empty() {
                    results.add_str(v);
                }
            }
        }
    }
    if cfg.truthy("websrv.debug") {
        tracing::info!("[META-EXTRACT] Result for {}: {} unique fields found", session.user, results.items.len());
    }
    static COORD: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^-?\d+\.\d+,-?\d+\.\d+$").unwrap());
    let has_gps = is_image && results.items.iter().any(|r| COORD.is_match(r));
    send(json!({ "success": true, "fields": results.items, "hasGpsData": has_gps }), 200)
}

/// City/state/country for GPS coordinates via Nominatim, through the configured SOCKS proxy only.
async fn reverse_geocode(socks: &str, lat: f64, lon: f64, results: &mut Results) {
    let proxy = if socks.to_lowercase().starts_with("socks5://") { format!("socks5h://{}", &socks[9..]) } else { socks.to_string() };
    let url = format!("https://nominatim.openstreetmap.org/reverse?lat={}&lon={}&format=json", crate::util::js_num(lat), crate::util::js_num(lon));
    let out = crate::proc::spawn_with(
        "curl",
        &["--proxy", &proxy, "-s", "--max-time", "10", "--user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36", &url],
        crate::proc::Opts { quiet: true, ..Default::default() },
    )
    .await;
    let Ok(o) = out else { return };
    let Ok(geo) = serde_json::from_slice::<Value>(&o.stdout) else { return };
    let a = &geo["address"];
    let place = ["city", "town", "village", "hamlet"].iter().map(|k| &a[*k]).find(|v| crate::config::truthy(v));
    for v in [place, Some(&a["state"]), Some(&a["country"])].into_iter().flatten() {
        if crate::config::truthy(v) {
            results.add(v);
        }
    }
}

/// `Content-Disposition` with an ASCII fallback and an RFC 5987 `filename*` for the real name.
pub fn attachment_header(name: &str) -> String {
    let clean = multipart::sanitize_filename(name);
    let clean = if clean.is_empty() { "image".to_string() } else { clean };
    let ascii: String = clean.chars().map(|c| if c.is_ascii() && c != '"' && c != '\\' { c } else { '_' }).collect();
    if ascii == clean {
        format!("attachment; filename=\"{clean}\"")
    } else {
        format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{}", crate::util::encode_uri_component(&clean))
    }
}

pub async fn handle_meta_strip(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        return fail(403, "Invalid CSRF token");
    }
    let cfg = st.cfg();
    let ct = req.content_type().to_string();
    if !ct.contains("multipart/form-data") || multipart::boundary(&ct).is_none() {
        return fail(400, "Invalid content type");
    }
    let body = req.take_body().unwrap_or_else(Body::empty);
    let form = match multipart::parse_small(body, &ct, declared_len(&req), &cfg.paths.tmp, STRIP_MAX).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[META-STRIP-GPS ERROR] {e}");
            return fail(500, "Failed to strip GPS data");
        }
    };
    let Some(file) = form.file("file") else { return fail(400, "No file provided") };

    // Work on a copy with a fixed name: exiftool writes `<name>_exiftool_tmp` next to it.
    let work = TempPath::new(cfg.paths.tmp.join(format!("strip_gps_{}", crate::util::random_hex(8))));
    if let Err(e) = tokio::fs::copy(&file.path, &work.path).await {
        tracing::error!("[META-STRIP-GPS ERROR] {e}");
        return fail(500, "Failed to strip GPS data");
    }
    let p = work.path.to_string_lossy().into_owned();
    let _ = crate::proc::spawn_with(
        "exiftool",
        &["-gps:all=", "-xmp:gps:all=", "-iptc:city=", "-iptc:province-state=", "-iptc:country-primary-location-name=", "-iptc:sub-location=", "-overwrite_original", "--", &p],
        crate::proc::Opts { quiet: true, ignore_exit_code: true, ..Default::default() },
    )
    .await;
    let data = match tokio::fs::read(&work.path).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("[META-STRIP-GPS ERROR] {e}");
            return fail(500, "Failed to strip GPS data");
        }
    };
    let mime_type = if file.content_type.is_empty() { "application/octet-stream" } else { file.content_type.as_str() };
    let len = data.len();
    let mut r = raw(200, data);
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime_type).unwrap_or(HeaderValue::from_static("application/octet-stream")));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    if let Ok(v) = HeaderValue::from_str(&attachment_header(&file.filename)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert("x-gps-stripped", HeaderValue::from_static("1"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_clean_and_dedupe() {
        let mut r = Results::default();
        r.add_str("<b>Canon</b>\u{1}");
        r.add_str("canon");
        r.add_str("x");
        r.add(&json!(["Alice", "Bob"]));
        r.add_str(&"y".repeat(300));
        assert_eq!(r.items, vec!["Canon", "Alice", "Bob"]);
    }

    #[test]
    fn exif_extraction() {
        let tags = json!({ "Make": "Canon", "Keywords": ["cat", "dog"], "GPSLatitude": 52.5, "GPSLongitude": "13.4 E", "City": "" });
        let mut r = Results::default();
        let (loc, gps) = exif_fields(tags.as_object().unwrap(), &mut r);
        assert!(!loc);
        assert_eq!(gps, Some((52.5, 13.4)));
        assert_eq!(r.items, vec!["Canon", "cat", "dog", "52.500000,13.400000"]);
    }

    #[test]
    fn disposition() {
        assert_eq!(attachment_header("a.jpg"), "attachment; filename=\"a.jpg\"");
        assert_eq!(attachment_header("x\"\r\n.jpg"), "attachment; filename=\"x_.jpg\"; filename*=UTF-8''x%22.jpg");
        assert_eq!(attachment_header("ä.png"), "attachment; filename=\"_.png\"; filename*=UTF-8''%C3%A4.png");
        assert_eq!(attachment_header(""), "attachment; filename=\"image\"");
    }
}
