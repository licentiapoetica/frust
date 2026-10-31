//! Shared authentication helpers: session creation/destruction, session cookies, reCAPTCHA.
//!
//! Session cookies hold 32 random bytes (hex); `user_sessions.session` stores only their SHA-256,
//! and every session gets its own random CSRF token.

use crate::anon_auth::NewSession;
use crate::config::Config;
use crate::db::RowExt;
use crate::http::{CookieExpiry, EXPIRED, Req, cookie_options};
use crate::sql;
use crate::state::AppState;

/// Far-future expiry used for persistent session cookies.
pub const FOREVER: &str = "Fri, 31 Dec 9999 23:59:59 GMT";

pub struct SessionSpec<'a> {
    pub user_id: i64,
    /// "keep me signed in": persistent cookie, session survives the 7-day idle cleanup.
    pub kmsi: bool,
    pub last_action: &'a str,
    pub passkey_credential_id: Option<&'a str>,
    /// CSRF token length in random bytes (32 for logins, 24 for anon handshakes like before).
    pub csrf_bytes: usize,
}

/// Insert a new `user_sessions` row and return the cookie value + CSRF token.
pub async fn insert_session(st: &AppState, req: &Req, spec: SessionSpec<'_>) -> anyhow::Result<NewSession> {
    let cfg = st.cfg();
    let session = crate::util::random_hex(32);
    let csrf_token = crate::util::random_hex(spec.csrf_bytes);
    let stamp = crate::util::now_secs() as i32;
    let browser: String = req.user_agent().chars().take(255).collect();
    let last_action: String = spec.last_action.chars().take(255).collect();
    let ip = crate::security::storable_ip(&cfg, &req.real_ip());
    sql!(
        r#"insert into "user_sessions" (user_id, session, csrf_token, browser, created_at, last_used, last_action, kmsi, ip, passkey_credential_id) values ("#,
        spec.user_id as i32, ", ", crate::util::sha256(&session), ", ", csrf_token.clone(), ", ", browser, ", ", stamp, ", ", stamp, ", ",
        last_action, ", ", if spec.kmsi { 1i16 } else { 0i16 }, ", ", ip, ", ", spec.passkey_credential_id, ")"
    )
    .execute(&st.db)
    .await?;
    Ok(NewSession { session, csrf_token })
}

/// `Set-Cookie` value for a session cookie: persistent with kmsi, a browser-session cookie without.
pub fn session_cookie(cfg: &Config, session: &str, persistent: bool) -> String {
    let exp = if persistent { CookieExpiry::Expires(FOREVER) } else { CookieExpiry::None };
    format!("session={session}; {}", cookie_options(cfg, exp, true))
}

/// `Set-Cookie` value that deletes the session cookie.
pub fn clear_session_cookie(cfg: &Config) -> String {
    format!("session=; {}", cookie_options(cfg, CookieExpiry::Expires(EXPIRED), true))
}

/// Delete one session row and drop its cached lookup.
pub async fn destroy_session(st: &AppState, req: &Req) -> anyhow::Result<()> {
    if let Some(sid) = req.session().map(|s| s.sess_id).filter(|id| *id != 0) {
        sql!(r#"delete from "user_sessions" where id = "#, sid as i32).execute(&st.db).await?;
    }
    if let Some(c) = req.cookie("session").filter(|c| !c.is_empty()) {
        st.invalidate_session_cache(&crate::util::sha256(c));
    }
    Ok(())
}

/// Delete all sessions of a user (password reset, unactivated upgrade) and drop their cache entries.
pub async fn destroy_user_sessions(st: &AppState, user_id: i64) -> anyhow::Result<()> {
    let rows = sql!(r#"delete from "user_sessions" where user_id = "#, user_id as i32, " returning session").fetch_all(&st.db).await?;
    for r in rows {
        if let Some(h) = r.str("session") {
            st.invalidate_session_cache(h);
        }
    }
    Ok(())
}

/// `x-requested-with: XMLHttpRequest` or an `Accept` that includes JSON (the old `isAJAX`).
pub fn is_ajax(req: &Req) -> bool {
    req.is_xhr() || req.accepts_json()
}

/// reCAPTCHA is required for this request (configured, and not a .onion request).
pub fn recaptcha_required(cfg: &Config, req: &Req) -> bool {
    !req.is_onion() && cfg.truthy("recaptcha.enabled") && cfg.truthy("recaptcha.site_key") && cfg.truthy("recaptcha.secret_key")
}

pub enum Captcha {
    Ok,
    Missing,
    Failed,
    Error,
}

/// Verify a reCAPTCHA v2 response token with Google.
pub async fn verify_recaptcha(st: &AppState, token: Option<&str>, ip: &str) -> Captcha {
    let Some(token) = token.filter(|t| !t.is_empty()) else { return Captcha::Missing };
    let cfg = st.cfg();
    let secret = cfg.str_or("recaptcha.secret_key", "").to_string();
    let res = st
        .http
        .post("https://www.google.com/recaptcha/api/siteverify")
        .timeout(std::time::Duration::from_secs(10))
        .form(&[("secret", secret.as_str()), ("response", token), ("remoteip", ip)])
        .send()
        .await;
    let body: Result<serde_json::Value, String> = match res {
        Ok(r) => r.json().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    match body {
        Ok(v) if v["success"].as_bool() == Some(true) => Captcha::Ok,
        Ok(_) => Captcha::Failed,
        Err(e) => {
            tracing::error!("[REGISTER] reCAPTCHA error: {e}");
            Captcha::Error
        }
    }
}

/// The passkey the current session was opened with (None: password login or an older session).
pub async fn session_passkey(st: &AppState, req: &Req) -> anyhow::Result<Option<String>> {
    let Some(sid) = req.session().map(|s| s.sess_id).filter(|s| *s != 0) else { return Ok(None) };
    let row = sql!("SELECT passkey_credential_id FROM user_sessions WHERE id = ", sid as i32).fetch_optional(&st.db).await?;
    Ok(row.and_then(|r| r.str("passkey_credential_id").filter(|c| !c.is_empty()).map(str::to_string)))
}
