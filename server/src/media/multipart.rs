//! Multipart form parsing for uploads (`src/inc/multipart.mjs`).
//!
//! The old parser buffered the whole body in memory and split it by hand. This one streams the
//! body through `multer`: text fields are kept in memory (each capped), file fields are written
//! straight to temporary files in `paths.tmp`, and the whole stream is capped at the caller's limit.
//! Temporary files are removed when the [`Form`] is dropped, unless the handler moved them away.
//!
//! Field names and file names are read the way the old parser did: a trailing `[]` is stripped
//! from names, repeated names become lists, and `filename*=UTF-8''…` (RFC 5987) wins over
//! `filename="…"` (with backslash escapes) and unquoted `filename=…`.

use axum::body::Body;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone)]
pub struct FilePart {
    /// File name as sent by the client (decoded, not sanitized — see [`sanitize_filename`]).
    pub filename: String,
    /// The part's Content-Type header (`application/octet-stream` when missing).
    pub content_type: String,
    /// Temporary file holding the data.
    pub path: PathBuf,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    File(FilePart),
}

/// Parsed form: entries in the order they arrived.
#[derive(Debug, Default)]
pub struct Form {
    pub entries: Vec<(String, Part)>,
}

impl Drop for Form {
    fn drop(&mut self) {
        for (_, p) in &self.entries {
            if let Part::File(f) = p {
                let _ = std::fs::remove_file(&f.path);
            }
        }
    }
}

impl Form {
    fn values<'a, 'n>(&'a self, name: &'n str) -> impl Iterator<Item = &'a Part> + use<'a, 'n> {
        self.entries.iter().filter(move |(n, _)| n == name).map(|(_, p)| p)
    }

    pub fn has(&self, name: &str) -> bool {
        self.values(name).next().is_some()
    }

    /// `typeof parts[name] === 'string'`: exactly one value, and it is text.
    pub fn text(&self, name: &str) -> Option<&str> {
        let mut it = self.values(name);
        match (it.next(), it.next()) {
            (Some(Part::Text(s)), None) => Some(s.as_str()),
            _ => None,
        }
    }

    /// `String(parts[name])` for text values: repeated values joined by `,`.
    pub fn joined(&self, name: &str) -> Option<String> {
        let texts: Vec<&str> = self
            .values(name)
            .filter_map(|p| match p {
                Part::Text(s) => Some(s.as_str()),
                Part::File(_) => None,
            })
            .collect();
        if texts.is_empty() { None } else { Some(texts.join(",")) }
    }

    /// JS truthiness of `parts[name]` (empty string is falsy).
    pub fn truthy(&self, name: &str) -> bool {
        let mut it = self.values(name);
        match (it.next(), it.next()) {
            (None, _) => false,
            (Some(Part::Text(s)), None) => !s.is_empty(),
            _ => true,
        }
    }

    /// `parts[name] === true || parts[name] === 'true' || parts[name] === '1'`
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.text(name), Some("true") | Some("1"))
    }

    pub fn files(&self, name: &str) -> Vec<&FilePart> {
        self.values(name)
            .filter_map(|p| match p {
                Part::File(f) => Some(f),
                Part::Text(_) => None,
            })
            .collect()
    }

    /// A single file under `name` (`parts[name].data` exists).
    pub fn file(&self, name: &str) -> Option<&FilePart> {
        let mut it = self.values(name);
        match (it.next(), it.next()) {
            (Some(Part::File(f)), None) => Some(f),
            _ => None,
        }
    }

    /// Build a form from a JSON body (`parts = JSON.parse(body)`): scalars become text, arrays
    /// repeated values, `false`/`null` are dropped (they are falsy and never `=== 'true'`).
    pub fn from_json(v: &serde_json::Value) -> Form {
        use serde_json::Value;
        let mut f = Form::default();
        let Some(obj) = v.as_object() else { return f };
        fn scalar(v: &Value) -> Option<String> {
            match v {
                Value::String(s) => Some(s.clone()),
                Value::Bool(true) => Some("true".into()),
                Value::Number(n) => Some(n.to_string()),
                _ => None,
            }
        }
        for (k, val) in obj {
            match val {
                Value::Array(a) => {
                    for x in a {
                        if let Some(s) = scalar(x) {
                            f.entries.push((k.clone(), Part::Text(s)));
                        }
                    }
                }
                other => {
                    if let Some(s) = scalar(other) {
                        f.entries.push((k.clone(), Part::Text(s)));
                    }
                }
            }
        }
        f
    }

    /// `new URLSearchParams(body)`; the last value of a repeated key wins.
    pub fn from_urlencoded(body: &[u8]) -> Form {
        let mut f = Form::default();
        for (k, v) in form_urlencoded::parse(body) {
            f.entries.retain(|(n, _)| *n != k);
            f.entries.push((k.into_owned(), Part::Text(v.into_owned())));
        }
        f
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Whole request body.
    pub max_total: u64,
    /// One text field.
    pub max_text: usize,
    /// Number of parts.
    pub max_parts: usize,
}

