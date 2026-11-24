//! Maintenance mode (admin panel → Maintenance): while it is on, everyone but admins gets the
//! maintenance page (`views/maintenance.html`, HTTP 503) instead of the site. The page is a TV
//! "off air" card: a video (or a test card), a terminal with the admin's text and a headline, each
//! of which can run the glitch effect (`/s/js/glitchfx.js`).
//!
//! The whole setup is one JSON value in `site_settings` (key `maintenance`), held in
//! `SiteSettings::maintenance`. The video is a copy in `public/s/maintenance/` (an upload, or the
//! file of an item), so it plays while item media is locked.

use crate::http::{Req, json_raw, raw, with_header};
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// `site_settings` key and pg_notify channel.
pub const KEY: &str = "maintenance";

/// Parts of the page that can glitch: (key, label in the admin form).
pub const ELEMENTS: [(&str, &str); 5] =
    [("headline", "Headline"), ("text", "Terminal text"), ("video", "Video / test card"), ("logo", "Logo / site name"), ("page", "Whole page")];
/// Page layouts: (key, label, description in the admin form). The first one is the default.
pub const LAYOUTS: [(&str, &str, &str); 3] = [
    ("testcard", "Test card / terminal", "TV off air: video left, a terminal with the text right, headline in the bottom bar."),
    ("signal", "Signal lost", "The video fills the screen (dimmed), a huge headline and the text in the middle."),
    ("panel", "Boxed panel", "Like a page of the site: logo on top, one bordered box with headline, video and text."),
];
pub const STRENGTHS: [&str; 3] = ["low", "medium", "high"];
pub const MODES: [(&str, &str); 3] = [("always", "Always"), ("bursts", "Random bursts"), ("hover", "On hover")];

pub const MAX_TITLE: usize = 80;
pub const MAX_TEXT: usize = 4000;
/// Text lines shown in the terminal (the CSS reveals them one by one).
pub const MAX_LINES: usize = 40;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Glitch {
    pub on: bool,
    /// One of [`STRENGTHS`]
    pub strength: String,
    /// One of [`MODES`]
    pub mode: String,
}

impl Default for Glitch {
    fn default() -> Self {
        Glitch { on: false, strength: "medium".into(), mode: "always".into() }
    }
}

impl Glitch {
    /// Unknown strength / mode values fall back to the defaults.
    pub fn clean(mut self) -> Glitch {
        if !STRENGTHS.contains(&self.strength.as_str()) {
            self.strength = "medium".into();
        }
        if !MODES.iter().any(|(m, _)| *m == self.mode) {
            self.mode = "always".into();
        }
        self
    }

    /// What the page template needs: the glitchfx.js attributes and the CSS-only stand-in classes.
    fn view(&self) -> Value {
        let level = match self.strength.as_str() {
            "low" => "0.3",
            "high" => "0.95",
            _ => "0.6",
        };
        json!({
            "on": self.on,
            "level": level,
            "idle": self.mode == "bursts",
            "hover": self.mode == "hover",
            "cls": if self.on { format!(" mt-fx mt-fx-{} mt-fx-{}", self.strength, self.mode) } else { String::new() },
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Maintenance {
    pub enabled: bool,
    /// Unix seconds of when it was switched on
    pub since: i64,
    /// One of [`LAYOUTS`]
    pub layout: String,
    pub title: String,
    pub text: String,
    /// Free text ("~16:00 UTC"), empty: not shown
    pub eta: String,
    /// `/s/maintenance/<file>` or empty (the test card shows)
    pub video_url: String,
    /// Where the video came from, for the admin form ("upload: x.mp4", "item 123")
    pub video_from: String,
    /// Keep the player controls on screen (the sound is on by itself, see maintenance.js)
    pub video_controls: bool,
    pub glitch: BTreeMap<String, Glitch>,
}

impl Maintenance {
    pub fn glitch(&self, key: &str) -> Glitch {
        self.glitch.get(key).cloned().unwrap_or_default().clean()
    }

    /// The layout key, unknown values as the default one.
    pub fn layout(&self) -> &'static str {
        layout_key(&self.layout)
    }

    pub fn title_or_default(&self) -> &str {
        let t = self.title.trim();
        if t.is_empty() { "MAINTENANCE" } else { t }
    }

    /// The video is an image (GIF, WebP, …) and goes into an `<img>`.
    pub fn video_is_image(&self) -> bool {
        !self.video_url.is_empty() && !(self.video_url.ends_with(".mp4") || self.video_url.ends_with(".webm"))
    }
}

/// A stored or posted layout key, anything else as the default layout.
pub fn layout_key(v: &str) -> &'static str {
    LAYOUTS.iter().map(|(k, _, _)| *k).find(|k| *k == v).unwrap_or(LAYOUTS[0].0)
}

/// The stored JSON value; anything unreadable is "off" with defaults.
pub fn parse(v: &str) -> Maintenance {
    let mut m: Maintenance = serde_json::from_str(v).unwrap_or_default();
    m.glitch = ELEMENTS.iter().map(|(k, _)| (k.to_string(), m.glitch(k))).collect();
    m.layout = m.layout().to_string();
    // Only our own files are ever linked
    if !own_file(&m.video_url) {
        m.video_url.clear();
        m.video_from.clear();
    }
    m
}

/// `/s/maintenance/<plain file name>`
pub fn own_file(url: &str) -> bool {
    url.strip_prefix("/s/maintenance/").is_some_and(crate::media::is_plain_name)
}

/// Re-read the setting from the database (after a pg_notify from the process that saved it).
pub async fn reload(st: &AppState) {
    match crate::sql!("SELECT value FROM site_settings WHERE key = ", KEY, " LIMIT 1").fetch_optional(&st.db).await {
        Ok(row) => {
            use crate::db::RowExt;
            let m = row.map(|r| parse(&r.string("value"))).unwrap_or_default();
            st.settings.update(|s| s.maintenance = m);
        }
        Err(e) => tracing::error!("[MAINTENANCE] reload failed: {e}"),
    }
}

/// Template data of the page (`mt`).
pub fn page_ctx(m: &Maintenance, preview: bool) -> Value {
    let lines: Vec<&str> = m.text.lines().map(str::trim_end).take(MAX_LINES).collect();
    let since = chrono::DateTime::from_timestamp(m.since, 0).filter(|_| m.since > 0).map(|d| d.format("%Y-%m-%d %H:%M UTC").to_string());
    let glitch: serde_json::Map<String, Value> = ELEMENTS.iter().map(|(k, _)| (k.to_string(), m.glitch(k).view())).collect();
    json!({ "mt": {
        "layout": m.layout(),
        "title": m.title_or_default(),
        "lines": lines,
        "eta": m.eta.trim(),
        "since": since,
        "video_url": m.video_url,
        "video_is_image": m.video_is_image(),
        "video_controls": m.video_controls,
        "glitch": glitch,
        "preview": preview,
    }})
}

pub fn render_page(st: &AppState, req: &Req, m: &Maintenance, preview: bool) -> String {
    crate::tpl::render(st, Some(req), "maintenance", page_ctx(m, preview))
}

/// Paths that stay reachable: signing in (admins have to get in), captchas, CSP reports and the
/// few root files browsers fetch by themselves. `/s/` (static files) never gets here.
pub(crate) fn is_open_path(path: &str) -> bool {
    static OPEN: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"^/(login|logout|api/cap|api/csp-report|api/v2/auth|api/v2/settings/passkeys/login|manifest\.json|sw\.js|robots\.txt|favicon\.(ico|png|gif))(/.*)?$").unwrap()
    });
    OPEN.is_match(path)
}

