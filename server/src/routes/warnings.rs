//! Moderator warnings (port of src/inc/routes/warnings.mjs).

use crate::http::{Req, Res, Router, json_resp};
use crate::social::helpers::{audit_log, js_number, js_str, notify};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    use crate::http::Guard::{LoggedIn, ModAuth};
    r.post_g(r"^\/api\/v2\/mod\/warnings\/issue\/?$", ModAuth, issue);
    r.get_g(r"^\/api\/v2\/user\/warnings\/?$", LoggedIn, list);
    r.post_g(r"^\/api\/v2\/user\/warnings\/(?<id>\d+)\/acknowledge\/?$", LoggedIn, acknowledge);
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn err500(e: anyhow::Error) -> Res {
    j(json!({ "success": false, "msg": crate::util::log_error(&format!("{e:#}"), "warnings") }), 500)
}

fn sid(req: &Req) -> i32 {
    req.session().map(|s| s.id as i32).unwrap_or(0)
}

async fn issue(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let reason = js_str(body.get("reason")).unwrap_or_default().trim().to_string();
    let user_id = body.get("user_id").filter(|v| crate::config::truthy(v));
    if user_id.is_none() || reason.is_empty() {
        return j(json!({ "success": false, "msg": "User ID and reason are required." }), 400);
    }
    let uid = match user_id {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => js_number(s),
        _ => None,
    }
    .filter(|f| f.fract() == 0.0 && f.abs() <= i32::MAX as f64)
    .map(|f| f as i32);
    let Some(uid) = uid else { return j(json!({ "success": false, "msg": "User ID and reason are required." }), 400) };

    let r: anyhow::Result<()> = async {
        let wid = sql!("INSERT INTO user_warnings (user_id, admin_id, reason) VALUES (", uid, ", ", sid(&req), ", ", &reason, ") RETURNING id")
            .fetch_scalar::<i32, _>(&st.db)
            .await?;
        if let Some(wid) = wid {
            sql!(
                "INSERT INTO notifications (user_id, type, reference_id, data, is_read) VALUES (",
                uid,
                ", 'warning', 0, ",
                json!({ "reason": reason, "warning_id": wid }),
                ", false)"
            )
            .execute(&st.db)
            .await?;
            notify(&st, "warnings", &json!({ "user_id": uid, "warning_id": wid, "reason": reason })).await;
        }
        let target: Option<String> = sql!(r#"SELECT "user" FROM "user" WHERE id = "#, uid, " LIMIT 1").fetch_scalar(&st.db).await?;
        let username = target.unwrap_or_else(|| uid.to_string());
        audit_log(&st, sid(&req) as i64, "issue_warning", "user", &uid.to_string(), Some(json!({ "reason": reason, "target_user": username }))).await;
        Ok(())
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true, "msg": "Warning issued successfully." }), 200),
        Err(e) => err500(e),
    }
}

async fn list(st: AppState, req: Req) -> Res {
    match sql!("SELECT id, reason, created_at FROM user_warnings WHERE user_id = ", sid(&req), " AND acknowledged = FALSE ORDER BY created_at ASC")
        .fetch_all(&st.db)
        .await
    {
        Ok(rows) => j(json!({ "success": true, "warnings": rows }), 200),
        Err(e) => err500(e),
    }
}

async fn acknowledge(st: AppState, req: Req) -> Res {
    let Some(id) = req.param("id").and_then(|s| s.parse::<i32>().ok()) else {
        return j(json!({ "success": false, "msg": "Warning not found or already acknowledged." }), 404);
    };
    match sql!("UPDATE user_warnings SET acknowledged = TRUE WHERE id = ", id, " AND user_id = ", sid(&req), " RETURNING id").fetch_optional(&st.db).await {
        Ok(None) => j(json!({ "success": false, "msg": "Warning not found or already acknowledged." }), 404),
        Ok(Some(_)) => j(json!({ "success": true, "msg": "Warning acknowledged." }), 200),
        Err(e) => err500(e),
    }
}
