//! Ban management: fingerprint/hardware/IP ban lists, user (un)ban, per-user IP history
//! (src/inc/routes/admin.mjs).

use super::{body_id, body_str, body_truthy, err_msg, totals};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_resp, reply};
use crate::security::{AnonBan, ban_anonymous_user};
use crate::sql;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/bans(\/)?$", Guard::ModAuth, bans_page);
    r.post_g(r"^\/api\/v2\/admin\/bans\/fingerprint\/ban\/?$", Guard::ModAuth, fingerprint_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/fingerprint\/unban\/?$", Guard::ModAuth, fingerprint_unban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/ip\/ban\/?$", Guard::ModAuth, ip_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/ip\/unban\/?$", Guard::ModAuth, ip_unban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/hardware\/ban\/?$", Guard::ModAuth, hardware_ban);
    r.post_g(r"^\/api\/v2\/admin\/bans\/hardware\/unban\/?$", Guard::ModAuth, hardware_unban);
    r.get_g(r"^\/admin\/user\/(?<userId>\d+)\/ips(\/)?$", Guard::Auth, user_ips);
    r.post_g(r"^\/api\/v2\/admin\/ban\/?$", Guard::ModAuth, user_ban);
    r.post_g(r"^\/api\/v2\/admin\/unban\/?$", Guard::ModAuth, user_unban);
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

/// Ban end for a `duration` in hours; `None` = permanent (`'permanent'` or empty).
fn ban_expiry(duration: Option<&str>) -> anyhow::Result<Option<DateTime<Utc>>> {
    match duration.map(str::trim) {
        None | Some("") | Some("permanent") => Ok(None),
        Some(d) => {
            let hours = crate::social::helpers::js_parse_int(d).ok_or_else(|| anyhow::anyhow!("Invalid duration"))?;
            Ok(Some(Utc::now() + chrono::Duration::hours(hours.clamp(-1_000_000, 1_000_000))))
        }
    }
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

async fn anon_ban(st: &AppState, req: &Req, body: &Value, hardware: bool) -> anyhow::Result<Value> {
    let field = if hardware { "hw_fingerprint" } else { "fingerprint" };
    let fp = body_str(body, field).filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!(if hardware { "Missing hardware fingerprint" } else { "Missing fingerprint" })
    })?;
    let duration = body_str(body, "duration");
    let expires = ban_expiry(duration.as_deref())?;
    let reason = body_str(body, "reason").filter(|r| !r.is_empty());
    let result = ban_anonymous_user(
        st,
        AnonBan {
            user_id: if body_truthy(body, "user_id") { body_id(body, "user_id") } else { None },
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
    let details = json!({ field: fp, "reason": body.get("reason"), "duration": body.get("duration") });
    crate::audit::log(st, sid(req), if hardware { "ban_hardware" } else { "ban_fingerprint" }, if hardware { "hardware" } else { "fingerprint" }, None, details).await;
    Ok(json!({ "success": true, "result": result }))
}

async fn fingerprint_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    answer(anon_ban(&st, &req, &body, false).await)
}

async fn hardware_ban(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    answer(anon_ban(&st, &req, &body, true).await)
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
    let r = async {
        let ip = body_str(&body, "ip").filter(|s| !s.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing IP address"))?;
        let expires = ban_expiry(body_str(&body, "duration").as_deref())?;
        // Already a stored hash (banned from the IP list) or a raw address typed by the moderator
        let is_hash = ip.len() == 64 && ip.bytes().all(|b| b.is_ascii_hexdigit());
        let ip_hash = if is_hash { ip.to_lowercase() } else { crate::security::hash_ip(&cfg, &ip) };
        // With hashing on, the raw address is never persisted, not even in the ban list
        let stored = if is_hash || crate::settings::hash_user_ips(&cfg) { ip_hash.clone() } else { ip.clone() };
        let reason = body_str(&body, "reason").filter(|r| !r.is_empty()).unwrap_or_else(|| "Banned by moderator".into());
        let exp_bind: crate::db::Bind = expires.map(Into::into).unwrap_or(crate::db::Bind::Null);
        sql!(
            "INSERT INTO banned_ips (ip, ip_hash, banned_by, reason, expires_at) VALUES (", stored, ", ", ip_hash.clone(), ", ", sid(&req) as i32, ", ",
            reason.clone(), ", ", exp_bind,
            ") ON CONFLICT (ip) DO UPDATE SET reason = EXCLUDED.reason, expires_at = EXCLUDED.expires_at, banned_by = EXCLUDED.banned_by, ip_hash = EXCLUDED.ip_hash"
        )
        .execute(&st.db)
        .await?;
        crate::audit::log(&st, sid(&req), "ban_ip", "ip", None, json!({ "ip": ip, "reason": body.get("reason"), "duration": body.get("duration") })).await;
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
        let duration = body_str(&body, "duration");
        let Some(target) = sql!(r#"select admin, "user", login from "user" where id = "#, user_id, " limit 1").fetch_optional(&st.db).await? else {
            anyhow::bail!("User not found");
        };
        if target.str("login") == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is a protected system ghost and cannot be modified.");
        }
        if target.bool("admin") && !session.admin {
            return Ok(Err("Moderators cannot ban administrators."));
        }
        // Moderators can ban for at most 48 hours
        if !session.admin {
            let hours = duration.as_deref().and_then(crate::social::helpers::js_parse_int);
            if duration.as_deref() == Some("permanent") || hours.map(|h| h > 48).unwrap_or(false) || hours.is_none() {
                return Ok(Err("Moderators can only issue bans for up to 48 hours."));
            }
        }
        // A missing duration used to become an invalid date; treat it as an error, not a permanent ban.
        if duration.as_deref().map(str::trim).unwrap_or("").is_empty() {
            anyhow::bail!("Invalid duration");
        }
        let expires = ban_expiry(duration.as_deref())?;
        let exp_bind: crate::db::Bind = expires.map(Into::into).unwrap_or(crate::db::Bind::Null);
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
            json!({ "reason": body.get("reason"), "duration": body.get("duration"), "target_user": target.val("user") }),
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
}
