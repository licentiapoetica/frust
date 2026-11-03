//! Plain HTML forms for visitors without JavaScript.
//!
//! The JSON API endpoints the scripts call (favourite, comment, ...) also take a normal form post.
//! Such a form carries a `back` field (the page it sits on, plus an optional `#anchor`); the
//! request pipeline (app.rs `dispatch`) then answers with a redirect back there instead of the JSON
//! reply. A failure's message travels in a short-lived flash cookie that the next page shows
//! ([`take_flash`], `nojs_flash` in templates: snippets/navbar.html). The cookie is only ever set
//! by the server, so a link can't put its own text on the page.
//!
//! Also the browser-side preferences (blur, hidden ratings, ...) that live in localStorage with
//! JavaScript: without it the settings form stores them in a cookie ([`PREFS_COOKIE`]) and
//! header.html puts their classes on <html> (header.early.js drops them again when JavaScript runs).

use crate::http::{CookieExpiry, Req, cookie_options};
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use http_body_util::BodyExt;
use serde_json::{Value, json};

const FLASH_COOKIE: &str = "f0ck_flash";
/// Longest flash message kept (the API messages are short).
const FLASH_MAX: usize = 300;
/// Largest JSON reply read to find the message.
const REPLY_MAX: usize = 512 * 1024;

pub const PREFS_COOKIE: &str = "f0ck_nojs_prefs";
/// Preference key -> class on <html> (the same classes header.early.js sets from localStorage)
pub const PREF_CLASSES: [(&str, &str); 6] = [
    ("blur_sfw", "blur-sfw-active"),
    ("blur_nsfw", "blur-nsfw-active"),
    ("blur_nsfl", "blur-nsfl-active"),
    ("blur_untagged", "blur-untagged-active"),
    ("hide_item_ratings", "hide-item-ratings-active"),
    ("hide_infobox", "hide-infobox-active"),
];

/// `back` of a plain form post (no JavaScript): the page to return to. None for script requests,
/// other content types, or a `back` that isn't a path on this site.
pub async fn form_back(req: &mut Req) -> Option<String> {
    if req.method != Method::POST || crate::auth::is_ajax(req) {
        return None;
    }
    if !req.content_type().to_ascii_lowercase().starts_with("application/x-www-form-urlencoded") {
        return None;
    }
    let back = req.post_str("back").await?;
    safe_back(&back)
}

/// A local path with an optional `#anchor` of plain characters; anything else is refused.
pub fn safe_back(back: &str) -> Option<String> {
    let (path, anchor) = back.split_once('#').unwrap_or((back, ""));
    let local = path.starts_with('/') && !path.starts_with("//") && !path.starts_with("/\\");
    if !local || back.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    if !anchor.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(path.to_string());
    }
    Some(back.to_string())
}

/// Turn an endpoint's reply into a redirect to `back`. Redirects pass through; a JSON reply that
/// failed (`success: false` or an error status) leaves its message in the flash cookie
/// (`cookie_attrs`: [`flash_cookie_attrs`]).
pub async fn finish(cookie_attrs: &str, back: &str, resp: Response) -> Response {
    if resp.status().is_redirection() {
        return resp;
    }
    let status = resp.status();
    let bytes = match resp.into_body().collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => Default::default(),
    };
    let reply: Value = if bytes.len() <= REPLY_MAX { serde_json::from_slice(&bytes).unwrap_or(Value::Null) } else { Value::Null };
    let failed = status.is_client_error() || status.is_server_error() || reply.get("success") == Some(&Value::Bool(false));

    // A new comment: land on it
    let back = match reply.pointer("/comment/id").and_then(Value::as_i64) {
        Some(id) if !failed => format!("{}#c{id}", back.split('#').next().unwrap_or(back)),
        _ => back.to_string(),
    };
    let mut r = Response::new(Body::empty());
    *r.status_mut() = axum::http::StatusCode::SEE_OTHER;
    let location = HeaderValue::from_str(&back).unwrap_or_else(|_| HeaderValue::from_static("/"));
    r.headers_mut().insert(header::LOCATION, location);
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    let msg = ["msg", "message", "error"].iter().find_map(|k| reply.get(*k).and_then(Value::as_str)).filter(|m| !m.trim().is_empty());
    // Failures always say something; a success only when its reply has a message ("Report submitted")
    let flash = match (failed, msg) {
        (true, m) => Some(format!("e:{}", m.unwrap_or("Something went wrong. Please try again."))),
        (false, Some(m)) => Some(format!("s:{m}")),
        (false, None) => None,
    };
    if let Some(flash) = flash {
        let flash: String = flash.chars().take(FLASH_MAX).collect();
        let cookie = format!("{FLASH_COOKIE}={}; {cookie_attrs}", crate::util::encode_uri_component(&flash));
        if let Ok(v) = HeaderValue::from_str(&cookie) {
            r.headers_mut().append(header::SET_COOKIE, v);
        }
    }
    r
}

