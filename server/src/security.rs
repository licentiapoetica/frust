//! IP hashing, login rate limiting, IP/fingerprint bans (port of src/inc/security.mjs).

use crate::config::Config;
use crate::db::{Row, RowExt};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

const RATE_LIMIT_WINDOW_MINUTES: i64 = 600;
const MAX_ATTEMPTS: i64 = 5;

pub struct IpHashSecrets {
    pub current: String,
    pub legacy: Vec<String>,
    pub source: &'static str,
}

pub fn ip_hash_secrets(cfg: &Config) -> IpHashSecrets {
    let env = std::env::var("IP_HASH_SECRET").unwrap_or_default();
    let dedicated = if !env.is_empty() { env.clone() } else { cfg.str_or("main.ip_hash_secret", "").to_string() };
    let invite = cfg.str_or("main.invite_secret", "").to_string();
    let current = if !dedicated.is_empty() { dedicated.clone() } else { invite.clone() };
    let mut legacy: Vec<String> = match cfg.get("main.ip_hash_legacy_secrets") {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string).collect(),
        Value::String(s) if !s.is_empty() => vec![s.clone()],
        _ => vec![],
    };
    if !dedicated.is_empty() && !invite.is_empty() && !legacy.contains(&invite) {
        legacy.push(invite);
    }
    legacy.retain(|s| *s != current);
    let source = if !env.is_empty() {
        "env IP_HASH_SECRET"
    } else if cfg.truthy("main.ip_hash_secret") {
        "config main.ip_hash_secret"
    } else {
        "config main.invite_secret (shared, set IP_HASH_SECRET)"
    };
    IpHashSecrets { current, legacy, source }
}

/// HMAC-SHA256 of an IP with the current secret. Panics without a secret, like the original
/// (refusing to fall back to a predictable salt).
pub fn hash_ip_with(ip: &str, secret: &str) -> String {
    if ip.is_empty() {
        return "unknown".into();
    }
    if secret.is_empty() {
        panic!("[FATAL] No IP hash secret configured. Set IP_HASH_SECRET (env) or main.ip_hash_secret. Refusing to use a predictable fallback salt.");
    }
    crate::util::hmac_sha256_hex(secret.as_bytes(), ip.as_bytes())
}

pub fn hash_ip(cfg: &Config, ip: &str) -> String {
    hash_ip_with(ip, &ip_hash_secrets(cfg).current)
}

pub fn hash_ip_candidates(cfg: &Config, ip: &str) -> Vec<String> {
    if ip.is_empty() {
        return vec![];
    }
    let s = ip_hash_secrets(cfg);
    std::iter::once(s.current).chain(s.legacy).filter(|x| !x.is_empty()).map(|sec| hash_ip_with(ip, &sec)).collect()
}

/// The form in which an IP may be stored: `None` when logging is off, hashed or raw otherwise.
pub fn storable_ip(cfg: &Config, ip: &str) -> Option<String> {
    if !cfg.truthy("websrv.log_user_ips") || ip.is_empty() || ip == "unknown" {
        return None;
    }
    Some(if cfg.truthy("websrv.hash_user_ips") { hash_ip(cfg, ip) } else { ip.to_string() })
}

fn window(kind: &str) -> (i64, i64, bool) {
    match kind {
        "password_reset_request" => (1440, 1, false),
        "password_reset_execution" => (60, 5, false),
        _ => (RATE_LIMIT_WINDOW_MINUTES, MAX_ATTEMPTS, true),
    }
}

async fn count_attempts(st: &AppState, col: &str, value: &str, kind: &str) -> i64 {
    let (minutes, _, only_failures) = window(kind);
    let start = chrono::Utc::now() - chrono::Duration::minutes(minutes);
    let mut q = sql!("select count(*) as count from login_attempts where ");
    q.push(col).push(" = ").bind(value).push(" and type = ").bind(kind);
    if only_failures {
        q.push(" and success = false");
    }
    q.push(" and attempted_at > ").bind(start);
    q.fetch_scalar::<i64, _>(&st.db).await.ok().flatten().unwrap_or(0)
}

