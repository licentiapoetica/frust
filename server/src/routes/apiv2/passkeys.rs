//! Passkey management and passkey login for registered users — the `/api/v2/settings/passkeys…`
//! routes of src/inc/routes/apiv2/settings.mjs (the rest of that file lives in its own module).

use crate::auth::SessionSpec;
use crate::db::RowExt;
use crate::http::{Group, Guard, Req, Res, Router, json_resp};
use crate::sql;
use crate::state::AppState;
use crate::webauthn as wa;
use serde_json::{Value, json};
use std::future::Future;

/// Passkey-only accounts store this instead of a hash; it never verifies.
const PASSWORD_DISABLED: &str = "!";
const LAST_PASSKEY_MSG: &str = "This is your last passkey and password login is disabled. Set a password first, or add another passkey.";
const IN_USE_PASSKEY_MSG: &str = "This passkey is the one you are signed in with right now and cannot be removed from this session.";

pub fn register(r: &mut Router) {
    let mut g = Group::new(r, r"^\/api\/v2\/settings");
    g.get_g(r"\/passkeys$", Guard::RegisteredUser, |st, req| guarded("List", list(st, req)));
    g.post_g(r"\/passkeys\/register\/begin$", Guard::RegisteredUser, |st, req| guarded("register/begin", register_begin(st, req)));
    g.post_g(r"\/passkeys\/register\/finish$", Guard::RegisteredUser, |st, req| guarded("register/finish", register_finish(st, req)));
    g.post_g(r"\/passkeys\/delete$", Guard::RegisteredUser, |st, req| guarded("Delete", delete_post(st, req)));
    g.delete_g(r"\/passkeys\/([^/]+)$", Guard::RegisteredUser, |st, req| guarded("Delete", delete_legacy(st, req)));
    g.post(r"\/passkeys\/login\/begin$", |st, req| guarded("login/begin", login_begin(st, req)));
    g.post(r"\/passkeys\/login\/finish$", |st, req| guarded("login/finish", login_finish(st, req)));
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn fail(msg: &str, code: u16) -> Res {
    j(json!({ "success": false, "msg": msg }), code)
}

async fn guarded(name: &str, f: impl Future<Output = Res>) -> Res {
    match f.await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[PASSKEYS] {name} error: {}", e.0);
            fail("Internal server error", 500)
        }
    }
}

fn body_str(b: &Value, k: &str) -> Option<String> {
    b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn session_id(req: &Req) -> i64 {
    req.session().map(|s| s.id).unwrap_or(0)
}

/// These routes are exempt from the CSRF middleware (register/begin is called without a token),
/// but the state-changing ones get the token from the client: check it here.
async fn csrf_ok(req: &mut Req) -> bool {
    crate::app::validate_csrf(req).await
}

/// True when removing a passkey would leave a passkey-only account without a way to log in.
async fn is_last_passkey_of_passkey_only(st: &AppState, user_id: i64) -> anyhow::Result<bool> {
    let row = sql!(
        r#"select (u.password = "#, PASSWORD_DISABLED,
        r#") as disabled, (select count(*)::int from webauthn_credentials wc where wc.user_id = u.id) as n from "user" u where u.id = "#,
        user_id as i32
    )
    .fetch_optional(&st.db)
    .await?;
    Ok(row.is_some_and(|r| r.bool("disabled") && r.i64("n").unwrap_or(0) <= 1))
}

async fn list(st: AppState, req: Req) -> Res {
    let rows = sql!(
        "SELECT id, cred_id AS credential_id, name, aaguid, created_at, last_used FROM webauthn_credentials WHERE user_id = ",
        session_id(&req) as i32, " ORDER BY created_at DESC"
    )
    .fetch_all(&st.db)
    .await?;
    let in_use = crate::auth::session_passkey(&st, &req).await?;
    j(json!({ "success": true, "passkeys": wa::decorate_rows(rows, in_use.as_deref()) }), 200)
}

fn passkey_name(body: &Value) -> Option<String> {
    body_str(body, "name").map(|n| n.trim().chars().take(64).collect::<String>()).filter(|n| !n.is_empty())
}

async fn register_begin(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let uid = session_id(&req);
    let body = req.post_value().await;
    let name = passkey_name(&body);
    let s = req.session().cloned().unwrap_or_default();
    let user_name = if s.login.is_empty() { s.user.clone() } else { s.login.clone() };
    let display = s.display_name.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| s.user.clone());
    let mut options = wa::start_registration(
        &cfg,
        &req,
        wa::StartRegistration {
            kind: "user-register",
            user_id: Some(uid),
            user_handle: wa::user_handle_for(&st, uid).await?,
            user_name: &user_name,
            display_name: &display,
            exclude: wa::credential_ids(&st, uid).await?,
        },
    )?;
    if let (Some(n), Value::Object(o)) = (&name, &mut options) {
        o.insert("_passkeyName".into(), Value::String(n.clone()));
    }
    j(json!({ "success": true, "options": options, "passkey_name": name }), 200)
}

