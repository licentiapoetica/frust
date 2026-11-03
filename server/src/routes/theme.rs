//! Theme, fullscreen and frontend toggles (port of src/inc/routes/theme.mjs).

use crate::http::{CookieExpiry, Req, Res, Router, cookie_options, raw, redirect_code};
use crate::state::AppState;
use axum::http::{HeaderValue, header};

pub fn register(r: &mut Router) {
    r.get(r"^\/theme\/", theme);
    r.get(r"^\/tfull\/", tfull);
    r.get(r"^\/frontend\/(?<name>[A-Za-z0-9_-]{1,32})\/?$", frontend);
}

fn redirect_301(target: &str, cookie: &str) -> axum::response::Response {
    let mut r = raw(301, "");
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(cookie) {
        h.insert(header::SET_COOKIE, v);
    }
    h.insert(header::LOCATION, HeaderValue::from_str(target).unwrap_or(HeaderValue::from_static("/")));
    r
}

async fn theme(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let themes = cfg.str_list("websrv.themes").unwrap_or_default();
    let first = themes.first().cloned().unwrap_or_default();
    let mut theme = req.path.split('/').nth(2).map(str::to_string).unwrap_or_else(|| first.clone());
    if !themes.contains(&theme) {
        theme = first;
    }
    let cookie = format!("theme={theme}; {}", cookie_options(&cfg, CookieExpiry::None, false));
    Ok(redirect_301(req.header("referer").unwrap_or("/"), &cookie))
}

async fn tfull(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let current = req.session().map(|s| crate::config::as_int(&s.fullscreen).unwrap_or(0)).unwrap_or(0);
    let full = if current == 1 { 0 } else { 1 };
    let cookie = format!("fullscreen={full}; {}", cookie_options(&cfg, CookieExpiry::None, false));
    Ok(redirect_301(req.header("referer").unwrap_or("/"), &cookie))
}

/// `/frontend/<name>`: remember the visitor's frontend in a cookie (see `tpl::frontend_cookie`)
/// and go back. 303, so browsers don't cache the switch.
async fn frontend(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let name = req.param("name").unwrap_or_default();
    let back = req.header("referer").unwrap_or("/");
    let mut r = redirect_code(back, 303);
    if let Some(v) = crate::tpl::frontend_cookie(&st, &cfg, name).and_then(|c| HeaderValue::from_str(&c).ok()) {
        r.headers_mut().insert(header::SET_COOKIE, v);
    }
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}
