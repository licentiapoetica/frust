//! Upload status for the uploader (port of src/inc/routes/pending.mjs).
//!   GET /pending                        own recent uploads with status (pending / live / denied / removed)
//!   GET /pending/t/:id.webp             thumbnail of an own pending upload (pending files are not public)
//!   POST /pending/delete                withdraw an own upload that is still pending (files + row are removed)
//!   GET /api/v2/uploads/:id/status      JSON status for API clients (session or X-Api-Key)

use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, files, html, json_raw, raw, redirect};
use crate::media::is_plain_name;
use crate::session::Session;
use crate::settings::{enable_item_slugs, session_owner_name};
use crate::social::helpers::{audit_log, js_number};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;

pub fn register(r: &mut Router) {
    r.get(r"^\/pending\/?$", page);
    r.get(r"^\/pending\/t\/(?<id>\d+)\.webp$", thumb);
    r.post(r"^\/pending\/delete\/?$", withdraw);
    r.get(r"^\/api\/v2\/uploads\/(?<id>\d+)\/status\/?$", status);
}

const RECENT_LIMIT: i64 = 50;

fn status_of(r: &Row) -> &'static str {
    if r.bool("is_purged") {
        "removed"
    } else if r.bool("is_deleted") {
        "denied"
    } else if r.bool("active") {
        "live"
    } else {
        "pending"
    }
}

fn item_path(st: &AppState, r: &Row) -> Value {
    match r.str("slug").filter(|s| !s.is_empty()) {
        Some(s) if enable_item_slugs(&st.cfg()) => json!(s),
        _ => r.val("id").clone(),
    }
}

/// Latest moderator reason for denied/removed items (deny, delete or purge).
async fn reasons_for(st: &AppState, ids: &[i64]) -> HashMap<i64, Value> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let ids: Vec<String> = ids.iter().map(i64::to_string).collect();
    let rows = sql!(
        "select distinct on (target_id) target_id, details->>'reason' as reason from audit_log where target_id = any(",
        ids,
        "::text[]) and action in ('deny_item', 'delete_item', 'purge_item') order by target_id, created_at desc"
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();
    rows.into_iter().filter_map(|r| r.str("target_id").and_then(|t| t.parse().ok()).map(|id| (id, r.val("reason").clone()))).collect()
}

fn is_owner_of(row: &Row, owner: Option<&str>) -> bool {
    match (owner, row.str("username")) {
        (Some(o), Some(u)) if !o.is_empty() && !u.is_empty() => u.to_lowercase() == o.to_lowercase(),
        _ => false,
    }
}

fn jr(code: u16, v: Value) -> Res {
    Ok(json_raw(&v, code))
}

// ── GET /pending ────────────────────────────────────────────────────────────

