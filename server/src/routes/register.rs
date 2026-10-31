//! Accounts: registration and activation (src/inc/routes/register.mjs) plus login, logout and the
//! password-reset flow (the auth routes at the top of src/inc/routes/admin.mjs).

use crate::auth::{SessionSpec, is_ajax};
use crate::cap::Captcha;
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

async fn login_page(_st: AppState, req: Req) -> Res {
    if req.session().is_some() {
        return Ok(location(302, "/?already_logged_in=1"));
    }
    Ok(redirect("/?login=1"))
}

/// Verified against when the account doesn't exist, so both paths cost one Argon2 run.
static DUMMY_HASH: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

async fn login(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let username = req.post_str("username").await;
    let password = req.post_str("password").await;
    let kmsi = req.post().await?.get("kmsi").is_some();
    let ajax = is_ajax(&req);

    if crate::security::is_rate_limited(&st, &ip, None, "login").await {
        let msg = "Too many attempts.";
        if ajax {
            return Ok(json_msg(false, msg, 429));
        }
        return Ok(reply(429, "text/html", msg));
    }

    let fail = |msg: &'static str| {
        let st = st.clone();
        let ip = ip.clone();
        let username = username.clone();
        let theme = req.theme.clone();
        async move {
            crate::security::record_attempt(&st, &ip, username.as_deref(), "login", false).await;
            // Slows down brute force and evens out timing
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if ajax {
                return json_msg(false, msg, 200);
            }
            html(crate::tpl::render(&st, None, "login", json!({ "error": msg, "theme": theme })))
        }
    };

    let (Some(username), Some(password)) = (username.clone().filter(|u| !u.is_empty()), password.filter(|p| !p.is_empty())) else {
        return Ok(fail("Invalid username or password.").await);
    };

    let user = sql!(
        r#"select id, password, activated, banned, ban_reason, ban_expires, force_password_change from "user" where "login" = "#,
        username.to_lowercase(), " or lower(email) = lower(", username.trim().to_string(), ") limit 1"
    )
    .fetch_optional(&st.db)
    .await?;

    let Some(user) = user else {
        let dummy = DUMMY_HASH.get_or_init(|| crate::util::hash_password(crate::util::random_hex(16))).await;
        let _ = crate::util::verify_password(password, dummy.clone()).await;
        return Ok(fail("user doesn't exist or wrong password").await);
    };
    let stored_hash = user.string("password");
    if !crate::util::verify_password(password.clone(), stored_hash.clone()).await {
        return Ok(fail("user doesn't exist or wrong password").await);
    }
    // activated is nullable (default true): only an explicit false blocks
    if user.val("activated") == &Value::Bool(false) {
        return Ok(fail("This account is not activated. Please check your email.").await);
    }
    let user_id = user.i64("id").unwrap_or(0);

    if user.bool("banned") {
        let expires = user.str("ban_expires").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok());
        if expires.is_some_and(|e| e < chrono::Utc::now()) {
            sql!(r#"update "user" set banned = false, ban_reason = null, ban_expires = null where id = "#, user_id as i32).execute(&st.db).await?;
        } else {
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

    crate::security::record_attempt(&st, &ip, Some(&username), "login", true).await;
    crate::security::clear_attempts(&st, &ip, Some(&username)).await;

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

    let mut r = location(301, "/");
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(&crate::auth::session_cookie(&cfg, &s.session, kmsi)) {
        h.insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

async fn logout(st: AppState, req: Req) -> Res {
    crate::auth::destroy_session(&st, &req).await?;
    let mut r = location(301, "/");
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, public"));
    if let Ok(v) = HeaderValue::from_str(&crate::auth::clear_session_cookie(&st.cfg())) {
        h.insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

// ── Password reset ──────────────────────────────────────────────────────────

async fn forgot_password_page(_st: AppState, _req: Req) -> Res {
    Ok(redirect("/"))
}

async fn forgot_password(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let ajax = is_ajax(&req);
    let email = req.post_str("email").await.filter(|e| !e.is_empty());
    let page = |code: u16, key: &str, msg: &str| html_code(crate::tpl::render(&st, None, "forgot-password", json!({ key: msg })), code);

    let Some(email) = email else {
        let msg = "Email is required";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page(200, "error", msg) });
    };

    if crate::security::is_rate_limited(&st, &ip, None, "password_reset_request").await {
        let msg = "Too many attempts. Please try again later.";
        return Ok(if ajax { json_msg(false, msg, 429) } else { page(429, "error", msg) });
    }

    let user = sql!(r#"select id, login, password from "user" where lower(email) = lower("#, email.trim().to_string(), ") limit 1")
        .fetch_optional(&st.db)
        .await?;
    let identity = user.as_ref().map(|u| u.string("login")).unwrap_or_else(|| email.clone());

    if crate::security::is_rate_limited(&st, &ip, Some(&identity), "password_reset_request").await {
        let msg = "Only one reset request per 24 hours is allowed for this account.";
        return Ok(if ajax { json_msg(false, msg, 429) } else { page(429, "error", msg) });
    }
    crate::security::record_attempt(&st, &ip, Some(&identity), "password_reset_request", true).await;

    let neutral = "If an account with that email exists, we have sent a reset link.";
    // Unknown address or passkey-only account (no password to reset): same answer, no mail
    let Some(user) = user.filter(|u| u.str("password") != Some("!")) else {
        return Ok(if ajax { json_msg(true, neutral, 200) } else { page(200, "success", neutral) });
    };

    let token = crate::util::random_hex(32);
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    sql!(r#"update "user" set reset_token = "#, token.clone(), ", reset_expires = ", expires, " where id = ", user.i64("id").unwrap_or(0) as i32)
        .execute(&st.db)
        .await?;

    let link = format!("{}/?token={token}", cfg.url_full());
    let body = format!(
        "Hello,\n\nYou requested a password reset for your account. Please click the link below to set a new password:\n\n{link}\n\nThis link will expire in 1 hour.\n\nIf you did not request this, please ignore this email."
    );
    if cfg.truthy("smtp.enabled") {
        if let Err(e) = crate::smtp::send_mail(&cfg, crate::smtp::Mail { to: &email, subject: "Password Reset Request", body: &body }).await {
            tracing::error!("[SMTP] Send failed: {e}");
        }
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
    let page = |view: &str, data: Value| html(crate::tpl::render(&st, None, view, data));

    let (Some(token), Some(password), Some(confirm)) = (token, password, confirm) else {
        return Ok(if ajax { json_msg(false, "Missing data", 400) } else { redirect("/login") });
    };
    if password != confirm {
        let msg = "Passwords do not match";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page("reset-password", json!({ "token": token, "error": msg })) });
    }
    if password.encode_utf16().count() < 20 {
        let msg = "Password must be at least 20 characters long";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page("reset-password", json!({ "token": token, "error": msg })) });
    }
    if crate::security::is_rate_limited(&st, &ip, None, "password_reset_execution").await {
        let msg = "Too many reset attempts. Please try again later.";
        return Ok(if ajax {
            json_msg(false, msg, 429)
        } else {
            html_code(crate::tpl::render(&st, None, "forgot-password", json!({ "error": msg })), 429)
        });
    }

    let user = sql!(r#"select id, login from "user" where reset_token = "#, token.clone(), " and reset_expires > now() and password <> '!' limit 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(user) = user else {
        let msg = "Invalid or expired reset token.";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page("forgot-password", json!({ "error": msg })) });
    };
    let user_id = user.i64("id").unwrap_or(0);

    let hash = crate::util::hash_password(password).await;
    sql!(r#"update "user" set password = "#, hash, ", reset_token = NULL, reset_expires = NULL where id = ", user_id as i32).execute(&st.db).await?;
    crate::auth::destroy_user_sessions(&st, user_id).await?;
    crate::security::record_attempt(&st, &ip, user.str("login"), "password_reset_execution", true).await;

    let msg = "Password reset successfully! You can now login with your new password.";
    Ok(if ajax { json_msg(true, msg, 200) } else { page("reset-password", json!({ "success": msg })) })
}

// ── Registration ────────────────────────────────────────────────────────────

async fn register_page(_st: AppState, req: Req) -> Res {
    if req.session().is_some() {
        return Ok(location(302, "/?already_logged_in=1"));
    }
    let mut url = "/#register".to_string();
    if let Some(t) = req.query("token").filter(|t| !t.is_empty()) {
        url.push(':');
        url.push_str(t);
    }
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

async fn register_post(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let ip = req.real_ip();
    let ajax = is_ajax(&req);
    let username = req.post_str("username").await.map(|u| u.trim().to_string());
    let email = req.post_str("email").await.filter(|e| !e.is_empty());
    let password = req.post_str("password").await.filter(|p| !p.is_empty());
    let password_confirm = req.post_str("password_confirm").await;
    let token = req.post_str("token").await.filter(|t| !t.is_empty());
    let honeypot = req.post_str("email_confirm_field").await.filter(|h| !h.is_empty());
    let captcha = req.post_str("cap-token").await;
    let theme = req.cookie("theme").map(str::to_string).unwrap_or_else(|| cfg.str("websrv.theme").filter(|t| !t.is_empty()).unwrap_or("f0ck").to_string());
    let registration_open = st.settings.registration_open(&cfg);

    if honeypot.is_some() {
        tracing::info!("[SPAM] Honeypot triggered by IP: {ip}, User: {}", username.as_deref().unwrap_or(""));
        return Ok(json_msg(false, "Spam detected.", 200));
    }

    let page = |key: &str, msg: &str| html(crate::tpl::render(&st, Some(&req), "register", json!({ "theme": theme, key: msg, "registration_open": registration_open })));

    if crate::security::is_rate_limited(&st, &ip, None, "register").await {
        let msg = "Too many attempts.";
        return Ok(if ajax { json_msg(false, msg, 200) } else { page("error", msg) });
    }

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

    if crate::cap::required(&cfg, &req) {
        match crate::cap::verify(captcha.as_deref()) {
            Captcha::Ok => {}
            Captcha::Missing => render_error!("Please complete the captcha."),
            Captcha::Failed => render_error!("Captcha expired or invalid. Please try again."),
        }
    }

    let mut activated = true;
    let mut activation_token: Option<String> = None;
    if !registration_open && token.is_none() {
        render_error!("Invite token is required for registration.");
    }
    if let Some(t) = &token {
        let ok = sql!("select id from invite_tokens where token = ", t.clone(), " and is_used = false").fetch_optional(&st.db).await?;
        if ok.is_none() {
            render_error!("Invalid or used invite token");
        }
    } else if crate::settings::registration_require_mail_andor_token(&cfg) {
        if !email.as_deref().is_some_and(|e| e.contains('@')) {
            render_error!("A valid email is required for registration.");
        }
        activated = false;
        activation_token = Some(crate::util::random_hex(32));
    }

    let mut q = sql!(r#"select id, login, email from "user" where "login" = "#, username.to_lowercase(), r#" or "user" = "#, username.clone());
    if let Some(e) = &email {
        q.push(r#" or ("email" is not null and "email" = "#).bind(e.clone()).push(")");
    }
    let existing = q.fetch_all(&st.db).await?;
    if !existing.is_empty() {
        let em = email.as_deref().unwrap_or("").to_lowercase();
        if existing.iter().any(|u| u.str("email").is_some_and(|e| !e.is_empty() && e.to_lowercase() == em)) {
            render_error!("Email already registered");
        }
        render_error!("Username taken");
    }

    let hash = crate::util::hash_password(password).await;
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
            let id: i32 = sql!(
                r#"insert into "user" ("login", "password", "user", "created_at", "admin", "is_moderator", "email", "activated", "activation_token") values ("#,
                username.to_lowercase(), ", ", hash.clone(), ", ", username.clone(), ", to_timestamp(", crate::util::now_secs() as f64, "), false, false, ",
                email.clone(), ", ", activated, ", ", activation_token.clone(), ") returning id"
            )
            .fetch_scalar::<i32, _>(&mut *tx)
            .await?
            .ok_or_else(|| anyhow::anyhow!("user insert returned no id"))?;
            sql!(
                "insert into user_options (user_id, mode, theme, fullscreen, avatar, avatar_file, use_new_layout, disable_autoplay, disable_swiping, use_alternative_infobox) values (",
                id, ", 3, 'amoled', 0, null, 'default.png', ", st.settings.default_layout() == "modern", ", ", cfg.is_false("websrv.enable_autoplay"), ", ",
                cfg.is_false("websrv.enable_swiping"), ", ", cfg.enabled("websrv.user_alternative_infobox"), ")"
            )
            .execute(&mut *tx)
            .await?;
            id as i64
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
    Ok(location(302, if upgrade_id.is_some() { "/settings" } else { "/?login=success" }))
}
