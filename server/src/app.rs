//! Request pipeline: the middleware chain from index.mjs, run in order, then routing.
//!
//! The old server started every middleware at once (flummpress `Promise.all`), so checks that read
//! the session (CSRF, bans) could run before the session was loaded. Here each step runs to
//! completion before the next one starts.

use crate::db::{Row, RowExt};
use crate::http::{
    CookieExpiry, EXPIRED, Req, Res, check_guard, cookie_options, fail, json_raw, not_found_route, raw, reply,
};
use crate::session::{SESSION_COLUMNS, Session, USER_COLUMNS};
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Instant;

pub async fn handle(State(st): State<AppState>, ConnectInfo(addr): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let start = Instant::now();
    let (parts, body) = request.into_parts();
    let req = Req::new(parts.method, &parts.uri, parts.headers, body, Some(addr));
    let out = req.out.clone();
    let method = req.method.clone();
    let path = req.path.clone();
    let wants_json = req.wants_json();

    let resp = match pipeline(&st, req).await {
        Ok(r) => r,
        Err(e) => e.into_response_for(wants_json),
    };
    let resp = apply_extras(resp, &out.lock());
    tracing::info!(
        "{:<15}{:<15}{:<15}{}",
        format!("{:.2}ms", start.elapsed().as_secs_f64() * 1000.0),
        method.as_str(),
        resp.status().as_u16(),
        path
    );
    resp
}

/// Headers queued with `set_header` apply unless the response sets its own (like `writeHead`
/// overriding `setHeader`); queued cookies are dropped when the response sets cookies itself.
fn apply_extras(mut resp: Response, out: &crate::http::OutExtras) -> Response {
    let h = resp.headers_mut();
    for (name, value) in &out.headers {
        if !h.contains_key(name) {
            h.insert(name.clone(), value.clone());
        }
    }
    if !h.contains_key(header::SET_COOKIE) {
        for c in &out.cookies {
            if let Ok(v) = HeaderValue::from_str(c) {
                h.append(header::SET_COOKIE, v);
            }
        }
    }
    resp
}

async fn pipeline(st: &AppState, mut req: Req) -> Res {
    if let Some(r) = security_headers(st, &req) {
        return Ok(r);
    }
    if let Some(r) = comments_disabled(st, &req) {
        return Ok(r);
    }
    if let Some(r) = cors(&req) {
        return Ok(r);
    }
    if let Some(r) = config_editor(st, &req).await {
        return Ok(r);
    }
    static_cache_headers(st, &req);
    if req.path.ends_with(".map") {
        return Ok(reply(404, "text/plain", "Not Found"));
    }
    if req.path == "/a/default.png" && !st.cfg().paths.a.join("default.png").exists() {
        let mut r = raw(302, "");
        r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/s/img/404.gif"));
        return Ok(r);
    }
    if let Some(r) = session_middleware(st, &mut req).await? {
        return Ok(r);
    }
    if let Some(r) = csrf_middleware(st, &mut req).await? {
        return Ok(r);
    }
    dispatch(st, req).await
}

pub async fn dispatch(st: &AppState, mut req: Req) -> Res {
    let router = st.router.get().expect("router initialised").clone();
    let Some(m) = router.find(&req.path, &req.method) else {
        return Ok(not_found_route(&req.method, &req.path));
    };
    req.params = m.params;
    if let Some(g) = m.endpoint.guard {
        if let Some(r) = check_guard(st, &mut req, g).await {
            return Ok(r);
        }
    }
    (m.endpoint.handler)(st.clone(), req).await
}

// ── Middleware steps ────────────────────────────────────────────────────────

fn security_headers(st: &AppState, req: &Req) -> Option<Response> {
    let cfg = st.cfg();
    req.set_header("x-frame-options", "SAMEORIGIN");
    req.set_header("x-content-type-options", "nosniff");
    req.set_header("referrer-policy", "strict-origin-when-cross-origin");
    req.set_header("permissions-policy", "geolocation=(), microphone=(), camera=()");

    if cfg.is_false("websrv.enable_tor_hs") && req.is_onion() {
        return Some(raw(503, ""));
    }
    if cfg.enabled("websrv.enable_tor_hs") && !req.is_onion() {
        if let Some(onion) = cfg.str("main.onion").filter(|o| !o.is_empty()) {
            let search = if req.raw_query.is_empty() { String::new() } else { format!("?{}", req.raw_query) };
            req.set_header("onion-location", &format!("{}{}{}", onion.trim_end_matches('/'), req.path, search));
        }
    }
    if cfg.is_secure() {
        req.set_header("strict-transport-security", "max-age=31536000; includeSubDomains");
    }
    None
}

