//! Anonymous passkey identities (port of src/inc/routes/apiv2/anon.mjs).

use crate::anon_auth::{create_anon_session, get_or_create_anon_user_by_credential, resolve_audit_ip};
use crate::db::{Row, RowExt};
use crate::http::{Group, Req, Res, Router, json_resp};
use crate::security::{AnonBan, ban_anonymous_user, is_fingerprint_banned, is_hardware_banned, is_ip_banned};
use crate::sql;
use crate::state::AppState;
use crate::webauthn as wa;
use serde_json::{Value, json};
use std::future::Future;

/// Maximum number of passkeys a single anonymous identity may hold
const MAX_ANON_PASSKEYS: i64 = 4;

pub fn register(r: &mut Router) {
    let mut g = Group::new(r, r"^\/api\/v2\/anon");
    g.post(r"\/session$", session_gone);
    g.post(r"\/passkey\/register\/begin$", |st, req| guarded("register/begin", register_begin(st, req)));
    g.post(r"\/passkey\/register\/finish$", |st, req| guarded("register/finish", register_finish(st, req)));
    g.post(r"\/passkey\/auth\/begin$", |st, req| guarded("auth/begin", auth_begin(st, req)));
    g.post(r"\/passkey\/auth\/finish$", |st, req| guarded("auth/finish", auth_finish(st, req)));
    g.post(r"\/passkey\/add\/begin$", |st, req| guarded("add/begin", add_begin(st, req)));
    g.post(r"\/passkey\/add\/finish$", |st, req| guarded("add/finish", add_finish(st, req)));
    g.get(r"\/passkeys$", |st, req| guarded("list", passkeys(st, req)));
    g.get(r"\/identity$", |st, req| guarded("identity", identity(st, req)));
    g.post(r"\/logout$", |st, req| guarded("logout", logout(st, req)));
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

/// Errors become a logged generic 500 (the old code echoed internal error messages).
async fn guarded(name: &str, f: impl Future<Output = Res>) -> Res {
    match f.await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[ANON_PASSKEY] {name} error: {}", e.0);
            j(json!({ "success": false, "msg": "Internal server error" }), 500)
        }
    }
}

fn disabled() -> Res {
    j(json!({ "success": false, "msg": "Anonymous access is disabled" }), 403)
}

fn iso_millis(ts: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn set_ban_cookie(req: &Req, reason: Option<&str>, expires: Option<&str>) {
    let payload = json!({
        "banned": true,
        "reason": reason.filter(|r| !r.is_empty()).unwrap_or("Banned"),
        "expires": expires.and_then(iso_millis),
    });
    req.set_cookie(format!(
        "f0ck_banned={}; Path=/; Max-Age=31536000; SameSite=Lax",
        crate::util::encode_uri_component(&payload.to_string())
    ));
}

/// IP ban check shared by the unauthenticated endpoints.
async fn ip_ban(st: &AppState, req: &Req) -> Option<Res> {
    let b = is_ip_banned(st, &req.real_ip()).await?;
    set_ban_cookie(req, Some(b.str("reason").filter(|r| !r.is_empty()).unwrap_or("IP address is banned")), b.str("expires"));
    Some(j(json!({ "success": false, "banned": true, "msg": "YOU ARE BANNED!", "reason": b.val("reason"), "redirect": "/banned" }), 403))
}

/// Client-supplied device fingerprint, or None when fingerprinting is disabled.
fn accept_hw(st: &AppState, v: &Value) -> Option<String> {
    if !crate::settings::hw_fingerprint_enabled(&st.cfg()) {
        return None;
    }
    let s = match v {
        Value::Null | Value::Bool(false) => return None,
        Value::String(s) if s.is_empty() => return None,
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(true) => "true".into(),
        Value::Array(_) | Value::Object(_) => return None,
    };
    Some(s.chars().take(128).collect())
}

struct Tombstone {
    banned: bool,
    fingerprint: Option<String>,
    hw_fingerprint: Option<String>,
}

fn accept_tombstone(st: &AppState, v: &Value) -> Option<Tombstone> {
    let o = v.as_object()?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    Some(Tombstone {
        banned: o.get("banned").map(crate::config::truthy).unwrap_or(false),
        fingerprint: s("fingerprint"),
        hw_fingerprint: if crate::settings::hw_fingerprint_enabled(&st.cfg()) { s("hw_fingerprint") } else { None },
    })
}

fn format_cascade_reason(source: Option<&str>, prefix: &str) -> String {
    let Some(src) = source.filter(|s| !s.is_empty()) else { return prefix.to_string() };
    static RE: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"^Cascade ban from (device|hardware ID|key) \((.*)\)$").unwrap());
    let mut clean = src.to_string();
    while let Some(c) = RE.captures(&clean) {
        clean = c.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
    }
    format!("{prefix} ({})", if clean.is_empty() { "Violation of community rules" } else { &clean })
}

