//! Anonymous passkey identities: shadow users, sessions, activity log (port of src/inc/anon_auth.mjs).

use crate::db::RowExt;
use crate::http::Req;
use crate::sql;
use crate::state::AppState;
use serde_json::Value;

/// IP of the request in its storable form (`None` when IP logging is off).
pub fn resolve_audit_ip(st: &AppState, req: &Req) -> Option<String> {
    crate::security::storable_ip(&st.cfg(), &req.real_ip())
}

pub struct Activity<'a> {
    pub action: &'a str,
    pub target_id: Option<i64>,
    pub details: Option<Value>,
    pub hw_fingerprint: Option<&'a str>,
}

/// Log activity of an anonymous user. `user_id`/`fingerprint` default to the request session.
pub async fn log_anon_activity_for(st: &AppState, req: &Req, user_id: Option<i64>, fingerprint: Option<String>, a: Activity<'_>) {
    let cfg = st.cfg();
    let raw_ip = req.real_ip();
    let ip = crate::security::storable_ip(&cfg, &raw_ip);
    // An explicit user (fresh handshake) has no session fingerprint, like `{...req, session: {id}}`.
    let sess = if user_id.is_some() { None } else { req.session() };
    let Some(user_id) = user_id.or_else(|| sess.map(|s| s.id)).filter(|u| *u != 0) else { return };
    let fingerprint = fingerprint.or_else(|| sess.and_then(|s| s.fingerprint.clone().or_else(|| s.anon_fingerprint.clone())));
    let hw = a.hw_fingerprint.map(str::to_string);
    let details = a.details.map(|d| d.to_string());
    let q = sql!(
        "INSERT INTO anon_activity_log (user_id, fingerprint, hw_fingerprint, ip, action, target_id, details) VALUES (",
        user_id as i32, ", ", fingerprint, ", ", hw, ", ", ip, ", ", a.action, ", ", a.target_id.map(|t| t as i32), ", ", details,
        "::jsonb)"
    );
    if let Err(e) = q.execute(&st.db).await {
        tracing::error!("[ANON_ACTIVITY_LOG] Failed to log activity: {e}");
        return;
    }
    crate::security::log_user_ip(st, user_id, &raw_ip).await;
}

/// `logAnonActivity(req, {...})` with the request's session.
pub async fn log_anon_activity(st: &AppState, req: &Req, a: Activity<'_>) {
    log_anon_activity_for(st, req, None, None, a).await
}

pub struct AnonUser {
    pub user_id: i64,
    pub is_new: bool,
    pub fingerprint: String,
}

/// Find or create the shadow user of a passkey identity.
pub async fn get_or_create_anon_user_by_credential(st: &AppState, credential_id: &str, req: &Req, hw: Option<&str>) -> anyhow::Result<AnonUser> {
    let audit_ip = resolve_audit_ip(st, req);
    let fingerprint = crate::webauthn::credential_fingerprint(credential_id);

    let existing = sql!("SELECT user_id FROM anon_identities WHERE credential_id = ", credential_id, " LIMIT 1").fetch_optional(&st.db).await?;
    if let Some(row) = existing {
        let mut q = sql!("UPDATE anon_identities SET last_seen = NOW()");
        if let Some(ip) = &audit_ip {
            q.push(", last_ip = ").bind(ip.clone());
        }
        if let Some(h) = hw {
            q.push(", hw_fingerprint = ").bind(h);
        }
        q.push(" WHERE credential_id = ").bind(credential_id);
        let _ = q.execute(&st.db).await;
        return Ok(AnonUser { user_id: row.i64("user_id").unwrap_or(0), is_new: false, fingerprint });
    }

    let digest = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(credential_id.as_bytes()))
    };
    let base_login = format!("anon_{}", &digest[..8]);
    let mut login = base_login.clone();
    let mut counter = 1;
    while sql!(r#"SELECT id FROM "user" WHERE login = "#, login.clone(), " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
        login = format!("{base_login}_{counter}");
        counter += 1;
    }

    let cfg = st.cfg();
    let mut tx = st.db.begin().await?;
    let user_id: i32 = sql!(
        r#"INSERT INTO "user" (login, "user", password, admin, is_moderator, activated) VALUES ("#,
        login.clone(), ", ", login.clone(), ", '!', false, false, true) RETURNING id"
    )
    .fetch_scalar::<i32, _>(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("user insert returned no id"))?;
    sql!(
        "INSERT INTO user_options (user_id, mode, theme, fullscreen, avatar, display_name, use_alternative_infobox) VALUES (",
        user_id, ", 0, 'amoled', 0, null, 'Anonymous', ", cfg.enabled("websrv.user_alternative_infobox"),
        ") ON CONFLICT (user_id) DO NOTHING"
    )
    .execute(&mut *tx)
    .await?;
    let mut q = sql!(
        "INSERT INTO anon_identities (user_id, credential_id, fingerprint, created_ip, last_ip, hw_fingerprint) VALUES (",
        user_id, ", ", credential_id, ", ", fingerprint.clone(), ", ", audit_ip.clone(), ", ", audit_ip.clone(), ", ", hw,
        ") ON CONFLICT (credential_id) DO UPDATE SET last_seen = NOW()"
    );
    if let Some(ip) = &audit_ip {
        q.push(", last_ip = ").bind(ip.clone());
    }
    if let Some(h) = hw {
        q.push(", hw_fingerprint = ").bind(h);
    }
    q.execute(&mut *tx).await?;
    tx.commit().await?;

    Ok(AnonUser { user_id: user_id as i64, is_new: true, fingerprint })
}