fn comments_disabled(st: &AppState, req: &Req) -> Option<Response> {
    if st.cfg().enabled("enable_comments") {
        return None;
    }
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(
            r"^(/api/comments|/api/comment/|/api/subscribe/|/api/subscriptions|/subscriptions(/|$)|/ajax/subscriptions(/|$)|/api/polls/|/activity(/|$)|/user/[^/]+/comments|/api/v2/comments|/api/v2/user/subscribe-all-uploads)",
        )
        .unwrap()
    });
    RE.is_match(&req.path).then(|| json_raw(&json!({ "success": false, "message": "Comments are disabled" }), 404))
}

fn cors(req: &Req) -> Option<Response> {
    if !req.path.starts_with("/api/") {
        return None;
    }
    let origin = req.header("origin").unwrap_or("*").to_string();
    req.set_header("access-control-allow-origin", &origin);
    req.set_header("access-control-allow-credentials", "true");
    req.set_header("access-control-allow-methods", "GET, POST, PUT, DELETE, OPTIONS");
    req.set_header(
        "access-control-allow-headers",
        "Content-Type, X-Requested-With, X-CSRF-Token, Authorization, X-API-Key, X-Filename, X-Upload-Expiry, X-Delete-Token",
    );
    (req.method == Method::OPTIONS).then(|| raw(204, ""))
}

async fn config_editor(st: &AppState, req: &Req) -> Option<Response> {
    if req.method != Method::GET || !(req.path == "/config_editor.html" || req.path == "/config.html") {
        return None;
    }
    let file = st.cfg().base.join("config_editor.html");
    Some(match tokio::fs::read_to_string(file).await {
        Ok(c) => {
            let mut r = raw(200, c);
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
            r
        }
        Err(_) => raw(404, "config_editor.html not found"),
    })
}

fn static_cache_headers(st: &AppState, req: &Req) {
    let p = req.path.as_str();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|x| p.starts_with(x));
    if starts(&["/t/", "/ca/", "/b/", "/s/", "/a/", "/c/"]) {
        req.set_header("access-control-allow-origin", "*");
    }
    if st.settings.protect_files() || crate::private_items::is_private_path(st, p) {
        return;
    }
    if starts(&["/t/", "/ca/", "/b/"]) {
        req.set_header("cache-control", "public, max-age=31536000, immutable");
    } else if p.starts_with("/s/") && !p.starts_with("/s/emojis/") && !p.starts_with("/s/koepfe/") {
        req.set_header("cache-control", "public, max-age=86400");
    } else if p.starts_with("/a/") {
        req.set_header("cache-control", "public, max-age=3600, stale-while-revalidate=86400");
    } else if starts(&["/memes/", "/s/emojis/", "/s/koepfe/"]) {
        req.set_header("cache-control", "public, max-age=3600");
    }
}

/// Minimal session row for media gating (id, user, admin, mod, banned).
async fn media_user(st: &AppState, session_cookie: &str) -> Option<Row> {
    let hash = crate::util::sha256(session_cookie);
    if let Some(r) = st.caches.session.get(&hash) {
        return Some(r);
    }
    let row = sql!(
        r#"select "user".id, "user".user, "user".admin, "user".is_moderator, "user".banned, "user".ban_expires
        from "user_sessions" left join "user" on "user".id = "user_sessions".user_id
        where "user_sessions".session = "#,
        hash.clone(),
        " limit 1"
    )
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten()?;
    st.caches.session.set(&hash, row.clone());
    Some(row)
}

fn no_store(r: &Req) {
    r.set_header("cache-control", "private, no-cache, no-store, must-revalidate");
}

fn html_200(body: String) -> Response {
    let mut r = raw(200, body);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    r
}