fn ban_expiry(row: &Row) -> Option<chrono::DateTime<chrono::Utc>> {
    row.str("expires").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()).map(|d| d.with_timezone(&chrono::Utc))
}

async fn cascade(st: &AppState, fp: &str, hw: Option<String>, source: &Row, prefix: &str) {
    let r = ban_anonymous_user(
        st,
        AnonBan {
            user_id: None,
            fingerprint: Some(fp.to_string()),
            hw_fingerprint: hw,
            banned_by: source.i64("banned_by"),
            reason: format_cascade_reason(source.str("reason"), prefix),
            expires: ban_expiry(source),
            ban_ips: true,
            ban_hardware: true,
        },
    )
    .await;
    if let Err(e) = r {
        tracing::error!("[ANON_PASSKEY] cascade ban failed: {e}");
    }
}

async fn check_and_cascade_bans(st: &AppState, fp: &str, hw: Option<&str>, tombstone: Option<&Tombstone>) -> Option<Row> {
    if let Some(t) = tombstone.filter(|t| t.banned) {
        let mut active = match &t.fingerprint {
            Some(f) => is_fingerprint_banned(st, f).await,
            None => None,
        };
        if active.is_none() {
            if let Some(h) = &t.hw_fingerprint {
                active = is_hardware_banned(st, h).await;
            }
        }
        if let Some(b) = active {
            if !fp.is_empty() && is_fingerprint_banned(st, fp).await.is_none() {
                cascade(st, fp, hw.map(str::to_string).or_else(|| t.hw_fingerprint.clone()), &b, "Cascade ban from device").await;
            }
            return Some(b);
        }
    }
    if let Some(h) = hw {
        if let Some(b) = is_hardware_banned(st, h).await {
            if !fp.is_empty() && is_fingerprint_banned(st, fp).await.is_none() {
                cascade(st, fp, Some(h.to_string()), &b, "Cascade ban from hardware ID").await;
            }
            return Some(b);
        }
    }
    if !fp.is_empty() {
        if let Some(b) = is_fingerprint_banned(st, fp).await {
            if let Some(h) = hw {
                if is_hardware_banned(st, h).await.is_none() {
                    cascade(st, fp, Some(h.to_string()), &b, "Cascade ban from key").await;
                }
            }
            return Some(b);
        }
    }
    None
}

fn banned_response(req: &Req, ban: &Row, fp: &str, hw: Option<&str>) -> Res {
    set_ban_cookie(req, Some(ban.str("reason").filter(|r| !r.is_empty()).unwrap_or("Banned")), ban.str("expires"));
    let expires = ban
        .str("expires")
        .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
        .map(|d| crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None))
        .unwrap_or_else(|| "Permanent".into());
    j(
        json!({
            "success": false, "banned": true,
            "fingerprint": fp, "hw_fingerprint": hw,
            "msg": "YOU ARE BANNED!", "reason": ban.val("reason"),
            "expires": expires,
            "redirect": "/banned"
        }),
        403,
    )
}

