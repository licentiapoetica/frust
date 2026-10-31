//! Tag overview (`/tags`, `/api/tags`) — port of src/inc/routes/toptags.mjs.
//! The query helper is shared with the per-user tag pages (user_tags.rs).

use crate::db::{Row, RowExt, Sql};
use crate::http::{Req, Res, Router, html, json_resp};
use crate::session::Session;
use crate::settings as s;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};

pub const TAGS_PER_PAGE: i64 = 50;

pub fn register(r: &mut Router) {
    r.get(r"^\/api\/tags$", api_tags);
    r.get(r"^\/tags?\/?$", tags_page);
}

/// `{ ...req.session }` for regular sessions (with a user name), else `false`.
pub fn member_session(req: &Req) -> Value {
    req.session().filter(|s| !s.user.is_empty()).map(Session::to_value).unwrap_or(Value::Bool(false))
}

pub fn no_store(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert(header::EXPIRES, HeaderValue::from_static("0"));
    h.insert("surrogate-control", HeaderValue::from_static("no-store"));
    r
}

/// `+(query.page ?? 1)` clamped to ≥ 1 (NaN → 1 would be `Math.max(1, NaN)` = NaN in JS, which
/// makes the query fail; use page 1 instead).
pub fn page_param(req: &Req) -> i64 {
    match req.query("page") {
        None => 1,
        Some(p) => crate::social::helpers::js_number(p).filter(|f| f.is_finite()).map(|f| f.trunc().max(1.0).min(1.0e9) as i64).unwrap_or(1),
    }
}

/// `strict=1` in the query or the session's strict mode.
pub fn strict_param(req: &Req) -> bool {
    req.query("strict") == Some("1") || req.session().and_then(|s| s.strict_mode).unwrap_or(false)
}

/// `getTagsQuery(...)`: tags by item count (merged by normalized name), optionally only those a
/// user assigned. Outside strict mode the counts are replaced by fuzzy (substring) counts.
pub async fn tags_query(
    st: &AppState,
    user_id: Option<i32>,
    mode: i64,
    offset: i64,
    limit: i64,
    session: Option<&Session>,
    strict: bool,
) -> anyhow::Result<Vec<Value>> {
    let cfg = st.cfg();
    let excluded: Vec<i32> = session.map(|s| s.excluded_tags.iter().map(|&t| t as i32).collect()).unwrap_or_default();
    let is_guest = session.is_none();
    let is_public_visitor = session.is_none() || s::is_anon_session(session);
    let nsfl = cfg.nsfl_tag_id();
    let mut modequery = crate::util::get_mode(&cfg, mode);
    if is_guest {
        let public_untagged = cfg.truthy("websrv.public_untagged");
        if mode == 3 || mode == 0 {
            modequery = match (cfg.truthy("websrv.public_nsfw"), public_untagged) {
                (true, true) => format!(
                    "(items.id in (select item_id from tags_assign where tag_id in (1, 2)) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))"
                ),
                (true, false) => "items.id in (select item_id from tags_assign where tag_id in (1, 2))".into(),
                (false, true) => format!(
                    "(items.id in (select item_id from tags_assign where tag_id = 1) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))"
                ),
                (false, false) => "items.id in (select item_id from tags_assign where tag_id = 1)".into(),
            };
        } else if !public_untagged && mode == 2 {
            modequery = "1 = 0".into();
        }
    }

    let mut restricted = Sql::empty();
    let nsfp = st.settings.nsfp_ids();
    if is_public_visitor && !nsfp.is_empty() {
        let mut eff: Vec<i64> = nsfp;
        if session.is_some() && s::is_anon_session(session) {
            let allowed = s::anon_allowed_modes(&cfg);
            if allowed.iter().any(|m| m == "nsfw") {
                eff.retain(|&id| id != 2);
            }
            if allowed.iter().any(|m| m == "nsfl") {
                eff.retain(|&id| id != nsfl);
            }
        } else if cfg.truthy("websrv.public_nsfw") {
            eff.retain(|&id| id != 2);
        }
        if !eff.is_empty() {
            let eff: Vec<i32> = eff.into_iter().map(|i| i as i32).collect();
            restricted.push(" AND t.id <> ALL(").bind(eff.clone()).push(") AND NOT EXISTS (SELECT 1 FROM tags_assign ta_res WHERE ta_res.item_id = items.id AND ta_res.tag_id = ANY(").bind(eff).push(")) ");
        }
    }
    let mut user_exclude = Sql::empty();
    if !excluded.is_empty() {
        user_exclude.push(" AND NOT EXISTS (SELECT 1 FROM tags_assign ta_ex WHERE ta_ex.item_id = items.id AND ta_ex.tag_id = ANY(").bind(excluded).push("::int[])) ");
    }
    let user_filter = |q: &mut Sql| {
        if let Some(uid) = user_id {
            q.push(" AND ta.user_id = ").bind(uid).push(" ");
        }
    };

    let mut q = Sql::new(
        "SELECT MIN(t.id) as id, MIN(t.tag) as tag, t.normalized, COUNT(DISTINCT items.id) AS total_items
      FROM tags t
      JOIN tags_assign ta ON t.id = ta.tag_id
      JOIN items ON items.id = ta.item_id
      WHERE items.active = true
        AND items.is_deleted = false
        AND COALESCE(items.visibility, 0) = 0
        AND t.id NOT IN (1, 2) ",
    );
    user_filter(&mut q);
    // modequery is built from integers only (getMode / config flags)
    q.push(" AND ").push(&modequery).push(" ");
    q.append(&restricted).append(&user_exclude);
    q.push(" GROUP BY t.normalized HAVING COUNT(DISTINCT items.id) >= 1 ORDER BY total_items DESC, MIN(t.id) DESC OFFSET ")
        .bind(offset)
        .push(" LIMIT ")
        .bind(limit);
    let mut rows: Vec<Row> = q.fetch_all(&st.db).await?;

    if !strict && !rows.is_empty() {
        let futs = rows.iter().map(|tag| {
            let normalized = tag.str("normalized").filter(|n| !n.is_empty()).map(str::to_string);
            let mut q = Sql::empty();
            if let Some(n) = &normalized {
                q.push(
                    "SELECT COUNT(DISTINCT items.id) as total
          FROM tags t
          JOIN tags_assign ta ON t.id = ta.tag_id
          JOIN items ON items.id = ta.item_id
          WHERE t.normalized LIKE '%' || ",
                )
                .bind(n.as_str())
                .push(
                    " || '%'
            AND items.active = true
            AND items.is_deleted = false
            AND COALESCE(items.visibility, 0) = 0 ",
                );
                user_filter(&mut q);
                q.push(" AND ").push(&modequery).push(" ");
                q.append(&restricted).append(&user_exclude);
            }
            async move {
                if normalized.is_none() {
                    return Ok(None);
                }
                q.fetch_scalar::<i64, _>(&st.db).await.map(|n| Some(n.unwrap_or(0)))
            }
        });
        let counts = futures::future::try_join_all(futs).await?;
        for (row, c) in rows.iter_mut().zip(counts) {
            if let Some(c) = c {
                row.insert("total_items".into(), json!(c));
            }
        }
    }
    Ok(rows.into_iter().map(Value::Object).collect())
}

