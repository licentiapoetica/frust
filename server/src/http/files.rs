//! Serving files from disk with Range / conditional request support.

use super::{Req, reply};
use axum::body::Body;
use axum::http::{HeaderValue, Request, header};
use axum::response::Response;
use std::path::{Component, Path, PathBuf};
use tower::ServiceExt;

/// Join a URL-supplied relative path onto `dir`, refusing anything that could escape it.
pub fn safe_join(dir: &Path, rel: &str) -> Option<PathBuf> {
    let decoded = crate::util::decode_uri_component(rel)?;
    if decoded.contains('\0') {
        return None;
    }
    let p = Path::new(&decoded);
    let mut out = dir.to_path_buf();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

pub fn not_found() -> Response {
    reply(404, "text/html", "404 - file not found.")
}

/// Content type for a file name (falls back to octet-stream).
pub fn mime_for(path: &Path) -> String {
    mime_guess::from_path(path).first_raw().unwrap_or("application/octet-stream").to_string()
}

/// Serve one file (ranges, Last-Modified, If-Modified-Since). `content_type` overrides detection.
pub async fn serve_file(req: &Req, path: &Path, content_type: Option<&str>) -> Response {
    match tokio::fs::metadata(path).await {
        Ok(m) if m.is_file() => {}
        _ => return not_found(),
    }
    let mut b = Request::builder().method(axum::http::Method::GET).uri("/");
    for h in [header::RANGE, header::IF_MODIFIED_SINCE, header::IF_UNMODIFIED_SINCE, header::IF_RANGE, header::IF_NONE_MATCH, header::IF_MATCH] {
        if let Some(v) = req.headers.get(&h) {
            b = b.header(h, v);
        }
    }
    let r = b.body(Body::empty()).expect("valid request");
    let mut svc = tower_http::services::ServeFile::new(path);
    if let Some(ct) = content_type {
        if let Ok(v) = HeaderValue::from_str(ct) {
            svc = tower_http::services::ServeFile::new_with_mime(path, &v.to_str().unwrap_or("").parse().unwrap_or(mime_guess::mime::APPLICATION_OCTET_STREAM));
        }
    }
    match svc.oneshot(r).await {
        Ok(resp) => {
            let (parts, body) = resp.into_parts();
            let mut out = Response::from_parts(parts, Body::new(body));
            if req.method == axum::http::Method::HEAD {
                *out.body_mut() = Body::empty();
            }
            out
        }
        Err(_) => reply(500, "text/html", "500 - internal server error"),
    }
}

/// `router.static({ dir, route })`: serve `req.path` minus `prefix` from `dir`.
pub async fn serve_dir(req: &Req, dir: &Path, prefix: &str) -> Response {
    let rel = req.path.strip_prefix(prefix).unwrap_or("");
    let rel = if rel.is_empty() { "index.html" } else { rel };
    match safe_join(dir, rel) {
        Some(p) => serve_file(req, &p, None).await,
        None => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_traversal() {
        let d = Path::new("/srv/b");
        assert!(safe_join(d, "../etc/passwd").is_none());
        assert!(safe_join(d, "%2e%2e/etc/passwd").is_none());
        assert!(safe_join(d, "/etc/passwd").is_none());
        assert_eq!(safe_join(d, "a%20b.png").unwrap(), Path::new("/srv/b/a b.png"));
    }
}