/// Account-level checks after the passkey was verified (activation, user ban).
async fn user_state(st: &AppState, req: &Req, user_id: i64) -> anyhow::Result<Option<Res>> {
    let Some(u) = sql!(r#"SELECT banned, ban_reason, ban_expires, activated FROM "user" WHERE id = "#, user_id as i32, " LIMIT 1").fetch_optional(&st.db).await? else {
        return Ok(None);
    };
    if u.val("activated") == &Value::Bool(false) {
        return Ok(Some(j(json!({ "success": false, "msg": "This account is not activated. Please check your email." }), 403)));
    }
    if u.bool("banned") {
        set_ban_cookie(req, Some(u.str("ban_reason").filter(|r| !r.is_empty()).unwrap_or("Banned")), u.str("ban_expires"));
        return Ok(Some(j(json!({ "success": false, "banned": true, "msg": "YOU ARE BANNED!", "reason": u.val("ban_reason"), "redirect": "/banned" }), 403)));
    }
    Ok(None)
}

fn body_str(b: &Value, k: &str) -> Option<String> {
    b.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

async fn count_passkeys(st: &AppState, user_id: i64) -> anyhow::Result<i64> {
    wa::count(st, user_id).await
}

fn domain(st: &AppState) -> String {
    st.cfg().str("main.url.domain").filter(|d| !d.is_empty()).unwrap_or("f0ck.dev").to_string()
}

// ── Handlers ────────────────────────────────────────────────────────────────

async fn session_gone(_st: AppState, _req: Req) -> Res {
    j(json!({ "success": false, "msg": "SSH-key anonymous authentication has been replaced by passkeys. Please refresh the page." }), 410)
}

async fn register_begin(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    if let Some(r) = ip_ban(&st, &req).await {
        return r;
    }
    let name = format!("anon_new@{}", domain(&st));
    let options = wa::start_registration(
        &cfg,
        &req,
        wa::StartRegistration {
            kind: "anon-register",
            user_id: None,
            user_handle: uuid::Uuid::new_v4(),
            user_name: &name,
            display_name: "Anonymous",
            exclude: vec![],
        },
    )?;
    j(json!({ "success": true, "options": options }), 200)
}

async fn register_finish(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    if let Some(r) = ip_ban(&st, &req).await {
        return r;
    }
    let body = req.post_value().await;
    let hw = accept_hw(&st, &body["hw_fingerprint"]);
    let tombstone = accept_tombstone(&st, &body["tombstone"]);
    let required = ["challenge", "clientDataJSON", "attestationObject", "credentialId"];
    if required.iter().any(|k| body_str(&body, k).is_none()) {
        return j(json!({ "success": false, "msg": "Missing required WebAuthn fields" }), 400);
    }
    let Ok(pending) = wa::take(&body_str(&body, "challenge").unwrap_or_default()) else {
        return j(json!({ "success": false, "msg": "Challenge expired or invalid" }), 400);
    };
    if pending.kind != "anon-register" {
        return j(json!({ "success": false, "msg": "Wrong challenge type" }), 400);
    }
    let reg = match wa::finish_registration(&cfg, &req, pending, &body) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[ANON_PASSKEY] Registration verification failed: {e}");
            return j(json!({ "success": false, "msg": format!("Registration failed: {e}") }), 400);
        }
    };
    // Never map a registration onto an identity that already uses this credential id (the old
    // code logged the caller in as that identity's owner without checking possession of the key).
    if wa::known(&st, &reg.cred_id).await? {
        return j(json!({ "success": false, "msg": "This passkey is already registered." }), 409);
    }

    let fp = wa::credential_fingerprint(&reg.cred_id);
    if let Some(ban) = check_and_cascade_bans(&st, &fp, hw.as_deref(), tombstone.as_ref()).await {
        return banned_response(&req, &ban, &fp, hw.as_deref());
    }

    let user = get_or_create_anon_user_by_credential(&st, &reg.cred_id, &req, hw.as_deref()).await?;
    if let Some(r) = user_state(&st, &req, user.user_id).await? {
        return r;
    }
    if !wa::store(&st.db, user.user_id, &reg, "Passkey").await? {
        return j(json!({ "success": false, "msg": "This passkey is already registered." }), 409);
    }

    let s = create_anon_session(&st, user.user_id, &req, hw.as_deref(), Some(&reg.cred_id)).await?;
    req.set_cookie(crate::auth::session_cookie(&cfg, &s.session, true));
    j(
        json!({
            "success": true,
            "is_new": user.is_new,
            "user_id": user.user_id,
            "fingerprint": user.fingerprint,
            "short_fingerprint": user.fingerprint.chars().skip(7).take(8).collect::<String>(),
            "credential_id": reg.cred_id,
            "hw_fingerprint": hw,
            "csrf_token": s.csrf_token
        }),
        200,
    )
}