async fn page(st: AppState, req: Req) -> Res {
    if req.session().is_none() {
        return Ok(redirect("/login"));
    }
    let owner = session_owner_name(req.session());
    let rows = match &owner {
        Some(o) => {
            sql!(
                "select id, slug, mime, title, original_filename, stamp, size, active, is_deleted, is_purged, is_album, album_count, visibility from items where lower(username) = ",
                o.to_lowercase(),
                " order by id desc limit ",
                RECENT_LIMIT
            )
            .fetch_all(&st.db)
            .await?
        }
        None => vec![],
    };
    let denied: Vec<i64> = rows.iter().filter(|r| r.bool("is_deleted")).filter_map(|r| r.i64("id")).collect();
    let reasons = reasons_for(&st, &denied).await;
    let tr = st.i18n.translator("en");
    let now = crate::util::now_secs();
    let mut counts = serde_json::Map::new();
    for k in ["pending", "live", "denied", "removed"] {
        counts.insert(k.into(), json!(0));
    }
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let status = status_of(r);
            let id = r.i64("id").unwrap_or(0);
            if let Some(Value::Number(n)) = counts.get_mut(status) {
                *n = (n.as_i64().unwrap_or(0) + 1).into();
            }
            let stamp = r.i64("stamp").filter(|s| *s != 0);
            let title = r.str("title").filter(|t| !t.is_empty()).or_else(|| r.str("original_filename").filter(|t| !t.is_empty())).unwrap_or("");
            let reason = if status == "denied" || status == "removed" {
                reasons.get(&id).filter(|v| crate::config::truthy(v)).cloned().unwrap_or(json!(""))
            } else {
                json!("")
            };
            json!({
                "id": id,
                "path": item_path(&st, r),
                "status": status,
                "title": title,
                "mime": r.val("mime"),
                "is_album": r.bool("is_album"),
                "album_count": r.i64("album_count").unwrap_or(0),
                "size_fmt": if crate::config::truthy(r.val("size")) { crate::util::format_size(r.val("size")) } else { String::new() },
                "time_ago": stamp.map(|s| crate::util::time_ago(&tr, now - s)).unwrap_or_default(),
                "time_full": stamp.and_then(|s| chrono::DateTime::from_timestamp(s, 0)).map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap_or_default(),
                "thumb": match status { "live" => format!("/t/{id}.webp"), "pending" => format!("/pending/t/{id}.webp"), _ => String::new() },
                "reason": reason,
            })
        })
        .collect();
    let data = json!({
        "items": items,
        "counts": counts,
        "manual_approval_on": st.settings.manual_approval(),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "pending", data)))
}

// ── GET /pending/t/:id.webp ─────────────────────────────────────────────────

async fn thumb(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return Ok(raw(401, "")) };
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return Ok(raw(404, "")) };
    let row = sql!("select username, active, is_deleted from items where id = ", id, " limit 1").fetch_optional(&st.db).await?;
    let staff = s.admin || s.is_moderator;
    let owner = session_owner_name(Some(s));
    let ok = row.as_ref().map(|r| !r.bool("active") && !r.bool("is_deleted") && (staff || is_owner_of(r, owner.as_deref()))).unwrap_or(false);
    if !ok {
        return Ok(raw(404, ""));
    }
    let file = st.cfg().paths.pending.join("t").join(format!("{id}.webp"));
    if !tokio::fs::metadata(&file).await.map(|m| m.is_file()).unwrap_or(false) {
        return Ok(raw(404, ""));
    }
    let mut r = files::serve_file(&req, &file, Some("image/webp")).await;
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-cache"));
    Ok(r)
}

// ── POST /pending/delete ────────────────────────────────────────────────────

/// Every file a pending upload keeps in the pending folder (main file, thumbs, cover art, album sub-items).
async fn pending_files_of(st: &AppState, row: &Row, id: i64) -> Vec<PathBuf> {
    let p = st.cfg().paths.pending.clone();
    let mut files = vec![p.join("t").join(format!("{id}.webp")), p.join("t").join(format!("{id}_blur.webp")), p.join("ca").join(format!("{id}.webp"))];
    let dest = row.string("dest");
    if !dest.is_empty() && row.str("mime") != Some("video/youtube") && is_plain_name(&dest) {
        files.push(p.join("b").join(&dest));
    }
    if row.bool("is_album") {
        let subs = sql!("select dest from album_items where item_id = ", id as i32).fetch_all(&st.db).await.unwrap_or_default();
        for sub in subs {
            let d = sub.string("dest");
            if !is_plain_name(&d) {
                continue;
            }
            files.push(p.join("b").join(&d));
            files.push(p.join("t").join(format!("{}.webp", crate::media::delete::album_thumb_name(&d).trim_end_matches(".webp"))));
        }
    }
    files
}

