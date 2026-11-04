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
    r.get(r"^\/user\/(?<user>[^\/]+)\/tags(?:\/p\/(?<page>\d+))?\/?$", user_tags_page);
}

/// Page from `/p/N` (what the infinite scroll writes into the address bar) or `?page=N`.
fn tags_page_param(req: &Req) -> i64 {
    match req.param("page").and_then(|p| p.parse::<i64>().ok()) {
        Some(p) => p.clamp(1, 1_000_000_000),
        None => page_param(req),
    }
}

/// Pagination + link in the shape the pagination snippets of every frontend expect. The total is
/// not counted, so the last page known is the next one.
fn tags_pagination(user_name: &str, page: i64, has_more: bool) -> (Value, Value) {
    let prev = (page > 1).then_some(page - 1);
    let next = has_more.then_some(page + 1);
    let cheat: Vec<i64> = prev.into_iter().chain([page]).chain(next).collect();
    let pagination = json!({
        "start": 1,
        "end": next.unwrap_or(page),
        "page": page,
        "current": page,
        "prev": prev,
        "next": next,
        "cheat": cheat,
    });
    let link = json!({
        "main": format!("/user/{}/tags/", crate::util::encode_uri_component(user_name)),
        "path": "p/",
        "suffix": "",
    });
    (pagination, link)
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
        let (pagination, link) = tags_pagination(&user_name, page, has_more);
        let pagination = crate::tpl::render(&st, Some(&req), "snippets/pagination", json!({ "pagination": pagination, "link": link }));
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
    let page = tags_page_param(&req);
    let offset = (page - 1) * TAGS_PER_PAGE;
    let toptags = process_tags(tags_query(&st, Some(uid), req.mode, offset, TAGS_PER_PAGE, req.session(), strict_param(&req)).await?);
    let n = toptags.len() as i64;
    let (pagination, link) = tags_pagination(&user_name, page, n == TAGS_PER_PAGE);
    let enc = crate::util::encode_uri_component(&user_name);
    let data = json!({
        "user": Value::Object(user_row),
        "tagger": user_name,
        "toptags": toptags,
        "tmp": null,
        "hidePagination": false,
        "pagination": pagination,
        "link": link,
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

#[cfg(test)]
mod tests {
    use super::tags_pagination;
    use crate::i18n::I18n;
    use std::{path::PathBuf, sync::Arc};

    /// Every frontend's pagination partial renders the user-tags pagination (w0bm used to fail on
    /// the missing `start`/`end`) and links to `/user/<name>/tags/p/N`.
    #[test]
    fn pagination_renders_in_every_frontend() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = crate::tpl::build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        for (page, has_more) in [(1, false), (1, true), (2, true), (5, false)] {
            let (pagination, link) = tags_pagination("w0bm", page, has_more);
            let ctx = crate::tpl::to_jvalue(&serde_json::json!({ "pagination": pagination, "link": link }));
            for tpl in ["snippets/pagination.html", "frontends/w0bm/partials/pagination.html", "frontends/f0ckv2/snippets/pagination.html"] {
                let html = env.get_template(tpl).unwrap().render(&ctx).unwrap_or_else(|e| panic!("{tpl} page {page}: {e:#}"));
                if has_more {
                    assert!(html.contains(&format!("href=\"/user/w0bm/tags/p/{}\"", page + 1)), "{tpl} page {page}: {html}");
                }
                if page > 1 {
                    assert!(html.contains(&format!("href=\"/user/w0bm/tags/p/{}\"", page - 1)), "{tpl} page {page}: {html}");
                }
            }
        }
    }
}
