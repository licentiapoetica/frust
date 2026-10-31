//! Comment subscriptions overview (port of src/inc/routes/subscriptions.mjs).

use crate::db::RowExt;
use crate::http::{Req, Res, Router, html, json_resp, redirect, reply};
use crate::social::helpers::js_parse_int;
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.exact(Method::GET, "/subscriptions", None, page);
    r.exact(Method::GET, "/ajax/subscriptions", None, ajax);
    r.post(r"\/api\/subscriptions\/(?<itemid>\d+)\/delete", unsubscribe);
}

struct Listing {
    items: Vec<Value>,
    pagination: Value,
    link: Value,
    total: i64,
    page: i64,
    pages: i64,
}

/// One page of the viewer's active subscriptions (`eps` per page).
async fn load(st: &AppState, user_id: i64, req: &Req) -> anyhow::Result<Listing> {
    let cfg = st.cfg();
    let page = req.query("page").and_then(js_parse_int).filter(|p| *p != 0).unwrap_or(1);
    let eps = cfg.int("websrv.eps").filter(|e| *e != 0).unwrap_or(300);
    let offset = (page - 1) * eps;
    let uid = user_id as i32;
    let total = sql!("SELECT count(*) as total FROM comment_subscriptions WHERE user_id = ", uid, " AND is_subscribed = true")
        .fetch_scalar::<i64, _>(&st.db)
        .await?
        .unwrap_or(0);
    let pages = (total + eps - 1) / eps;
    let subs = sql!(
        r#"SELECT
                    s.created_at as sub_date,
                    i.id, i.slug, i.dest, i.mime, i.username as uploader_name
                FROM comment_subscriptions s
                JOIN items i ON s.item_id = i.id
                WHERE s.user_id = "#,
        uid,
        " AND s.is_subscribed = true ORDER BY s.created_at DESC LIMIT ",
        eps,
        " OFFSET ",
        offset
    )
    .fetch_all(&st.db)
    .await?;
    let items = subs
        .iter()
        .map(|s| {
            let created = s
                .str("sub_date")
                .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
                .map(|d| crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None))
                .unwrap_or_else(|| "Invalid Date".into());
            let slug = s.val("slug");
            json!({
                "id": s.val("id"),
                "slug": if crate::config::truthy(slug) { slug.clone() } else { Value::Null },
                "user": s.str("uploader_name").filter(|u| !u.is_empty()).unwrap_or("System"),
                "sub_created": created,
                "thumb": format!("/t/{}.webp", s.string("id")),
            })
        })
        .collect();
    let pagination = json!({
        "start": 1,
        "end": pages,
        "prev": if page > 1 { json!(page - 1) } else { Value::Null },
        "next": if page < pages { json!(page + 1) } else { Value::Null },
        "page": page,
    });
    Ok(Listing { items, pagination, link: json!({ "main": "/subscriptions", "path": "?page=" }), total, page, pages })
}

async fn page(st: AppState, req: Req) -> Res {
    let Some(uid) = req.session().map(|s| s.id) else { return Ok(redirect("/login")) };
    match load(&st, uid, &req).await {
        Ok(l) => {
            let cfg = st.cfg();
            let data = json!({
                "items": l.items,
                "pagination": l.pagination,
                "link": l.link,
                "totalCount": l.total,
                "hidePagination": true,
                "page_meta": {
                    "title": "subscriptions",
                    "description": "Your comment subscriptions",
                    "url": format!("https://{}/subscriptions", cfg.domain()),
                }
            });
            Ok(html(crate::tpl::render(&st, Some(&req), "subscriptions", data)))
        }
        Err(e) => {
            tracing::error!("[DEBUG SUB ERROR] {e:#}");
            Ok(reply(500, "text/html", "Database Error"))
        }
    }
}

async fn ajax(st: AppState, req: Req) -> Res {
    let Some(uid) = req.session().map(|s| s.id) else { return Ok(json_resp(&json!({ "success": false }), 401)) };
    match load(&st, uid, &req).await {
        Ok(l) => {
            let grid = crate::tpl::render(&st, Some(&req), "snippets/subscriptions-grid", json!({ "items": l.items }));
            let pag = crate::tpl::render(&st, Some(&req), "snippets/pagination", json!({ "pagination": l.pagination, "link": l.link }));
            Ok(json_resp(&json!({ "success": true, "html": grid, "pagination": pag, "hasMore": l.page < l.pages, "currentPage": l.page }), 200))
        }
        Err(e) => {
            tracing::error!("[DEBUG AJAX SUB ERROR] {e:#}");
            Ok(json_resp(&json!({ "success": false }), 500))
        }
    }
}

async fn unsubscribe(st: AppState, req: Req) -> Res {
    let Some(uid) = req.session().map(|s| s.id) else { return Ok(json_resp(&json!({ "success": false }), 401)) };
    let Some(item_id) = req.param("itemid").and_then(|i| i.parse::<i32>().ok()) else { return Ok(json_resp(&json!({ "success": false }), 500)) };
    match sql!("UPDATE comment_subscriptions SET is_subscribed = false WHERE user_id = ", uid as i32, " AND item_id = ", item_id).execute(&st.db).await {
        Ok(_) => Ok(json_resp(&json!({ "success": true }), 200)),
        Err(e) => {
            tracing::error!("{e}");
            Ok(json_resp(&json!({ "success": false }), 500))
        }
    }
}
