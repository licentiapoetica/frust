//! Request context, responses, route guards.

pub mod files;
pub mod router;

use crate::session::Session;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

pub use router::{Group, Router};

pub type Res = Result<Response, AppError>;

/// Handler error: logged with a reference id, answered with a generic 500.
#[derive(Debug)]
pub struct AppError(pub anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(e: E) -> Self {
        AppError(e.into())
    }
}

impl AppError {
    pub fn into_response_for(self, wants_json: bool) -> Response {
        let msg = crate::util::log_error(&format!("{:#}", self.0), "Unhandled route error");
        if wants_json {
            json_resp(&json!({ "success": false, "msg": msg }), 500)
        } else {
            reply(500, "text/html", "500 - internal server error")
        }
    }
}

/// Default body size limit for ordinary (non-upload) requests.
pub const DEFAULT_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Headers and cookies queued by middleware/handlers for the final response
/// (the old `res.setHeader(...)` before `writeHead`).
#[derive(Default, Debug)]
pub struct OutExtras {
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub cookies: Vec<String>,
}

pub struct Req {
    pub method: Method,
    /// Path with trailing slashes removed (still percent-encoded), like flummpress.
    pub path: String,
    /// Path + query as received.
    pub original_url: String,
    pub raw_query: String,
    pub headers: HeaderMap,
    pub cookies: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub remote: Option<SocketAddr>,
    pub session: Option<Session>,
    pub mode: i64,
    pub theme: String,
    pub fullscreen: Option<String>,
    query: Vec<(String, String)>,
    body: Mutex<Option<Body>>,
    body_cache: Option<Bytes>,
    post_cache: Option<Value>,
    pub out: Arc<Mutex<OutExtras>>,
    /// Nonce for inline scripts in this response's Content-Security-Policy (`csp_nonce` in templates).
    pub csp_nonce: String,
    /// HTTP version and hyper extensions of the incoming request; the WebSocket handshake needs the
    /// connection's upgrade handle stored there.
    pub version: axum::http::Version,
    pub extensions: axum::http::Extensions,
}

