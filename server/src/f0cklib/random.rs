//! `getRandom` and `getRandomPool`: the same candidate filters, one random pick vs. the whole pool.

use super::*;
use crate::util::{LinkEnv, gen_link};

struct Prepared {
    user: Option<String>,
    tag: Option<String>,
    hall: Option<String>,
    query: Sql,
}

/// Build the candidate query. `pool`: select `id, slug` ordered by id (getRandomPool) instead of
/// one random `id` (getRandom).
async fn prepare(st: &AppState, cfg: &Config, o: &FeedOpts<'_>, pool: bool) -> anyhow::Result<Prepared> {
    let session = o.session;
    let user = o.user.as_deref().filter(|u| !u.is_empty()).map(crate::http::decode_uri);
    let user_like = user.as_deref().map(escape_like);
    let hall = o.hall.clone().filter(|h| !h.is_empty());
    let (_, is_title_search, title_query, tag) = split_title_tag(o.tag.as_deref());
    let mime = o.mime.clone().unwrap_or_default();

    let mut user_hall_id: Option<i64> = None;
    if let (Some(slug), Some(owner)) = (o.user_hall.as_deref().filter(|s| !s.is_empty()), o.user_hall_owner.as_deref().filter(|s| !s.is_empty())) {
        user_hall_id = user_hall_row(st, owner, slug).await?.and_then(|r| r.i64("id")).filter(|&i| i != 0);
    }

    let (mime_parts, mime_sql) = resolve_mime_sql(cfg, Some(&mime), session, "items");
    let excluded: Vec<i64> = if session.is_some() { o.exclude.clone() } else { vec![] };
    let sp = strict_params(o.strict, tag.as_deref());
    let is_strict = !sp.is_empty();
    let multi = o.ratings.as_ref().filter(|r| !r.is_empty()).and_then(|r| get_multi_rating_mode(cfg, r));
    let modequery = compute_base_mode(cfg, o.mode, o.ratings.as_ref(), session);
    let gf = global_filter_sql(st, cfg, session);
    let ex = exclude_sql(&excluded);

    let cols = "items.id, items.slug";
    let tail = if pool { " ORDER BY items.id" } else { " ORDER BY random() LIMIT 1" };
    let vis = " AND items.active = true AND coalesce(items.visibility, 0) = 0 ";

    let mut q = Sql::empty();
    if is_title_search && title_query.as_deref().map(|t| !t.is_empty()).unwrap_or(false) {
        q.push(&format!("SELECT {cols} FROM items WHERE ")).push(&modequery).push(vis);
        q.push("AND items.title ILIKE ").bind(format!("%{}%", title_query.unwrap_or_default())).push(" AND items.title IS NOT NULL ");
        q.append(&mime_sql).push(" ").append(&gf).push(" ").append(&ex).push(tail);
    } else if o.fav && user.is_some() {
        if pool {
            q.push(r#"SELECT DISTINCT items.id, items.slug FROM favorites INNER JOIN items ON favorites.item_id = items.id INNER JOIN "user" ON "user".id = favorites.user_id WHERE "#);
        } else {
            q.push(r#"select items.id from favorites inner join items on favorites.item_id = items.id inner join "user" on "user".id = favorites.user_id left join tags_assign on tags_assign.item_id = items.id left join tags on tags.id = tags_assign.tag_id where "#);
        }
        q.push(&modequery).push(r#" and "user".user ilike "#).bind(user_like.clone()).push(vis);
        q.append(&mime_sql).push(" ").append(&gf);
        q.push(if pool { " ORDER BY items.id" } else { " group by items.id order by random() limit 1" });
    } else if user.is_some() || tag.as_deref().map(|t| !t.is_empty()).unwrap_or(false) {
        let tag_filter = match tag.as_deref().filter(|t| !t.is_empty()) {
            Some(t) => tag_filter_sql(t, is_strict, None, false),
            None => Sql::empty(),
        };
        q.push(&format!("SELECT {cols} FROM items WHERE ")).push(&modequery).push(vis);
        q.append(&tag_filter).push(" ");
        if let Some(like) = &user_like {
            q.push("and items.username ilike ").bind(like.as_str()).push(" ");
        }
        if let Some(h) = &hall {
            q.push("and items.id in (select item_id from halls_assign ha join halls h on h.id = ha.hall_id where h.slug = ").bind(h.as_str()).push(") ");
        }
        q.append(&mime_sql).push(" ").append(&gf).push(" ").append(&ex).push(tail);
    } else if let Some(h) = &hall {
        q.push(&format!("SELECT {cols} FROM items JOIN halls_assign ha ON ha.item_id = items.id JOIN halls h ON h.id = ha.hall_id WHERE "));
        q.push(&modequery).push(" AND h.slug = ").bind(h.as_str()).push(vis);
        q.append(&mime_sql).push(" ").append(&gf).push(" ").append(&ex).push(tail);
    } else if let Some(uh) = user_hall_id {
        q.push(&format!("SELECT {cols} FROM items JOIN user_halls_assign uha ON uha.item_id = items.id WHERE "));
        q.push(&modequery).push(" AND uha.hall_id = ").bind(uh as i32).push(vis);
        q.append(&mime_sql).push(" ").append(&ex).push(tail);
    } else {
        // Global: join on the rating tag for plain single modes of members.
        let nsfl_cfg = {
            let v = cfg.get("nsfl_tag_id");
            if crate::config::truthy(v) { crate::config::as_int(v).unwrap_or(3) } else { 3 }
        };
        let tag_id = match o.mode {
            Some(m @ (0 | 1 | 4)) if session.is_some() && !is_anon_session(session) && multi.is_none() => {
                Some(if m == 4 { nsfl_cfg } else if m == 1 { 2 } else { 1 })
            }
            _ => None,
        };
        let use_tag_id = tag_id.filter(|_| !mime_parts.iter().any(|m| m == "audio"));
        let nsfp = st.settings.nsfp_ids();
        // nsfp only for SFW/All — an explicit NSFW/NSFL choice must not exclude those items
        let check_filter = is_anon_session(session) && !nsfp.is_empty() && matches!(o.mode, None | Some(0) | Some(3));
        q.push(&format!("SELECT {cols} FROM items "));
        if let Some(t) = use_tag_id {
            q.push("INNER JOIN tags_assign ta ON ta.item_id = items.id AND ta.tag_id = ").bind(t).push(" ");
        }
        if check_filter {
            q.push("LEFT JOIN tags_assign filter_ta ON filter_ta.item_id = items.id AND filter_ta.tag_id IN ").bind_list(&nsfp).push(" ");
        }
        q.push("WHERE items.active = true AND coalesce(items.visibility, 0) = 0 ");
        q.append(&mime_sql).push(" ");
        if check_filter {
            q.push("AND filter_ta.tag_id IS NULL ");
        }
        q.append(&ex).push(" ");
        if use_tag_id.is_none() {
            q.push("AND ").push(&modequery);
        }
        // The capped global pool is a random sample: ordered by id it only ever held the oldest 10000
        // items, and random never reached anything newer. (The client shuffles it anyway.)
        q.push(if pool { " ORDER BY random() LIMIT 10000" } else { " ORDER BY random() LIMIT 1" });
    }
    Ok(Prepared { user, tag, hall, query: q })
}

/// `getRandom({...})` → `{ success, link, itemid, slug }`, or `null` for private favourites.
pub async fn get_random(st: &AppState, o: &FeedOpts<'_>) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    if o.fav {
        if let Some(u) = o.user.as_deref().filter(|u| !u.is_empty()) {
            let (is_private, allowed) = check_favorites_access(st, u, o).await?;
            if is_private && !allowed {
                return Ok(Value::Null);
            }
        }
    }
    let p = prepare(st, &cfg, o, false).await?;
    let Some(row) = p.query.fetch_optional(&st.db).await? else {
        return Ok(json!({ "success": false, "message": "no uploads found :(" }));
    };
    let link = gen_link(&LinkEnv {
        tag: p.tag.as_deref().filter(|t| !t.is_empty()),
        hall: p.hall.as_deref(),
        user: p.user.as_deref(),
        kind: Some(if o.fav { "favs" } else { "uploads" }),
        path: None,
        strict: false,
        tagger: None,
    });
    Ok(json!({ "success": true, "link": link, "itemid": row.val("id"), "slug": row.val("slug") }))
}

/// `getRandomPool({...})` → `{ items, id_map, total, sampled }` (ids/slugs for client-side random).
pub async fn get_random_pool(st: &AppState, o: &FeedOpts<'_>) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    if o.fav {
        if let Some(u) = o.user.as_deref().filter(|u| !u.is_empty()) {
            let (is_private, allowed) = check_favorites_access(st, u, o).await?;
            if is_private && !allowed {
                return Ok(json!({ "items": [], "total": 0 }));
            }
        }
    }
    let p = prepare(st, &cfg, o, true).await?;
    let rows = p.query.fetch_all(&st.db).await?;
    let mut items = Vec::with_capacity(rows.len());
    let mut id_map = serde_json::Map::new();
    // slugs only when they're enabled; otherwise the client would put them in the url bar
    let slugs = crate::settings::enable_item_slugs(&cfg);
    for r in &rows {
        let slug = r.str("slug").filter(|s| slugs && !s.is_empty());
        match slug {
            Some(s) => {
                items.push(json!(s));
                id_map.insert(s.to_string(), r.val("id").clone());
            }
            None => items.push(json!(r.string("id"))),
        }
    }
    let total = items.len();
    Ok(json!({ "items": items, "id_map": id_map, "total": total, "sampled": total >= 10000 }))
}
