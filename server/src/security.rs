//! IP hashing, login rate limiting, IP/fingerprint bans (port of src/inc/security.mjs).

use crate::config::Config;
use crate::db::{Row, RowExt};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

const RATE_LIMIT_WINDOW_MINUTES: i64 = 600;
const MAX_ATTEMPTS: i64 = 5;
/// Failed logins / registrations from one IP (within the rate-limit window) after which a captcha
/// is required, and a harder one than usual (see `cap::challenge`).
pub const CAPTCHA_AFTER_FAILURES: i64 = MAX_ATTEMPTS;
/// Hard lockout for login / register while the captcha is on: past the captcha step the captcha
/// slows guessing down, so the lockout comes later than the `MAX_ATTEMPTS` used without one.
const LOCKOUT_WITH_CAPTCHA: i64 = 30;

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

/// Key for per-client rate limits: the IPv4 address, or the /64 of an IPv6 address (one client
/// usually gets a whole /64 and can pick a fresh address from it for every request).
pub fn rate_key(ip: &str) -> String {
    match ip.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) if v6.to_ipv4_mapped().is_none() => {
            let s = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", s[0], s[1], s[2], s[3])
        }
        _ => ip.to_string(),
    }
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
        log_failed_attempt(st, ip, username, kind).await;
    }
}

/// Warn about a failed `kind` attempt (already recorded) with the IP's count.
pub async fn log_failed_attempt(st: &AppState, ip: &str, username: Option<&str>, kind: &str) {
    let ip_hash = hash_ip(&st.cfg(), ip);
    let (_, max, _) = window(kind);
    let count = count_attempts(st, "ip_hash", &ip_hash, kind).await;
    tracing::warn!(
        "[SECURITY] Failed {kind} attempt: user={}, ip_hash={ip_hash}, ip_banned={} ({count}/{max})",
        username.unwrap_or(""),
        count >= max
    );
}

/// Forget failed attempts at `username`. With `ip`, only the ones from that IP: a successful login
/// clears its own typos, never the IP's failures at other accounts (logging into an own account
/// between guesses at someone else's would otherwise reset the IP's count every time).
pub async fn clear_attempts(st: &AppState, ip: Option<&str>, username: &str) {
    let cfg = st.cfg();
    let mut q = sql!("delete from login_attempts where username = ", username.to_lowercase());
    if let Some(ip) = ip {
        q.push(" and ip_hash = ").bind(hash_ip(&cfg, ip));
    }
    if let Err(e) = q.execute(&st.db).await {
        tracing::error!("[SECURITY] Failed to clear attempts: {e}");
    }
}

/// Record a `kind` attempt as failed *before* checking it, so parallel requests see each other in
/// their counts (counting first and recording after the check let a burst of guesses all pass as
/// "under the limit"). [`settle_attempt`] turns it into a success or takes it back.
pub async fn begin_attempt(st: &AppState, ip: &str, username: Option<&str>, kind: &str) -> Option<i64> {
    let cfg = st.cfg();
    let uname = username.map(str::to_lowercase).filter(|u| !u.is_empty());
    let q = sql!("insert into login_attempts (ip_hash, username, type, success) values (", hash_ip(&cfg, ip), ", ", uname, ", ", kind, ", false) returning id");
    match q.fetch_scalar::<i32, _>(&st.db).await {
        Ok(id) => id.map(i64::from),
        Err(e) => {
            tracing::error!("[SECURITY] Failed to record {kind} attempt: {e}");
            None
        }
    }
}

pub enum Settle {
    /// The attempt succeeded.
    Success,
    /// The attempt never got to the check (missing captcha, lockout): don't count it.
    Withdraw,
}

pub async fn settle_attempt(st: &AppState, id: Option<i64>, how: Settle) {
    let Some(id) = id.and_then(|i| i32::try_from(i).ok()) else { return };
    let q = match how {
        Settle::Success => sql!("update login_attempts set success = true where id = ", id),
        Settle::Withdraw => sql!("delete from login_attempts where id = ", id),
    };
    if let Err(e) = q.execute(&st.db).await {
        tracing::error!("[SECURITY] Failed to settle attempt: {e}");
    }
}

/// `kind` attempts at one account (any IP) within the kind's window: failed ones for login and
/// register, all of them for the password-reset kinds.
pub async fn failed_attempts_for_user(st: &AppState, username: &str, kind: &str) -> i64 {
    count_attempts(st, "username", &username.to_lowercase(), kind).await
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

/// Failed `kind` attempts from this IP within the rate-limit window.
pub async fn failed_attempts(st: &AppState, ip: &str, kind: &str) -> i64 {
    let ip_hash = hash_ip(&st.cfg(), ip);
    count_attempts(st, "ip_hash", &ip_hash, kind).await
}

/// This IP failed login or registration often enough to get the harder captcha.
pub async fn needs_hard_captcha(st: &AppState, ip: &str) -> bool {
    for kind in ["login", "register"] {
        if failed_attempts(st, ip, kind).await >= CAPTCHA_AFTER_FAILURES {
            return true;
        }
    }
    false
}

pub enum AuthGate {
    /// Too many failures: refuse outright.
    Locked,
    /// Go ahead; `captcha`: a (hard) captcha must be solved first.
    Open { captcha: bool, failures: i64 },
}

/// Rate limit for login / register (`kind`) from this IP. `captcha_on`: Cap is required for this
/// request (`cap::required`). Without a captcha the old lockout after `MAX_ATTEMPTS` failures stays.
/// `pending`: this request's own attempt, already recorded by [`begin_attempt`] (not counted).
pub async fn auth_gate(st: &AppState, ip: &str, kind: &str, captcha_on: bool, pending: bool) -> AuthGate {
    let failures = (failed_attempts(st, ip, kind).await - i64::from(pending)).max(0);
    let lockout = if captcha_on { LOCKOUT_WITH_CAPTCHA } else { MAX_ATTEMPTS };
    if failures >= lockout {
        tracing::warn!("[SECURITY] Rate limit hit for {kind}: ip_hash={}, attempts={failures}/{lockout}", hash_ip(&st.cfg(), ip));
        return AuthGate::Locked;
    }
    AuthGate::Open { captcha: captcha_on && failures >= CAPTCHA_AFTER_FAILURES, failures }
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

#[cfg(test)]
mod tests {
    #[test]
    fn rate_key_groups_ipv6_by_64() {
        assert_eq!(super::rate_key("2001:db8:1:2:aaaa::1"), super::rate_key("2001:db8:1:2:bbbb::9"));
        assert_ne!(super::rate_key("2001:db8:1:2::1"), super::rate_key("2001:db8:1:3::1"));
        assert_eq!(super::rate_key("198.51.100.7"), "198.51.100.7");
    }
}