/// Session loading, bans, impersonation, per-request counters and filter cookies.
async fn session_middleware(st: &AppState, req: &mut Req) -> Result<Option<Response>, crate::http::AppError> {
    let cfg = st.cfg();
    req.session = None;
    let path = req.path.clone();

    if path.starts_with("/s/") && !path.starts_with("/s/emojis/") {
        return Ok(None);
    }
    if path == "/manifest.json" || path == "/sw.js" {
        return Ok(None);
    }

    // Media URLs: private/unavailable items and protect_files
    static MEDIA: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/(b|c|t|ca|a|memes)/").unwrap());
    if MEDIA.is_match(&path) || path.starts_with("/s/emojis/") {
        if let Some(hit) = crate::private_items::from_path(st, &path) {
            let user = match req.cookie("session") {
                Some(c) => media_user(st, &c.to_string()).await,
                None => None,
            };
            let banned = user.as_ref().map(|u| u.bool("banned")).unwrap_or(true);
            if hit.is_unavailable {
                let is_admin = !banned && user.as_ref().map(|u| u.bool("admin")).unwrap_or(false);
                if !is_admin {
                    return Ok(Some(crate::private_items::render_451(req)));
                }
                no_store(req);
                return Ok(None);
            }
            let authorized = !banned
                && user
                    .as_ref()
                    .map(|u| {
                        let name = u.string("user").to_lowercase();
                        (!name.is_empty() && name == hit.owner.to_lowercase()) || u.bool("admin") || u.bool("is_moderator")
                    })
                    .unwrap_or(false);
            if !authorized {
                return Ok(Some(crate::private_items::render_502(st, req)));
            }
            no_store(req);
            return Ok(None);
        }
        if st.settings.protect_files() && req.cookie("session").is_none() {
            if cfg.truthy("websrv.private_society") {
                return Ok(Some(html_200(crate::gate::gate_or_nginx(st, req))));
            }
            return Ok(Some(raw(401, "Unauthorized")));
        }
        return Ok(None);
    }

    let themes: Vec<String> = cfg
        .str_list("websrv.themes")
        .unwrap_or_else(|| ["amoled", "atmos", "f0ck", "f0ck95d", "iced", "orange", "p1nk", "4d"].map(String::from).to_vec());
    let default_theme = cfg.str_or("websrv.theme", "amoled").to_string();
    req.theme = match req.cookie("theme") {
        Some(t) if themes.iter().any(|x| x == t) => t.to_string(),
        _ => default_theme,
    };
    req.fullscreen = req.cookie("fullscreen").map(str::to_string);

    if let Some(cookie) = req.cookie("session").map(str::to_string) {
        let hash = crate::util::sha256(&cookie);
        let row = match st.caches.session.get(&hash).filter(|r| r.contains_key("sess_id")) {
            Some(r) => Some(r),
            None => {
                let mut q = sql!("select ");
                q.push(SESSION_COLUMNS).push(
                    r#" from "user_sessions"
                    left join "user" on "user".id = "user_sessions".user_id
                    left join "user_options" on "user_options".user_id = "user_sessions".user_id
                    left join "anon_identities" on "anon_identities".user_id = "user_sessions".user_id
                    where "user_sessions".session = "#,
                );
                q.bind(hash.clone()).push(" limit 1");
                let r = q.fetch_optional(&st.db).await?;
                if let Some(r) = &r {
                    st.caches.session.set(&hash, r.clone());
                }
                r
            }
        };

        let Some(row) = row else {
            // Unknown session: delete the cookie and reload
            let mut r = raw(307, "");
            let h = r.headers_mut();
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
            if let Ok(v) = HeaderValue::from_str(&format!("session=; {}", cookie_options(&cfg, CookieExpiry::Expires(EXPIRED), true))) {
                h.insert(header::SET_COOKIE, v);
            }
            let loc = HeaderValue::from_str(&req.original_url).unwrap_or(HeaderValue::from_static("/"));
            h.insert(header::LOCATION, loc);
            return Ok(Some(r));
        };

        let real_row = row.clone();
        let mut session = Session::from_row(row)?;
        let mut has_session = true;
        if let Some(fp) = session.anon_fingerprint.clone() {
            if !s::enable_anonymous_access(&cfg) {
                has_session = false;
                req.set_cookie(format!("session=; {}", cookie_options(&cfg, CookieExpiry::Expires(EXPIRED), true)));
            } else {
                session.is_anon = true;
                session.fingerprint = Some(fp);
                session.anon_login = Some(session.login.clone());
                session.user = "anonymous".into();
                session.display_name = Some("Anonymous".into());
            }
        }

        if has_session {
            session.can_chan = s::can_use_chan(&cfg, Some(&session));

            // Ban check (account)
            static BAN_EXEMPT: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/(banned|logout)(/)?$").unwrap());
            if session.banned && !BAN_EXEMPT.is_match(&path) {
                let expired = session
                    .ban_expires
                    .as_deref()
                    .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
                    .map(|e| e < chrono::Utc::now())
                    .unwrap_or(false);
                if expired {
                    sql!(r#"update "user" set banned = false, ban_reason = null, ban_expires = null where id = "#, session.id as i32).execute(&st.db).await?;
                    session.banned = false;
                } else {
                    return Ok(Some(crate::security::ban_response(req, session.ban_reason.as_deref(), session.ban_expires.as_deref())));
                }
            }

            // Ban check (IP and fingerprint)
            static IPBAN_EXEMPT: once_cell::sync::Lazy<regex::Regex> =
                once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/(banned|logout|s/|a/|t/|b/|c/|favicon\.ico)(/)?$").unwrap());
            if !IPBAN_EXEMPT.is_match(&path) {
                let ip = req.real_ip();
                let mut ban = crate::security::is_ip_banned(st, &ip).await;
                if ban.is_none() {
                    if let Some(fp) = session.fingerprint.clone().or_else(|| session.anon_fingerprint.clone()) {
                        ban = crate::security::is_fingerprint_banned(st, &fp).await;
                    }
                }
                if let Some(b) = ban {
                    return Ok(Some(crate::security::ban_response(req, b.str("reason"), b.str("expires"))));
                }
            }

            // Forced password change: block API/XHR calls
            static PW_EXEMPT: once_cell::sync::Lazy<regex::Regex> =
                once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/(api/v2/settings/password|logout)(/)?$").unwrap());
            if session.force_password_change && !PW_EXEMPT.is_match(&path) && (req.is_xhr() || path.starts_with("/api/")) {
                return Ok(Some(json_raw(&json!({ "success": false, "msg": "Password change required", "force_password_change": true }), 403)));
            }

            // Admin impersonation overlay
            if session.admin && !path.starts_with("/api/v2/admin/stop-impersonate") && !path.starts_with("/api/v2/admin/impersonate") {
                if let Some(imp) = req.cookie("impersonate").map(str::to_string) {
                    if let Some(target) = impersonation_target(st, &imp, &cookie).await {
                        let admin_user = session.user.clone();
                        let admin_display = session.display_name.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| session.user.clone());
                        let mut t = target;
                        t.csrf_token = session.csrf_token.clone();
                        t.sess_id = session.sess_id;
                        t.is_impersonating = true;
                        t.impersonated_by = Some(admin_user);
                        t.impersonated_by_display = Some(admin_display);
                        t.admin = false;
                        t.is_moderator = false;
                        session = t;
                    }
                }
            }

            // Admin role preview
            if let Some(role) = req.cookie("preview_role").map(str::to_string) {
                if (role == "mod" || role == "user") && session.admin && !session.is_impersonating {
                    session.admin = false;
                    session.is_moderator = role == "mod";
                    session.preview_role = Some(role);
                }
            }

            // Last action (fire and forget)
            if !path.starts_with("/api/notifications") {
                let st2 = st.clone();
                let ip = req.real_ip();
                let ua = req.user_agent().chars().take(255).collect::<String>();
                let last_action = path.chars().take(255).collect::<String>();
                let sess_id = real_row.i64("sess_id").unwrap_or(0) as i32;
                let user_id = real_row.i64("id").unwrap_or(0);
                tokio::spawn(async move {
                    let cfg = st2.cfg();
                    let mut q = sql!("update \"user_sessions\" set last_used = ", crate::util::now_secs() as i32, ", last_action = ", last_action, ", browser = ", ua);
                    if s::log_user_ips(&cfg) {
                        let ip_val = if s::hash_user_ips(&cfg) { crate::security::hash_ip(&cfg, &ip) } else { ip.clone() };
                        q.push(", ip = ").bind(ip_val);
                    }
                    q.push(" where id = ").bind(sess_id);
                    if let Err(e) = q.execute(&st2.db).await {
                        tracing::error!("[MIDDLEWARE] Session update failed: {e}");
                    }
                    if let Err(e) = sql!(r#"update "user" set last_seen = "#, crate::util::now_secs() as i32, " where id = ", user_id as i32).execute(&st2.db).await {
                        tracing::error!("[MIDDLEWARE] last_seen update failed: {e}");
                    }
                    crate::security::log_user_ip(&st2, user_id, &ip).await;
                });
            }

            if session.admin {
                let c = sql!(r#"select count(*) as c from "items" where active = false and is_deleted = false"#).fetch_scalar::<i64, _>(&st.db).await?;
                session.pending_count = Some(c.unwrap_or(0));
            }

            if let Some(owner) = s::session_owner_name(Some(&session)) {
                let key = owner.to_lowercase();
                session.own_pending_count = Some(match st.caches.pending_count.get(&key) {
                    Some(c) => c,
                    None => {
                        let c = sql!("select count(*)::int as c from items where lower(username) = ", key.clone(), " and active = false and is_deleted = false and is_purged = false")
                            .fetch_scalar::<i32, _>(&st.db)
                            .await
                            .ok()
                            .flatten()
                            .unwrap_or(0) as i64;
                        st.caches.pending_count.set(&key, c);
                        c
                    }
                });
            }

            if !session.admin && !session.is_moderator {
                let count = match st.caches.upload_count.get(&session.user) {
                    Some(c) => c,
                    None => {
                        let since = crate::util::now_secs() - 12 * 3600;
                        let c = sql!("SELECT count(*) as count FROM items WHERE username = ", session.user.clone(), " AND stamp > ", since, " AND is_deleted = false")
                            .fetch_scalar::<i64, _>(&st.db)
                            .await?
                            .unwrap_or(0);
                        st.caches.upload_count.set(&session.user, c);
                        c
                    }
                };
                session.uploads_remaining = Some((cfg.int_or("main.upload_limit", 0) - count).max(0));
            }

            session.theme = Some(req.theme.clone());
            session.fullscreen = req.fullscreen.clone().map(Value::String).unwrap_or(Value::Null);
            session.min_tags = st.settings.min_tags();
            session.mimes_json = crate::util::json_string(&Value::Object(cfg.mimes()));
            session.allowed_mimes = cfg.mimes().keys().cloned().collect::<Vec<_>>().join(",");
            let mult = if session.admin { cfg.f64("main.adminmultiplier").unwrap_or(1.0) } else { 1.0 };
            let max = cfg.f64("main.maxfilesize").unwrap_or(0.0) * mult;
            session.max_file_size = crate::util::format_size_f(max);
            session.max_file_size_bytes = max.floor() as i64;

            sync_user_options(st, &real_row, &session, &req.theme);
            req.session = Some(session);
        }
    }

    // Feed mode
    let query_mode = req.query("mode").map(|m| m.trim().parse::<i64>().unwrap_or(0));
    req.mode = match (query_mode, &req.session) {
        (Some(m), _) => m,
        (None, Some(sess)) => sess.mode.unwrap_or(0),
        (None, None) => req.cookie("mode").and_then(|m| m.trim().parse::<i64>().ok()).unwrap_or(0),
    };

    let is_guest = req.session.as_ref().map(|s| s.user.is_empty()).unwrap_or(true);
    let long = |c: &Req, name: &str, val: &str| c.set_cookie(format!("{name}={val}; {}", cookie_options(&cfg, CookieExpiry::MaxAge(31536000), false)));
    let clear = |c: &Req, name: &str| c.set_cookie(format!("{name}=; {}", cookie_options(&cfg, CookieExpiry::MaxAge(0), false)));
    const ALL_MIMES: [&str; 5] = ["video", "audio", "image", "flash", "pdf"];

    if is_guest {
        req.mode = 0;
        if req.cookie("mode") != Some("0") {
            req.cookies.insert("mode".into(), "0".into());
            long(req, "mode", "0");
        }
        if req.cookie("ratings") != Some("sfw") {
            req.cookies.insert("ratings".into(), "sfw".into());
            long(req, "ratings", "sfw");
        }
        let allowed: Vec<String> = cfg.str_list("anonymous_permissions.allowed_mimes").unwrap_or_else(|| ["image", "video", "audio"].map(String::from).to_vec());
        if let Some(m) = req.cookie("mime").map(str::to_string) {
            let raw_m: Vec<String> = crate::util::decode_lossy(&m).split(',').filter(|x| ALL_MIMES.contains(x)).map(str::to_string).collect();
            let valid: Vec<String> = raw_m.iter().filter(|x| allowed.contains(x)).cloned().collect();
            if valid.len() != raw_m.len() {
                if !valid.is_empty() {
                    let v = valid.join(",");
                    req.cookies.insert("mime".into(), v.clone());
                    long(req, "mime", &v);
                } else {
                    req.cookies.remove("mime");
                    clear(req, "mime");
                }
            }
        }
    } else if s::is_anon_session(req.session.as_ref()) {
        let can_filter = s::can_anon_do(&cfg, "filter");
        let modes = s::anon_allowed_modes(&cfg);
        let mimes = s::anon_allowed_mimes(&cfg);
        let cur = s::MODE_NAMES.get(req.mode as usize).copied().unwrap_or("sfw");
        if !can_filter || !modes.iter().any(|m| m == cur) {
            let fallback = modes.iter().find(|m| s::MODE_NAMES.contains(&m.as_str())).map(String::as_str).unwrap_or("sfw");
            req.mode = s::MODE_NAMES.iter().position(|m| *m == fallback).unwrap_or(0) as i64;
        }
        if let Some(cm) = req.cookie("mode").map(str::to_string) {
            if cm.trim().parse::<i64>().ok() != Some(req.mode) {
                let m = req.mode.to_string();
                req.cookies.insert("mode".into(), m.clone());
                long(req, "mode", &m);
            }
        }
        if let Some(r) = req.cookie("ratings").map(str::to_string) {
            let raw_r: Vec<String> = crate::util::decode_lossy(&r)
                .split(['|', ','])
                .filter(|x| ["sfw", "nsfw", "nsfl", "untagged"].contains(x))
                .map(str::to_string)
                .collect();
            let mut valid: Vec<String> = if can_filter { raw_r.iter().filter(|x| modes.contains(x)).cloned().collect() } else { vec![] };
            if valid.len() != raw_r.len() {
                if valid.is_empty() && modes.len() == 1 && !modes.iter().any(|m| m == "all") {
                    valid = vec![modes[0].clone()];
                }
                if !valid.is_empty() {
                    let v = valid.join("|");
                    req.cookies.insert("ratings".into(), v.clone());
                    long(req, "ratings", &v);
                } else {
                    req.cookies.remove("ratings");
                    clear(req, "ratings");
                }
            }
        }
        if let Some(m) = req.cookie("mime").map(str::to_string) {
            let raw_m: Vec<String> = crate::util::decode_lossy(&m).split(',').filter(|x| ALL_MIMES.contains(x)).map(str::to_string).collect();
            if mimes.len() == 1 {
                if m != mimes[0] {
                    let v = mimes[0].clone();
                    req.cookies.insert("mime".into(), v.clone());
                    long(req, "mime", &v);
                }
            } else if !can_filter {
                req.cookies.remove("mime");
                clear(req, "mime");
            } else {
                let valid: Vec<String> = raw_m.iter().filter(|x| mimes.contains(x)).cloned().collect();
                if valid.len() != raw_m.len() {
                    if !valid.is_empty() {
                        let v = valid.join(",");
                        req.cookies.insert("mime".into(), v.clone());
                        long(req, "mime", &v);
                    } else {
                        req.cookies.remove("mime");
                        clear(req, "mime");
                    }
                }
            }
        }
    }

    // Private society: login required for everything else
    if cfg.truthy("websrv.private_society") && req.session.is_none() {
        static PUBLIC: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"^/(s|login|logout|register|activate|forgot-password|reset-password|banned|api/v2/auth|api/v2/upload|api/chat/upload|api/chat/preview|cu|manifest\.json|sw\.js|robots\.txt|favicon\.(ico|png|gif)|s/img/duck-icon-(192|512)\.png)(/.*)?$").unwrap()
        });
        if !PUBLIC.is_match(&path) {
            if req.is_xhr() {
                return Ok(Some(json_raw(&json!({ "success": false, "msg": "Bad Gateway" }), 502)));
            }
            if let Some(tpl) = crate::gate::custom_template(st) {
                return Ok(Some(html_200(crate::tpl::render(st, Some(req), &tpl, json!({})))));
            }
            if path != "/" || crate::gate::is_cloudflare_mode(st) {
                return Ok(Some(html_200(crate::gate::gate_or_nginx(st, req))));
            }
        }
    }
    Ok(None)
}