async fn auth_begin(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    if let Some(r) = ip_ban(&st, &req).await {
        return r;
    }
    let options = wa::start_authentication(&cfg, &req, "anon-auth")?;
    j(json!({ "success": true, "options": options }), 200)
}

async fn auth_finish(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    if let Some(r) = ip_ban(&st, &req).await {
        return r;
    }
    let body = req.post_value().await;
    let hw = accept_hw(&st, &body["hw_fingerprint"]);
    let tombstone = accept_tombstone(&st, &body["tombstone"]);
    let required = ["challenge", "clientDataJSON", "authenticatorData", "signature", "credentialId"];
    if required.iter().any(|k| body_str(&body, k).is_none()) {
        return j(json!({ "success": false, "msg": "Missing required WebAuthn fields" }), 400);
    }
    let cred_id = body_str(&body, "credentialId").unwrap_or_default();
    let Ok(pending) = wa::take(&body_str(&body, "challenge").unwrap_or_default()) else {
        return j(json!({ "success": false, "msg": "Challenge expired or invalid" }), 400);
    };
    if pending.kind != "anon-auth" {
        return j(json!({ "success": false, "msg": "Wrong challenge type" }), 400);
    }

    let Some(row) = wa::find(&st, &cred_id).await? else {
        return j(json!({ "success": false, "msg": "Passkey not registered. Please register first." }), 401);
    };
    let user_id = row.i64("user_id").unwrap_or(0);
    let updated = match wa::finish_authentication(&cfg, &req, pending, &body, wa::Stored { passkey: row.val("passkey"), user_handle: row.str("user_handle") }) {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!("[ANON_PASSKEY] Auth verification failed: {e}");
            return j(json!({ "success": false, "msg": format!("Authentication failed: {e}") }), 401);
        }
    };

    let fp = sql!("SELECT fingerprint FROM anon_identities WHERE user_id = ", user_id as i32, " AND credential_id = ", cred_id.clone(), " LIMIT 1")
        .fetch_optional(&st.db)
        .await?
        .and_then(|r| r.str("fingerprint").filter(|f| !f.is_empty()).map(str::to_string))
        .unwrap_or_else(|| wa::credential_fingerprint(&cred_id));
    if let Some(ban) = check_and_cascade_bans(&st, &fp, hw.as_deref(), tombstone.as_ref()).await {
        return banned_response(&req, &ban, &fp, hw.as_deref());
    }
    if let Some(r) = user_state(&st, &req, user_id).await? {
        return r;
    }

    wa::touch(&st, &cred_id, updated).await?;
    let mut q = sql!("UPDATE anon_identities SET last_seen = NOW()");
    if let Some(h) = &hw {
        q.push(", hw_fingerprint = ").bind(h.clone());
    }
    q.push(" WHERE user_id = ").bind(user_id as i32).push(" AND credential_id = ").bind(cred_id.clone());
    let _ = q.execute(&st.db).await;

    let s = create_anon_session(&st, user_id, &req, hw.as_deref(), Some(&cred_id)).await?;
    req.set_cookie(crate::auth::session_cookie(&cfg, &s.session, true));
    j(
        json!({
            "success": true,
            "user_id": user_id,
            "fingerprint": fp,
            "short_fingerprint": fp.chars().skip(7).take(8).collect::<String>(),
            "credential_id": cred_id,
            "hw_fingerprint": hw,
            "csrf_token": s.csrf_token
        }),
        200,
    )
}