impl Limits {
    pub fn total(max_total: u64) -> Limits {
        Limits { max_total, max_text: 1024 * 1024, max_parts: 1000 }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MpError {
    #[error("Request body too large")]
    TooLarge,
    #[error("invalid multipart body: {0}")]
    Invalid(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Boundary from the Content-Type header (`/boundary=(?:"([^"]+)"|([^;]+))/`).
pub fn boundary(content_type: &str) -> Option<String> {
    let i = content_type.find("boundary=")?;
    let rest = &content_type[i + "boundary=".len()..];
    if let Some(q) = rest.strip_prefix('"') {
        if let Some(end) = q.find('"') {
            if end > 0 {
                return Some(q[..end].to_string());
            }
        }
    }
    let end = rest.find(';').unwrap_or(rest.len());
    let b = &rest[..end];
    if b.is_empty() { None } else { Some(b.to_string()) }
}

/// `decodeURIComponent`, falling back to the raw value on malformed input.
fn decode_component(s: &str) -> String {
    crate::util::decode_uri_component(s).unwrap_or_else(|| s.to_string())
}

/// Field name from a Content-Disposition value (the `name` parameter, not part of `filename`).
pub fn disposition_name(h: &str) -> Option<String> {
    static RE: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r#"(?i)(?:^|[;\s])name\s*=\s*(?:"([^"]*)"|([^;\s]+))"#).unwrap());
    let c = RE.captures(h)?;
    let v = c.get(1).or_else(|| c.get(2))?.as_str();
    if v.is_empty() { None } else { Some(v.to_string()) }
}

/// File name from a Content-Disposition value, like the old parser: RFC 5987 `filename*`
/// first, then quoted `filename="…"` (backslash escapes removed), then unquoted.
pub fn disposition_filename(h: &str) -> Option<String> {
    static STAR: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)filename\*\s*=\s*utf-8''([^\r\n;]+)").unwrap());
    static QUOTED: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r#"filename="((?:[^"\\]|\\.)*)""#).unwrap());
    static UNESCAPE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"\\(.)").unwrap());
    static PLAIN: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"filename=([^\r\n;]+)").unwrap());
    if let Some(c) = STAR.captures(h) {
        return Some(decode_component(c[1].trim()));
    }
    if let Some(c) = QUOTED.captures(h) {
        return Some(UNESCAPE.replace_all(&c[1], "$1").into_owned());
    }
    PLAIN.captures(h).map(|c| c[1].trim().to_string())
}

/// Make a client-supplied file name safe to store and to echo in headers: last path component
/// only, no control characters, at most 255 characters.
pub fn sanitize_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base.chars().filter(|c| !c.is_control()).collect();
    let clean = clean.trim();
    let clean = if clean == "." || clean == ".." { "" } else { clean };
    clean.chars().take(255).collect()
}

