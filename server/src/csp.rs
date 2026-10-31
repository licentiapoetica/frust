//! Content-Security-Policy.
//!
//! Every response gets a strict policy: scripts run only from this origin or with the per-request
//! nonce (`csp_nonce` in templates), so injected `<script>` tags and inline handlers (onclick=…) never
//! run. Page markup binds events through public/s/js/csp-actions.js instead.
//!
//! The Flash player runs in a sandboxed iframe (`/flash-frame`) with its own, tighter policy: the
//! sandbox gives it an opaque origin, so requests made by an uploaded SWF carry no cookies.
//!
//! Config (`websrv.csp:`): `enabled` (default true), `report_only` (default false; log violations
//! without blocking), and extra sources per directive: `extra_img`, `extra_media`, `extra_connect`,
//! `extra_frame`, `extra_script` (lists of origins like `https://cdn.example.com`).

use crate::config::Config;
use crate::http::{Req, Res, Router, raw, reply};
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

pub const REPORT_PATH: &str = "/api/csp-report";

/// Random per-request nonce (hex, so it never needs quoting).
pub fn new_nonce() -> String {
    crate::util::random_hex(16)
}

fn extra(cfg: &Config, key: &str) -> Vec<String> {
    cfg.str_list(&format!("websrv.csp.{key}"))
        .unwrap_or_default()
        .into_iter()
        .filter(|s| is_source(s))
        .collect()
}