/// The logged-in anonymous user, or None if the session isn't an anon identity.
async fn anon_session_user(st: &AppState, req: &Req) -> anyhow::Result<Option<i64>> {
    let Some(id) = req.session().map(|s| s.id).filter(|id| *id != 0) else { return Ok(None) };
    let row = sql!("SELECT 1 AS x FROM anon_identities WHERE user_id = ", id as i32, " LIMIT 1").fetch_optional(&st.db).await?;
    Ok(row.map(|_| id))
}

fn not_anon() -> Res {
    j(json!({ "success": false, "msg": "Not logged in as anonymous" }), 401)
}

fn too_many() -> Res {
    j(json!({ "success": false, "msg": format!("You can have at most {MAX_ANON_PASSKEYS} passkeys.") }), 400)
}

async fn add_begin(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    let Some(user_id) = anon_session_user(&st, &req).await? else { return not_anon() };
    let existing = wa::credential_ids(&st, user_id).await?;
    if existing.len() as i64 >= MAX_ANON_PASSKEYS {
        return too_many();
    }
    let name = format!("anon@{}", domain(&st));
    let options = wa::start_registration(
        &cfg,
        &req,
        wa::StartRegistration {
            kind: "anon-add",
            user_id: Some(user_id),
            user_handle: wa::user_handle_for(&st, user_id).await?,
            user_name: &name,
            display_name: "Anonymous",
            exclude: existing,
        },
    )?;
    j(json!({ "success": true, "options": options }), 200)
}

async fn add_finish(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::settings::enable_anonymous_access(&cfg) {
        return disabled();
    }
    let Some(user_id) = anon_session_user(&st, &req).await? else { return not_anon() };
    let expected = req.session().and_then(|s| s.csrf_token.clone()).filter(|t| !t.is_empty());
    let sent = req.header("x-csrf-token").unwrap_or("");
    if !expected.is_some_and(|t| crate::util::ct_eq(&t, sent)) {
        return j(json!({ "success": false, "msg": "Invalid CSRF token" }), 403);
    }

    let body = req.post_value().await;
    let hw = accept_hw(&st, &body["hw_fingerprint"]);
    let required = ["challenge", "clientDataJSON", "attestationObject", "credentialId"];
    if required.iter().any(|k| body_str(&body, k).is_none()) {
        return j(json!({ "success": false, "msg": "Missing required WebAuthn fields" }), 400);
    }
    let Ok(pending) = wa::take(&body_str(&body, "challenge").unwrap_or_default()) else {
        return j(json!({ "success": false, "msg": "Challenge expired or invalid" }), 400);
    };
    if pending.kind != "anon-add" || pending.user_id != Some(user_id) {
        return j(json!({ "success": false, "msg": "Wrong challenge type" }), 400);
    }
    // Re-check: two add flows could have been started in parallel
    if count_passkeys(&st, user_id).await? >= MAX_ANON_PASSKEYS {
        return too_many();
    }
    let reg = match wa::finish_registration(&cfg, &req, pending, &body) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[ANON_PASSKEY] Add verification failed: {e}");
            return j(json!({ "success": false, "msg": format!("Registration failed: {e}") }), 400);
        }
    };
    if wa::known(&st, &reg.cred_id).await? {
        return j(json!({ "success": false, "msg": "This passkey is already registered." }), 409);
    }

    let fp = wa::credential_fingerprint(&reg.cred_id);
    let audit_ip = resolve_audit_ip(&st, &req);
    let mut tx = st.db.begin().await?;
    if !wa::store(&mut *tx, user_id, &reg, "Passkey").await? {
        return j(json!({ "success": false, "msg": "This passkey is already registered." }), 409);
    }
    sql!(
        "INSERT INTO anon_identities (user_id, credential_id, fingerprint, created_ip, last_ip, hw_fingerprint) VALUES (",
        user_id as i32, ", ", reg.cred_id.clone(), ", ", fp, ", ", audit_ip.clone(), ", ", audit_ip, ", ", hw, ") ON CONFLICT (credential_id) DO NOTHING"
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;

    let n = count_passkeys(&st, user_id).await?;
    j(json!({ "success": true, "passkey_count": n, "passkey_max": MAX_ANON_PASSKEYS }), 200)
}

