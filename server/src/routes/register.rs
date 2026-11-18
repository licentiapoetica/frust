//! Accounts: registration and activation (src/inc/routes/register.mjs) plus login, logout and the
//! password-reset flow (the auth routes at the top of src/inc/routes/admin.mjs).

use crate::auth::{SessionSpec, is_ajax};
use crate::cap::Captcha;
use crate::security::Settle;
use crate::db::RowExt;
use crate::http::{Req, Res, Router, html, html_code, json_raw, raw, redirect, reply};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    // admin.mjs
    r.get(r"^\/login(\/)?$", login_page);
    r.post(r"^\/login(\/)?$", login);
    r.get(r"^\/logout$", logout);
    // POST (CSRF-checked by the middleware) for clients that log out without a logout link
    r.post(r"^\/logout$", logout_post);
    r.get(r"^\/forgot-password(\/)?$", forgot_password_page);
    r.post(r"^\/forgot-password(\/)?$", forgot_password);
    r.get(r"^\/reset-password(\/)?$", reset_password_page);
    r.post(r"^\/reset-password(\/)?$", reset_password);
    // register.mjs
    r.get(r"^\/register(\/)?$", register_page);
    r.get(r"^\/activate\/(?<token>.+)$", activate);
    r.post(r"^\/register(\/)?$", register_post);
}

/// `res.writeHead(code, { Location })` (no other headers).
fn location(code: u16, url: &str) -> Response {
    let mut r = raw(code, "");
    let v = HeaderValue::from_str(url).unwrap_or_else(|_| HeaderValue::from_str(&crate::util::encode_uri(url)).unwrap_or(HeaderValue::from_static("/")));
    r.headers_mut().insert(header::LOCATION, v);
    r
}

fn json_msg(success: bool, msg: &str, code: u16) -> Response {
    json_raw(&json!({ "success": success, "msg": msg }), code)
}

// ── Login / logout ──────────────────────────────────────────────────────────

async fn login_page(st: AppState, req: Req) -> Res {
    if req.session().is_some() {
        return Ok(location(302, "/?already_logged_in=1"));
    }
    // ?form: the plain login page (no JavaScript needed). Everyone else gets the login modal, which
    // also opens without JavaScript (/#login-modal, see the <noscript> refresh on /?login=1).
    // (also during maintenance: the page with the modal is locked for everyone who isn't in yet)
    if req.query("form").is_some() || st.settings.maintenance_on() {
        return Ok(html(crate::tpl::render(&st, Some(&req), "login", json!({ "theme": req.theme, "captcha": false }))));
    }
    Ok(redirect("/?login=1"))
}

/// Reset requests per account and day (see `forgot_password`).
const RESET_REQUESTS_PER_ACCOUNT: i64 = 3;

