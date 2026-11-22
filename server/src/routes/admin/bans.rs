//! Ban management: fingerprint/hardware/IP ban lists, user (un)ban, per-user IP history
//! (src/inc/routes/admin.mjs).

use super::{body_id, body_str, body_truthy, err_msg, totals};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_resp, reply};
use crate::security::{AnonBan, ban_anonymous_user};
use crate::sql;
use crate::state::AppState;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/bans(\/)?$", crate::http::Guard::Perm(crate::perms::P::ModBan), bans_page);
    r.post_g(r"^\/api\/v2\/admin\/bans\/fingerprint\/ban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), fingerprint_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/fingerprint\/unban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), fingerprint_unban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/ip\/ban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), ip_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/ip\/unban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), ip_unban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/hardware\/ban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), hardware_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/hardware\/unban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), hardware_unban);
    r.get_g(r"^\/admin\/user\/(?<userId>\d+)\/ips(\/)?$", Guard::Auth, user_ips);
    r.post_g(r"^\/api\/v2\/admin\/ban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), user_ban);
    r.post_g(r"^\/api\/v2\/admin\/unban\/?$", crate::http::Guard::Perm(crate::perms::P::ModBan), user_unban);
}

fn rows(v: Vec<Row>) -> Value {
    Value::Array(v.into_iter().map(Value::Object).collect())
}

fn sid(req: &Req) -> i64 {
    req.session().map(|s| s.id).unwrap_or(0)
}

/// `res.json({ success: true, ... })` / `{ success: false, msg }`.
fn answer(r: anyhow::Result<Value>) -> Res {
    Ok(json_resp(
        &match r {
            Ok(v) => v,
            Err(e) => json!({ "success": false, "msg": err_msg(&e) }),
        },
        200,
    ))
}

/// Longest ban length in hours (about 114 years).
const MAX_BAN_HOURS: i64 = 1_000_000;

/// A typed ban length: a plain number is hours (what the preset options send), anything else is
/// number + unit pairs like `90m`, `36h`, `10d`, `2w`, `3mo`, `1y` or `1d 12h`.
fn parse_ban_length(text: &str) -> Option<Duration> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }
    if text.bytes().all(|b| b.is_ascii_digit()) {
        let hours: i64 = text.parse().ok()?;
        return (1..=MAX_BAN_HOURS).contains(&hours).then(|| Duration::hours(hours));
    }
    let mut minutes = 0i64;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
        let n: i64 = rest[..digits].parse().ok()?;
        rest = rest[digits..].trim_start();
        let letters = rest.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(rest.len());
        let unit = match &rest[..letters] {
            "m" | "min" | "mins" | "minute" | "minutes" => 1,
            "h" | "hr" | "hrs" | "hour" | "hours" => 60,
            "d" | "day" | "days" => 60 * 24,
            "w" | "week" | "weeks" => 60 * 24 * 7,
            "mo" | "month" | "months" => 60 * 24 * 30,
            "y" | "year" | "years" => 60 * 24 * 365,
            _ => return None,
        };
        minutes = minutes.checked_add(n.checked_mul(unit)?)?;
        rest = rest[letters..].trim_start();
    }
    (1..=MAX_BAN_HOURS * 60).contains(&minutes).then(|| Duration::minutes(minutes))
}

/// The length a ban form asked for: the `duration` preset, or `duration_custom` when the preset is
/// `custom`. A custom pick with nothing typed stays `custom`, which is refused instead of permanent.
fn requested_duration(body: &Value) -> Option<String> {
    let duration = body_str(body, "duration");
    if duration.as_deref().map(str::trim) != Some("custom") {
        return duration;
    }
    let custom = body_str(body, "duration_custom").map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
    Some(custom.unwrap_or_else(|| "custom".into()))
}