async fn passkeys(st: AppState, req: Req) -> Res {
    let Some(user_id) = anon_session_user(&st, &req).await? else { return not_anon() };
    let rows = sql!(
        "SELECT wc.id, wc.cred_id AS credential_id, wc.name, wc.aaguid, wc.created_at, wc.last_used, (ai.credential_id IS NOT NULL) AS primary
         FROM webauthn_credentials wc
         LEFT JOIN anon_identities ai ON ai.user_id = wc.user_id AND ai.credential_id = wc.cred_id
         WHERE wc.user_id = ", user_id as i32, " ORDER BY wc.created_at ASC"
    )
    .fetch_all(&st.db)
    .await?;
    let in_use = crate::auth::session_passkey(&st, &req).await?;
    j(json!({ "success": true, "max": MAX_ANON_PASSKEYS, "passkeys": wa::decorate_rows(rows, in_use.as_deref()) }), 200)
}

async fn identity(st: AppState, req: Req) -> Res {
    if !crate::settings::enable_anonymous_access(&st.cfg()) {
        return j(json!({ "logged_in": false, "is_anon": false, "disabled": true }), 200);
    }
    let Some(s) = req.session() else {
        return j(json!({ "logged_in": false, "is_anon": false }), 200);
    };
    let row = sql!(
        "SELECT ai.credential_id, ai.fingerprint, ai.hw_fingerprint, ai.created_at, ai.last_seen, wc.name AS passkey_name, wc.aaguid
         FROM anon_identities ai LEFT JOIN webauthn_credentials wc ON wc.cred_id = ai.credential_id
         WHERE ai.user_id = ", s.id as i32, " ORDER BY ai.created_at ASC LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    if let Some(r) = row {
        let fp = r.str("fingerprint");
        return j(
            json!({
                "logged_in": true,
                "is_anon": true,
                "user_id": s.id,
                "fingerprint": r.val("fingerprint"),
                "short_fingerprint": fp.filter(|f| !f.is_empty()).map(|f| f.chars().skip(7).take(8).collect::<String>()),
                "hw_fingerprint": r.val("hw_fingerprint"),
                "credential_id": r.val("credential_id"),
                "passkey_name": r.val("passkey_name"),
                "passkey_count": count_passkeys(&st, s.id).await?,
                "passkey_max": MAX_ANON_PASSKEYS,
                "csrf_token": s.csrf_token
            }),
            200,
        );
    }
    j(json!({ "logged_in": true, "is_anon": false, "user": s.user, "user_id": s.id, "csrf_token": s.csrf_token }), 200)
}

async fn logout(st: AppState, req: Req) -> Res {
    crate::auth::destroy_session(&st, &req).await?;
    req.set_cookie(crate::auth::clear_session_cookie(&st.cfg()));
    j(json!({ "success": true }), 200)
}
