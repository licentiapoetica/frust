//! Tags a user assigned (`/user/:user/tags`, `/api/user/:user/tags`) — port of
//! src/inc/routes/user_tags.mjs.

use super::toptags::{TAGS_PER_PAGE, member_session, no_store, page_param, process_tags, strict_param, tags_query};
use crate::db::RowExt;
use crate::http::{Req, Res, Router, html, redirect, reply};
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/api\/user\/(?<user>[^\/]+)\/tags$", api_user_tags);
    r.get(r"^\/user\/(?<user>[^\/]+)\/tags$", user_tags_page);
}

fn user_param(req: &Req) -> String {
    crate::util::decode_lossy(req.param("user").unwrap_or(""))
}

/// `"user".user ILIKE $1` with the LIKE wildcards escaped: the old code matched the raw name as a
/// pattern, so `/user/%/tags` resolved to an arbitrary user.
fn user_like(name: &str) -> String {
    crate::util::escape_like(name)
}

async fn api_user_tags(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if s::is_anonymize_session(&cfg, req.session()) {
        return Ok(reply(403, "text/html", json!({ "success": false, "msg": "Access denied" }).to_string()));
    }
    let name = user_param(&req);
    let row = sql!(r#"SELECT "user".id, "user".user FROM "user" WHERE "user".user ILIKE "#, user_like(&name), " LIMIT 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(u) = row else {
        return Ok(reply(404, "text/html", json!({ "success": false, "msg": "User not found" }).to_string()));
    };
    let uid = u.i64("id").unwrap_or(0) as i32;
    let user_name = u.string("user");

    let page = page_param(&req);
    let offset = (page - 1) * TAGS_PER_PAGE;
    let tags = process_tags(tags_query(&st, Some(uid), req.mode, offset, TAGS_PER_PAGE, req.session(), strict_param(&req)).await?);
    let has_more = tags.len() as i64 == TAGS_PER_PAGE;
    if req.is_xhr() || req.query("ajax").map(|v| !v.is_empty()).unwrap_or(false) {
        let link = format!("/user/{}/tags", crate::util::encode_uri_component(&user_name));
        let pagination = crate::tpl::render(
            &st,
            Some(&req),
            "snippets/pagination",
            json!({
                "pagination": {
                    "page": page,
                    "prev": if page > 1 { json!(page - 1) } else { Value::Null },
                    "next": if has_more { json!(page + 1) } else { Value::Null },
                    "cheat": [1],
                },
                "link": link,
            }),
        );
        let html = crate::tpl::render(
            &st,
            Some(&req),
            "tag-cards",
            json!({ "toptags": tags, "tagger": user_name, "user": { "user": user_name }, "session": member_session(&req) }),
        );
        return Ok(crate::http::json_resp(
            &json!({ "success": true, "html": html, "currentPage": page, "pagination": pagination, "hasMore": has_more }),
            200,
        ));
    }
    Ok(crate::http::json_resp(&json!({ "success": true, "tags": tags, "currentPage": page, "hasMore": has_more }), 200))
}

async fn user_tags_page(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if s::is_anonymize_session(&cfg, req.session()) {
        return Ok(redirect(if req.session().is_some() { "/" } else { "/login" }));
    }
    let name = user_param(&req);
    let row = sql!(
        r#"SELECT "user".id, "user".user, user_options.avatar, user_options.avatar_file, user_options.username_color
        FROM "user"
        LEFT JOIN user_options ON "user".id = user_options.user_id
        WHERE "user".user ILIKE "#,
        user_like(&name),
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(user_row) = row else {
        return Ok(reply(404, "text/html", "User not found"));
    };
    let uid = user_row.i64("id").unwrap_or(0) as i32;
    let user_name = user_row.string("user");
    let toptags = process_tags(tags_query(&st, Some(uid), req.mode, 0, TAGS_PER_PAGE, req.session(), strict_param(&req)).await?);
    let n = toptags.len() as i64;
    let enc = crate::util::encode_uri_component(&user_name);
    let data = json!({
        "user": Value::Object(user_row),
        "tagger": user_name,
        "toptags": toptags,
        "tmp": null,
        "hidePagination": false,
        "pagination": {
            "page": 1,
            "prev": null,
            "next": if n == TAGS_PER_PAGE { json!(2) } else { Value::Null },
            "cheat": [1],
        },
        "link": format!("/user/{enc}/tags"),
        "session": member_session(&req),
        "page_meta": {
            "title": format!("{user_name}'s Tags"),
            "description": format!("Browse {n}+ tags made by {user_name}"),
            "url": format!("https://{}/user/{enc}/tags", cfg.str_or("main.url.domain", "")),
        }
    });
    let view = if req.is_xhr() { "tags_user-partial" } else { "tags_user" };
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), view, data))))
}