/// Validate the signed `impersonate` cookie and load the target user.
async fn impersonation_target(st: &AppState, cookie: &str, session_cookie: &str) -> Option<Session> {
    let (payload, sig) = cookie.split_once('.')?;
    let expected = crate::util::hmac_sha256_hex(st.impersonation_secret().as_bytes(), payload.as_bytes());
    if !crate::util::ct_eq(sig, &expected) {
        return None;
    }
    use base64::Engine;
    let data: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?).ok()?;
    if data["orig"].as_str()? != crate::util::sha256(session_cookie) {
        return None;
    }
    let uid = crate::config::as_int(&data["uid"])?;
    let mut q = sql!("SELECT ");
    q.push(USER_COLUMNS).push(r#" FROM "user" LEFT JOIN "user_options" ON "user_options".user_id = "user".id WHERE "user".id = "#).bind(uid as i32).push(" LIMIT 1");
    match q.fetch_optional(&st.db).await {
        Ok(Some(r)) => Session::from_row(r).ok(),
        Ok(None) => None,
        Err(e) => {
            tracing::error!("[IMPERSONATE] lookup failed: {e}");
            None
        }
    }
}

/// Upsert user_options (creates the row for new users; keeps theme/fullscreen in sync).
fn sync_user_options(st: &AppState, row: &Row, session: &Session, theme: &str) {
    let cfg = st.cfg();
    let user_id = row.i64("id").unwrap_or(0) as i32;
    if user_id == 0 {
        return;
    }
    let mode = row.i64("mode").unwrap_or(0) as i32;
    let fullscreen = session.fullscreen.as_str().and_then(|f| f.parse::<i16>().ok()).or_else(|| session.fullscreen.as_i64().map(|f| f as i16)).unwrap_or(0);
    let excluded: Vec<i32> = session.excluded_tags.iter().map(|t| *t as i32).collect();
    let ruffle_volume: Option<f64> = session.ruffle_volume.as_deref().and_then(|v| v.parse().ok());
    let lang = session.language.as_deref().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string);
    let cdm = session.comment_display_mode.unwrap_or_else(|| cfg.int("websrv.default_comment_display_mode").unwrap_or(0)) as i32;
    let q = sql!(
        r#"insert into "user_options" (user_id, mode, theme, fullscreen, excluded_tags, font, disable_autoplay, disable_swiping, show_background, ruffle_volume, ruffle_background, quote_emojis, embed_youtube_in_comments, hide_koepfe, language, use_alternative_infobox, use_alternative_steuerung, comment_display_mode, force_comment_display_mode) values ("#,
        user_id, ", ", mode, ", ", theme.to_string(), ", ", fullscreen, ", ", excluded, ", ", session.font.clone(), ", ",
        session.disable_autoplay.unwrap_or(cfg.is_false("websrv.enable_autoplay")), ", ",
        session.disable_swiping.unwrap_or(cfg.is_false("websrv.enable_swiping")), ", ",
        session.show_background.unwrap_or(cfg.enabled("websrv.background")), ", ",
        match ruffle_volume { Some(v) => crate::db::Bind::F64(v), None => crate::db::Bind::NullText }, "::numeric, ",
        session.ruffle_background.unwrap_or(true), ", ",
        session.quote_emojis.unwrap_or(true), ", ",
        session.embed_youtube_in_comments.unwrap_or(cfg.enabled("websrv.embed_youtube_in_comments")), ", ",
        session.hide_koepfe.unwrap_or(false), ", ",
        lang, ", ",
        session.use_alternative_infobox.unwrap_or(cfg.enabled("websrv.user_alternative_infobox")), ", ",
        session.use_alternative_steuerung.unwrap_or(cfg.enabled("websrv.user_alternative_steuerung")), ", ",
        cdm, ", ",
        session.force_comment_display_mode.unwrap_or(0) as i32,
        r#") on conflict ("user_id") do update set
          theme = excluded.theme, fullscreen = excluded.fullscreen, excluded_tags = excluded.excluded_tags,
          font = excluded.font, disable_autoplay = excluded.disable_autoplay, disable_swiping = excluded.disable_swiping,
          show_background = excluded.show_background, ruffle_volume = excluded.ruffle_volume,
          ruffle_background = excluded.ruffle_background, quote_emojis = excluded.quote_emojis,
          embed_youtube_in_comments = excluded.embed_youtube_in_comments, hide_koepfe = excluded.hide_koepfe,
          language = excluded.language, use_alternative_infobox = excluded.use_alternative_infobox,
          use_alternative_steuerung = excluded.use_alternative_steuerung,
          comment_display_mode = excluded.comment_display_mode,
          force_comment_display_mode = excluded.force_comment_display_mode, user_id = excluded.user_id"#
    );
    let st2 = st.clone();
    tokio::spawn(async move {
        if let Err(e) = q.execute(&st2.db).await {
            tracing::error!("[MIDDLEWARE] Options sync failed: {e}");
        }
    });
}