/// Length of a ban; `None` = permanent (`'permanent'` or empty).
fn ban_length(duration: Option<&str>) -> anyhow::Result<Option<Duration>> {
    match duration.map(str::trim) {
        None | Some("") => Ok(None),
        Some(d) if d.eq_ignore_ascii_case("permanent") => Ok(None),
        Some(d) => parse_ban_length(d).map(Some).ok_or_else(|| anyhow::anyhow!("Invalid duration. Use for example 90m, 36h, 10d, 2w, 3mo or 1y.")),
    }
}

/// Ban end for a `duration`; `None` = permanent.
fn ban_expiry(duration: Option<&str>) -> anyhow::Result<Option<DateTime<Utc>>> {
    Ok(ban_length(duration)?.map(|d| Utc::now() + d))
}

/// Moderators can ban for at most 48 hours; admins without limit. An unreadable length passes
/// here and is refused as invalid by `ban_expiry`.
fn mod_duration_error(is_admin: bool, duration: Option<&str>) -> Option<&'static str> {
    if is_admin {
        return None;
    }
    match ban_length(duration) {
        Ok(Some(d)) if d <= Duration::hours(48) => None,
        Err(_) => None,
        _ => Some("Moderators can only issue bans for up to 48 hours."),
    }
}

/// 403 `{ success: false, msg }` for a refused ban.
fn refuse(msg: &str) -> Res {
    Ok(json_resp(&json!({ "success": false, "msg": msg }), 403))
}

fn iso(d: Option<DateTime<Utc>>) -> Value {
    d.map(|d| json!(d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())).unwrap_or(Value::Null)
}

/// JS `String.prototype.substring(0, n)` by chars.
fn prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

async fn bans_page(st: AppState, req: Req) -> Res {
    let fingerprints = sql!(
        r#"select bf.*, u.login as anon_login, u.user as anon_user, admin.user as banned_by_user
           from banned_fingerprints bf
           left join anon_identities ai on ai.fingerprint = bf.fingerprint
           left join "user" u on u.id = ai.user_id
           left join "user" admin on admin.id = bf.banned_by
           order by bf.created_at desc"#
    )
    .fetch_all(&st.db)
    .await?;
    let hardware = sql!(
        r#"select bh.*, admin.user as banned_by_user
           from banned_hardware_fingerprints bh
           left join "user" admin on admin.id = bh.banned_by
           order by bh.created_at desc"#
    )
    .fetch_all(&st.db)
    .await?;
    let ips = sql!(
        r#"select bi.*, admin.user as banned_by_user
           from banned_ips bi
           left join "user" admin on admin.id = bi.banned_by
           order by bi.created_at desc"#
    )
    .fetch_all(&st.db)
    .await?;
    let identities = sql!(
        r#"select ai.*,
                  u.id as user_id, u.login, u.user, u.banned as is_banned, u.ban_reason,
                  bf.id as is_fp_banned,
                  bh.id as is_hw_banned
           from anon_identities ai
           left join "user" u on u.id = ai.user_id
           left join banned_fingerprints bf on bf.fingerprint = ai.fingerprint
           left join banned_hardware_fingerprints bh on bh.hw_fingerprint = ai.hw_fingerprint
           order by ai.last_seen desc
           limit 100"#
    )
    .fetch_all(&st.db)
    .await?;
    let activity = sql!(
        r#"select al.*, u.login, u.user
           from anon_activity_log al
           left join "user" u on u.id = al.user_id
           order by al.created_at desc
           limit 100"#
    )
    .fetch_all(&st.db)
    .await?;
    let data = json!({
        "session": req.session_value(),
        "csrf_token": req.session().and_then(|s| s.csrf_token.clone()).unwrap_or_default(),
        "bannedFingerprints": rows(fingerprints),
        "bannedHardware": rows(hardware),
        "bannedIps": rows(ips),
        "anonIdentities": rows(identities),
        "recentActivity": rows(activity),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/bans", data)))
}

/// Why a fingerprint / hardware ban with `user_id` is refused: the id has to be an anonymous
/// identity (registered accounts go through `user_ban` with its checks), never staff for a moderator.
async fn anon_ban_target_error(st: &AppState, req: &Req, user_id: i64) -> anyhow::Result<Option<&'static str>> {
    let Some(target) = sql!(
        r#"select u.admin, u.is_moderator, exists(select 1 from anon_identities ai where ai.user_id = u.id) as is_anon from "user" u where u.id = "#,
        user_id as i32,
        " limit 1"
    )
    .fetch_optional(&st.db)
    .await?
    else {
        return Ok(Some("User not found"));
    };
    if !target.bool("is_anon") {
        return Ok(Some("Only anonymous identities can be banned by fingerprint; use the user ban for accounts."));
    }
    let is_admin = req.session().map(|s| s.admin).unwrap_or(false);
    if !is_admin && (target.bool("admin") || target.bool("is_moderator")) {
        return Ok(Some("Moderators cannot ban staff."));
    }
    Ok(None)
}

async fn anon_ban(st: &AppState, req: &Req, body: &Value, hardware: bool) -> anyhow::Result<Value> {
    let field = if hardware { "hw_fingerprint" } else { "fingerprint" };
    let fp = body_str(body, field).filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!(if hardware { "Missing hardware fingerprint" } else { "Missing fingerprint" })
    })?;
    let duration = requested_duration(body);
    let expires = ban_expiry(duration.as_deref())?;
    let reason = body_str(body, "reason").filter(|r| !r.is_empty());
    let result = ban_anonymous_user(
        st,
        AnonBan {
            user_id: anon_ban_user_id(body),
            fingerprint: (!hardware).then(|| fp.clone()),
            hw_fingerprint: hardware.then(|| fp.clone()),
            banned_by: Some(sid(req)),
            reason: reason.clone().unwrap_or_else(|| "Banned by moderator".into()),
            expires,
            ban_ips: body.get("ban_ips") != Some(&Value::Bool(false)),
            ban_hardware: hardware,
        },
    )
    .await?;
    let details = json!({ field: fp, "reason": body.get("reason"), "duration": duration });
    crate::audit::log(st, sid(req), if hardware { "ban_hardware" } else { "ban_fingerprint" }, if hardware { "hardware" } else { "fingerprint" }, None, details).await;
    Ok(json!({ "success": true, "result": result }))
}