/// JavaScript `decodeURI`: like decodeURIComponent but keeps reserved characters encoded.
pub fn decode_uri(s: &str) -> String {
    const RESERVED: &[u8] = b";/?:@&=+$,#";
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                if b < 0x80 && RESERVED.contains(&b) {
                    out.extend_from_slice(&bytes[i..i + 3]);
                } else {
                    out.push(b);
                }
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Strip trailing slashes but keep a lone `/` (flummpress: `/(?!^.)(\/+)?$/`).
fn normalize_path(p: &str) -> String {
    let trimmed = p.trim_end_matches('/');
    if trimmed.is_empty() {
        if p.is_empty() { "/".into() } else { p[..1].to_string() }
    } else {
        trimmed.to_string()
    }
}

/// Resolve `.` / `..` segments like the WHATWG URL parser.
fn remove_dot_segments(p: &str) -> String {
    if !p.contains("/.") {
        return p.to_string();
    }
    let mut out: Vec<&str> = Vec::new();
    let segs: Vec<&str> = p.split('/').collect();
    for (i, seg) in segs.iter().enumerate().skip(1) {
        let last = i == segs.len() - 1;
        match *seg {
            "." | "%2e" | "%2E" => {
                if last {
                    out.push("");
                }
            }
            ".." | ".%2e" | "%2e." | "%2e%2e" | ".%2E" | "%2E." | "%2E%2E" => {
                out.pop();
                if last {
                    out.push("");
                }
            }
            s => out.push(s),
        }
    }
    format!("/{}", out.join("/"))
}

pub fn parse_query(q: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(q.as_bytes()).map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
}

impl Req {
    pub fn new(method: Method, uri: &axum::http::Uri, headers: HeaderMap, body: Body, remote: Option<SocketAddr>) -> Req {
        let raw_path = uri.path();
        let raw_query = uri.query().unwrap_or("").to_string();
        // flummpress strips trailing slashes from the whole URL, so `/x/?q` keeps its slash.
        let path = if uri.query().is_some() { raw_path.to_string() } else { normalize_path(raw_path) };
        let path = remove_dot_segments(&path);
        let original_url = match uri.query() {
            Some(q) => format!("{raw_path}?{q}"),
            None => raw_path.to_string(),
        };
        let mut cookies = HashMap::new();
        for hv in headers.get_all(header::COOKIE) {
            if let Ok(s) = hv.to_str() {
                for c in s.split("; ") {
                    let mut parts = c.splitn(2, '=');
                    let k = parts.next().unwrap_or("").trim();
                    if k.is_empty() {
                        continue;
                    }
                    let v = parts.next().unwrap_or("");
                    cookies.insert(k.to_string(), decode_uri(v));
                }
            }
        }
        Req {
            method,
            path,
            original_url,
            query: parse_query(&raw_query),
            raw_query,
            headers,
            cookies,
            params: HashMap::new(),
            remote,
            session: None,
            mode: 0,
            theme: String::new(),
            fullscreen: None,
            body: Mutex::new(Some(body)),
            body_cache: None,
            post_cache: None,
            out: Arc::new(Mutex::new(OutExtras::default())),
            csp_nonce: crate::csp::new_nonce(),
            version: axum::http::Version::HTTP_11,
            extensions: axum::http::Extensions::new(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn cookie(&self, name: &str) -> Option<&str> {
        self.cookies.get(name).map(String::as_str)
    }

    /// First query value for `name` (`req.url.qs.name` / `searchParams.get`).
    pub fn query(&self, name: &str) -> Option<&str> {
        self.query.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    pub fn query_all(&self, name: &str) -> Vec<&str> {
        self.query.iter().filter(|(k, _)| k == name).map(|(_, v)| v.as_str()).collect()
    }

    pub fn query_pairs(&self) -> &[(String, String)] {
        &self.query
    }

    /// Query string as a JSON object (`req.url.qs`); repeated keys become arrays.
    pub fn query_object(&self) -> Value {
        let mut m = Map::new();
        for (k, v) in &self.query {
            match m.get_mut(k) {
                Some(Value::Array(a)) => a.push(Value::String(v.clone())),
                Some(existing) => {
                    let prev = existing.take();
                    *existing = Value::Array(vec![prev, Value::String(v.clone())]);
                }
                None => {
                    m.insert(k.clone(), Value::String(v.clone()));
                }
            }
        }
        Value::Object(m)
    }

    /// Named regex group from the route (`req.params.name`).
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params.get(name).map(String::as_str)
    }

    pub fn is_xhr(&self) -> bool {
        self.header("x-requested-with") == Some("XMLHttpRequest")
    }

    pub fn is_api(&self) -> bool {
        self.path.starts_with("/api/")
    }

    pub fn accepts_json(&self) -> bool {
        self.header("accept").map(|a| a.contains("application/json")).unwrap_or(false)
    }

    /// Whether errors should be answered as JSON.
    pub fn wants_json(&self) -> bool {
        self.is_api() || self.is_xhr() || self.accepts_json()
    }

    pub fn content_type(&self) -> &str {
        self.header("content-type").unwrap_or("")
    }

    pub fn user_agent(&self) -> &str {
        self.header("user-agent").unwrap_or("")
    }

    /// Take the raw body for streaming (uploads). Returns `None` if it was already read.
    pub fn take_body(&mut self) -> Option<Body> {
        self.body.get_mut().take()
    }

    /// Read the whole body (cached).
    pub async fn body_bytes(&mut self, limit: usize) -> Result<Bytes, AppError> {
        if let Some(b) = &self.body_cache {
            return Ok(b.clone());
        }
        let body = self.body.get_mut().take().unwrap_or_else(Body::empty);
        let bytes = axum::body::to_bytes(body, limit).await.map_err(|e| anyhow::anyhow!("body read failed: {e}"))?;
        self.body_cache = Some(bytes.clone());
        Ok(bytes)
    }

    /// Parsed body (`req.post`): JSON for `application/json`, otherwise urlencoded form fields.
    pub async fn post(&mut self) -> Result<&Value, AppError> {
        if self.post_cache.is_none() {
            let ct = self.content_type().to_string();
            let bytes = self.body_bytes(DEFAULT_BODY_LIMIT).await.unwrap_or_default();
            let v = parse_body(&ct, &bytes);
            self.post_cache = Some(v);
        }
        Ok(self.post_cache.as_ref().expect("set above"))
    }

    /// Parsed body or an empty object if reading failed.
    pub async fn post_value(&mut self) -> Value {
        match self.post().await {
            Ok(v) => v.clone(),
            Err(_) => Value::Object(Map::new()),
        }
    }

    /// `req.post[name]` as a string (numbers/bools stringified).
    pub async fn post_str(&mut self, name: &str) -> Option<String> {
        let v = self.post().await.ok()?.get(name)?.clone();
        match v {
            Value::String(s) => Some(s),
            Value::Null => None,
            other => Some(other.to_string()),
        }
    }

    pub fn set_cookie(&self, cookie: String) {
        self.out.lock().cookies.push(cookie);
    }

    pub fn set_header(&self, name: &'static str, value: &str) {
        if let Ok(v) = HeaderValue::from_str(value) {
            let mut out = self.out.lock();
            let name = HeaderName::from_static(name);
            out.headers.retain(|(n, _)| n != name);
            out.headers.push((name, v));
        }
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Session as template value (`false` when logged out).
    pub fn session_value(&self) -> Value {
        self.session.as_ref().map(Session::to_value).unwrap_or(Value::Bool(false))
    }

    pub fn host_without_port(&self) -> String {
        let raw = self
            .header("x-forwarded-host")
            .or_else(|| self.header("host"))
            .or_else(|| self.header("x-forwarded-server"))
            .unwrap_or("");
        let first = raw.split(',').next().unwrap_or("").trim();
        first.split(':').next().unwrap_or("").trim().to_lowercase()
    }

    pub fn is_onion(&self) -> bool {
        self.host_without_port().ends_with(".onion")
    }

    pub fn is_localhost(&self) -> bool {
        matches!(self.host_without_port().as_str(), "localhost" | "127.0.0.1" | "::1")
    }

    /// Client IP from proxy headers or the socket (security.getRealIP).
    pub fn real_ip(&self) -> String {
        let raw = ["cf-connecting-ip", "true-client-ip", "x-client-ip", "x-real-ip", "x-forwarded-for"]
            .iter()
            .find_map(|h| self.header(h).filter(|v| !v.is_empty()).map(str::to_string))
            .or_else(|| self.remote.map(|r| r.ip().to_string()));
        let Some(raw) = raw else { return "unknown".into() };
        let mut ip = raw.split(',').next().unwrap_or("").trim().to_string();
        if ip == "::1" {
            ip = "127.0.0.1".into();
        }
        if let Some(v4) = ip.strip_prefix("::ffff:") {
            ip = v4.to_string();
        }
        if ip.contains(':') {
            ip = ip.to_lowercase();
        }
        if ip.is_empty() { "unknown".into() } else { ip }
    }
}

/// flummpress readBody: JSON when the content type is JSON, otherwise a querystring where every
/// value is additionally passed through decodeURIComponent and repeated keys are joined by ",".
pub fn parse_body(content_type: &str, bytes: &[u8]) -> Value {
    if content_type.split(';').next().map(str::trim) == Some("application/json") {
        if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
            return v;
        }
    }
    let mut map: Map<String, Value> = Map::new();
    let mut order: Vec<(String, Vec<String>)> = Vec::new();
    for (k, v) in form_urlencoded::parse(bytes) {
        match order.iter_mut().find(|(key, _)| *key == k) {
            Some((_, vals)) => vals.push(v.into_owned()),
            None => order.push((k.into_owned(), vec![v.into_owned()])),
        }
    }
    for (k, vals) in order {
        let joined = vals.join(",");
        let decoded = crate::util::decode_uri_component(&joined).unwrap_or(joined);
        map.insert(k, Value::String(decoded));
    }
    Value::Object(map)
}

// ── Responses ───────────────────────────────────────────────────────────────

pub fn status(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// `res.reply({ code, type, body })`
pub fn reply(code: u16, content_type: &str, body: impl Into<Body>) -> Response {
    let mut r = Response::new(body.into());
    *r.status_mut() = status(code);
    if let Ok(v) = HeaderValue::from_str(&format!("{content_type}; charset=utf-8")) {
        r.headers_mut().insert(header::CONTENT_TYPE, v);
    }
    r
}

pub fn html(body: impl Into<Body>) -> Response {
    reply(200, "text/html", body)
}

pub fn html_code(body: impl Into<Body>, code: u16) -> Response {
    reply(code, "text/html", body)
}

/// `res.json(body, code)`
pub fn json_resp(v: &Value, code: u16) -> Response {
    reply(code, "application/json", serde_json::to_string(v).unwrap_or_else(|_| "null".into()))
}

pub fn json_ok(v: Value) -> Res {
    Ok(json_resp(&v, 200))
}

/// `res.writeHead(code, {'Content-Type': 'application/json'}).end(JSON.stringify(v))` (no charset).
pub fn json_raw(v: &Value, code: u16) -> Response {
    let mut r = Response::new(Body::from(serde_json::to_string(v).unwrap_or_else(|_| "null".into())));
    *r.status_mut() = status(code);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r
}

/// `res.redirect(target, code = 307)`
pub fn redirect(target: &str) -> Response {
    redirect_code(target, 307)
}

pub fn redirect_code(target: &str, code: u16) -> Response {
    let mut r = Response::new(Body::empty());
    *r.status_mut() = status(code);
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(target) {
        r.headers_mut().insert(header::LOCATION, v);
    } else {
        r.headers_mut().insert(header::LOCATION, HeaderValue::from_str(&crate::util::encode_uri(target)).unwrap_or(HeaderValue::from_static("/")));
    }
    r
}

/// Plain response with only a status and body (no content type), like `res.writeHead(code).end(body)`.
pub fn raw(code: u16, body: impl Into<Body>) -> Response {
    let mut r = Response::new(body.into());
    *r.status_mut() = status(code);
    r
}

pub fn with_header(mut r: Response, name: &'static str, value: &str) -> Response {
    if let Ok(v) = HeaderValue::from_str(value) {
        r.headers_mut().insert(HeaderName::from_static(name), v);
    }
    r
}

/// `{ success: false, msg }` with a status code.
pub fn fail(code: u16, msg: &str) -> Response {
    json_resp(&json!({ "success": false, "msg": msg }), code)
}

pub fn not_found_route(method: &Method, path: &str) -> Response {
    raw(404, format!("404 - {method} {path}"))
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        self.into_response_for(false)
    }
}

// ── Cookies ─────────────────────────────────────────────────────────────────

pub enum CookieExpiry<'a> {
    None,
    MaxAge(i64),
    Expires(&'a str),
}

/// lib.getCookieOptions
pub fn cookie_options(cfg: &crate::config::Config, expires: CookieExpiry, http_only: bool) -> String {
    let mut o = String::from("Path=/; SameSite=Lax");
    if http_only {
        o.push_str("; HttpOnly");
    }
    if cfg.is_secure() {
        o.push_str("; Secure");
    }
    match expires {
        CookieExpiry::None | CookieExpiry::MaxAge(0) => {}
        CookieExpiry::MaxAge(n) => o.push_str(&format!("; Max-Age={n}")),
        CookieExpiry::Expires(s) => o.push_str(&format!("; Expires={s}")),
    }
    o
}

pub const EXPIRED: &str = "Thu, 01 Jan 1970 00:00:00 GMT";

// ── Guards (route middleware) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// lib.auth — admin
    Auth,
    /// lib.adminAuth — admin
    AdminAuth,
    /// lib.modAuth — admin or moderator
    ModAuth,
    /// lib.userauth — registered (non-anonymous) user
    UserAuth,
    /// lib.registeredUser — registered (non-anonymous) user
    RegisteredUser,
    /// lib.loggedin — any session
    LoggedIn,
    /// lib.chanAuth — 4chan group
    ChanAuth,
    /// lib.apiKeyAuth — X-Api-Key header
    ApiKeyAuth,
}

fn unauthorized() -> Response {
    reply(401, "text/html", "401 - Unauthorized")
}

fn password_change_required() -> Response {
    json_resp(&json!({ "success": false, "msg": "Password change required", "force_password_change": true }), 403)
}

fn pw_exempt(path: &str, extra: &[&str]) -> bool {
    path == "/api/v2/settings/password" || path == "/logout" || extra.contains(&path)
}

/// Run a guard; `Some(response)` means the request is rejected.
pub async fn check_guard(st: &AppState, req: &mut Req, guard: Guard) -> Option<Response> {
    let cfg = st.cfg();
    match guard {
        Guard::Auth | Guard::AdminAuth => {
            let Some(s) = req.session.as_ref() else { return Some(unauthorized()) };
            if !s.admin {
                return Some(unauthorized());
            }
            (s.force_password_change && !pw_exempt(&req.path, &[])).then(password_change_required)
        }
        Guard::ModAuth => {
            let Some(s) = req.session.as_ref() else { return Some(unauthorized()) };
            if !s.is_admin_or_mod() {
                return Some(unauthorized());
            }
            (s.force_password_change && !pw_exempt(&req.path, &[])).then(password_change_required)
        }
        Guard::UserAuth | Guard::RegisteredUser => {
            let Some(s) = req.session.as_ref() else { return Some(unauthorized()) };
            if s.is_anon || s.user.starts_with("anon_") {
                if req.path.starts_with("/api/") {
                    let msg = if guard == Guard::UserAuth { "Registered account required" } else { "Action requires a registered account" };
                    return Some(fail(403, msg));
                }
                return Some(redirect("/login"));
            }
            let extra: &[&str] = if guard == Guard::UserAuth { &["/settings"] } else { &[] };
            (s.force_password_change && !pw_exempt(&req.path, extra)).then(password_change_required)
        }
        Guard::LoggedIn => {
            let Some(s) = req.session.as_ref() else { return Some(unauthorized()) };
            (s.force_password_change && !pw_exempt(&req.path, &[])).then(password_change_required)
        }
        Guard::ChanAuth => {
            let is_api = req.path.starts_with("/api/") || req.is_xhr() || req.accepts_json();
            let Some(s) = req.session.as_ref().filter(|s| !s.user.is_empty()) else {
                return Some(if is_api { fail(401, "Unauthorized") } else { redirect("/login") });
            };
            if !crate::settings::can_use_chan(&cfg, Some(s)) {
                return Some(if is_api {
                    fail(403, "4chan group required")
                } else {
                    reply(403, "text/html", CHAN_DENIED_HTML)
                });
            }
            None
        }
        Guard::ApiKeyAuth => api_key_auth(st, req).await,
    }
}

const CHAN_DENIED_HTML: &str = r#"<!DOCTYPE html><html><head><meta charset="utf-8"><title>Access Denied</title><link rel="stylesheet" href="/s/css/f0ckm.css"></head><body class="chan-denied"><div class="chan-denied-box"><div class="chan-denied-icon">🍀</div><h2 class="chan-denied-title">Access Restricted</h2><p class="chan-denied-text">This 4chan viewer is only accessible to users with the <strong>4chan</strong> group.</p><a href="/" class="chan-denied-link">← Return to Home</a></div></body></html>"#;

async fn api_key_auth(st: &AppState, req: &mut Req) -> Option<Response> {
    let Some(key) = req.header("x-api-key").map(str::to_string) else {
        return Some(fail(401, "API key required"));
    };
    let q = crate::sql!(
        r#"SELECT u.id, u.user, u.login, u.admin, u.is_moderator, u.banned,
          uo.display_name, uo.mode, uo.theme, uo.avatar, uo.avatar_file,
          uo.username_color, uo.show_motd, uo.disable_autoplay,
          uo.disable_swiping, uo.use_new_layout, uo.excluded_tags,
          uo.ruffle_background, uo.ruffle_volume, uo.quote_emojis,
          uo.embed_youtube_in_comments, uo.hide_koepfe,
          uo.use_alternative_infobox, uo.language, uo.comment_display_mode,
          uo.force_comment_display_mode, uo.show_background,
          uo.font, uo.receive_system_notifications, uo.receive_user_notifications,
          uo.do_not_disturb, uo.description
        FROM user_api_keys k
        JOIN "user" u ON u.id = k.user_id
        LEFT JOIN user_options uo ON uo.user_id = u.id
        WHERE k.api_key = "#,
        key,
        " LIMIT 1"
    );
    let row = match q.fetch_optional(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[API KEY AUTH] DB error: {e}");
            return Some(fail(500, "Internal server error"));
        }
    };
    let Some(row) = row else { return Some(fail(401, "Invalid API key")) };
    let Ok(mut s) = Session::from_row(row) else { return Some(fail(500, "Internal server error")) };
    if s.banned {
        return Some(fail(403, "Account banned"));
    }
    s.api_key_auth = true;
    req.session = Some(s);
    None
}