pub async fn record_attempt(st: &AppState, ip: &str, username: Option<&str>, kind: &str, success: bool) {
    let cfg = st.cfg();
    let ip_hash = hash_ip(&cfg, ip);
    let uname = username.map(str::to_lowercase).filter(|u| !u.is_empty());
    if let Err(e) = sql!("insert into login_attempts (ip_hash, username, type, success) values (", ip_hash.clone(), ", ", uname.clone(), ", ", kind, ", ", success, ")")
        .execute(&st.db)
        .await
    {
        tracing::error!("[SECURITY] Failed to record {kind} attempt: {e}");
    }
    if !success {
        let (_, max, _) = window(kind);
        let count = count_attempts(st, "ip_hash", &ip_hash, kind).await;
        tracing::warn!(
            "[SECURITY] Failed {kind} attempt: user={}, ip_hash={ip_hash}, ip_banned={} ({count}/{max})",
            username.unwrap_or(""),
            count >= max
        );
    }
}

pub async fn clear_attempts(st: &AppState, ip: &str, username: Option<&str>) {
    let cfg = st.cfg();
    let ip_hash = hash_ip(&cfg, ip);
    let uname = username.map(str::to_lowercase).unwrap_or_default();
    if let Err(e) = sql!("delete from login_attempts where (ip_hash = ", ip_hash, " OR username = ", uname, ")").execute(&st.db).await {
        tracing::error!("[SECURITY] Failed to clear attempts: {e}");
    }
}

pub async fn is_rate_limited(st: &AppState, ip: &str, username: Option<&str>, kind: &str) -> bool {
    let cfg = st.cfg();
    let ip_hash = hash_ip(&cfg, ip);
    let (_, max, _) = window(kind);
    let ip_count = count_attempts(st, "ip_hash", &ip_hash, kind).await;
    if ip_count >= max {
        tracing::warn!("[SECURITY] Rate limit hit for {kind}: ip_hash={ip_hash}, attempts={ip_count}/{max}");
        return true;
    }
    if let Some(u) = username.filter(|u| !u.is_empty()) {
        let c = count_attempts(st, "username", &u.to_lowercase(), kind).await;
        if c >= max {
            tracing::warn!("[SECURITY] Rate limit hit for {kind}: user={u}, attempts={c}/{max}");
            return true;
        }
    }
    false
}

pub async fn log_user_ip(st: &AppState, user_id: i64, ip: &str) {
    let cfg = st.cfg();
    let Some(ip) = storable_ip(&cfg, ip) else { return };
    if user_id == 0 {
        return;
    }
    if let Err(e) = sql!(
        "insert into user_ips (user_id, ip) values (", user_id as i32, ", ", ip,
        ") on conflict (user_id, ip) do update set last_seen = now()"
    )
    .execute(&st.db)
    .await
    {
        tracing::error!("[SECURITY] Failed to log user IP: {e}");
    }
}

pub async fn is_ip_banned(st: &AppState, ip: &str) -> Option<Row> {
    if ip.is_empty() || ip == "unknown" {
        return None;
    }
    let cfg = st.cfg();
    if cfg.development() && ip == "127.0.0.1" && !cfg.truthy("test_ban_localhost") {
        return None;
    }
    let hashes = hash_ip_candidates(&cfg, ip);
    sql!(
        "select id, ip, ip_hash, reason, expires_at as expires from banned_ips where (ip = ", ip,
        " or ip = any(", hashes.clone(), ") or ip_hash = any(", hashes, ") or ip_hash = ", ip,
        ") and (expires_at is null or expires_at > now()) limit 1"
    )
    .fetch_optional(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[SECURITY] Error checking isIpBanned: {e}");
        None
    })
}

pub async fn is_fingerprint_banned(st: &AppState, fp: &str) -> Option<Row> {
    if fp.is_empty() {
        return None;
    }
    sql!(
        "select id, fingerprint, reason, expires_at as expires, banned_by from banned_fingerprints where fingerprint = ", fp,
        " and (expires_at is null or expires_at > now()) limit 1"
    )
    .fetch_optional(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[SECURITY] Error checking isFingerprintBanned: {e}");
        None
    })
}

pub async fn is_hardware_banned(st: &AppState, hw: &str) -> Option<Row> {
    if hw.is_empty() {
        return None;
    }
    sql!(
        "select id, hw_fingerprint, reason, expires_at as expires, banned_by from banned_hardware_fingerprints where hw_fingerprint = ", hw,
        " and (expires_at is null or expires_at > now()) limit 1"
    )
    .fetch_optional(&st.db)
    .await
    .unwrap_or_else(|e| {
        tracing::error!("[SECURITY] Error checking isHardwareBanned: {e}");
        None
    })
}