fn anon_ban_user_id(body: &Value) -> Option<i64> {
    if body_truthy(body, "user_id") { body_id(body, "user_id") } else { None }
}

/// Role and duration checks of a fingerprint / hardware ban; `Some(msg)` refuses it.
async fn check_anon_ban(st: &AppState, req: &Req, body: &Value) -> anyhow::Result<Option<&'static str>> {
    let is_admin = req.session().map(|s| s.admin).unwrap_or(false);
    if let Some(msg) = mod_duration_error(is_admin, requested_duration(body).as_deref()) {
        return Ok(Some(msg));
    }
    match anon_ban_user_id(body) {
        Some(uid) => anon_ban_target_error(st, req, uid).await,
        None => Ok(None),
    }
}

async fn fingerprint_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    match check_anon_ban(&st, &req, &body).await {
        Ok(Some(msg)) => refuse(msg),
        Ok(None) => answer(anon_ban(&st, &req, &body, false).await),
        Err(e) => answer(Err(e)),
    }
}

async fn hardware_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    match check_anon_ban(&st, &req, &body).await {
        Ok(Some(msg)) => refuse(msg),
        Ok(None) => answer(anon_ban(&st, &req, &body, true).await),
        Err(e) => answer(Err(e)),
    }
}

async fn fingerprint_unban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let fp = body_str(&body, "fingerprint").filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing fingerprint"))?;
        sql!("DELETE FROM banned_fingerprints WHERE fingerprint = ", fp.clone()).execute(&st.db).await?;
        // Also unban the shadow user of this fingerprint
        let ident = sql!("SELECT user_id FROM anon_identities WHERE fingerprint = ", fp.clone()).fetch_optional(&st.db).await?;
        if let Some(uid) = ident.and_then(|r| r.i64("user_id")) {
            sql!(r#"UPDATE "user" SET banned = false, ban_reason = null, ban_expires = null WHERE id = "#, uid as i32).execute(&st.db).await?;
        }
        crate::audit::log(&st, sid(&req), "unban_fingerprint", "fingerprint", None, json!({ "fingerprint": fp })).await;
        Ok(json!({ "success": true }))
    }
    .await;
    answer(r)
}