/// Something that can be an email address: `local@domain`, no spaces or control characters.
fn valid_email(e: &str) -> bool {
    let Some((local, domain)) = e.rsplit_once('@') else { return false };
    e.len() <= 254 && !local.is_empty() && !domain.is_empty() && !e.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// Verified against when the account doesn't exist, so both paths cost one Argon2 run.
static DUMMY_HASH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

async fn login(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let username = req.post_str("username").await;
    let password = req.post_str("password").await;
    let kmsi = req.post().await?.get("kmsi").is_some();
    let captcha_token = req.post_str("cap-token").await;
    let cap_image_id = req.post_str("cap-image-id").await;
    let cap_image_answer = req.post_str("cap-image-answer").await;
    let ajax = is_ajax(&req);
    // The navbar modal posted as a plain form (no JavaScript): answer with a redirect back to that page
    let return_to = if ajax { None } else { req.post_str("return_to").await.and_then(|p| modal_return_path(&p)) };
    let captcha_on = crate::cap::required(&cfg, &req);

    // Counted as a failure from here on unless settled below (see security::begin_attempt)
    let attempt = crate::security::begin_attempt(&st, &ip, username.as_deref(), "login").await;
    let pending = attempt.is_some();
    // After repeated failures from this IP a (hard) captcha is needed, much later a lockout
    let (captcha_needed, failures) = match crate::security::auth_gate(&st, &ip, "login", captcha_on, pending).await {
        crate::security::AuthGate::Locked => {
            crate::security::settle_attempt(&st, attempt, Settle::Withdraw).await;
            let msg = "Too many attempts.";
            if ajax {
                return Ok(json_msg(false, msg, 429));
            }
            return Ok(reply(429, "text/html", msg));
        }
        crate::security::AuthGate::Open { captcha, failures } => (captcha, failures),
    };
    // Failures at this account from any IP: guessing one password from many addresses also needs
    // the captcha (no lockout here, that would let anyone lock a victim out)
    let account_failures = match username.as_deref().filter(|u| !u.is_empty()) {
        Some(u) => (crate::security::failed_attempts_for_user(&st, u, "login").await - i64::from(pending)).max(0),
        None => 0,
    };
    let captcha_needed = captcha_needed || (captcha_on && account_failures >= crate::security::CAPTCHA_AFTER_FAILURES);
    // Whether the next try needs the captcha, counting this one as failed
    let captcha_next = captcha_on && failures.max(account_failures) + 1 >= crate::security::CAPTCHA_AFTER_FAILURES;

    let respond = |msg: &str, captcha: bool| {
        if ajax {
            return json_raw(&json!({ "success": false, "msg": msg, "captcha": captcha }), 200);
        }
        // Back to the page with the modal open; `login_captcha` makes it show the image captcha
        if let (Some(path), Some(code)) = (return_to.as_deref(), auth_code(AUTH_ERRORS, msg)) {
            let code = if captcha { format!("{code}&login_captcha=1") } else { code.to_string() };
            return back_to_modal(path, "login_error", &code, "login-modal");
        }
        html(crate::tpl::render(&st, Some(&req), "login", json!({ "error": msg, "theme": req.theme, "captcha": captcha })))
    };

    macro_rules! fail {
        ($msg:expr) => {{
            // The attempt is already recorded as failed (begin_attempt)
            crate::security::log_failed_attempt(&st, &ip, username.as_deref(), "login").await;
            // Slows down brute force and evens out timing
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            return Ok(respond($msg, captcha_next));
        }};
    }

    let (Some(username), Some(password)) = (username.clone().filter(|u| !u.is_empty()), password.filter(|p| !p.is_empty())) else {
        fail!("Invalid username or password.");
    };

    // Checked before the password, so guesses without a solved captcha learn nothing. A missing or
    // wrong captcha isn't counted as a failed login (it would only push the lockout closer).
    if captcha_needed {
        match crate::cap::verify_form(captcha_token.as_deref(), cap_image_id.as_deref(), cap_image_answer.as_deref(), true) {
            Captcha::Ok => {}
            Captcha::Missing => {
                crate::security::settle_attempt(&st, attempt, Settle::Withdraw).await;
                return Ok(respond("Too many failed logins, please complete the captcha.", true));
            }
            Captcha::Failed => {
                crate::security::settle_attempt(&st, attempt, Settle::Withdraw).await;
                return Ok(respond("Captcha expired or invalid. Please try again.", true));
            }
        }
    }

    // By email only when the input looks like one, and a login match always wins: someone else's
    // email set to "admin" must not shadow the account "admin"
    let login = username.to_lowercase();
    let by_email = username.contains('@');
    let user = sql!(
        r#"select id, password, activated, banned, ban_reason, ban_expires, force_password_change from "user" where "login" = "#,
        login.clone(), " or (", by_email, " and lower(email) = lower(", username.trim().to_string(), r#")) order by ("login" = "#, login.clone(),
        ") desc, id limit 1"
    )
    .fetch_optional(&st.db)
    .await?;

    let Some(user) = user else {
        // No account yet: with the password of an application (see `register_post`) the answer is
        // its state. One Argon2 run, as for the dummy hash.
        let app = sql!(
            "select password, status from account_applications where login = ", login,
            " and status in ('pending', 'denied') and password <> '' order by id desc limit 1"
        )
        .fetch_optional(&st.db)
        .await?;
        let hash = match &app {
            Some(a) => a.string("password"),
            None => DUMMY_HASH.get_or_init(|| crate::util::hash_password(crate::util::random_hex(16))).await.clone(),
        };
        let matches = crate::util::verify_password(password, hash).await;
        if let (true, Some(app)) = (matches, &app) {
            // Right password: not a failed guess
            crate::security::settle_attempt(&st, attempt, Settle::Withdraw).await;
            let msg = if app.str("status") == Some("pending") { APPLICATION_PENDING } else { APPLICATION_DENIED };
            return Ok(respond(msg, false));
        }
        fail!("user doesn't exist or wrong password");
    };
    let stored_hash = user.string("password");
    if stored_hash == "!" {
        // Passkey-only account: cost the same Argon2 run, so the answer time doesn't tell
        let dummy = DUMMY_HASH.get_or_init(|| crate::util::hash_password(crate::util::random_hex(16))).await;
        let _ = crate::util::verify_password(password, dummy.clone()).await;
        fail!("user doesn't exist or wrong password");
    }
    if !crate::util::verify_password(password.clone(), stored_hash.clone()).await {
        fail!("user doesn't exist or wrong password");
    }
    // activated is nullable (default true): only an explicit false blocks
    if user.val("activated") == &Value::Bool(false) {
        fail!("This account is not activated. Please check your email.");
    }
    let user_id = user.i64("id").unwrap_or(0);

    if user.bool("banned") {
        let expires = user.str("ban_expires").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok());
        if expires.is_some_and(|e| e < chrono::Utc::now()) {
            sql!(r#"update "user" set banned = false, ban_reason = null, ban_expires = null where id = "#, user_id as i32).execute(&st.db).await?;
        } else {
            // Right password: not a failed guess
            crate::security::settle_attempt(&st, attempt, Settle::Withdraw).await;
            let reason = user.str("ban_reason").filter(|r| !r.is_empty()).unwrap_or("none");
            let exp_text = expires
                .map(|e| e.with_timezone(&chrono::Utc).format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".into());
            if ajax {
                return Ok(json_raw(
                    &json!({ "success": false, "banned": true, "msg": format!("You are banned! reason: {reason} expire: {exp_text}"), "redirect": "/banned" }),
                    200,
                ));
            }
            return Ok(location(302, "/banned"));
        }
    }

    crate::security::settle_attempt(&st, attempt, Settle::Success).await;
    crate::security::clear_attempts(&st, Some(&ip), &username).await;

    // Upgrade old (scrypt) hashes to Argon2id
    if crate::util::password_needs_rehash(&stored_hash) {
        let new_hash = crate::util::hash_password(password).await;
        if !new_hash.is_empty() {
            sql!(r#"update "user" set password = "#, new_hash, " where id = ", user_id as i32, " and password = ", stored_hash).execute(&st.db).await?;
        }
    }

    // Clean up stale non-KMSI sessions unused for 7 days
    let stamp = crate::util::now_secs();
    sql!("delete from user_sessions where last_used <= ", (stamp - 604800) as i32, " and kmsi = 0").execute(&st.db).await?;

    let s = crate::auth::insert_session(&st, &req, SessionSpec { user_id, kmsi, last_action: "/login", passkey_credential_id: None, csrf_bytes: 32 }).await?;
    crate::security::log_user_ip(&st, user_id, &ip).await;

    let mut r = match return_to.as_deref() {
        Some(path) => location(303, path),
        None => location(301, "/"),
    };
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(&crate::auth::session_cookie(&cfg, &s.session, kmsi)) {
        h.insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

// ── Login / register modals without JavaScript ──────────────────────────────
// Their forms post normally with `return_to` (the page they were opened on). The answer redirects
// back there with the modal open (`#login-modal` / `#register-modal`, opened by :target in
// f0ckm.css) and a code in the query; tpl.rs turns the code into the message. Only codes travel,
// so a link can't put its own words into the modals.

/// `?login_error=` / `?register_error=`
/// Invite tokens nobody registered with expire this long after they were created.
pub const INVITE_TOKEN_TTL_SECS: i64 = 7 * 24 * 3600;

// ── Account applications ────────────────────────────────────────────────────
// With registration closed and applications on (settings: `registration_applications`), the
// register form also takes a message instead of an invite token. That stores an
// `account_applications` row (name, password hash, message); the "user" row is created when an admin
// approves it (routes/admin/applications.rs). The applicant learns the state by logging in.

/// Length of an application message, in characters.
const APPLICATION_MSG_MIN: usize = 20;
const APPLICATION_MSG_MAX: usize = 1000;
/// A declined applicant (same IP hash) can apply again after this many days.
const APPLICATION_COOLDOWN_DAYS: i32 = 7;
/// Decided applications are deleted after this many days, unanswered ones after
/// [`APPLICATION_PENDING_DAYS`].
const APPLICATION_KEEP_DAYS: i32 = 30;
const APPLICATION_PENDING_DAYS: i32 = 90;

const APPLICATION_SENT: &str = "Application submitted! Log in with your username and password to see its status.";
const APPLICATION_PENDING: &str = "Your application is still under review.";
const APPLICATION_DENIED: &str = "Your application was declined.";

/// Deletes unused invite tokens older than [`INVITE_TOKEN_TTL_SECS`] (a minute after boot, then
/// hourly). Approved invite requests whose token expires become `revoked`, as when an admin
/// deletes the token. Registration checks the age itself, so a token is refused the moment it
/// expires even before this runs.
pub fn spawn_invite_expiry(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        loop {
            if let Err(e) = purge_expired_invites(&st).await {
                tracing::error!("[INVITE EXPIRY] Failed: {e}");
            }
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    });
}

async fn purge_expired_invites(st: &AppState) -> anyhow::Result<()> {
    let cutoff = crate::util::now_secs() - INVITE_TOKEN_TTL_SECS;
    sql!(
        "UPDATE invite_requests SET status = 'revoked' WHERE status = 'approved' AND token_id IN
         (SELECT id FROM invite_tokens WHERE is_used IS NOT TRUE AND created_at <= ", cutoff, ")"
    )
    .execute(&st.db)
    .await?;
    let n = sql!("DELETE FROM invite_tokens WHERE is_used IS NOT TRUE AND created_at <= ", cutoff).execute(&st.db).await?;
    if n > 0 {
        tracing::info!("[INVITE EXPIRY] Deleted {n} unused invite token(s) older than 7 days");
    }
    // Account applications: decided ones (name, message, IP hash) and ones nobody answered
    let n = sql!(
        "DELETE FROM account_applications WHERE (status <> 'pending' AND reviewed_at < now() - make_interval(days => ", APPLICATION_KEEP_DAYS,
        ")) OR (status = 'pending' AND created_at < now() - make_interval(days => ", APPLICATION_PENDING_DAYS, "))"
    )
    .execute(&st.db)
    .await?;
    if n > 0 {
        tracing::info!("[APPLICATIONS] Deleted {n} old account application(s)");
    }
    Ok(())
}

const AUTH_ERRORS: &[(&str, &str)] = &[
    ("invalid", "Invalid username or password."),
    ("wrong", "user doesn't exist or wrong password"),
    ("inactive", "This account is not activated. Please check your email."),
    ("name_required", "Username is required."),
    ("name_chars", "Username contains invalid characters. Only A-Z, 0-9, _, -, and . are allowed."),
    ("name_taken", "Username taken"),
    ("pw_short", "Password must be at least 20 characters long."),
    ("pw_mismatch", "Passwords do not match."),
    ("token_required", "Invite token is required for registration."),
    ("token_invalid", "Invalid or used invite token"),
    ("token_expired", "Invalid, used or expired invite token"),
    ("app_required", "An invite token or a message why you want an account is required."),
    ("app_both", "Fill in either an invite token or an application, not both."),
    ("app_length", "Tell us in 20 to 1000 characters why you want an account."),
    ("app_email", "A valid email is required to apply."),
    ("app_anon", "Anonymous accounts request an invite in their settings."),
    ("app_open", "You already have a pending application."),
    ("app_cooldown", "Your last application was declined. You can apply again later."),
    ("app_pending", APPLICATION_PENDING),
    ("app_denied", APPLICATION_DENIED),
    ("email_required", "A valid email is required for registration."),
    ("email_taken", "Email already registered"),
    ("locked", "Too many attempts."),
    ("failed", "An unexpected error occurred. Please try again later."),
    ("captcha_needed", "Too many failed logins, please complete the captcha."),
    ("captcha_missing", "Please complete the captcha."),
    ("captcha_invalid", "Captcha expired or invalid. Please try again."),
    // password reset (`?forgot_error=` / `?reset_error=`)
    ("email_missing", "Email is required"),
    ("reset_limited", "Too many attempts. Please try again later."),
    ("reset_daily", "Too many reset requests for this account. Please try again tomorrow."),
    ("reset_missing", "Missing data"),
    ("reset_mismatch", "Passwords do not match"),
    ("reset_short", "Password must be at least 20 characters long"),
    ("reset_locked", "Too many reset attempts. Please try again later."),
    ("reset_token", "Invalid or expired reset token."),
];

/// `?login_done=` / `?register_done=`
const AUTH_DONE: &[(&str, &str)] = &[
    ("registered", "Registration successful! You can now login."),
    ("activate", "Registration successful! Please check your email to activate your account."),
    ("applied", APPLICATION_SENT),
    ("reset_sent", "If an account with that email exists, we have sent a reset link."),
    ("pw_reset", "Password reset successfully! You can now login with your new password."),
];

/// Query keys of the modal messages, so `return_to` drops the ones of an earlier try
const AUTH_QUERY_KEYS: [&str; 7] = ["login_error", "login_done", "register_error", "register_done", "forgot_error", "forgot_done", "reset_error"];
/// `?login_captcha=1`: the login modal shows the image captcha (cap_image.rs)
const LOGIN_CAPTCHA_KEY: &str = "login_captcha";

fn auth_code(table: &[(&'static str, &str)], msg: &str) -> Option<&'static str> {
    table.iter().find(|(_, m)| *m == msg).map(|(c, _)| *c)
}

/// The message for a modal's query parameter (`key` one of [`AUTH_QUERY_KEYS`]), for tpl.rs.
pub(crate) fn auth_message(key: &str, code: &str) -> Option<&'static str> {
    let table = if key.ends_with("_done") { AUTH_DONE } else { AUTH_ERRORS };
    table.iter().find(|(c, _)| *c == code).map(|(_, m)| *m)
}

pub(crate) fn auth_query_keys() -> &'static [&'static str] {
    &AUTH_QUERY_KEYS
}

/// `return_to` of a modal form: a local path (no other host), without the messages of an earlier try.
fn modal_return_path(p: &str) -> Option<String> {
    let p = p.split('#').next().unwrap_or("");
    if !p.starts_with('/') || p.starts_with("//") || p.starts_with("/\\") || p.chars().any(char::is_control) {
        return None;
    }
    let (path, query) = p.split_once('?').unwrap_or((p, ""));
    let query: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            let key = kv.split('=').next().unwrap_or("");
            !kv.is_empty() && *kv != "login=1" && key != LOGIN_CAPTCHA_KEY && !AUTH_QUERY_KEYS.contains(&key)
        })
        .collect();
    Some(if query.is_empty() { path.to_string() } else { format!("{path}?{}", query.join("&")) })
}

fn back_to_modal(path: &str, key: &str, code: &str, modal: &str) -> Response {
    let sep = if path.contains('?') { '&' } else { '?' };
    location(303, &format!("{path}{sep}{key}={code}#{modal}"))
}

/// `GET /logout?t=<logout token>` (the links in the navbars, see `auth::logout_url`). Without a valid
/// token (a link or `<img>` on another site) nothing happens: back to the front page, still logged in.
async fn logout(st: AppState, req: Req) -> Res {
    let expected = req.session().and_then(|s| s.csrf_token.clone()).filter(|t| !t.is_empty());
    if let Some(csrf) = expected {
        let given = req.query("t").unwrap_or("");
        if !crate::util::ct_eq(given, &crate::auth::logout_token(&csrf)) {
            return Ok(location(303, "/"));
        }
    }
    end_session(&st, &req).await
}

async fn logout_post(st: AppState, req: Req) -> Res {
    end_session(&st, &req).await
}

async fn end_session(st: &AppState, req: &Req) -> Res {
    crate::auth::destroy_session(st, req).await?;
    let mut r = location(301, "/");
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(&crate::auth::clear_session_cookie(&st.cfg())) {
        h.insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

// ── Password reset ──────────────────────────────────────────────────────────

/// The forgot-password form is a modal (w0bm: `#forgot-modal`; f0ckm: a view of its login modal)
async fn forgot_password_page(_st: AppState, _req: Req) -> Res {
    Ok(redirect("/#forgot-modal"))
}

async fn forgot_password(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let ajax = is_ajax(&req);
    let email = req.post_str("email").await.filter(|e| !e.is_empty());
    // A modal's plain form post (w0bm without JavaScript): back to that page with the modal open
    let return_to = if ajax { None } else { req.post_str("return_to").await.and_then(|p| modal_return_path(&p)) };
    let page = |code: u16, key: &str, msg: &str| {
        let (qkey, table) = if key == "success" { ("forgot_done", AUTH_DONE) } else { ("forgot_error", AUTH_ERRORS) };
        if let (Some(path), Some(c)) = (return_to.as_deref(), auth_code(table, msg)) {
            return back_to_modal(path, qkey, c, "forgot-modal");
        }
        html_code(crate::tpl::render(&st, None, "forgot-password", json!({ key: msg })), code)
    };

    let Some(email) = email else {
        let msg = "Email is required";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page(200, "error", msg) });
    };

    if crate::security::is_rate_limited(&st, &ip, None, "password_reset_request").await {
        let msg = "Too many attempts. Please try again later.";
        return Ok(if ajax { json_msg(false, msg, 429) } else { page(429, "error", msg) });
    }

    let user = sql!(
        r#"select id, login, password, email, (reset_token is not null and reset_expires > now()) as token_valid, reset_token from "user" where lower(email) = lower("#,
        email.trim().to_string(),
        ") order by id limit 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let identity = user.as_ref().map(|u| u.string("login")).unwrap_or_else(|| email.clone());

    // Per account a few a day (the per-IP limit above is one): with one, anyone could use up the
    // owner's request for the day by asking first
    if crate::security::failed_attempts_for_user(&st, &identity, "password_reset_request").await >= RESET_REQUESTS_PER_ACCOUNT {
        let msg = "Too many reset requests for this account. Please try again tomorrow.";
        return Ok(if ajax { json_msg(false, msg, 429) } else { page(429, "error", msg) });
    }
    crate::security::record_attempt(&st, &ip, Some(&identity), "password_reset_request", true).await;

    let neutral = "If an account with that email exists, we have sent a reset link.";
    // Unknown address or passkey-only account (no password to reset): same answer, no mail
    let Some(user) = user.filter(|u| u.str("password") != Some("!")) else {
        return Ok(if ajax { json_msg(true, neutral, 200) } else { page(200, "success", neutral) });
    };

    // A still valid link is sent again instead of replaced, so a request by someone else doesn't
    // void the link the owner already has
    let token = match user.str("reset_token").filter(|_| user.bool("token_valid")) {
        Some(t) => t.to_string(),
        None => {
            let token = crate::util::random_hex(32);
            let expires = chrono::Utc::now() + chrono::Duration::hours(1);
            sql!(r#"update "user" set reset_token = "#, token.clone(), ", reset_expires = ", expires, " where id = ", user.i64("id").unwrap_or(0) as i32)
                .execute(&st.db)
                .await?;
            token
        }
    };

    let link = format!("{}/?token={token}", cfg.url_full());
    let body = format!(
        "Hello,\n\nYou requested a password reset for your account. Please click the link below to set a new password:\n\n{link}\n\nThis link will expire in 1 hour.\n\nIf you did not request this, please ignore this email."
    );
    if cfg.truthy("smtp.enabled") {
        // To the stored address (not the typed spelling), and in the background: waiting for the
        // mail server made the answer slower for registered addresses than for unknown ones
        let to = user.string("email");
        let cfg = cfg.clone();
        tokio::spawn(async move {
            if let Err(e) = crate::smtp::send_mail(&cfg, crate::smtp::Mail { to: &to, subject: "Password Reset Request", body: &body }).await {
                tracing::error!("[SMTP] Send failed: {e}");
            }
        });
    } else {
        tracing::info!("[SMTP] No configuration found. Reset link: {link}");
    }

    Ok(if ajax { json_msg(true, neutral, 200) } else { page(200, "success", neutral) })
}

async fn reset_password_page(_st: AppState, req: Req) -> Res {
    match req.query("token") {
        Some(t) if !t.is_empty() => Ok(redirect(&format!("/?token={t}"))),
        _ => Ok(redirect("/")),
    }
}

async fn reset_password(st: AppState, mut req: Req) -> Res {
    let ip = req.real_ip();
    let ajax = is_ajax(&req);
    let token = req.post_str("token").await.filter(|v| !v.is_empty());
    let password = req.post_str("password").await.filter(|v| !v.is_empty());
    let confirm = req.post_str("password_confirm").await.filter(|v| !v.is_empty());
    // A modal's plain form post (w0bm without JavaScript): back to that page with the reset modal
    // open (the token goes back into the URL, only if it is a token at all), or the forgot modal
    // when the token is no good
    let return_to = if ajax { None } else { req.post_str("return_to").await.and_then(|p| modal_return_path(&p)) };
    let to_reset_modal = |token: &str, msg: &str| {
        let (path, code) = (return_to.as_deref()?, auth_code(AUTH_ERRORS, msg)?);
        let tok = if !token.is_empty() && token.chars().all(|c| c.is_ascii_hexdigit()) { token } else { "" };
        let sep = if path.contains('?') { '&' } else { '?' };
        Some(back_to_modal(&format!("{path}{sep}token={tok}"), "reset_error", code, "reset-modal"))
    };
    let page = |view: &str, data: Value| html(crate::tpl::render(&st, None, view, data));

    let token_raw = token.clone();
    let (Some(token), Some(password), Some(confirm)) = (token, password, confirm) else {
        let msg = "Missing data";
        if ajax {
            return Ok(json_msg(false, msg, 400));
        }
        return Ok(to_reset_modal(token_raw.as_deref().unwrap_or(""), msg).unwrap_or_else(|| redirect("/login")));
    };
    if password != confirm {
        let msg = "Passwords do not match";
        return Ok(if ajax {
            json_msg(false, msg, 200)
        } else {
            to_reset_modal(&token, msg).unwrap_or_else(|| page("reset-password", json!({ "token": token, "error": msg })))
        });
    }
    if password.encode_utf16().count() < 20 {
        let msg = "Password must be at least 20 characters long";
        return Ok(if ajax {
            json_msg(false, msg, 200)
        } else {
            to_reset_modal(&token, msg).unwrap_or_else(|| page("reset-password", json!({ "token": token, "error": msg })))
        });
    }
    if crate::security::is_rate_limited(&st, &ip, None, "password_reset_execution").await {
        let msg = "Too many reset attempts. Please try again later.";
        return Ok(if ajax {
            json_msg(false, msg, 429)
        } else {
            to_reset_modal(&token, msg).unwrap_or_else(|| html_code(crate::tpl::render(&st, None, "forgot-password", json!({ "error": msg })), 429))
        });
    }

    let user = sql!(r#"select id, login from "user" where reset_token = "#, token.clone(), " and reset_expires > now() and password <> '!' limit 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(user) = user else {
        let msg = "Invalid or expired reset token.";
        // without JavaScript from a modal: the forgot modal, to ask for a new link
        if let (false, Some(path), Some(code)) = (ajax, return_to.as_deref(), auth_code(AUTH_ERRORS, msg)) {
            return Ok(back_to_modal(path, "forgot_error", code, "forgot-modal"));
        }
        return Ok(if ajax { json_msg(false, msg, 200) } else { page("forgot-password", json!({ "error": msg })) });
    };
    let user_id = user.i64("id").unwrap_or(0);

    let hash = crate::util::hash_password(password).await;
    sql!(r#"update "user" set password = "#, hash, ", reset_token = NULL, reset_expires = NULL where id = ", user_id as i32).execute(&st.db).await?;
    crate::auth::destroy_user_sessions(&st, user_id).await?;
    // A reset is the way back into a lost account: an API key someone else made stops working too
    sql!("DELETE FROM user_api_keys WHERE user_id = ", user_id as i32).execute(&st.db).await?;
    crate::security::record_attempt(&st, &ip, user.str("login"), "password_reset_execution", true).await;

    let msg = "Password reset successfully! You can now login with your new password.";
    if let (false, Some(path), Some(code)) = (ajax, return_to.as_deref(), auth_code(AUTH_DONE, msg)) {
        return Ok(back_to_modal(path, "login_done", code, "login-modal"));
    }
    Ok(if ajax { json_msg(true, msg, 200) } else { page("reset-password", json!({ "success": msg })) })
}

// ── Registration ────────────────────────────────────────────────────────────

async fn register_page(_st: AppState, req: Req) -> Res {
    if req.session().is_some() {
        return Ok(location(302, "/?already_logged_in=1"));
    }
    // #register-modal: f0ckm.js opens the modal on it, and without JavaScript :target does. The
    // invite token goes in the query, so the template can fill it in for both.
    let url = match req.query("token").filter(|t| !t.is_empty()) {
        Some(t) => format!("/?invite={}#register-modal", crate::util::encode_uri_component(t)),
        None => "/#register-modal".to_string(),
    };
    Ok(location(302, &url))
}

async fn activate(st: AppState, req: Req) -> Res {
    let token = req.param("token").unwrap_or("").to_string();
    let user = sql!(r#"SELECT id FROM "user" WHERE activation_token = "#, token).fetch_optional(&st.db).await?;
    let Some(user) = user else {
        let msg = crate::util::encode_uri_component("Invalid or expired activation token.");
        return Ok(location(302, &format!("/#register:error:{msg}")));
    };
    sql!(r#"UPDATE "user" SET activated = TRUE, activation_token = NULL WHERE id = "#, user.i64("id").unwrap_or(0) as i32).execute(&st.db).await?;
    let msg = crate::util::encode_uri_component("Account activated! You can now login.");
    Ok(location(302, &format!("/#register:success:{msg}")))
}

/// SQLSTATE of a database error inside an anyhow chain.
fn pg_code(e: &anyhow::Error) -> Option<String> {
    e.chain()
        .find_map(|c| c.downcast_ref::<sqlx::Error>())
        .and_then(|e| e.as_database_error())
        .and_then(|d| d.code().map(|c| c.into_owned()))
}

enum Created {
    Ok,
    TokenGone,
}

pub(crate) struct NewUser<'a> {
    /// As typed; the login is its lower case
    pub username: &'a str,
    pub hash: &'a str,
    pub email: Option<&'a str>,
    pub activated: bool,
    pub activation_token: Option<&'a str>,
}

/// Registration by application: either an invite token or an application, never both. The answer
/// to the forms' "Do you have an invite token?" (`has_invite`: `yes` / `no`) picks one and drops what
/// was typed into the other (a field hidden after switching the answer is still sent). Without an
/// answer (older forms, scripts) both filled in is refused, rather than trying only the token and
/// losing the application. Returns the token and application to go on with.
fn invite_or_application(has_invite: Option<&str>, token: Option<String>, application: String) -> Result<(Option<String>, String), &'static str> {
    match has_invite {
        Some("yes") if token.is_none() => Err("Invite token is required for registration."),
        Some("yes") => Ok((token, String::new())),
        Some("no") => Ok((None, application)),
        _ if token.is_some() && !application.is_empty() => Err("Fill in either an invite token or an application, not both."),
        _ => Ok((token, application)),
    }
}

/// Insert a regular account with its default options (registration, an approved application).
/// The new user's id.
pub(crate) async fn insert_user(st: &AppState, conn: &mut sqlx::PgConnection, u: NewUser<'_>) -> anyhow::Result<i32> {
    let cfg = st.cfg();
    let id: i32 = sql!(
        r#"insert into "user" ("login", "password", "user", "created_at", "admin", "is_moderator", "email", "activated", "activation_token") values ("#,
        u.username.to_lowercase(), ", ", u.hash.to_string(), ", ", u.username.to_string(), ", to_timestamp(", crate::util::now_secs() as f64,
        "), false, false, ", u.email.map(str::to_string), ", ", u.activated, ", ", u.activation_token.map(str::to_string), ") returning id"
    )
    .fetch_scalar::<i32, _>(&mut *conn)
    .await?
    .ok_or_else(|| anyhow::anyhow!("user insert returned no id"))?;
    sql!(
        "insert into user_options (user_id, mode, theme, fullscreen, avatar, avatar_file, use_new_layout, disable_autoplay, disable_swiping, use_alternative_infobox) values (",
        id, ", 3, 'amoled', 0, null, 'default.png', ", st.settings.default_layout() == "modern", ", ", cfg.is_false("websrv.enable_autoplay"), ", ",
        cfg.is_false("websrv.enable_swiping"), ", ", cfg.enabled("websrv.user_alternative_infobox"), ")"
    )
    .execute(&mut *conn)
    .await?;
    Ok(id)
}

async fn register_post(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let ajax = is_ajax(&req);
    // The navbar modal posted as a plain form (no JavaScript): answer with a redirect back to that page
    let return_to = if ajax { None } else { req.post_str("return_to").await.and_then(|p| modal_return_path(&p)) };
    let username = req.post_str("username").await.map(|u| u.trim().to_string());
    let email = req.post_str("email").await.filter(|e| !e.is_empty());
    let password = req.post_str("password").await.filter(|p| !p.is_empty());
    let password_confirm = req.post_str("password_confirm").await;
    let mut token = req.post_str("token").await.map(|t| t.trim().to_string()).filter(|t| !t.is_empty());
    let mut application = req.post_str("application").await.map(|m| m.trim().to_string()).unwrap_or_default();
    // The application forms ask "Do you have an invite token?" first: `yes` or `no`
    let has_invite = req.post_str("has_invite").await;
    let honeypot = req.post_str("email_confirm_field").await.filter(|h| !h.is_empty());
    let captcha = req.post_str("cap-token").await;
    let cap_image_id = req.post_str("cap-image-id").await;
    let cap_image_answer = req.post_str("cap-image-answer").await;
    let theme = req.cookie("theme").map(str::to_string).unwrap_or_else(|| cfg.str("websrv.theme").filter(|t| !t.is_empty()).unwrap_or("f0ck").to_string());
    let registration_open = st.settings.registration_open(&cfg);

    if honeypot.is_some() {
        tracing::info!("[SPAM] Honeypot triggered by IP: {ip}, User: {}", username.as_deref().unwrap_or(""));
        return Ok(json_msg(false, "Spam detected.", 200));
    }

    let page = |key: &str, msg: &str| {
        if let Some(path) = return_to.as_deref() {
            let (qkey, table) = if key == "success" { ("register_done", AUTH_DONE) } else { ("register_error", AUTH_ERRORS) };
            if let Some(code) = auth_code(table, msg) {
                return back_to_modal(path, qkey, code, "register-modal");
            }
        }
        html(crate::tpl::render(&st, Some(&req), "register", json!({ "theme": theme, key: msg, "registration_open": registration_open })))
    };

    // Register always needs the captcha (when on); after repeated failures from this IP a hard one
    let captcha_on = crate::cap::required(&cfg, &req);
    let hard_captcha = match crate::security::auth_gate(&st, &ip, "register", captcha_on, false).await {
        crate::security::AuthGate::Locked => {
            let msg = "Too many attempts.";
            return Ok(if ajax { json_msg(false, msg, 200) } else { page("error", msg) });
        }
        // Also hard after repeated failed logins, like the challenge the widget got (cap::challenge)
        crate::security::AuthGate::Open { captcha, .. } => captcha_on && (captcha || crate::security::needs_hard_captcha(&st, &ip).await),
    };

    macro_rules! render_error {
        ($msg:expr) => {{
            let msg: &str = $msg;
            crate::security::record_attempt(&st, &ip, username.as_deref(), "register", false).await;
            return Ok(if ajax { json_msg(false, msg, 200) } else { page("error", msg) });
        }};
    }

    let Some(username) = username.clone().filter(|u| !u.is_empty()) else { render_error!("Username is required.") };
    static NAME: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^[a-zA-Z0-9._-]+$").unwrap());
    if !NAME.is_match(&username) {
        render_error!("Username contains invalid characters. Only A-Z, 0-9, _, -, and . are allowed.");
    }
    // anon_* logins are reserved for anonymous shadow users
    if username.to_lowercase().starts_with("anon_") || username.to_lowercase() == "anonymous" {
        render_error!("Username taken");
    }
    // JS `.length` counts UTF-16 units
    let Some(password) = password.filter(|p| p.encode_utf16().count() >= 20) else {
        render_error!("Password must be at least 20 characters long.")
    };
    if password_confirm.as_deref() != Some(password.as_str()) {
        render_error!("Passwords do not match.");
    }

    if captcha_on {
        let checked = crate::cap::verify_form(captcha.as_deref(), cap_image_id.as_deref(), cap_image_answer.as_deref(), hard_captcha);
        match checked {
            Captcha::Ok => {}
            Captcha::Missing => render_error!("Please complete the captcha."),
            Captcha::Failed => render_error!("Captcha expired or invalid. Please try again."),
        }
    }

    let mut activated = true;
    let mut activation_token: Option<String> = None;
    if !registration_open && st.settings.registration_applications(&cfg) {
        match invite_or_application(has_invite.as_deref(), token.take(), std::mem::take(&mut application)) {
            Ok((t, a)) => (token, application) = (t, a),
            Err(msg) => render_error!(msg),
        }
    }
    // No token while registration is closed: an application, when the site takes them
    let applying = !registration_open && token.is_none() && st.settings.registration_applications(&cfg);
    if !registration_open && token.is_none() && !applying {
        render_error!("Invite token is required for registration.");
    }
    if applying {
        // An anonymous passkey identity has its own way (invite requests in the settings)
        if req.session().is_some_and(|s| s.is_anon) {
            render_error!("Anonymous accounts request an invite in their settings.");
        }
        let len = application.chars().count();
        if len == 0 {
            render_error!("An invite token or a message why you want an account is required.");
        }
        if !(APPLICATION_MSG_MIN..=APPLICATION_MSG_MAX).contains(&len) {
            render_error!("Tell us in 20 to 1000 characters why you want an account.");
        }
        if st.settings.applications_require_email() && !email.as_deref().is_some_and(valid_email) {
            render_error!("A valid email is required to apply.");
        }
    }
    if let Some(t) = &token {
        let ok = sql!(
            "select id from invite_tokens where token = ", t.clone(), " and is_used = false and created_at > ",
            crate::util::now_secs() - INVITE_TOKEN_TTL_SECS
        )
        .fetch_optional(&st.db)
        .await?;
        if ok.is_none() {
            render_error!("Invalid, used or expired invite token");
        }
    } else if !applying && crate::settings::registration_require_mail_andor_token(&cfg) {
        if !email.as_deref().is_some_and(|e| e.contains('@')) {
            render_error!("A valid email is required for registration.");
        }
        activated = false;
        activation_token = Some(crate::util::random_hex(32));
    }

    // Any email given has to look like one (login matches an email only when the input has an `@`)
    if email.as_deref().is_some_and(|e| !valid_email(e)) {
        render_error!("A valid email is required for registration.");
    }

    let mut q = sql!(r#"select id, login, email from "user" where "login" = "#, username.to_lowercase(), r#" or "user" = "#, username.clone());
    if let Some(e) = &email {
        q.push(r#" or ("email" is not null and lower("email") = lower("#).bind(e.clone()).push("))");
    }
    let existing = q.fetch_all(&st.db).await?;
    if !existing.is_empty() {
        let em = email.as_deref().unwrap_or("").to_lowercase();
        if existing.iter().any(|u| u.str("email").is_some_and(|e| !e.is_empty() && e.to_lowercase() == em)) {
            render_error!("Email already registered");
        }
        render_error!("Username taken");
    }
    // Names and emails of applications waiting for an answer are taken as well
    let mut q = sql!("select email from account_applications where status = 'pending' and (login = ", username.to_lowercase());
    if let Some(e) = &email {
        q.push(" or (email is not null and lower(email) = lower(").bind(e.clone()).push("))");
    }
    q.push(") limit 1");
    if let Some(open) = q.fetch_optional(&st.db).await? {
        let em = email.as_deref().unwrap_or("").to_lowercase();
        if open.str("email").is_some_and(|e| !e.is_empty() && e.to_lowercase() == em) {
            render_error!("Email already registered");
        }
        render_error!("Username taken");
    }

    let hash = crate::util::hash_password(password).await;

    if applying {
        let ip_hash = (!ip.is_empty()).then(|| crate::security::hash_ip(&cfg, &ip));
        // One open application per address, and a pause after a declined one
        if let Some(h) = &ip_hash {
            let last = sql!(
                "select status from account_applications where ip_hash = ", h.clone(),
                " and (status = 'pending' or (status = 'denied' and reviewed_at > now() - make_interval(days => ", APPLICATION_COOLDOWN_DAYS,
                "))) order by (status = 'pending') desc limit 1"
            )
            .fetch_optional(&st.db)
            .await?;
            match last.as_ref().and_then(|r| r.str("status")) {
                Some("pending") => render_error!("You already have a pending application."),
                Some(_) => render_error!("Your last application was declined. You can apply again later."),
                None => {}
            }
        }
        let inserted = sql!(
            "insert into account_applications (login, username, password, email, message, ip_hash) values (", username.to_lowercase(), ", ",
            username.clone(), ", ", hash, ", ", email.clone(), ", ", application.clone(), ", ", ip_hash, ")"
        )
        .execute(&st.db)
        .await;
        if let Err(e) = inserted {
            // The unique index on pending logins: someone applied with this name a moment ago
            if pg_code(&e).as_deref() == Some("23505") {
                render_error!("Username taken");
            }
            tracing::error!("[APPLICATIONS] Insert failed: {e:#}");
            render_error!("An unexpected error occurred. Please try again later.");
        }
        // Admins review them under /admin/applications
        let admins: Vec<i32> = sql!(r#"SELECT id FROM "user" WHERE admin = true"#).fetch_scalar_list(&st.db).await.unwrap_or_default();
        if !admins.is_empty() {
            let data = json!({ "username": username, "reason": application.chars().take(100).collect::<String>() });
            let sent = sql!(
                "INSERT INTO notifications (user_id, type, reference_id, data) SELECT a, 'account_application', 0, ", data,
                "::jsonb FROM unnest(", admins, "::int[]) AS a"
            )
            .execute(&st.db)
            .await;
            if let Err(e) = sent {
                tracing::error!("[APPLICATIONS] Notifying admins failed: {e:#}");
            }
        }
        tracing::info!("[APPLICATIONS] New application from {username}");
        crate::security::record_attempt(&st, &ip, Some(&username), "register", true).await;
        return Ok(if ajax { json_raw(&json!({ "success": true, "msg": APPLICATION_SENT, "applied": true }), 200) } else { page("success", APPLICATION_SENT) });
    }
    // An anonymous passkey identity registering upgrades its shadow user in place (favs, comments,
    // uploads and passkeys stay); dropping the anon_identities row makes it a regular account.
    let upgrade_id = req.session().filter(|s| s.is_anon).map(|s| s.id);

    let created: anyhow::Result<(i64, Created)> = async {
        let mut tx = st.db.begin().await?;
        let user_id = if let Some(uid) = upgrade_id {
            let old = sql!(r#"select login from "user" where id = "#, uid as i32, " for update").fetch_optional(&mut *tx).await?;
            if let Some(old_login) = old.as_ref().and_then(|o| o.str("login")).filter(|l| !l.is_empty()) {
                sql!("update items set username = ", username.clone(), " where lower(username) = lower(", old_login.to_string(), ")").execute(&mut *tx).await?;
            }
            sql!(
                r#"update "user" set "login" = "#, username.to_lowercase(), r#", "password" = "#, hash.clone(), r#", "user" = "#, username.clone(),
                r#", "email" = "#, email.clone(), r#", "activated" = "#, activated, r#", "activation_token" = "#, activation_token.clone(),
                " where id = ", uid as i32
            )
            .execute(&mut *tx)
            .await?;
            sql!("update user_options set display_name = null, avatar_file = coalesce(avatar_file, 'default.png') where user_id = ", uid as i32)
                .execute(&mut *tx)
                .await?;
            sql!("delete from anon_identities where user_id = ", uid as i32).execute(&mut *tx).await?;
            uid
        } else {
            let new = NewUser { username: &username, hash: &hash, email: email.as_deref(), activated, activation_token: activation_token.as_deref() };
            insert_user(&st, &mut tx, new).await? as i64
        };
        // The invite token is consumed in the same transaction, so a token can't be used twice
        if let Some(t) = &token {
            let used = sql!(
                "update invite_tokens set is_used = true, used_by = ", user_id as i32, ", used_at = now() where token = ", t.clone(),
                " and is_used = false"
            )
            .execute(&mut *tx)
            .await?;
            if used == 0 {
                tx.rollback().await?;
                return Ok((user_id, Created::TokenGone));
            }
        }
        tx.commit().await?;
        Ok((user_id, Created::Ok))
    }
    .await;

    let user_id = match created {
        Ok((_, Created::TokenGone)) => render_error!("Invalid or used invite token"),
        Ok((id, Created::Ok)) => id,
        Err(e) => {
            tracing::error!("[REGISTER] DB Error during user creation: {e:#}");
            if pg_code(&e).as_deref() == Some("23505") {
                render_error!("Username taken");
            }
            render_error!("An unexpected error occurred. Please try again later.");
        }
    };
    if upgrade_id.is_some() {
        if let Some(c) = req.cookie("session") {
            st.invalidate_session_cache(&crate::util::sha256(c));
        }
    }

    if !activated {
        let link = format!("{}/activate/{}", cfg.url_full(), activation_token.as_deref().unwrap_or(""));
        let body = format!(
            "Hello {username},\n\nThank you for registering. Please activate your account by clicking the link below:\n\n{link}\n\nIf you did not request this, please ignore this email."
        );
        if cfg.truthy("smtp.host") {
            let to = email.as_deref().unwrap_or("");
            if let Err(e) = crate::smtp::send_mail(&cfg, crate::smtp::Mail { to, subject: "Activate your account", body: &body }).await {
                tracing::error!("[SMTP] Send failed: {e}");
            }
        } else {
            tracing::info!("[SMTP] No configuration found. Activation link: {link}");
        }
        if let Some(uid) = upgrade_id {
            // Unactivated accounts may not stay logged in until the email link is used
            crate::auth::destroy_user_sessions(&st, uid).await?;
            req.set_cookie(crate::auth::clear_session_cookie(&cfg));
        }
        let msg = "Registration successful! Please check your email to activate your account.";
        return Ok(if ajax { json_msg(true, msg, 200) } else { page("success", msg) });
    }
    let _ = user_id;

    crate::security::record_attempt(&st, &ip, Some(&username), "register", true).await;

    let msg = if upgrade_id.is_some() { "Username claimed, you can still use your passkey." } else { "Registration successful! You can now login." };
    if ajax {
        return Ok(json_raw(&json!({ "success": true, "msg": msg, "upgraded": upgrade_id.is_some() }), 200));
    }
    if let (Some(path), None) = (return_to.as_deref(), upgrade_id) {
        return Ok(back_to_modal(path, "login_done", "registered", "login-modal"));
    }
    Ok(location(302, if upgrade_id.is_some() { "/settings" } else { "/?login=success" }))
}

#[cfg(test)]
mod modal_return_tests {
    use super::{AUTH_DONE, AUTH_ERRORS, auth_code, auth_message, modal_return_path};

    #[test]
    fn return_path_stays_local() {
        assert_eq!(modal_return_path("/3").as_deref(), Some("/3"));
        assert_eq!(modal_return_path("/user/x?page=2&login_error=wrong#c5").as_deref(), Some("/user/x?page=2"));
        assert_eq!(modal_return_path("/?login=1&login_error=invalid").as_deref(), Some("/"));
        assert_eq!(modal_return_path("/?invite=abc&register_error=pw_short&register_done=activate").as_deref(), Some("/?invite=abc"));
        assert_eq!(modal_return_path("/?login_error=captcha_needed&login_captcha=1").as_deref(), Some("/"));
        assert_eq!(modal_return_path("//evil.example/"), None);
        assert_eq!(modal_return_path("/\\evil.example/"), None);
        assert_eq!(modal_return_path("https://evil.example/"), None);
        assert_eq!(modal_return_path("/a\r\nSet-Cookie: x=1"), None);
    }

    #[test]
    fn messages_travel_as_codes() {
        assert_eq!(auth_code(AUTH_ERRORS, "user doesn't exist or wrong password"), Some("wrong"));
        assert_eq!(auth_code(AUTH_ERRORS, "Captcha expired or invalid. Please try again."), Some("captcha_invalid"));
        assert_eq!(auth_code(AUTH_ERRORS, "Spam detected."), None);
        assert_eq!(auth_code(AUTH_DONE, "Registration successful! You can now login."), Some("registered"));
        assert_eq!(auth_message("register_error", "pw_mismatch"), Some("Passwords do not match."));
        // a success code is no error and the other way round
        assert_eq!(auth_message("login_error", "registered"), None);
        assert_eq!(auth_message("login_done", "wrong"), None);
        assert_eq!(auth_message("login_error", "<b>made up</b>"), None);
    }

    #[test]
    fn password_reset_codes() {
        assert_eq!(modal_return_path("/?forgot_error=email_missing&forgot_done=reset_sent&reset_error=reset_short").as_deref(), Some("/"));
        assert_eq!(auth_code(AUTH_ERRORS, "Passwords do not match"), Some("reset_mismatch"));
        assert_eq!(auth_code(AUTH_ERRORS, "Invalid or expired reset token."), Some("reset_token"));
        assert_eq!(auth_message("forgot_done", "reset_sent"), Some("If an account with that email exists, we have sent a reset link."));
        assert_eq!(auth_message("login_done", "pw_reset"), Some("Password reset successfully! You can now login with your new password."));
        assert_eq!(auth_message("reset_error", "reset_short"), Some("Password must be at least 20 characters long"));
        assert_eq!(auth_message("forgot_error", "reset_sent"), None);
        // every message has its own code (auth_code finds the first)
        for table in [AUTH_ERRORS, AUTH_DONE] {
            for (i, (c, m)) in table.iter().enumerate() {
                assert!(table[i + 1..].iter().all(|(c2, m2)| c2 != c && m2 != m), "{c} twice");
            }
        }
    }
}

#[cfg(test)]
mod low_severity_tests {
    #[test]
    fn emails() {
        assert!(super::valid_email("a@b.example"));
        for bad in ["admin", "@x", "a@", "a b@c.d", "a@b\n"] {
            assert!(!super::valid_email(bad), "{bad:?}");
        }
    }

    #[test]
    fn invite_token_or_application() {
        use super::invite_or_application as pick;
        let tok = || Some("abc".to_string());
        let app = || "please let me in, I upload a lot".to_string();
        // the answer keeps one and drops what was typed into the other field
        assert_eq!(pick(Some("yes"), tok(), app()), Ok((tok(), String::new())));
        assert_eq!(pick(Some("no"), tok(), app()), Ok((None, app())));
        assert_eq!(pick(Some("yes"), None, app()), Err("Invite token is required for registration."));
        assert_eq!(pick(Some("no"), None, String::new()), Ok((None, String::new())));
        // no answer: both filled in is refused, one alone goes through as before
        assert_eq!(pick(None, tok(), app()), Err("Fill in either an invite token or an application, not both."));
        assert_eq!(pick(Some("maybe"), tok(), app()), Err("Fill in either an invite token or an application, not both."));
        assert_eq!(pick(None, tok(), String::new()), Ok((tok(), String::new())));
        assert_eq!(pick(None, None, app()), Ok((None, app())));
        // the refusal travels to the no-JS modal as a code
        assert_eq!(super::auth_code(super::AUTH_ERRORS, "Fill in either an invite token or an application, not both."), Some("app_both"));
    }

    #[test]
    fn logout_links_need_the_session_token() {
        let t = crate::auth::logout_token("csrf-a");
        assert_eq!(t.len(), 16);
        assert_ne!(t, crate::auth::logout_token("csrf-b"));
        assert_eq!(crate::auth::logout_url(""), "/logout");
        assert_eq!(crate::auth::logout_url("csrf-a"), format!("/logout?t={t}"));
    }
}
