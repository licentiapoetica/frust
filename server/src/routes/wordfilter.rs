//! Word filter admin page and API (port of src/inc/routes/wordfilter.mjs). The filter itself is
//! `crate::wordfilter`; the table is created by the startup migrations.

use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_resp};
use crate::social::helpers::audit_log;
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/wordfilter\/?$", Guard::Auth, page);
    r.exact(Method::GET, "/api/v2/admin/wordfilter", Some(Guard::Auth), list);
    r.exact(Method::POST, "/api/v2/admin/wordfilter", Some(Guard::Auth), create);
    r.exact(Method::POST, "/api/v2/admin/wordfilter/delete", Some(Guard::Auth), delete);
}

async fn page(st: AppState, req: Req) -> Res {
    let csrf = req.session().and_then(|s| s.csrf_token.clone()).unwrap_or_default();
    // The old route also passed `totals: lib.countf0cks()`, which the template never uses.
    let data = json!({ "session": req.session_value(), "csrf_token": csrf });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/wordfilter", data)))
}

fn err500(e: impl std::fmt::Display) -> Res {
    let msg = crate::util::log_error(&e, "WORDFILTER");
    Ok(json_resp(&json!({ "success": false, "msg": msg }), 500))
}

async fn list(st: AppState, _req: Req) -> Res {
    match sql!("SELECT * FROM wordfilter ORDER BY created_at DESC").fetch_all(&st.db).await {
        Ok(rows) => Ok(json_resp(&json!({ "success": true, "filters": rows.into_iter().map(Value::Object).collect::<Vec<_>>() }), 200)),
        Err(e) => err500(e),
    }
}

fn str_field(body: &Value, k: &str) -> Option<String> {
    body.get(k).and_then(Value::as_str).map(str::to_string)
}

async fn create(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let word = str_field(&body, "word").unwrap_or_default();
    let replacement = str_field(&body, "replacement").unwrap_or_default();
    let (word, replacement) = (word.trim(), replacement.trim());
    if word.is_empty() || replacement.is_empty() {
        return Ok(json_resp(&json!({ "success": false, "msg": "Word and replacement are required" }), 400));
    }
    if word.to_lowercase() == replacement.to_lowercase() {
        return Ok(json_resp(&json!({ "success": false, "msg": "Word and replacement cannot be identical" }), 400));
    }
    let r = sql!(
        "INSERT INTO wordfilter (word, replacement) VALUES (", word, ", ", replacement,
        ") ON CONFLICT (word) DO UPDATE SET replacement = EXCLUDED.replacement"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        return err500(e);
    }
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    audit_log(&st, uid, "add_wordfilter", "wordfilter", "0", Some(json!({ "word": word, "replacement": replacement }))).await;
    Ok(json_resp(&json!({ "success": true }), 200))
}

async fn delete(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let id = body.get("id").and_then(crate::social::helpers::num);
    let Some(id) = id.filter(|i| *i != 0) else {
        return Ok(json_resp(&json!({ "success": false, "msg": "ID required" }), 400));
    };
    let rows: Vec<Row> = match sql!("DELETE FROM wordfilter WHERE id = ", id as i32, " RETURNING word").fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => return err500(e),
    };
    if let Some(row) = rows.first() {
        let uid = req.session().map(|s| s.id).unwrap_or(0);
        audit_log(&st, uid, "delete_wordfilter", "wordfilter", &id.to_string(), Some(json!({ "word": row.string("word") }))).await;
    }
    Ok(json_resp(&json!({ "success": true }), 200))
}