async fn hardware_unban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let hw = body_str(&body, "hw_fingerprint").filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing hardware fingerprint"))?;
        sql!("DELETE FROM banned_hardware_fingerprints WHERE hw_fingerprint = ", hw.clone()).execute(&st.db).await?;
        crate::audit::log(&st, sid(&req), "unban_hardware", "hardware", None, json!({ "hw_fingerprint": hw })).await;
        Ok(json!({ "success": true }))
    }
    .await;
    answer(r)
}

async fn ip_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let cfg = st.cfg();
    let duration = requested_duration(&body);
    if let Some(msg) = mod_duration_error(req.session().map(|s| s.admin).unwrap_or(false), duration.as_deref()) {
        return refuse(msg);
    }
    let r = async {
        let ip = body_str(&body, "ip").filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing IP address"))?;
        let expires = ban_expiry(duration.as_deref())?;
        // Already a stored hash (banned from the IP list) or a raw address typed by the moderator
        let is_hash = ip.len() == 64 && ip.bytes().all(|b| b.is_ascii_hexdigit());
        let ip_hash = if is_hash { ip.to_lowercase() } else { crate::security::hash_ip(&cfg, &ip) };
        // With hashing on, the raw address is never persisted, not even in the ban list
        let stored = if is_hash || crate::settings::hash_user_ips(&cfg) { ip_hash.clone() } else { ip.clone() };
        let reason = body_str(&body, "reason").filter(|r| !r.is_empty()).unwrap_or_else(|| "Banned by moderator".into());
        let exp_bind: crate::db::Bind = expires.into();
        sql!(
            "INSERT INTO banned_ips (ip, ip_hash, banned_by, reason, expires_at) VALUES (", stored, ", ", ip_hash.clone(), ", ", sid(&req) as i32, ", ",
            reason.clone(), ", ", exp_bind,
            ") ON CONFLICT (ip) DO UPDATE SET reason = EXCLUDED.reason, expires_at = EXCLUDED.expires_at, banned_by = EXCLUDED.banned_by, ip_hash = EXCLUDED.ip_hash"
        )
        .execute(&st.db)
        .await?;
        crate::audit::log(&st, sid(&req), "ban_ip", "ip", None, json!({ "ip": ip, "reason": body.get("reason"), "duration": duration })).await;
        // Broadcast the ban to live SSE clients
        let payload = json!({ "ip": ip, "ipHash": ip_hash, "reason": prefix(&reason, 300), "expires": iso(expires) });
        let _ = sql!("SELECT pg_notify('bans', ", payload.to_string(), ")").execute(&st.db).await;
        Ok(json!({ "success": true }))
    }
    .await;
    answer(r)
}

async fn ip_unban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let ip = body_str(&body, "ip").filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing IP"))?;
        sql!("DELETE FROM banned_ips WHERE ip = ", ip.clone(), " OR ip_hash = ", ip.clone()).execute(&st.db).await?;
        crate::audit::log(&st, sid(&req), "unban_ip", "ip", None, json!({ "ip": ip })).await;
        Ok(json!({ "success": true }))
    }
    .await;
    answer(r)
}

