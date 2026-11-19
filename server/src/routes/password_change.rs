//! The forced password change (an admin set the account's password: `"user".force_password_change`).
//! While the flag is on, the session middleware (app.rs) sends every page here and refuses API
//! calls, so the user changes it right after logging in, in every frontend. A shared page
//! (views/password-change.html, in the frontend's shell); the form posts here without JavaScript,
//! with it the page script uses `PUT /api/v2/settings/password` (same check, `change_password`).

use crate::http::{Req, Res, Router, html_code, redirect_code};
use crate::state::AppState;
use serde_json::json;

pub const PATH: &str = "/password-change";

pub fn register(r: &mut Router) {
    r.get(r"^\/password-change\/?$", page);
    r.post(r"^\/password-change\/?$", submit);
}

fn flagged(req: &Req) -> bool {
    req.session().is_some_and(|s| s.force_password_change && !s.is_anon)
}

fn render(st: &AppState, req: &Req, error: Option<&str>, code: u16) -> Res {
    let lang = req.session().and_then(|s| s.language.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into());
    let title = st.i18n.translator(&lang).t("auth.password_change_required");
    let body = crate::tpl::render(
        st,
        Some(req),
        "password-change",
        json!({
            "session": req.session_value(),
            "error": error,
            "page_meta": { "title": title }
        }),
    );
    Ok(html_code(body, code))
}

async fn page(st: AppState, req: Req) -> Res {
    if !flagged(&req) {
        return Ok(redirect_code("/", 303));
    }
    render(&st, &req, None, 200)
}

async fn submit(st: AppState, mut req: Req) -> Res {
    if !flagged(&req) {
        return Ok(redirect_code("/", 303));
    }
    let body = req.post_value().await;
    match crate::routes::apiv2::settings::change_password(&st, &req, &body).await? {
        Ok(()) => Ok(redirect_code("/", 303)),
        Err((msg, code)) => render(&st, &req, Some(msg), code),
    }
}