/// Paths that do their own CSRF handling (or authenticate without a session cookie).
const CSRF_EXEMPT: &[&str] = &[
    "/login",
    "/register",
    "/api/v2/anon/session",
    "/api/v2/anon/logout",
    "/api/v2/anon/passkey/register/begin",
    "/api/v2/anon/passkey/register/finish",
    "/api/v2/anon/passkey/auth/begin",
    "/api/v2/anon/passkey/auth/finish",
    "/api/v2/settings/passkeys/register/begin",
    "/api/v2/settings/passkeys/register/finish",
    "/api/v2/settings/passkeys/delete",
    "/api/v2/settings/passkeys/login/begin",
    "/api/v2/settings/passkeys/login/finish",
    "/api/v2/upload",
    "/api/v2/settings/uploadAvatar",
    "/api/v2/settings/uploadBanner",
    "/api/v2/admin/memes",
    "/api/v2/admin/emojis",
    "/api/v2/meta/extract-file",
    "/api/v2/meta/strip-gps",
    "/api/v2/scroller/external/rehost-meta",
    "/api/v2/comments/upload",
    "/api/v2/admin/sticker-packs/import",
    "/admin/brand_image/upload",
    "/admin/brand_image/delete",
];

/// Check the CSRF token of the session against header, body or query.
pub async fn validate_csrf(req: &mut Req) -> bool {
    let Some(expected) = req.session.as_ref().and_then(|s| s.csrf_token.clone()).filter(|t| !t.is_empty()) else {
        return true;
    };
    let mut token = req.header("x-csrf-token").map(str::to_string).or_else(|| req.query("csrf_token").map(str::to_string));
    // Only JSON / urlencoded bodies are read here; multipart and raw uploads stay unread for their handlers.
    let ct = req.content_type().to_ascii_lowercase();
    let readable = ct.starts_with("application/json") || ct.starts_with("application/x-www-form-urlencoded") || ct.starts_with("text/plain") || ct.is_empty();
    if token.is_none() && readable && matches!(req.method, Method::POST | Method::PUT | Method::DELETE | Method::PATCH) {
        token = req.post_str("csrf_token").await;
    }
    match token {
        Some(t) if crate::util::ct_eq(&t, &expected) => true,
        t => {
            tracing::error!(
                "[CSRF] Blocked {} {} for user {}. Reason: {}",
                req.method,
                req.path,
                req.session.as_ref().map(|s| s.user.as_str()).unwrap_or(""),
                if t.is_none() { "Missing token" } else { "Token mismatch" }
            );
            false
        }
    }
}

async fn csrf_middleware(st: &AppState, req: &mut Req) -> Result<Option<Response>, crate::http::AppError> {
    if matches!(req.method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Ok(None);
    }
    let p = req.path.as_str();
    if CSRF_EXEMPT.contains(&p) {
        return Ok(None);
    }
    static SKIP: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"^(/api/chat/upload(/|$)|/api/dm/attachment/upload/)").unwrap()
    });
    if SKIP.is_match(p) {
        return Ok(None);
    }
    let cfg = st.cfg();
    static HALLS: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/api/v2/admin/halls(/|$)").unwrap());
    if cfg.enabled("websrv.halls_enabled") && HALLS.is_match(p) {
        return Ok(None);
    }
    static USER_HALL_IMG: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^/api/v2/me/halls/[^/]+/image$").unwrap());
    if cfg.enabled("websrv.userhalls_enabled") && cfg.enabled("websrv.enable_userhall_image_upload") && USER_HALL_IMG.is_match(p) {
        return Ok(None);
    }
    if !validate_csrf(req).await {
        return Ok(Some(json_raw(&json!({ "success": false, "msg": "Invalid CSRF token" }), 403)));
    }
    let _ = fail;
    Ok(None)
}