async fn withdraw(st: AppState, mut req: Req) -> Res {
    if req.session().is_none() {
        return jr(401, json!({ "success": false, "msg": "Login required" }));
    }
    let body = req.post().await?.clone();
    let id = match body.get("id") {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => js_number(s),
        _ => None,
    }
    .filter(|f| f.fract() == 0.0 && *f > 0.0 && *f <= i32::MAX as f64)
    .map(|f| f as i32);
    let Some(id) = id else { return jr(400, json!({ "success": false, "msg": "No ID provided" })) };

    let row = sql!("select id, username, dest, mime, is_album, active, is_deleted, is_purged from items where id = ", id, " limit 1").fetch_optional(&st.db).await?;
    let owner = session_owner_name(req.session());
    let Some(row) = row.filter(|r| is_owner_of(r, owner.as_deref())) else { return jr(404, json!({ "success": false, "msg": "Upload not found" })) };
    if row.bool("active") || row.bool("is_deleted") || row.bool("is_purged") {
        return jr(409, json!({ "success": false, "msg": "Only pending uploads can be deleted" }));
    }

    // Collect files before the row goes (album_items cascade). The active = false guard lets a concurrent
    // approval win; files are only touched once the row is gone. Cascades clear tags, notifications, reports.
    let files = pending_files_of(&st, &row, id as i64).await;
    let deleted = sql!("delete from items where id = ", id, " and active = false and is_deleted = false returning id").fetch_optional(&st.db).await?;
    if deleted.is_none() {
        return jr(409, json!({ "success": false, "msg": "Upload was already reviewed" }));
    }
    crate::hidden_items::invalidate();
    for f in files {
        let _ = tokio::fs::remove_file(f).await;
    }
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    audit_log(&st, uid, "withdraw_item", "item", &id.to_string(), Some(json!({ "filename": row.val("dest"), "uploader_name": row.val("username") }))).await;
    jr(200, json!({ "success": true, "id": id }))
}

// ── GET /api/v2/uploads/:id/status ──────────────────────────────────────────

/// Session user, or the account behind an X-Api-Key header (for API clients).
async fn resolve_user(st: &AppState, req: &Req) -> Option<Session> {
    if let Some(s) = req.session().filter(|s| s.id != 0) {
        return Some(s.clone());
    }
    let key = req.header("x-api-key").filter(|k| !k.is_empty())?;
    if st.cfg().is_false("websrv.enable_user_api_keys") {
        return None;
    }
    let row = sql!(
        r#"select u.id, u.user, u.login, u.admin, u.is_moderator, u.banned from user_api_keys k join "user" u on u.id = k.user_id where k.api_key = "#,
        key,
        " limit 1"
    )
    .fetch_optional(&st.db)
    .await
    .ok()
    .flatten()?;
    if row.bool("banned") {
        return None;
    }
    Session::from_row(row).ok()
}

async fn status(st: AppState, req: Req) -> Res {
    let Some(user) = resolve_user(&st, &req).await else {
        return jr(401, json!({ "success": false, "msg": "Login or X-Api-Key required" }));
    };
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return jr(404, json!({ "success": false, "msg": "Upload not found" })) };
    let row = sql!("select id, slug, username, active, is_deleted, is_purged from items where id = ", id, " limit 1").fetch_optional(&st.db).await?;
    let owner = session_owner_name(Some(&user));
    let staff = user.admin || user.is_moderator;
    let Some(row) = row.filter(|r| staff || is_owner_of(r, owner.as_deref())) else {
        return jr(404, json!({ "success": false, "msg": "Upload not found" }));
    };
    let status = status_of(&row);
    let reason = if status == "denied" || status == "removed" {
        reasons_for(&st, &[id as i64]).await.remove(&(id as i64)).filter(|v| crate::config::truthy(v)).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let base = st.cfg().url_full().to_string();
    let path = item_path(&st, &row);
    let path = path.as_str().map(str::to_string).unwrap_or_else(|| path.to_string());
    jr(
        200,
        json!({
            "success": true,
            "itemid": row.val("id"),
            "slug": row.str("slug").filter(|s| !s.is_empty()).map(Value::from).unwrap_or(Value::Null),
            "status": status,
            "reason": reason,
            "url": if status == "live" { format!("{base}/{path}") } else { format!("{base}/pending#i{id}") },
            "status_url": format!("{base}/pending#i{id}"),
        }),
    )
}