async fn user_ips(st: AppState, req: Req) -> Res {
    let Some(user_id) = req.param("userId").and_then(|s| s.parse::<i32>().ok()) else {
        return Ok(reply(404, "text/html", "User not found"));
    };
    let Some(user) = sql!(r#"select "user", login from "user" where id = "#, user_id, " limit 1").fetch_optional(&st.db).await? else {
        return Ok(reply(404, "text/html", "User not found"));
    };
    let ips = sql!("select * from user_ips where user_id = ", user_id, " order by last_seen desc").fetch_all(&st.db).await?;
    let data = json!({
        "session": req.session_value(),
        "targetUser": user.clone(),
        "ips": rows(ips),
        "page_meta": { "title": format!("IP History - {}", user.string("user")) },
        "totals": totals(&st).await,
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/user_ips", data)))
}

async fn user_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let session = req.session().cloned().unwrap_or_default();
    let r: anyhow::Result<Result<(), &'static str>> = async {
        let user_id = body_id(&body, "user_id").ok_or_else(|| anyhow::anyhow!("User not found"))? as i32;
        let reason = body_str(&body, "reason");
        let duration = requested_duration(&body);
        let Some(target) = sql!(r#"select admin, "user", login from "user" where id = "#, user_id, " limit 1").fetch_optional(&st.db).await? else {
            anyhow::bail!("User not found");
        };
        if target.str("login") == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is a protected system ghost and cannot be modified.");
        }
        if target.bool("admin") && !session.admin {
            return Ok(Err("Moderators cannot ban administrators."));
        }
        if let Some(msg) = mod_duration_error(session.admin, duration.as_deref()) {
            return Ok(Err(msg));
        }
        // A missing duration used to become an invalid date; treat it as an error, not a permanent ban.
        if duration.as_deref().map(str::trim).unwrap_or("").is_empty() {
            anyhow::bail!("Invalid duration");
        }
        let expires = ban_expiry(duration.as_deref())?;
        let exp_bind: crate::db::Bind = expires.into();
        sql!(r#"update "user" set banned = true, ban_reason = "#, reason.clone(), ", ban_expires = ", exp_bind, " where id = ", user_id)
            .execute(&st.db)
            .await?;

        // Anonymous identities: cascade the ban to fingerprint and IPs
        let fp = sql!("SELECT fingerprint FROM anon_identities WHERE user_id = ", user_id, " LIMIT 1").fetch_optional(&st.db).await?.map(|r| r.string("fingerprint"));
        if fp.is_some() || target.str("login").map(|l| l.starts_with("anon_")).unwrap_or(false) {
            ban_anonymous_user(
                &st,
                AnonBan {
                    user_id: Some(user_id as i64),
                    fingerprint: fp,
                    hw_fingerprint: None,
                    banned_by: Some(session.id),
                    reason: reason.clone().unwrap_or_default(),
                    expires,
                    ban_ips: true,
                    ban_hardware: false,
                },
            )
            .await?;
        } else {
            // Broadcast the ban to the user's live SSE sessions
            let ips: Vec<String> = sql!("SELECT distinct ip FROM user_ips WHERE user_id = ", user_id).fetch_scalar_list(&st.db).await.unwrap_or_default();
            let ips: Vec<String> = ips.into_iter().filter(|i| !i.is_empty()).collect();
            let r = reason.clone().filter(|r| !r.is_empty()).unwrap_or_else(|| "Violation of community rules".into());
            let payload = json!({ "userId": user_id, "reason": prefix(&r, 300), "expires": iso(expires), "ips": ips });
            let _ = sql!("SELECT pg_notify('bans', ", payload.to_string(), ")").execute(&st.db).await;
        }
        crate::audit::log(
            &st,
            session.id,
            "ban_user",
            "user",
            user_id.to_string(),
            json!({ "reason": body.get("reason"), "duration": duration, "target_user": target.val("user") }),
        )
        .await;
        // Terminate all sessions of this user
        sql!(r#"delete from "user_sessions" where user_id = "#, user_id).execute(&st.db).await?;
        Ok(Ok(()))
    }
    .await;
    Ok(match r {
        Ok(Ok(())) => json_resp(&json!({ "success": true }), 200),
        Ok(Err(msg)) => json_resp(&json!({ "success": false, "msg": msg }), 403),
        Err(e) => json_resp(&json!({ "success": false, "msg": err_msg(&e) }), 200),
    })
}

async fn user_unban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let user_id = super::require_id(&body, "user_id")?;
        let target = sql!(r#"SELECT login FROM "user" WHERE id = "#, user_id, " LIMIT 1").fetch_optional(&st.db).await?;
        if target.as_ref().and_then(|t| t.str("login")) == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is a protected system ghost and cannot be unbanned.");
        }
        sql!(r#"update "user" set banned = false, ban_reason = null, ban_expires = null, last_seen = "#, crate::util::now_secs() as i32, " where id = ", user_id)
            .execute(&st.db)
            .await?;
        // Clean up any banned fingerprint of this identity
        let fp = sql!("SELECT fingerprint FROM anon_identities WHERE user_id = ", user_id, " LIMIT 1").fetch_optional(&st.db).await?;
        if let Some(fp) = fp {
            sql!("DELETE FROM banned_fingerprints WHERE fingerprint = ", fp.string("fingerprint")).execute(&st.db).await?;
        }
        crate::audit::log(&st, req.session().map(|s| s.id).unwrap_or(0), "unban_user", "user", user_id.to_string(), json!({})).await;
        Ok(json!({ "success": true }))
    }
    .await;
    answer(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry() {
        assert_eq!(ban_expiry(None).unwrap(), None);
        assert_eq!(ban_expiry(Some("permanent")).unwrap(), None);
        assert!(ban_expiry(Some("abc")).is_err());
        let e = ban_expiry(Some("24")).unwrap().unwrap();
        assert!((e - Utc::now()).num_minutes() > 23 * 60);
    }

    #[test]
    fn custom_lengths() {
        let mins = |s| parse_ban_length(s).map(|d| d.num_minutes());
        assert_eq!(mins("36"), Some(36 * 60));
        assert_eq!(mins("90m"), Some(90));
        assert_eq!(mins("36h"), Some(36 * 60));
        assert_eq!(mins("10D"), Some(10 * 1440));
        assert_eq!(mins("2w"), Some(14 * 1440));
        assert_eq!(mins("3mo"), Some(90 * 1440));
        assert_eq!(mins("1y"), Some(365 * 1440));
        assert_eq!(mins("1d 12h"), Some(36 * 60));
        assert_eq!(mins("2 hours 30 min"), Some(150));
        for bad in ["", "0", "0h", "-5", "-5h", "h", "5x", "1.5h", "custom", "99999999999999999999", "9999999y"] {
            assert_eq!(mins(bad), None, "{bad}");
        }
    }

    #[test]
    fn custom_pick_needs_a_value() {
        let d = |v: Value| requested_duration(&v);
        assert_eq!(d(json!({ "duration": "24", "duration_custom": "3d" })).as_deref(), Some("24"));
        assert_eq!(d(json!({ "duration": "custom", "duration_custom": " 3d " })).as_deref(), Some("3d"));
        // Nothing typed must fail, never fall through to a permanent ban
        let empty = d(json!({ "duration": "custom", "duration_custom": " " }));
        assert!(ban_expiry(empty.as_deref()).is_err());
    }

    #[test]
    fn moderator_limit_reads_units() {
        assert!(mod_duration_error(false, Some("48h")).is_none());
        assert!(mod_duration_error(false, Some("2d")).is_none());
        assert!(mod_duration_error(false, Some("3d")).is_some());
        assert!(mod_duration_error(false, Some("49")).is_some());
        assert!(mod_duration_error(false, Some("permanent")).is_some());
        assert!(mod_duration_error(false, None).is_some());
        assert!(mod_duration_error(true, Some("permanent")).is_none());
    }

    #[test]
    fn permanent_ban_binds_a_timestamp_null() {
        let exp: crate::db::Bind = None::<DateTime<Utc>>.into();
        assert!(sql!("update t set ban_expires = ", exp).sql().ends_with("/* timestamptz */"));
    }
}
