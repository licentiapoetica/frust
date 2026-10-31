//! Tag/title/source search page (`/search`) — port of src/inc/routes/search.mjs.

use super::toptags::{member_session, no_store};
use crate::db::Row;
use crate::http::{Guard, Req, Res, Router, html};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

const EPS: i64 = 20;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/search(\/)?$", Guard::LoggedIn, search);
}

/// Page numbers around the current one (`cheat`) plus prev/next.
fn pagination(act_page: i64, pages: i64) -> Value {
    let cheat: Vec<i64> = ((act_page - 3).max(1)..=(act_page + 3).min(pages)).collect();
    json!({
        "start": 1,
        "end": pages,
        "prev": if act_page > 1 { json!(act_page - 1) } else { Value::Null },
        "next": if act_page < pages { json!(act_page + 1) } else { Value::Null },
        "page": act_page,
        "cheat": cheat,
        "uff": false,
    })
}

fn pages_of(total: i64) -> i64 {
    (total + EPS - 1) / EPS
}

fn offset_of(act_page: i64) -> i64 {
    ((act_page - 1) * EPS).max(0)
}

async fn search(st: AppState, req: Req) -> Res {
    let tag: String = req.query("tag").unwrap_or("").to_string();
    let mode = req.query("mode").map(str::to_string);
    // `page || 1`; a non-numeric page made the old query fail, use 1 instead
    let page: i64 = match req.query("page").filter(|p| !p.is_empty()) {
        Some(p) => crate::social::helpers::js_number(p).filter(|f| f.is_finite() && *f != 0.0).map(|f| f.trunc() as i64).unwrap_or(1),
        None => 1,
    };
    let mut total: i64 = 0;
    let mut result: Value = Value::Null;
    let mut pag: Value = Value::Null;
    let mut link: Value = Value::Null;

    if !tag.is_empty() {
        if let Some(s) = req.session().filter(|s| s.id != 0) {
            if !tag.starts_with("src:") && !tag.starts_with("title:") {
                crate::f0cklib::recommend::spawn_update_user_tag_affinity(&st, s.id, &tag, 2.0);
            }
        }
        if let Some(src) = tag.strip_prefix("src:") {
            let pattern = format!("%{src}%");
            total = sql!(r#"select count(*) from "items" where src ilike "#, pattern.as_str(), " and active = true")
                .fetch_scalar::<i64, _>(&st.db)
                .await?
                .unwrap_or(0);
            let pages = pages_of(total);
            let act = pages.min(page);
            let rows: Vec<Row> = sql!(
                r#"select * from "items" where src ilike "#, pattern.as_str(), r#" and active = true order by "items".id desc offset "#,
                offset_of(act), " limit ", EPS
            )
            .fetch_all(&st.db)
            .await?;
            result = json!(rows);
            pag = pagination(act, pages);
            link = json!({ "main": format!("/search/?tag={tag}"), "path": "&page=" });
        } else if let Some(title) = tag.strip_prefix("title:") {
            let pattern = format!("%{}%", title.trim());
            total = sql!(r#"select count(*) from "items" where title ilike "#, pattern.as_str(), " and active = true")
                .fetch_scalar::<i64, _>(&st.db)
                .await?
                .unwrap_or(0);
            let pages = pages_of(total);
            let act = pages.max(1).min(page);
            let rows: Vec<Row> = sql!(
                r#"select * from "items" where title ilike "#, pattern.as_str(), " and active = true order by id desc offset ", offset_of(act), " limit ", EPS
            )
            .fetch_all(&st.db)
            .await?;
            result = json!(rows);
            pag = pagination(act, pages);
            link = json!({ "main": format!("/search/?tag={}", crate::util::encode_uri_component(&tag)), "path": "&page=" });
        } else if mode.as_deref() == Some("strict") {
            let tags: Vec<String> = tag.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect();
            if tags.is_empty() {
                total = 0;
                result = json!([]);
            } else {
                let n = tags.len() as i64;
                total = sql!(
                    r#"select count(sub.id) from (
                      select "items".id from "items"
                      join "tags_assign" on "tags_assign".item_id = "items".id
                      join "tags" on "tags".id = "tags_assign".tag_id
                      where "tags".normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest("#,
                    tags.clone(),
                    r#"::text[]) AS x))
                        and "items".active = true
                      group by "items".id
                      having count(distinct "tags".normalized) = "#,
                    n,
                    ") sub"
                )
                .fetch_scalar::<i64, _>(&st.db)
                .await?
                .unwrap_or(0);
                let pages = pages_of(total);
                let act = pages.min(page);
                let rows: Vec<Row> = sql!(
                    r#"select "items".id, "items".username, "items".mime, min("tags".tag) as tag
                    from "items"
                    join "tags_assign" on "tags_assign".item_id = "items".id
                    join "tags" on "tags".id = "tags_assign".tag_id
                    where "tags".normalized = ANY(ARRAY(SELECT slugify(x) FROM unnest("#,
                    tags,
                    r#"::text[]) AS x))
                      and "items".active = true
                    group by "items".id
                    having count(distinct "tags".normalized) = "#,
                    n,
                    r#" order by "items".id desc offset "#,
                    offset_of(act),
                    " limit ",
                    EPS
                )
                .fetch_all(&st.db)
                .await?;
                result = Value::Array(
                    rows.into_iter()
                        .map(|mut r| {
                            r.insert("score".into(), json!(1.0));
                            Value::Object(r)
                        })
                        .collect(),
                );
                pag = pagination(act, pages);
                link = json!({ "main": format!("/search/?tag={}&mode=strict", crate::util::encode_uri_component(&tag)), "path": "&page=" });
            }
        } else {
            let pattern = format!("%{tag}%");
            total = sql!(
                r#"select count(*) from (
                  select 1 from "items"
                  join "tags_assign" on "tags_assign".item_id = "items".id
                  join "tags" on "tags".id = "tags_assign".tag_id
                  where ("tags".tag ilike "#,
                pattern.as_str(),
                r#" or "tags".normalized like '%' || slugify("#,
                tag.as_str(),
                r#") || '%')
                    and "items".active = true
                  group by "items".id
                ) sub"#
            )
            .fetch_scalar::<i64, _>(&st.db)
            .await?
            .unwrap_or(0);
            let pages = pages_of(total);
            let act = pages.min(page);
            let rows: Vec<Row> = sql!(
                r#"select "items".id, "items".username, "items".mime, min("tags".tag) as tag
                from "items"
                join "tags_assign" on "tags_assign".item_id = "items".id
                join "tags" on "tags".id = "tags_assign".tag_id
                where ("tags".tag ilike "#,
                pattern.as_str(),
                r#" or "tags".normalized like '%' || slugify("#,
                tag.as_str(),
                r#") || '%')
                  and "items".active = true
                group by "items".id
                order by "items".id desc
                offset "#,
                offset_of(act),
                " limit ",
                EPS
            )
            .fetch_all(&st.db)
            .await?;
            let rows = json!(rows);
            result = crate::f0cklib::search::search(&rows, &tag).map(Value::Array).unwrap_or(Value::Bool(false));
            pag = pagination(act, pages);
            link = json!({ "main": format!("/search/?tag={}", crate::util::encode_uri_component(&tag)), "path": "&page=" });
        }
    }

    let totals = super::ranking::count_f0cks(&st).await?;
    let mut data = json!({
        "result": result,
        "totals": totals,
        "searchstring": tag,
        "count": total,
        "session": member_session(&req),
        "mode": mode,
        // The template's strict checkbox; `mode` itself is replaced by the feed mode when rendering
        "search_mode": mode,
    });
    if result.is_null() {
        if let Some(m) = data.as_object_mut() {
            m.shift_remove("result");
        }
    }
    // `pagination`/`link` stay undefined when there was no search
    if !pag.is_null() {
        data["pagination"] = pag;
    }
    if !link.is_null() {
        data["link"] = link;
    }
    // Without a search term the old page received `searchstring: []`
    if tag.is_empty() {
        data["searchstring"] = json!([]);
    }
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), "search", data))))
}