/// Attributes of the flash cookie: a minute is plenty for the redirect.
pub fn flash_cookie_attrs(cfg: &crate::config::Config) -> String {
    cookie_options(cfg, CookieExpiry::MaxAge(60), true)
}

/// The flash message left by [`finish`] for this page, if any; the cookie is cleared with this
/// response, so it shows once.
pub fn take_flash(cfg: &crate::config::Config, req: &Req) -> Option<Value> {
    let raw = req.cookie(FLASH_COOKIE).filter(|c| !c.is_empty())?;
    req.set_cookie(format!("{FLASH_COOKIE}=; {}", cookie_options(cfg, CookieExpiry::Expires(crate::http::EXPIRED), true)));
    let text = crate::util::decode_uri_component(raw).unwrap_or_else(|| raw.to_string());
    let (kind, text) = match text.split_once(':') {
        Some(("s", t)) => ("success", t),
        Some(("e", t)) => ("error", t),
        _ => ("error", text.as_str()),
    };
    let text: String = text.chars().take(FLASH_MAX).collect();
    Some(json!({ "kind": kind, "text": text }))
}

/// The no-JavaScript preferences of this browser ([`PREFS_COOKIE`]), only known keys.
pub fn prefs(req: &Req) -> Vec<&'static str> {
    let raw = req.cookie(PREFS_COOKIE).unwrap_or("");
    PREF_CLASSES.iter().map(|(k, _)| *k).filter(|k| raw.split('.').any(|p| p == *k)).collect()
}

/// Classes for <html> from [`prefs`].
pub fn pref_classes(prefs: &[&str]) -> String {
    PREF_CLASSES.iter().filter(|(k, _)| prefs.contains(k)).map(|(_, c)| *c).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_stays_local() {
        assert_eq!(safe_back("/3#comments-container").as_deref(), Some("/3#comments-container"));
        assert_eq!(safe_back("/user/x?page=2").as_deref(), Some("/user/x?page=2"));
        assert_eq!(safe_back("/3#<x>").as_deref(), Some("/3"));
        assert_eq!(safe_back("//evil.example/"), None);
        assert_eq!(safe_back("/\\evil.example/"), None);
        assert_eq!(safe_back("https://evil.example/"), None);
        assert_eq!(safe_back("/a\r\nSet-Cookie: x=1"), None);
    }

    #[test]
    fn pref_classes_known_keys_only() {
        assert_eq!(pref_classes(&["blur_nsfw", "hide_infobox"]), "blur-nsfw-active hide-infobox-active");
        assert_eq!(pref_classes(&[]), "");
    }

    #[tokio::test]
    async fn failed_reply_becomes_redirect_with_flash() {
        let attrs = "Path=/; SameSite=Lax; HttpOnly; Max-Age=60";
        let resp = crate::http::json_resp(&json!({ "success": false, "message": "Empty comment" }), 200);
        let r = finish(attrs, "/3#comments-container", resp).await;
        assert_eq!(r.status(), 303);
        assert_eq!(r.headers()[header::LOCATION], "/3#comments-container");
        let cookie = r.headers()[header::SET_COOKIE].to_str().unwrap().to_string();
        assert!(cookie.starts_with("f0ck_flash=e%3AEmpty%20comment;"));

        let ok = crate::http::json_resp(&json!({ "success": true }), 200);
        let r = finish(attrs, "/3", ok).await;
        assert_eq!(r.status(), 303);
        assert!(r.headers().get(header::SET_COOKIE).is_none());

        let reported = crate::http::json_resp(&json!({ "success": true, "msg": "Report submitted successfully." }), 200);
        let r = finish(attrs, "/3", reported).await;
        assert!(r.headers()[header::SET_COOKIE].to_str().unwrap().starts_with("f0ck_flash=s%3AReport%20submitted"));

        let posted = crate::http::json_resp(&json!({ "success": true, "comment": { "id": 42 } }), 200);
        let r = finish(attrs, "/3#comments-container", posted).await;
        assert_eq!(r.headers()[header::LOCATION], "/3#c42");
    }
}
