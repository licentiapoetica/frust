//! Theme and fullscreen toggles (port of src/inc/routes/theme.mjs).

use crate::http::{CookieExpiry, Req, Res, Router, cookie_options, raw};
use crate::state::AppState;
use axum::http::{HeaderValue, header};

pub fn register(r: &mut Router) {
    r.get(r"^\/theme\/", theme);
    r.get(r"^\/tfull\/", tfull);
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