/// Accept only plain origins/host patterns, so config values can't inject directives.
fn is_source(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-._:/*".contains(c))
}

/// Origin of a configured media path when it points at another host (`https://cdn.x/b` -> `https://cdn.x`).
fn media_origins(cfg: &Config) -> Vec<String> {
    ["images", "thumbnails", "coverarts", "emojis", "memes"]
        .iter()
        .filter_map(|k| cfg.str(&format!("websrv.paths.{k}")))
        .filter(|p| p.starts_with("http://") || p.starts_with("https://"))
        .filter_map(|p| {
            let rest = p.split_once("://")?;
            let host = rest.1.split('/').next()?;
            Some(format!("{}://{host}", rest.0))
        })
        .filter(|s| is_source(s))
        .collect()
}

/// Hosts allowed for images in comments (`websrv.allowed_comment_images`), including subdomains
/// (tenor/giphy serve from media.*).
fn comment_image_hosts(cfg: &Config) -> Vec<String> {
    cfg.str_list("websrv.allowed_comment_images")
        .unwrap_or_default()
        .into_iter()
        .map(|h| h.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string())
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)))
        .flat_map(|h| [format!("https://{h}"), format!("https://*.{h}")])
        .collect()
}

/// The site-wide policy for one response.
pub fn policy(cfg: &Config, nonce: &str) -> String {
    let media = media_origins(cfg);
    let comment_imgs = comment_image_hosts(cfg);
    let join = |base: &[&str], more: &[&Vec<String>]| {
        let mut v: Vec<String> = base.iter().map(|s| s.to_string()).collect();
        for m in more {
            v.extend(m.iter().cloned());
        }
        v.join(" ")
    };
    let script = join(
        // YouTube's iframe API script (and the widget script it loads) come from www.youtube.com.
        // wasm-unsafe-eval: the Cap captcha solver is WebAssembly.
        &["'self'", &format!("'nonce-{nonce}'"), "'wasm-unsafe-eval'", "https://www.youtube.com"],
        &[&extra(cfg, "extra_script")],
    );
    let img = join(
        &["'self'", "data:", "blob:", "https://img.youtube.com", "https://i.ytimg.com", "https://i.4cdn.org"],
        &[&media, &comment_imgs, &extra(cfg, "extra_img")],
    );
    let media_src = join(&["'self'", "data:", "blob:", "https://i.4cdn.org"], &[&media, &comment_imgs, &extra(cfg, "extra_media")]);
    // YouTube oEmbed (titles of linked videos) is fetched from the browser, since the server may
    // reach YouTube only through Tor and hit its consent wall.
    let connect = join(&["'self'", "https://www.youtube.com"], &[&media, &extra(cfg, "extra_connect")]);
    let frame = join(
        &["'self'", "https://www.youtube.com", "https://www.youtube-nocookie.com", "https://vocaroo.com"],
        &[&extra(cfg, "extra_frame")],
    );
    let mut p = vec![
        "default-src 'self'".to_string(),
        format!("script-src {script}"),
        // Inline style attributes are used throughout the templates; only scripts are locked down.
        "style-src 'self' 'unsafe-inline'".to_string(),
        format!("img-src {img}"),
        format!("media-src {media_src}"),
        "font-src 'self' data:".to_string(),
        format!("connect-src {connect}"),
        format!("frame-src {frame}"),
        // The Cap widget starts its solver workers from blob: URLs
        "worker-src 'self' blob:".to_string(),
        "manifest-src 'self'".to_string(),
        "object-src 'none'".to_string(),
        "base-uri 'self'".to_string(),
        "form-action 'self'".to_string(),
        "frame-ancestors 'self'".to_string(),
        format!("report-uri {REPORT_PATH}"),
    ];
    if cfg.is_secure() {
        p.push("upgrade-insecure-requests".to_string());
    }
    p.join("; ")
}

/// Header name for the site policy, or None when disabled.
pub fn header_name(cfg: &Config) -> Option<&'static str> {
    if cfg.is_false("websrv.csp.enabled") {
        None
    } else if cfg.truthy("websrv.csp.report_only") {
        Some("content-security-policy-report-only")
    } else {
        Some("content-security-policy")
    }
}

/// Queue the site policy on the response (routes that set their own policy keep theirs).
pub fn apply(cfg: &Config, req: &Req) {
    if let Some(name) = header_name(cfg) {
        req.set_header(name, &policy(cfg, &req.csp_nonce));
    }
}

pub fn register(r: &mut Router) {
    r.post(r"^\/api\/csp-report\/?$", report);
    r.get(r"^\/flash-frame\/?$", flash_frame);
}

/// Violation reports from browsers. Logged (at most 60 a minute) so missed inline handlers show up.
async fn report(_st: AppState, mut req: Req) -> Res {
    static WINDOW: AtomicI64 = AtomicI64::new(0);
    static COUNT: AtomicU32 = AtomicU32::new(0);
    let now = crate::util::now_secs() / 60;
    if WINDOW.swap(now, Ordering::Relaxed) != now {
        COUNT.store(0, Ordering::Relaxed);
    }
    if COUNT.fetch_add(1, Ordering::Relaxed) < 60 {
        let body = req.body_bytes(16 * 1024).await.unwrap_or_default();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        // application/csp-report: {"csp-report": {...}}; Reporting API: [{"body": {...}}]
        let r = v.get("csp-report").cloned().or_else(|| v.get(0).and_then(|x| x.get("body")).cloned()).unwrap_or(v);
        let field = |a: &str, b: &str| {
            r.get(a).or_else(|| r.get(b)).map(|x| x.as_str().map(str::to_string).unwrap_or_else(|| x.to_string())).unwrap_or_default()
        };
        let clip = |s: String| s.chars().take(200).collect::<String>();
        tracing::warn!(
            "[CSP] {} blocked {} on {} ({}:{}) sample={:?}",
            clip(field("violated-directive", "effectiveDirective")),
            clip(field("blocked-uri", "blockedURL")),
            clip(field("document-uri", "documentURL")),
            clip(field("source-file", "sourceFile")),
            field("line-number", "lineNumber"),
            clip(field("script-sample", "sample")),
        );
    }
    Ok(raw(204, ""))
}

/// Policy of the Flash frame. `sandbox` without allow-same-origin gives the document an opaque
/// origin even when it is opened directly, so nothing in it can use the viewer's session. The SWF
/// itself may only talk to this origin (cookieless) and can't open windows or navigate the page.
const FLASH_FRAME_POLICY: &str = "sandbox allow-scripts allow-pointer-lock; default-src 'none'; \
     script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; \
     media-src 'self' data: blob:; font-src 'self' data:; connect-src 'self' data: blob:; \
     frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; \
     frame-ancestors 'self'";

async fn flash_frame(st: AppState, _req: Req) -> Res {
    if !st.cfg().truthy("websrv.enable_swf") {
        return Ok(reply(404, "text/plain", "Not Found"));
    }
    let html = st.tpl.render("flash-frame", &serde_json::json!({}));
    let mut r = reply(200, "text/html", html);
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(FLASH_FRAME_POLICY));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_cannot_inject_directives() {
        assert!(is_source("https://cdn.example.com"));
        assert!(!is_source("https://x; script-src *"));
        assert!(!is_source("'unsafe-inline'"));
    }
}