/// `processTags`: link-safe names.
pub fn process_tags(tags: Vec<Value>) -> Vec<Value> {
    tags.into_iter()
        .map(|mut t| {
            let tag = crate::tpl::js_to_string(&minijinja::Value::from_serialize(&t["tag"]));
            let enc = crate::util::encode_uri_component(&tag);
            let safe = match t["normalized"].as_str().filter(|n| !n.is_empty()) {
                Some(n) => n.to_string(),
                None => enc.clone(),
            };
            t["safe_tag"] = json!(safe);
            t["encoded_tag"] = json!(enc);
            t
        })
        .collect()
}

async fn api_tags(st: AppState, req: Req) -> Res {
    let page = page_param(&req);
    let offset = (page - 1) * TAGS_PER_PAGE;
    let tags = process_tags(tags_query(&st, None, req.mode, offset, TAGS_PER_PAGE, req.session(), strict_param(&req)).await?);
    let has_more = tags.len() as i64 == TAGS_PER_PAGE;
    if req.is_xhr() || req.query("ajax").map(|v| !v.is_empty()).unwrap_or(false) {
        let html = crate::tpl::render(&st, Some(&req), "tag-cards", json!({ "toptags": tags, "session": member_session(&req) }));
        return Ok(json_resp(&json!({ "success": true, "html": html, "currentPage": page, "hasMore": has_more }), 200));
    }
    Ok(json_resp(&json!({ "success": true, "tags": tags, "currentPage": page, "hasMore": has_more }), 200))
}

async fn tags_page(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let phrase = match cfg.get("websrv.phrases").as_array().filter(|a| !a.is_empty()) {
        Some(a) => {
            use rand::Rng;
            a[rand::thread_rng().gen_range(0..a.len())].clone()
        }
        None => Value::Null,
    };
    let toptags = process_tags(tags_query(&st, None, req.mode, 0, TAGS_PER_PAGE, req.session(), strict_param(&req)).await?);
    let n = toptags.len();
    let data = json!({
        "toptags": toptags,
        "phrase": phrase,
        "tmp": null,
        "hidePagination": true,
        "session": member_session(&req),
        "page_meta": {
            "title": "Tags",
            "description": format!("Browse {n}+ tags"),
            "url": format!("https://{}/tags", cfg.str_or("main.url.domain", "")),
        }
    });
    let view = if req.is_xhr() { "tags-partial" } else { "tags" };
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), view, data))))
}
