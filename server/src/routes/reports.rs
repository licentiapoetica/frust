//! User reports (port of src/inc/routes/reports.mjs).

use crate::cap::Captcha;
use crate::db::RowExt;
use crate::http::{Req, Res, Router, json_resp};
use crate::social::helpers::{audit_log, js_number, js_str};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    use crate::http::Guard::ModAuth;
    r.post(r"^\/api\/v2\/report\/?$", submit);
    r.get_g(r"^\/api\/v2\/mod\/reports\/?$", ModAuth, list);
    r.post_g(r"^\/api\/v2\/mod\/reports\/(?<id>\d+)\/resolve\/?$", ModAuth, resolve);
}

const VALID_CATEGORIES: [&str; 6] = ["wrong_rating", "spam", "duplicate", "copyright", "illegal", "other"];

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn err500(e: impl std::fmt::Display) -> Res {
    j(json!({ "success": false, "msg": crate::util::log_error(&e, "reports") }), 500)
}

/// `x ? +x : null` — `Err` for a set value that isn't a usable integer id.
fn opt_id(v: Option<&Value>) -> Result<Option<i32>, ()> {
    if !v.map(crate::config::truthy).unwrap_or(false) {
        return Ok(None);
    }
    let f = match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => js_number(s),
        Some(Value::Bool(true)) => Some(1.0),
        _ => None,
    };
    match f {
        Some(f) if f.fract() == 0.0 && f.abs() <= i32::MAX as f64 => Ok(Some(f as i32)),
        _ => Err(()),
    }
}

// ── POST /api/v2/report ─────────────────────────────────────────────────────

async fn submit(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let reason = js_str(body.get("reason")).unwrap_or_default();

    // Urlencoded repeated `categories[]` keys arrive joined with ","; JSON may send an array.
    let raw = body.get("categories[]").filter(|v| crate::config::truthy(v)).or_else(|| body.get("categories")).cloned().unwrap_or(Value::Null);
    let cats: Vec<String> = match raw {
        Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        Value::String(s) if !s.is_empty() => s.split(',').map(|c| c.trim().to_string()).collect(),
        _ => vec![],
    };
    let categories: Vec<String> = cats.into_iter().filter(|c| VALID_CATEGORIES.contains(&c.as_str())).collect();

    if reason.trim().is_empty() && categories.is_empty() {
        return j(json!({ "success": false, "msg": "Please select at least one reason or provide a description." }), 400);
    }
    let (Ok(item_id), Ok(comment_id), Ok(user_id)) = (opt_id(body.get("item_id")), opt_id(body.get("comment_id")), opt_id(body.get("reported_user_id"))) else {
        return j(json!({ "success": false, "msg": "Invalid id." }), 400);
    };
    if item_id.is_none() && comment_id.is_none() && user_id.is_none() {
        return j(json!({ "success": false, "msg": "Must specify an item, comment, or user to report." }), 400);
    }

    let cfg = st.cfg();
    let ip = req.real_ip();
    if crate::cap::required(&cfg, &req) {
        let token = js_str(body.get("cap-token"));
        // Without JavaScript: the image captcha (cap_image.rs)
        let (image_id, image_answer) = (js_str(body.get("cap-image-id")), js_str(body.get("cap-image-answer")));
        match crate::cap::verify_form(token.as_deref(), image_id.as_deref(), image_answer.as_deref(), false) {
            Captcha::Ok => {}
            Captcha::Missing => return j(json!({ "success": false, "msg": "Please complete the captcha." }), 400),
            Captcha::Failed => return j(json!({ "success": false, "msg": "Captcha expired or invalid. Please try again." }), 400),
        }
    }

    let reporter = req.session().map(|s| s.id as i32);
    let ins = sql!(
        "INSERT INTO reports (reporter_id, reporter_ip, item_id, comment_id, user_id, reason, categories) VALUES (",
        reporter,
        ", ",
        crate::security::storable_ip(&cfg, &ip),
        ", ",
        item_id,
        ", ",
        comment_id,
        ", ",
        user_id,
        ", ",
        reason.trim(),
        ", ",
        categories,
        ") RETURNING id"
    )
    .fetch_scalar::<i32, _>(&st.db)
    .await;
    let report_id = match ins {
        Ok(Some(id)) => id,
        Ok(None) => return err500("insert returned no id"),
        Err(e) => return err500(e),
    };

    // Notify all mods and admins
    let notify = async {
        let mut resolved_item = item_id;
        if resolved_item.is_none() {
            if let Some(cid) = comment_id {
                resolved_item = sql!("SELECT item_id FROM comments WHERE id = ", cid).fetch_scalar::<i32, _>(&st.db).await?;
            }
        }
        sql!(
            r#"INSERT INTO notifications (user_id, type, reference_id, item_id) SELECT id, 'report', "#,
            report_id,
            ", ",
            resolved_item,
            r#" FROM "user" WHERE admin = true OR is_moderator = true"#
        )
        .execute(&st.db)
        .await?;
        anyhow::Ok(())
    };
    if let Err(e) = notify.await {
        tracing::error!("[REPORT] Failed to send notifications: {e:#}");
    }
    j(json!({ "success": true, "msg": "Report submitted successfully." }), 200)
}

// ── GET /api/v2/mod/reports ─────────────────────────────────────────────────