/// Stream a multipart body: text fields into memory, files into `tmp_dir`.
pub async fn parse(body: Body, boundary: &str, declared_len: Option<u64>, tmp_dir: &Path, limits: Limits) -> Result<Form, MpError> {
    if declared_len.is_some_and(|l| l > limits.max_total) {
        return Err(MpError::TooLarge);
    }
    let constraints = multer::Constraints::new().size_limit(multer::SizeLimit::new().whole_stream(limits.max_total));
    let mut mp = multer::Multipart::with_constraints(body.into_data_stream(), boundary.to_string(), constraints);
    let mut form = Form::default();
    let map_err = |e: multer::Error| match e {
        multer::Error::StreamSizeExceeded { .. } | multer::Error::FieldSizeExceeded { .. } => MpError::TooLarge,
        other => MpError::Invalid(other.to_string()),
    };
    tokio::fs::create_dir_all(tmp_dir).await?;

    while let Some(mut field) = mp.next_field().await.map_err(map_err)? {
        if form.entries.len() >= limits.max_parts {
            return Err(MpError::Invalid("too many parts".into()));
        }
        let disposition = field.headers().get(axum::http::header::CONTENT_DISPOSITION).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned()).unwrap_or_default();
        let content_type = field
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "application/octet-stream".into());
        let Some(mut name) = disposition_name(&disposition) else {
            // Unnamed part: read and discard
            while field.chunk().await.map_err(map_err)?.is_some() {}
            continue;
        };
        if let Some(n) = name.strip_suffix("[]") {
            name = n.to_string();
        }
        match disposition_filename(&disposition) {
            Some(filename) => {
                let path = tmp_dir.join(format!("mp_{}.part", crate::util::random_hex(16)));
                // Register first so Drop cleans up even if writing fails half-way
                form.entries.push((name, Part::File(FilePart { filename, content_type, path: path.clone(), size: 0 })));
                let mut opts = tokio::fs::OpenOptions::new();
                opts.write(true).create_new(true);
                #[cfg(unix)]
                opts.mode(0o600);
                let mut out = opts.open(&path).await?;
                let mut size = 0u64;
                while let Some(chunk) = field.chunk().await.map_err(map_err)? {
                    size += chunk.len() as u64;
                    out.write_all(&chunk).await?;
                }
                out.flush().await?;
                if let Some((_, Part::File(f))) = form.entries.last_mut() {
                    f.size = size;
                }
            }
            None => {
                let mut buf: Vec<u8> = Vec::new();
                while let Some(chunk) = field.chunk().await.map_err(map_err)? {
                    if buf.len() + chunk.len() > limits.max_text {
                        return Err(MpError::TooLarge);
                    }
                    buf.extend_from_slice(&chunk);
                }
                let text = String::from_utf8_lossy(&buf).trim().to_string();
                form.entries.push((name, Part::Text(text)));
            }
        }
    }
    Ok(form)
}