fn unavailable(r: Response) -> Response {
    let mut r = with_header(r, "cache-control", "no-store");
    r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("300"));
    r
}

/// The answer for a request while maintenance is on, `None` when it may pass: admins, the open paths, and guests of a private society (they keep getting
/// the gate, which hides that there is a site at all).
pub fn intercept(st: &AppState, req: &Req) -> Option<Response> {
    let m = st.settings.maintenance_if_on()?;
    match &req.session {
        // also an admin impersonating a user or previewing a role (they could not switch back)
        Some(s) if s.admin || s.is_impersonating || s.preview_role.is_some() => return None,
        None if st.cfg().truthy("websrv.private_society") => return None,
        _ => {}
    }
    if is_open_path(&req.path) {
        return None;
    }
    if req.is_xhr() || req.path.starts_with("/api/") || req.method != axum::http::Method::GET {
        return Some(unavailable(json_raw(&json!({ "success": false, "maintenance": true, "msg": "Down for maintenance" }), 503)));
    }
    let mut r = raw(503, render_page(st, req, &m, false));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    Some(unavailable(r))
}

/// Media files (`/b/`, `/t/`, …) while maintenance is on.
pub fn media_unavailable() -> Response {
    unavailable(raw(503, "Down for maintenance"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cleans_values() {
        let m = parse(r#"{"enabled":true,"video_url":"/b/../x.mp4","glitch":{"headline":{"on":true,"strength":"x","mode":"hover"},"bogus":{"on":true}}}"#);
        assert!(m.enabled);
        assert_eq!(m.video_url, "");
        assert_eq!(m.glitch.len(), ELEMENTS.len());
        assert_eq!(m.glitch("headline"), Glitch { on: true, strength: "medium".into(), mode: "hover".into() });
        assert_eq!(parse("not json"), parse("{}"));
        assert_eq!(m.layout, "testcard");
        assert_eq!(parse(r#"{"layout":"signal"}"#).layout, "signal");
        assert_eq!(layout_key("nope"), "testcard");
        assert!(!parse("").enabled);
    }

    #[test]
    fn open_paths() {
        for p in ["/login", "/login/", "/logout", "/api/cap/challenge", "/api/v2/auth/login", "/favicon.ico"] {
            assert!(is_open_path(p), "{p}");
        }
        for p in ["/", "/loginx", "/admin", "/api/v2/upload", "/123"] {
            assert!(!is_open_path(p), "{p}");
        }
    }

    #[test]
    fn video_kind() {
        let mut m = Maintenance { video_url: "/s/maintenance/video-ab.webm".into(), ..Default::default() };
        assert!(!m.video_is_image());
        m.video_url = "/s/maintenance/video-ab.gif".into();
        assert!(m.video_is_image());
        assert!(own_file(&m.video_url));
        assert!(!own_file("/s/maintenance/a/b.gif"));
    }
}