async fn list(st: AppState, req: Req) -> Res {
    let status = req.query("status").filter(|s| !s.is_empty()).unwrap_or("pending").to_string();
    let page = req.query("page").filter(|s| !s.is_empty()).map(|p| js_number(p).unwrap_or(f64::NAN)).unwrap_or(1.0);
    let page = if page.is_finite() && page >= 1.0 { (page.floor() as i64).min(1_000_000) } else { 1 };
    let limit = 20i64;
    let offset = (page - 1) * limit;

    let r: anyhow::Result<Value> = async {
        let reports = sql!(
            r#"SELECT r.*,
                rep.user AS reporter_name,
                COALESCE(tgt_u.user, tgt_auth.user, comm_auth.user) AS reported_user_name,
                COALESCE(NULLIF(r.user_id, 0), tgt_auth.id, comm_auth.id) AS reported_user_id,
                COALESCE(tgt_u.admin, tgt_auth.admin, comm_auth.admin) AS reported_user_is_admin,
                resolver.user AS resolver_name,
                i.dest AS item_dest,
                tgt_auth.id AS item_user_id,
                tgt_auth.user AS item_user_name,
                (SELECT coalesce(json_agg(json_build_object('id', t.id, 'tag', t.tag, 'normalized', t.normalized)), '[]')
                 FROM tags_assign ta
                 JOIN tags t ON ta.tag_id = t.id
                 WHERE ta.item_id = COALESCE(r.item_id, c.item_id)) as item_tags,
                c.content AS comment_body,
                COALESCE(r.item_id, c.item_id) AS resolved_item_id,
                COALESCE(i.dest, ci.dest) AS resolved_item_dest,
                COALESCE(i.mime, ci.mime) AS resolved_item_mime,
                COALESCE(i.visibility, ci.visibility, 0) AS resolved_item_visibility
            FROM reports r
            LEFT JOIN "user" rep ON r.reporter_id = rep.id
            LEFT JOIN "user" tgt_u ON r.user_id = tgt_u.id
            LEFT JOIN "user" resolver ON r.resolved_by = resolver.id
            LEFT JOIN items i ON r.item_id = i.id
            LEFT JOIN "user" tgt_auth ON i.username = tgt_auth.user
            LEFT JOIN comments c ON r.comment_id = c.id
            LEFT JOIN items ci ON c.item_id = ci.id
            LEFT JOIN "user" comm_auth ON c.user_id = comm_auth.id
            WHERE r.status = "#,
            &status,
            " ORDER BY r.created_at DESC LIMIT ",
            limit,
            " OFFSET ",
            offset
        )
        .fetch_all(&st.db)
        .await?;

        let reports: Vec<Value> = reports
            .into_iter()
            .map(|mut r| {
                if let Some(Value::Array(tags)) = r.get_mut("item_tags") {
                    for t in tags.iter_mut() {
                        let b = super::mod_panel::tag_badge(t["tag"].as_str().unwrap_or(""), t["normalized"].as_str().unwrap_or(""));
                        t["badge"] = json!(b);
                    }
                }
                Value::Object(r)
            })
            .collect();

        let total = sql!("SELECT count(*) as c FROM reports WHERE status = ", &status).fetch_optional(&st.db).await?.unwrap_or_default();
        let total_n = total.i64("c").unwrap_or(0);
        let emojis: Vec<Value> = sql!("SELECT name, url FROM custom_emojis").fetch_all(&st.db).await?.into_iter().map(Value::Object).collect();
        Ok(json!({
            "success": true,
            "reports": reports,
            "emojis": emojis,
            "page": page,
            "pages": (total_n + limit - 1) / limit,
            "total": total.val("c"),
        }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => err500(format!("{e:#}")),
    }
}

// ── POST /api/v2/mod/reports/:id/resolve ────────────────────────────────────

async fn resolve(st: AppState, mut req: Req) -> Res {
    let Some(id) = req.param("id").and_then(|s| s.parse::<i32>().ok()) else { return j(json!({ "success": false, "msg": "Report not found." }), 404) };
    let action = js_str(req.post().await?.get("action")).unwrap_or_default();
    if action != "resolved" && action != "rejected" {
        return j(json!({ "success": false, "msg": "Invalid action. Must be 'resolved' or 'rejected'." }), 400);
    }
    let uid = req.session().map(|s| s.id as i32);
    let r: anyhow::Result<Option<i32>> = async {
        let updated = sql!("UPDATE reports SET status = ", &action, ", resolved_by = ", uid, " WHERE id = ", id, " RETURNING id").fetch_scalar::<i32, _>(&st.db).await?;
        if updated.is_some() {
            // Mark the report's notifications read for every moderator
            sql!("UPDATE notifications SET is_read = true WHERE type = 'report' AND reference_id = ", id).execute(&st.db).await?;
        }
        Ok(updated)
    }
    .await;
    match r {
        Ok(None) => j(json!({ "success": false, "msg": "Report not found." }), 404),
        Ok(Some(_)) => {
            audit_log(&st, uid.unwrap_or(0) as i64, "resolve_report", "report", &id.to_string(), Some(json!({ "status": action }))).await;
            j(json!({ "success": true, "msg": format!("Report marked as {action}.") }), 200)
        }
        Err(e) => err500(format!("{e:#}")),
    }
}