/// Read a whole (small) multipart body into memory-backed parts — convenience for handlers that
/// only accept a few MB (meta extraction, GPS stripping, custom thumbnails).
pub async fn parse_small(body: Body, content_type: &str, declared_len: Option<u64>, tmp_dir: &Path, max: u64) -> Result<Form, MpError> {
    let b = boundary(content_type).ok_or_else(|| MpError::Invalid("missing boundary".into()))?;
    parse(body, &b, declared_len, tmp_dir, Limits { max_total: max, max_text: 64 * 1024, max_parts: 64 }).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(parts: &[(&str, &[u8])], boundary: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for (headers, data) in parts {
            out.extend_from_slice(format!("--{boundary}\r\n{headers}\r\n\r\n").as_bytes());
            out.extend_from_slice(data);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        out
    }

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("f0ckm_mp_test_{}", crate::util::random_hex(6)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn boundary_forms() {
        assert_eq!(boundary("multipart/form-data; boundary=abc").as_deref(), Some("abc"));
        assert_eq!(boundary("multipart/form-data; boundary=\"a b\"; x=y").as_deref(), Some("a b"));
        assert_eq!(boundary("multipart/form-data; boundary=abc; charset=utf-8").as_deref(), Some("abc"));
        assert_eq!(boundary("multipart/form-data"), None);
    }

    #[test]
    fn filenames() {
        assert_eq!(disposition_filename(r#"form-data; name="file"; filename="a.png""#).as_deref(), Some("a.png"));
        assert_eq!(disposition_filename(r#"form-data; name="file"; filename="a\"b.png""#).as_deref(), Some("a\"b.png"));
        assert_eq!(
            disposition_filename(r#"form-data; name="file"; filename="x.png"; filename*=UTF-8''%C3%A4%20b.png"#).as_deref(),
            Some("ä b.png")
        );
        assert_eq!(disposition_filename("form-data; name=file; filename=plain.gif").as_deref(), Some("plain.gif"));
        assert_eq!(disposition_filename(r#"form-data; name="file"; filename="""#).as_deref(), Some(""));
        assert_eq!(disposition_filename(r#"form-data; name="tags""#), None);
        // malformed percent-encoding falls back to the raw value
        assert_eq!(disposition_filename("form-data; filename*=utf-8''%E0%A4%A.png").as_deref(), Some("%E0%A4%A.png"));
    }

    #[test]
    fn names() {
        assert_eq!(disposition_name(r#"form-data; name="file"; filename="a""#).as_deref(), Some("file"));
        // `name=` inside `filename=` must not be mistaken for the field name
        assert_eq!(disposition_name(r#"form-data; filename="evil"; name="files[]""#).as_deref(), Some("files[]"));
        assert_eq!(disposition_name(r#"form-data; filename="x""#), None);
        assert_eq!(disposition_name("form-data; name=rating").as_deref(), Some("rating"));
    }

    #[test]
    fn sanitize() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("C:\\Users\\x\\pic.jpg"), "pic.jpg");
        assert_eq!(sanitize_filename("a\r\nb\0.png"), "ab.png");
        assert_eq!(sanitize_filename(".."), "");
        assert_eq!(sanitize_filename(&"x".repeat(400)).len(), 255);
    }

    #[tokio::test]
    async fn parses_fields_and_files() {
        let dir = tmpdir();
        let b = body(
            &[
                (r#"Content-Disposition: form-data; name="rating""#, b" sfw \r\n"),
                (r#"Content-Disposition: form-data; name="tags""#, b"a,b"),
                ("Content-Disposition: form-data; name=\"files[]\"; filename=\"one.png\"\r\nContent-Type: image/png", b"\x89PNG data"),
                ("Content-Disposition: form-data; name=\"files[]\"; filename=\"two.gif\"", b"GIF89a"),
                (r#"Content-Disposition: form-data; name="tags""#, b"c"),
            ],
            "XyZ",
        );
        let form = parse(Body::from(b), "XyZ", None, &dir, Limits::total(1 << 20)).await.unwrap();
        assert_eq!(form.text("rating"), Some("sfw"));
        assert_eq!(form.text("tags"), None, "repeated → not a string");
        assert_eq!(form.joined("tags").as_deref(), Some("a,b,c"));
        let files = form.files("files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].filename, "one.png");
        assert_eq!(files[0].content_type, "image/png");
        assert_eq!(files[1].content_type, "application/octet-stream");
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"\x89PNG data");
        assert_eq!(files[1].size, 6);
        let paths: Vec<PathBuf> = files.iter().map(|f| f.path.clone()).collect();
        drop(form);
        assert!(paths.iter().all(|p| !p.exists()), "temp files removed on drop");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn enforces_total_limit() {
        let dir = tmpdir();
        let b = body(&[("Content-Disposition: form-data; name=\"file\"; filename=\"big.bin\"", &vec![0u8; 4096])], "B");
        let r = parse(Body::from(b.clone()), "B", None, &dir, Limits::total(1024)).await;
        assert!(matches!(r, Err(MpError::TooLarge)));
        let r = parse(Body::from(b), "B", Some(1 << 30), &dir, Limits::total(1024)).await;
        assert!(matches!(r, Err(MpError::TooLarge)));
        // nothing left behind
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn enforces_text_limit() {
        let dir = tmpdir();
        let b = body(&[("Content-Disposition: form-data; name=\"comment\"", &vec![b'a'; 300])], "B");
        let r = parse(Body::from(b), "B", None, &dir, Limits { max_total: 1 << 20, max_text: 100, max_parts: 10 }).await;
        assert!(matches!(r, Err(MpError::TooLarge)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_and_urlencoded() {
        let f = Form::from_json(&serde_json::json!({ "url": "https://x", "is_oc": true, "is_shitpost": false, "visibility": 2, "tags": ["a", "b"] }));
        assert_eq!(f.text("url"), Some("https://x"));
        assert!(f.flag("is_oc"));
        assert!(!f.truthy("is_shitpost"));
        assert_eq!(f.joined("visibility").as_deref(), Some("2"));
        assert_eq!(f.joined("tags").as_deref(), Some("a,b"));
        let u = Form::from_urlencoded(b"a=1&b=x%20y&a=2");
        assert_eq!(u.text("a"), Some("2"));
        assert_eq!(u.text("b"), Some("x y"));
    }
}