async fn register_finish(st: AppState, mut req: Req) -> Res {
    if !csrf_ok(&mut req).await {
        return fail("Invalid CSRF token", 403);
    }
    let cfg = st.cfg();
    let uid = session_id(&req);
    let body = req.post_value().await;
    if ["challenge", "clientDataJSON", "attestationObject", "credentialId"].iter().any(|k| body_str(&body, k).is_none()) {
        return fail("Missing required WebAuthn fields", 400);
    }
    let Ok(pending) = wa::take(&body_str(&body, "challenge").unwrap_or_default()) else { return fail("Challenge expired or invalid", 400) };
    if pending.kind != "user-register" || pending.user_id != Some(uid) {
        return fail("Challenge mismatch", 400);
    }
    let reg = match wa::finish_registration(&cfg, &req, pending, &body) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[PASSKEYS] Registration verification failed: {e}");
            return fail(&format!("Registration failed: {e}"), 400);
        }
    };
    let name = passkey_name(&body).unwrap_or_else(|| "Passkey".into());
    // The old upsert let a client-chosen credential id overwrite another user's passkey row
    // (name, sign count); a known id is refused instead.
    if wa::known(&st, &reg.cred_id).await? || !wa::store(&st.db, uid, &reg, &name).await? {
        return fail("This passkey is already registered.", 409);
    }
    j(json!({ "success": true, "credential_id": reg.cred_id, "name": name }), 200)
}

async fn delete_credential(st: &AppState, req: &Req, cred_id: &str) -> Res {
    if crate::auth::session_passkey(st, req).await?.as_deref() == Some(cred_id) {
        return fail(IN_USE_PASSKEY_MSG, 400);
    }
    let uid = session_id(req);
    if is_last_passkey_of_passkey_only(st, uid).await? {
        return fail(LAST_PASSKEY_MSG, 400);
    }
    let rows = sql!("DELETE FROM webauthn_credentials WHERE cred_id = ", cred_id, " AND user_id = ", uid as i32, " RETURNING id")
        .fetch_all(&st.db)
        .await?;
    if rows.is_empty() {
        return fail("Passkey not found", 404);
    }
    j(json!({ "success": true }), 200)
}

async fn delete_post(st: AppState, mut req: Req) -> Res {
    if !csrf_ok(&mut req).await {
        return fail("Invalid CSRF token", 403);
    }
    let body = req.post_value().await;
    let Some(cred_id) = body_str(&body, "credential_id") else { return fail("Missing credential_id", 400) };
    delete_credential(&st, &req, &cred_id).await
}

async fn delete_legacy(st: AppState, req: Req) -> Res {
    let last = req.path.rsplit('/').next().unwrap_or("");
    let cred_id = crate::util::decode_uri_component(last).unwrap_or_else(|| last.to_string());
    delete_credential(&st, &req, &cred_id).await
}

async fn login_begin(st: AppState, req: Req) -> Res {
    let options = wa::start_authentication(&st.cfg(), &req, "user-login")?;
    j(json!({ "success": true, "options": options }), 200)
}

async fn login_finish(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let body = req.post_value().await;
    if ["challenge", "clientDataJSON", "authenticatorData", "signature", "credentialId"].iter().any(|k| body_str(&body, k).is_none()) {
        return fail("Missing required WebAuthn fields", 400);
    }
    let cred_id = body_str(&body, "credentialId").unwrap_or_default();
    let Ok(pending) = wa::take(&body_str(&body, "challenge").unwrap_or_default()) else { return fail("Challenge expired or invalid", 400) };
    if pending.kind != "user-login" {
        return fail("Wrong challenge type", 400);
    }

    let row = sql!(
        r#"SELECT wc.user_id, wc.passkey, wc.user_handle::text AS user_handle, u.login, u.user, u.activated, u.banned, u.ban_reason, u.ban_expires, u.force_password_change
           FROM webauthn_credentials wc JOIN "user" u ON u.id = wc.user_id
           WHERE wc.cred_id = "#,
        cred_id.clone(),
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(row) = row else { return fail("No registered account found for this passkey.", 401) };
    let user_id = row.i64("user_id").unwrap_or(0);

    let ban_active = row.bool("banned")
        && !row.str("ban_expires").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()).is_some_and(|e| e < chrono::Utc::now());
    if ban_active {
        return fail(&format!("Account is banned: {}", row.str("ban_reason").unwrap_or("")), 403);
    }
    if !row.bool("activated") {
        return fail("This account is not activated. Please check your email.", 403);
    }

    let updated = match wa::finish_authentication(&cfg, &req, pending, &body, wa::Stored { passkey: row.val("passkey"), user_handle: row.str("user_handle") }) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("[PASSKEYS] login/finish verification failed: {e}");
            return fail(&format!("Authentication failed: {e}"), 401);
        }
    };
    wa::touch(&st, &cred_id, updated).await?;
    if row.bool("banned") {
        // expired ban
        sql!(r#"update "user" set banned = false, ban_reason = null, ban_expires = null where id = "#, user_id as i32).execute(&st.db).await?;
    }

    let s = crate::auth::insert_session(
        &st,
        &req,
        SessionSpec { user_id, kmsi: true, last_action: "/passkey-login", passkey_credential_id: Some(&cred_id), csrf_bytes: 32 },
    )
    .await?;
    crate::security::log_user_ip(&st, user_id, &req.real_ip()).await;
    req.set_cookie(crate::auth::session_cookie(&cfg, &s.session, true));
    j(json!({ "success": true, "user": row.val("user"), "login": row.val("login"), "force_password_change": row.bool("force_password_change") }), 200)
}
