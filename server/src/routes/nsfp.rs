//! NSFP ("not safe for public") tag list (port of src/inc/routes/nsfp.mjs).

use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, html, json_resp};
use crate::social::helpers::{audit_log, js_parse_int, js_str};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Map, Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/nsfp\/?$", Guard::Auth, page);
    r.exact(Method::GET, "/api/v2/admin/nsfp", Some(Guard::Auth), list);
    r.exact(Method::GET, "/api/v2/admin/nsfp/search", Some(Guard::Auth), search);
    r.exact(Method::POST, "/api/v2/admin/nsfp/add", Some(Guard::Auth), add);
    r.exact(Method::POST, "/api/v2/admin/nsfp/remove", Some(Guard::Auth), remove);
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn err500(e: anyhow::Error) -> Res {
    j(json!({ "success": false, "msg": crate::util::log_error(&format!("{e:#}"), "[NSFP]") }), 500)
}

/// `lib.countf0cks()`
pub async fn count_f0cks(st: &AppState) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    let nsfl = cfg.nsfl_tag_id() as i32;
    let r = sql!(
        r#"SELECT
            (select count(*) from items where id in (select item_id from tags_assign group by item_id) and active = true and is_deleted = false) as tagged,
            (select count(*) from items where not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, "#,
        nsfl,
        r#")) and active = true and is_deleted = false) as untagged,
            (select count(*) from items where id in (select item_id from tags_assign where tag_id = 1 group by item_id) and active = true and is_deleted = false) as sfw,
            (select count(*) from items where id in (select item_id from tags_assign where tag_id = 2 group by item_id) and active = true and is_deleted = false) as nsfw,
            (select count(*) from items where id in (select item_id from tags_assign where tag_id = "#,
        nsfl,
        r#" group by item_id) and active = true and is_deleted = false) as nsfl,
            (select count(*) from items where active = false and is_deleted = true) as deleted,
            (select count(*) from items where active = false and is_deleted = false) as pending,
            (select max(id) from items) as last"#
    )
    .fetch_optional(&st.db)
    .await?
    .unwrap_or_default();
    let n = |k: &str| r.i64(k).unwrap_or(0);
    let (tagged, untagged, deleted, pending) = (n("tagged"), n("untagged"), n("deleted"), n("pending"));
    Ok(json!({
        "tagged": tagged,
        "untagged": untagged,
        "total": tagged + untagged,
        "deleted": deleted,
        "pending": pending,
        "untracked": n("last") - (tagged + untagged + deleted + pending),
        "sfw": n("sfw"),
        "nsfw": n("nsfw"),
        "nsfl": if cfg.enable_nsfl() { n("nsfl") } else { 0 },
    }))
}

async fn page(st: AppState, req: Req) -> Res {
    let totals = match count_f0cks(&st).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[NSFP] Page render failed: {e:#}");
            return Ok(crate::http::reply(500, "text/html", "Internal server error"));
        }
    };
    let csrf = req.session().and_then(|s| s.csrf_token.clone()).unwrap_or_default();
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/nsfp", json!({ "totals": totals, "csrf_token": csrf }))))
}

async fn list(st: AppState, _req: Req) -> Res {
    let ids = st.settings.nsfp_ids();
    let r: anyhow::Result<Value> = async {
        let ids32: Vec<i32> = ids.iter().filter_map(|i| i32::try_from(*i).ok()).collect();
        let rows = if ids32.is_empty() {
            vec![]
        } else {
            sql!("SELECT id, tag, normalized FROM tags WHERE id = ANY(", ids32.clone(), ") ORDER BY id").fetch_all(&st.db).await?
        };
        let enriched: Vec<Value> = ids
            .iter()
            .map(|id| match rows.iter().find(|r| r.i64("id") == Some(*id)) {
                Some(r) => Value::Object(r.clone()),
                None => json!({ "id": id, "tag": "(unknown)", "normalized": null }),
            })
            .collect();
        let blocked: i64 = if ids32.is_empty() {
            0
        } else {
            sql!(
                "SELECT COUNT(DISTINCT item_id) AS cnt FROM tags_assign WHERE tag_id = ANY(",
                ids32,
                ") AND item_id IN (SELECT id FROM items WHERE active = true)"
            )
            .fetch_scalar(&st.db)
            .await?
            .unwrap_or(0)
        };
        Ok(json!({ "success": true, "nsfp": enriched, "raw_ids": ids, "blocked_count": blocked }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => err500(e),
    }
}

async fn search(st: AppState, req: Req) -> Res {
    let q = req.query("q").unwrap_or("").trim().to_string();
    if q.is_empty() {
        return j(json!({ "success": true, "tags": [] }), 200);
    }
    let pattern = format!("%{q}%");
    match sql!("SELECT id, tag, normalized FROM tags WHERE tag ILIKE ", &pattern, " OR normalized ILIKE ", &pattern, " ORDER BY tag LIMIT 20")
        .fetch_all(&st.db)
        .await
    {
        Ok(tags) => j(json!({ "success": true, "tags": tags }), 200),
        Err(e) => err500(e.into()),
    }
}

fn tag_id_of(body: &Value) -> Option<i64> {
    js_str(body.get("tag_id")).and_then(|s| js_parse_int(&s))
}

async fn save_ids(st: &AppState, ids: &[i64]) -> anyhow::Result<()> {
    st.settings.set_nsfp_ids(ids);
    crate::settings::save(st, "nsfp", &crate::util::json_string(&json!(ids))).await
}

async fn add(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(tag_id) = tag_id_of(&body).filter(|n| *n > 0 && *n <= i32::MAX as i64) else {
        return j(json!({ "success": false, "msg": "A valid tag_id is required" }), 400);
    };
    let r: anyhow::Result<Res> = async {
        let Some(tag) = sql!("SELECT id, tag FROM tags WHERE id = ", tag_id as i32, " LIMIT 1").fetch_optional(&st.db).await? else {
            return Ok(j(json!({ "success": false, "msg": format!("Tag with id {tag_id} does not exist") }), 404));
        };
        let current = st.settings.nsfp_ids();
        if current.contains(&tag_id) {
            return Ok(j(json!({ "success": false, "msg": format!("Tag #{tag_id} ({}) is already in the NSFP list", tag.string("tag")) }), 409));
        }
        let mut updated = current;
        updated.push(tag_id);
        save_ids(&st, &updated).await?;
        let uid = req.session().map(|s| s.id).unwrap_or(0);
        audit_log(&st, uid, "nsfp_add", "tag", &tag_id.to_string(), Some(json!({ "tag": tag.val("tag"), "nsfp": updated }))).await;
        Ok(j(json!({ "success": true, "nsfp_ids": st.settings.nsfp_ids(), "added": Value::Object(tag) }), 200))
    }
    .await;
    match r {
        Ok(x) => x,
        Err(e) => err500(e),
    }
}

async fn remove(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(tag_id) = tag_id_of(&body).filter(|n| *n != 0) else {
        return j(json!({ "success": false, "msg": "tag_id is required" }), 400);
    };
    let current = st.settings.nsfp_ids();
    if !current.contains(&tag_id) {
        return j(json!({ "success": false, "msg": format!("Tag #{tag_id} is not in the NSFP list") }), 404);
    }
    let updated: Vec<i64> = current.into_iter().filter(|i| *i != tag_id).collect();
    if let Err(e) = save_ids(&st, &updated).await {
        return err500(e);
    }
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    let mut d = Map::new();
    d.insert("nsfp".into(), json!(updated));
    audit_log(&st, uid, "nsfp_remove", "tag", &tag_id.to_string(), Some(Value::Object(d))).await;
    j(json!({ "success": true, "nsfp_ids": st.settings.nsfp_ids() }), 200)
}