pub struct AnonBan {
    pub user_id: Option<i64>,
    pub fingerprint: Option<String>,
    pub hw_fingerprint: Option<String>,
    pub banned_by: Option<i64>,
    pub reason: String,
    /// ISO timestamp or None for permanent
    pub expires: Option<chrono::DateTime<chrono::Utc>>,
    pub ban_ips: bool,
    pub ban_hardware: bool,
}

async fn first_str(st: &AppState, q: crate::db::Sql, col: &str) -> Option<String> {
    q.fetch_optional(&st.db).await.ok().flatten().and_then(|r| r.str(col).map(str::to_string))
}

/// Ban an anonymous identity with its fingerprints and associated IPs.
pub async fn ban_anonymous_user(st: &AppState, b: AnonBan) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    let mut user_id = b.user_id;
    let mut fp = b.fingerprint.clone();
    let mut hw = b.hw_fingerprint.clone();
    let banned_by = b.banned_by.map(|v| v as i32);
    let expires: crate::db::Bind = match b.expires {
        Some(e) => e.into(),
        None => crate::db::Bind::Null,
    };

    if let (Some(uid), None) = (user_id, &fp) {
        fp = first_str(st, sql!("select fingerprint from anon_identities where user_id = ", uid as i32, " limit 1"), "fingerprint").await;
    }
    if let (Some(uid), None) = (user_id, &hw) {
        hw = first_str(st, sql!("select hw_fingerprint from anon_identities where user_id = ", uid as i32, " and hw_fingerprint is not null limit 1"), "hw_fingerprint").await;
    }
    if let (Some(f), None) = (&fp, user_id) {
        user_id = sql!("select user_id from anon_identities where fingerprint = ", f.clone(), " limit 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("user_id"));
    }
    if let (Some(f), None) = (&fp, &hw) {
        hw = first_str(st, sql!("select hw_fingerprint from anon_identities where fingerprint = ", f.clone(), " and hw_fingerprint is not null limit 1"), "hw_fingerprint").await;
    }

    if let Some(uid) = user_id {
        let mut q = sql!(r#"update "user" set banned = true, ban_reason = "#, b.reason.clone(), ", ban_expires = ");
        q.bind(expires.clone()).push(" where id = ").bind(uid as i32);
        q.execute(&st.db).await?;
        sql!(r#"delete from "user_sessions" where user_id = "#, uid as i32).execute(&st.db).await?;
    }

    if let Some(f) = &fp {
        let mut q = sql!("insert into banned_fingerprints (fingerprint, banned_by, reason, expires_at) values (", f.clone(), ", ", banned_by, ", ", b.reason.clone(), ", ");
        q.bind(expires.clone()).push(") on conflict (fingerprint) do update set reason = excluded.reason, expires_at = excluded.expires_at, banned_by = excluded.banned_by");
        q.execute(&st.db).await?;
    }

    let collect = |rows: Vec<Row>, col: &str, set: &mut Vec<String>| {
        for r in rows {
            if let Some(v) = r.str(col).filter(|v| !v.is_empty()) {
                if !set.iter().any(|x| x == v) {
                    set.push(v.to_string());
                }
            }
        }
    };

    let mut hws: Vec<String> = Vec::new();
    if b.ban_hardware {
        if let Some(h) = &hw {
            hws.push(h.clone());
        }
        if let Some(uid) = user_id {
            collect(sql!("select distinct hw_fingerprint from anon_activity_log where user_id = ", uid as i32, " and hw_fingerprint is not null").fetch_all(&st.db).await?, "hw_fingerprint", &mut hws);
            collect(sql!("select hw_fingerprint from anon_identities where user_id = ", uid as i32, " and hw_fingerprint is not null").fetch_all(&st.db).await?, "hw_fingerprint", &mut hws);
        }
        if let Some(f) = &fp {
            collect(sql!("select distinct hw_fingerprint from anon_activity_log where fingerprint = ", f.clone(), " and hw_fingerprint is not null").fetch_all(&st.db).await?, "hw_fingerprint", &mut hws);
        }
        for h in &hws {
            let mut q = sql!("insert into banned_hardware_fingerprints (hw_fingerprint, banned_by, reason, expires_at) values (", h.clone(), ", ", banned_by, ", ", b.reason.clone(), ", ");
            q.bind(expires.clone()).push(") on conflict (hw_fingerprint) do update set reason = excluded.reason, expires_at = excluded.expires_at, banned_by = excluded.banned_by");
            if let Err(e) = q.execute(&st.db).await {
                tracing::error!("[SECURITY] Error inserting banned hardware fingerprint: {e}");
            }
        }
    }

    let mut ips: Vec<String> = Vec::new();
    if b.ban_ips {
        if let Some(uid) = user_id {
            let uid = uid as i32;
            collect(sql!("select distinct ip from anon_activity_log where user_id = ", uid).fetch_all(&st.db).await?, "ip", &mut ips);
            collect(sql!("select distinct ip from user_ips where user_id = ", uid).fetch_all(&st.db).await?, "ip", &mut ips);
            collect(sql!("select distinct ip from comments where user_id = ", uid, " and ip is not null").fetch_all(&st.db).await?, "ip", &mut ips);
        }
        if let Some(f) = &fp {
            collect(sql!("select distinct ip from anon_activity_log where fingerprint = ", f.clone()).fetch_all(&st.db).await?, "ip", &mut ips);
            let rows = sql!("select created_ip, last_ip from anon_identities where fingerprint = ", f.clone()).fetch_all(&st.db).await?;
            collect(rows.clone(), "created_ip", &mut ips);
            collect(rows, "last_ip", &mut ips);
        }
        if let Some(h) = &hw {
            collect(sql!("select distinct ip from anon_activity_log where hw_fingerprint = ", h.clone()).fetch_all(&st.db).await?, "ip", &mut ips);
        }
        static HASH: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new("(?i)^[a-f0-9]{64}$").unwrap());
        for ip in &ips {
            if ip == "unknown" {
                continue;
            }
            let ip_hash = if HASH.is_match(ip) { ip.clone() } else { hash_ip(&cfg, ip) };
            let mut q = sql!("insert into banned_ips (ip, ip_hash, banned_by, reason, expires_at) values (", ip.clone(), ", ", ip_hash, ", ", banned_by, ", ", b.reason.clone(), ", ");
            q.bind(expires.clone()).push(") on conflict (ip) do update set reason = excluded.reason, expires_at = excluded.expires_at, banned_by = excluded.banned_by, ip_hash = excluded.ip_hash");
            if let Err(e) = q.execute(&st.db).await {
                tracing::error!("[SECURITY] Error inserting banned IP: {e}");
            }
        }
    }

    let payload = json!({
        "userId": user_id,
        "fingerprint": fp,
        "hwFingerprints": hws.iter().take(50).collect::<Vec<_>>(),
        "ips": ips.iter().take(50).collect::<Vec<_>>(),
        "reason": b.reason.chars().take(300).collect::<String>(),
        "expires": b.expires.map(|e| e.to_rfc3339()),
    });
    if let Err(e) = sql!("select pg_notify('bans', ", payload.to_string(), ")").execute(&st.db).await {
        tracing::error!("[SECURITY] Error notifying bans channel: {e}");
    }

    Ok(json!({ "success": true, "userId": user_id, "fingerprint": fp, "hwFingerprint": hw }))
}

/// Answer for a banned client (JSON for XHR/API, redirect otherwise).
pub fn ban_response(req: &crate::http::Req, reason: Option<&str>, expires: Option<&str>) -> axum::response::Response {
    if req.is_xhr() || req.path.starts_with("/api/") {
        let expires_text = expires
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
            .map(|d| crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None))
            .unwrap_or_else(|| "Permanent".into());
        return crate::http::json_raw(
            &json!({
                "success": false,
                "banned": true,
                "msg": "YOU ARE BANNED!",
                "reason": reason.filter(|r| !r.is_empty()).unwrap_or("Banned"),
                "expires": expires_text,
                "redirect": "/banned"
            }),
            403,
        );
    }
    let mut r = crate::http::raw(307, "");
    r.headers_mut().insert(axum::http::header::LOCATION, axum::http::HeaderValue::from_static("/banned"));
    r
}
