//! Automatic deletion of personal data after a configurable period (`src/inc/retention.mjs`).
//!
//! All periods are in days, configured under `websrv.*` (0 = keep forever):
//!   `retention_ip_days`              stored IPs: `user_ips` rows are deleted; IP columns on sessions,
//!                                    anon identities, uploads, comments, reports and ToS acceptances are set to NULL
//!   `retention_activity_log_days`    `anon_activity_log` rows are deleted
//!   `retention_login_attempts_days`  `login_attempts` rows are deleted
//!   `retention_sessions_days`        sessions unused for this long are deleted
//!   `retention_fingerprint_days`     hardware fingerprint is cleared from anonymous identities inactive this long
//!
//! Unless `websrv.anon_hw_fingerprint` is true, every stored device fingerprint (identities and
//! activity log) is cleared on each run, regardless of age. Active bans are not touched.

use crate::config::Config;
use crate::db::Sql;
use crate::sql;
use crate::state::AppState;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retention {
    pub ip: i64,
    pub activity_log: i64,
    pub login_attempts: i64,
    pub sessions: i64,
    pub fingerprint: i64,
}

/// `days(key)`: missing/null/'' → default; otherwise `parseInt`, non-positive or NaN → 0 (forever).
fn days(cfg: &Config, key: &str, default: i64) -> i64 {
    let v = cfg.get(&format!("websrv.retention_{key}_days"));
    match v {
        Value::Null => default,
        Value::String(s) if s.is_empty() => default,
        Value::String(s) => crate::social::helpers::js_parse_int(s).filter(|n| *n > 0).unwrap_or(0),
        Value::Number(n) => n.as_f64().map(|f| f.trunc() as i64).filter(|n| *n > 0).unwrap_or(0),
        // parseInt(true / [..] / {..}) is NaN (arrays: parseInt of their string form)
        Value::Array(a) => a.first().and_then(|f| match f {
            Value::Number(n) => n.as_f64().map(|f| f.trunc() as i64),
            Value::String(s) => crate::social::helpers::js_parse_int(s),
            _ => None,
        }).filter(|n| *n > 0).unwrap_or(0),
        _ => 0,
    }
}

pub fn get_retention(cfg: &Config) -> Retention {
    Retention {
        ip: days(cfg, "ip", 30),
        activity_log: days(cfg, "activity_log", 90),
        login_attempts: days(cfg, "login_attempts", 30),
        sessions: days(cfg, "sessions", 365),
        fingerprint: days(cfg, "fingerprint", 365),
    }
}

/// `now() - n days` as SQL with `n` bound.
fn before(n: i64) -> Sql {
    sql!("now() - make_interval(days => ", n as i32, ")")
}

pub async fn run_retention(st: &AppState) {
    let cfg = st.cfg();
    let r = get_retention(&cfg);
    let now_secs = crate::util::now_secs();
    let mut counts: Vec<(&'static str, u64)> = Vec::new();
    macro_rules! step {
        ($name:expr, $q:expr) => {
            match $q.execute(&st.db).await {
                Ok(n) if n > 0 => counts.push(($name, n)),
                Ok(_) => {}
                Err(e) => tracing::error!("[RETENTION] {} failed: {e}", $name),
            }
        };
    }

    if r.ip > 0 {
        let cutoff_secs = (now_secs - r.ip * 86400) as i32;
        let c = before(r.ip);
        step!("user_ips", sql!("delete from user_ips where last_seen < ", @c));
        step!("sessions.ip", sql!("update user_sessions set ip = null where ip is not null and last_used < ", cutoff_secs));
        step!("anon.created_ip", sql!("update anon_identities set created_ip = null where created_ip is not null and created_at < ", @c));
        step!("anon.last_ip", sql!("update anon_identities set last_ip = null where last_ip is not null and last_seen < ", @c));
        step!("items.uploader_ip", sql!("update items set uploader_ip = null where uploader_ip is not null and stamp < ", cutoff_secs));
        step!("comments.ip", sql!("update comments set ip = null where ip is not null and created_at < ", @c));
        step!("reports.reporter_ip", sql!("update reports set reporter_ip = null where reporter_ip is not null and created_at < ", @c));
        step!("tos.accepted_ip", sql!("update user_tos_acceptance set accepted_ip = null where accepted_ip is not null and accepted_at < ", @c));
    }

    if r.activity_log > 0 {
        let c = before(r.activity_log);
        step!("anon_activity_log", sql!("delete from anon_activity_log where created_at < ", @c));
    }

    if r.login_attempts > 0 {
        let c = before(r.login_attempts);
        step!("login_attempts", sql!("delete from login_attempts where attempted_at < ", @c));
    }

    if r.sessions > 0 {
        let cutoff = (now_secs - r.sessions * 86400) as i32;
        step!("user_sessions", sql!("delete from user_sessions where last_used < ", cutoff));
    }

    if !crate::settings::hw_fingerprint_enabled(&cfg) {
        // Fingerprinting switched off: don't keep any fingerprints collected while it was on
        step!("anon.hw_fingerprint", sql!("update anon_identities set hw_fingerprint = null where hw_fingerprint is not null"));
        step!("activity.hw_fingerprint", sql!("update anon_activity_log set hw_fingerprint = null where hw_fingerprint is not null"));
    } else if r.fingerprint > 0 {
        let c = before(r.fingerprint);
        step!("anon.hw_fingerprint", sql!("update anon_identities set hw_fingerprint = null where hw_fingerprint is not null and last_seen < ", @c));
    }

    if !counts.is_empty() {
        let summary = counts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(", ");
        tracing::info!("[RETENTION] Cleaned: {summary}");
    }
}

/// `startRetention()`: log the periods, run 30 s after boot and then hourly.
pub fn start(st: &AppState) {
    let cfg = st.cfg();
    let r = get_retention(&cfg);
    let fmt = |d: i64| if d > 0 { format!("{d}d") } else { "forever".to_string() };
    let fp = if crate::settings::hw_fingerprint_enabled(&cfg) { fmt(r.fingerprint) } else { "disabled (purged)".to_string() };
    tracing::info!(
        "[BOOT] Data retention: IPs {}, activity log {}, login attempts {}, sessions {}, device fingerprints {fp}",
        fmt(r.ip),
        fmt(r.activity_log),
        fmt(r.login_attempts),
        fmt(r.sessions)
    );
    let s = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            run_retention(&s).await;
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods() {
        let c = crate::media::test_cfg("websrv: {}");
        assert_eq!(get_retention(&c), Retention { ip: 30, activity_log: 90, login_attempts: 30, sessions: 365, fingerprint: 365 });
        let c = crate::media::test_cfg("websrv: { retention_ip_days: 0, retention_sessions_days: '7', retention_activity_log_days: -3, retention_login_attempts_days: 'x', retention_fingerprint_days: '' }");
        assert_eq!(get_retention(&c), Retention { ip: 0, activity_log: 0, login_attempts: 0, sessions: 7, fingerprint: 365 });
    }
}