/// A session cookie value and its CSRF token.
pub struct NewSession {
    pub session: String,
    pub csrf_token: String,
}

/// Create (or reuse) a session for an anonymous user.
pub async fn create_anon_session(st: &AppState, user_id: i64, req: &Req, hw: Option<&str>, credential_id: Option<&str>) -> anyhow::Result<NewSession> {
    let audit_ip = resolve_audit_ip(st, req);
    let uid = user_id as i32;

    let mut q = sql!("UPDATE anon_identities SET last_ip = ", audit_ip.clone(), ", created_ip = COALESCE(created_ip, ", audit_ip.clone(), ")");
    if let Some(h) = hw {
        q.push(", hw_fingerprint = COALESCE(").bind(h).push(", hw_fingerprint)");
    }
    q.push(" WHERE user_id = ").bind(uid);
    let _ = q.execute(&st.db).await;

    let mark = |hash: String| {
        let cred = credential_id.map(str::to_string);
        async move {
            if let Some(c) = cred {
                let _ = sql!("UPDATE user_sessions SET passkey_credential_id = ", c, " WHERE session = ", hash).execute(&st.db).await;
            }
        }
    };
    let activity = || Activity { action: "handshake", target_id: None, details: None, hw_fingerprint: hw };

    let cookie = req.cookie("session").filter(|c| !c.is_empty()).map(str::to_string);

    // Already logged in as this identity: keep the session
    if let (Some(s), Some(c)) = (req.session(), &cookie) {
        if s.id == user_id && s.csrf_token.as_deref().is_some_and(|t| !t.is_empty()) {
            mark(crate::util::sha256(c)).await;
            log_anon_activity(st, req, activity()).await;
            return Ok(NewSession { session: c.clone(), csrf_token: s.csrf_token.clone().unwrap_or_default() });
        }
    }

    // Cookie of a session that belongs to this user: reuse it
    if let Some(c) = &cookie {
        let hash = crate::util::sha256(c);
        let row = sql!("SELECT session, csrf_token FROM user_sessions WHERE user_id = ", uid, " AND session = ", hash.clone(), " LIMIT 1")
            .fetch_optional(&st.db)
            .await?;
        if let Some(row) = row {
            sql!("UPDATE user_sessions SET last_used = ", crate::util::now_secs() as i32, " WHERE session = ", hash.clone()).execute(&st.db).await?;
            mark(hash).await;
            log_anon_activity_for(st, req, Some(user_id), None, activity()).await;
            return Ok(NewSession { session: c.clone(), csrf_token: row.string("csrf_token") });
        }
    }

    let s = crate::auth::insert_session(
        st,
        req,
        crate::auth::SessionSpec { user_id, kmsi: true, last_action: "/anon/passkey/auth", passkey_credential_id: credential_id, csrf_bytes: 24 },
    )
    .await?;
    log_anon_activity_for(st, req, Some(user_id), None, activity()).await;
    Ok(s)
}
