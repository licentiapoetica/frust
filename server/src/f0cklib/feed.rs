//! `getItemPage` and `getf0cks` (feed grids).

use super::*;
use crate::util::{LinkEnv, encode_uri_component, gen_link};
use std::collections::HashMap;

/// The WHERE conditions shared by getItemPage and getf0cks (after `modequery`).
fn feed_conditions(st: &AppState, cfg: &Config, f: &Filters, o: &FeedOpts<'_>, fav_join: bool, ids_filter: &Sql) -> Sql {
    let mut q = Sql::new("");
    q.push(&f.modequery).push("\n        and items.active = true\n        ");
    q.append(&f.visibility_filter).push(" ");
    q.append(&f.tag_filter).push(" ");
    q.append(&f.title_filter).push(" ");
    q.append(ids_filter).push(" ");
    if fav_join {
        let like = f.user_like.clone();
        q.append(&sql!("and (fav_u.user ilike ", like.clone(), " or fav_u.login ilike ", like, ")")).push(" ");
    }
    if !o.fav {
        if let Some(like) = &f.user_like {
            q.append(&sql!("and items.username ilike ", like.as_str())).push(" ");
        }
    }
    q.append(&f.mime_sql).push(" ");
    q.append(&f.hall_filter).push(" ");
    q.append(&f.user_hall_filter).push(" ");
    q.append(&global_filter_sql(st, cfg, o.session)).push(" ");
    q.append(&exclude_sql(&f.excluded_tags)).push(" ");
    if let Some(n) = f.newer_than {
        q.append(&sql!("and items.id > ", n)).push(" ");
    }

    q
}

const FAV_JOIN: &str = r#"inner join favorites on favorites.item_id = items.id inner join "user" fav_u on fav_u.id = favorites.user_id"#;

/// `getItemPage({...})` — the feed page (1-based) that contains `target_item_id`.
pub async fn get_item_page(st: &AppState, o: &FeedOpts<'_>) -> anyhow::Result<i64> {
    let cfg = st.cfg();
    let Some(num_id) = resolve_numeric_item_id(st, o.target_item_id.as_deref().unwrap_or("")).await else {
        return Ok(1);
    };
    let is_pinned = match o.target_item_pinned {
        Some(p) => p,
        None => {
            let row = sql!("SELECT is_pinned FROM items WHERE id = ", num_id, " LIMIT 1").fetch_optional(&st.db).await?;
            match row {
                Some(r) => r.bool("is_pinned"),
                None => return Ok(1),
            }
        }
    };
    let eps = o.limit.unwrap_or_else(|| cfg.int("websrv.eps").unwrap_or(0));
    let f = build_feed_filters(st, &cfg, o).await?;
    let join = if o.fav { Sql::new(FAV_JOIN) } else { Sql::empty() };
    let cond = feed_conditions(st, &cfg, &f, o, o.fav, &Sql::empty());

    let item_match = sql!("select 1 from items ", @join, " where items.id = ", num_id, " and ", @cond, " limit 1")
        .fetch_optional(&st.db)
        .await?;
    if item_match.is_none() {
        return Ok(1);
    }
    let total_before: Option<i64> = sql!(
        "select count(distinct items.id) as total_before from items ", @join, " where ", @cond,
        " and ((items.is_pinned is true and ", is_pinned, " is false) or (items.is_pinned = ", is_pinned, " and items.id > ", num_id, "))"
    )
    .fetch_scalar(&st.db)
    .await?;
    let total_before = total_before.unwrap_or(0);
    if eps <= 0 {
        // JS: Math.floor(x / 0) + 1 → Infinity / NaN; there is always an eps in practice
        return Ok(1);
    }
    Ok(total_before / eps + 1)
}

fn not_found() -> Value {
    json!({ "success": false, "message": "404 - no uploads found" })
}

/// `getf0cks({...})` — one page of a feed grid.
pub async fn getf0cks(st: &AppState, o: &FeedOpts<'_>) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    if o.fav {
        if let Some(u) = o.user.as_deref().filter(|u| !u.is_empty()) {
            let (is_private, allowed) = check_favorites_access(st, u, o).await?;
            if is_private && !allowed {
                return Ok(json!({ "success": false, "is_private": true, "message": "private favorites" }));
            }
        }
    }
    let clean_ids: Option<Vec<i32>> = o.ids.as_ref().map(|ids| ids.iter().filter(|&&n| n > 0).map(|&n| n as i32).collect());
    if let Some(ids) = &clean_ids {
        if ids.is_empty() {
            return Ok(json!({ "success": false, "message": "404 - no uploads found", "items": [], "total": 0 }));
        }
    }
    let ids_filter = match &clean_ids {
        Some(ids) => sql!("and items.id = ANY(", ids.clone(), "::int[])"),
        None => Sql::empty(),
    };

    let f = build_feed_filters(st, &cfg, o).await?;
    let eff_mode = o.mode.unwrap_or(0);
    let nsfl = nsfl_id(&cfg);
    let safe = safe_ratings(&cfg, o.ratings.as_ref(), o.session, true);

    let act_page_raw = o.page.unwrap_or(1);
    let eps = o.limit.unwrap_or_else(|| cfg.int("websrv.eps").unwrap_or(0)).max(1);
    let view_mode = if o.fav { "favs" } else { "uploads" };
    let user_hall_val = match (&f.user_hall_obj, &f.user_hall_slug) {
        (Some(u), _) => Value::Object(u.clone()),
        (None, Some(s)) => json!(s),
        _ => Value::Null,
    };
    let mut tmp = json!({
        "user": f.user,
        "tag": if f.is_title_search { json!(f.decoded_tag) } else { json!(f.tag) },
        "hall": f.hall_obj.clone().unwrap_or_else(|| json!(f.hall)),
        "mime": f.mime,
        "page": act_page_raw,
        "view_mode": view_mode,
        "strict": o.strict,
        "userHall": user_hall_val,
        "userHallOwner": f.user_hall_owner,
        "tagger": f.tagger,
    });
    if let Some(m) = o.mode {
        tmp["mode"] = json!(m);
    }

    let fav_user = o.fav && f.user.is_some();
    let join = if fav_user { Sql::new(FAV_JOIN) } else { Sql::empty() };
    let cond = feed_conditions(st, &cfg, &f, o, fav_user, &ids_filter);

    let cache_key = count_cache_key(&f, o.fav, o.session);
    let mut total = o.total.or_else(|| st.caches.feed_count.get(&cache_key));
    if total.is_none() {
        let t: i64 = sql!("select count(distinct items.id) as total from items ", @join, " where ", @cond)
            .fetch_scalar(&st.db)
            .await?
            .unwrap_or(0);
        if t > 0 && clean_ids.is_none() {
            st.caches.feed_count.set(&cache_key, t);
        }
        total = Some(t);
    }
    let total = total.unwrap_or(0);
    if total == 0 {
        return Ok(not_found());
    }

    let pages = (total + eps - 1) / eps;
    let act_page = act_page_raw.min(pages);
    let offset = ((act_page - 1) * eps).max(0);

    // Deferred-join pagination: page ids first on the bare items table, then enrich those.
    let order = if o.random {
        Sql::new("random()")
    } else if o.fav && f.user.is_none() && clean_ids.as_ref().map(|i| !i.is_empty()).unwrap_or(false) {
        sql!("array_position(", clean_ids.clone().unwrap_or_default(), "::int[], items.id)")
    } else {
        Sql::new("items.is_pinned desc, items.id desc")
    };
    let group = if fav_user { Sql::new(" group by items.id, items.is_pinned") } else { Sql::empty() };
    let page_rows = sql!(
        "select items.id, items.is_pinned from items ", @join, " where ", @cond, @group,
        " order by ", @order, " offset ", if f.newer_than.is_some() { 0 } else { offset }, " limit ", eps
    )
    .fetch_all(&st.db)
    .await?;
    if page_rows.is_empty() {
        return Ok(not_found());
    }
    let page_ids: Vec<i32> = page_rows.iter().filter_map(|r| r.i64("id")).map(|n| n as i32).collect();
    let page_order: HashMap<i64, usize> = page_ids.iter().enumerate().map(|(i, &id)| (id as i64, i)).collect();

    let uid = o.user_id();
    let mut q = Sql::new(
        "select items.id, items.title, items.slug, items.stamp, items.visibility, items.mime, items.dest, items.username as username, \
         items.is_pinned, items.is_oc, items.has_coverart, items.is_album, items.album_count, ",
    );
    match uid {
        Some(u) => {
            q.push("max(coalesce(uvv.view_count, 0)) as my_views, EXISTS (SELECT 1 FROM notifications WHERE user_id = ")
                .bind(u)
                .push(" AND item_id = items.id AND is_read = false) as has_notification, ");
        }
        None => {
            q.push("false as has_notification, ");
        }
    }
    let nsfl_cfg = {
        let v = cfg.get("nsfl_tag_id");
        if crate::config::truthy(v) { crate::config::as_int(v).unwrap_or(3) } else { 3 }
    };
    q.push("(case when min(ta.tag_id) = 1 then 'SFW' when min(ta.tag_id) = 2 then 'NSFW' when min(ta.tag_id) = ")
        .bind(nsfl_cfg)
        .push(" then 'NSFL' else null end) as tag, min(ta.tag_id) as tag_id, max(uo.display_name) as display_name, max(uo.username_color) as username_color, ");
    if cfg.truthy("websrv.enable_dynamic_thumbs") {
        q.push("((SELECT count(*) FROM favorites WHERE item_id = items.id) + (SELECT count(*) FROM comments WHERE item_id = items.id AND is_deleted = false)) as contribution ");
    } else {
        q.push("0 as contribution ");
    }
    q.push(
        r#"from items
      left join "user" author_u on author_u."user" = items.username or author_u.login = items.username
      left join user_options uo on uo.user_id = author_u.id
      left join tags_assign ta on ta.item_id = items.id and (ta.tag_id = 1 or ta.tag_id = 2 "#,
    );
    if cfg.enable_nsfl() {
        q.push("or ta.tag_id = ").bind(nsfl_cfg);
    }
    q.push(") ");
    if let Some(u) = uid {
        q.push("left join user_video_views uvv on uvv.video_id = items.id and uvv.user_id = ").bind(u).push(" ");
    }
    q.push("where items.id = any(").bind(page_ids.clone()).push(") group by items.id");
    let mut rows = q.fetch_all(&st.db).await?;
    rows.sort_by_key(|r| r.i64("id").and_then(|id| page_order.get(&id).copied()).unwrap_or(usize::MAX));

    for row in rows.iter_mut() {
        let is_audio = row.str("mime").map(|m| m.starts_with("audio/")).unwrap_or(false);
        row.insert("is_audio".into(), json!(is_audio));
        let t_id = row.i64("tag_id");
        let (class, rlabel) = match t_id {
            Some(1) => ("sfw", "SFW"),
            Some(2) => ("nsfw", "NSFW"),
            Some(n) if n == nsfl => ("nsfl", "NSFL"),
            _ => ("untagged", "?"),
        };
        row.insert("rating_class".into(), json!(class));
        row.insert("rating_label".into(), json!(rlabel));
    }

    if rows.iter().any(|r| r.bool("is_album")) {
        if let Err(e) = album_visibility(st, &cfg, o, &f, eff_mode, safe.clone(), &mut rows).await {
            tracing::warn!("[FEED] Error calculating visible album count for feed: {e}");
        }
    }

    // Dynamic thumb sizing only on the main feed (no user/tag/hall/user hall/favs).
    let empty = |s: &Option<String>| s.as_deref().map(str::is_empty).unwrap_or(true);
    let is_main_feed = cfg.truthy("websrv.enable_dynamic_thumbs") && empty(&o.user) && empty(&o.tag) && empty(&o.hall) && empty(&o.user_hall) && !o.fav;
    for row in rows.iter_mut() {
        let size = if is_main_feed {
            let c = row.f64("contribution").unwrap_or(0.0);
            if c >= 5.0 { 2 } else { 1 }
        } else {
            1
        };
        row.insert("thumb_size".into(), json!(size));
    }

    let cheat: Vec<i64> = ((act_page - 3).max(1)..=(act_page + 3).min(pages)).collect();
    // genLink receives the hall slug; the old code passed the ILIKE-escaped tagger here, which
    // double-escaped `_`/`%` in pagination links — use the raw value.
    let hall_slug = f.hall_obj.as_ref().and_then(|h| h["slug"].as_str().map(str::to_string)).or_else(|| f.hall.clone());
    let mut link = gen_link(&LinkEnv {
        tag: f.tag.as_deref().filter(|t| !t.is_empty()),
        hall: hall_slug.as_deref().filter(|h| !h.is_empty()),
        user: f.user.as_deref().filter(|u| !u.is_empty()),
        kind: Some(view_mode),
        path: Some("p/"),
        strict: o.strict,
        tagger: o.tagger.as_deref(),
    });
    if f.is_title_search {
        if let Some(tq) = f.title_query.as_deref().filter(|q| !q.is_empty()) {
            link["main"] = json!(format!("/tag/title:{}/", encode_uri_component(tq)));
            link["mainDisplay"] = json!(format!("/tag/title:{tq}/"));
            link["path"] = json!("p/");
            link["suffix"] = json!("");
        }
    }
    if let (Some(uh), Some(owner)) = (&f.user_hall_obj, f.user_hall_owner.as_deref().filter(|o| !o.is_empty())) {
        let owner_name = uh.str("owner_name").filter(|s| !s.is_empty()).unwrap_or(owner).to_string();
        let slug = uh.string("slug");
        link["main"] = json!(format!("/user/{}/hall/{}/", encode_uri_component(&owner_name), encode_uri_component(&slug)));
        link["mainDisplay"] = json!(format!("/user/{owner_name}/hall/{slug}/"));
        link["path"] = json!("p/");
        link["suffix"] = json!("");
    }

    let location = format!("{}{}", link["main"].as_str().unwrap_or(""), link["path"].as_str().unwrap_or(""));
    Ok(json!({
        "success": true,
        "items": rows.into_iter().map(Value::Object).collect::<Vec<_>>(),
        "pagination": {
            "start": 1,
            "end": pages,
            "current": act_page,
            "location": location,
            "suffix": link["suffix"],
            "prev": if act_page > 1 { json!(act_page - 1) } else { Value::Null },
            "next": if act_page < pages { json!(act_page + 1) } else { Value::Null },
            "page": act_page,
            "cheat": cheat,
        },
        "link": link,
        "tmp": tmp,
        "total": total,
        "view_mode": view_mode,
    }))
}

/// Album rows in a feed: count only the sub-items the viewer may see and, on tag feeds, show the
/// first matching sub-item.
async fn album_visibility(
    st: &AppState,
    cfg: &Config,
    o: &FeedOpts<'_>,
    f: &Filters,
    eff_mode: i64,
    safe: Option<Vec<String>>,
    rows: &mut [Row],
) -> anyhow::Result<()> {
    let album_ids: Vec<i32> = rows.iter().filter(|r| r.bool("is_album")).filter_map(|r| r.i64("id")).map(|n| n as i32).collect();
    if album_ids.is_empty() {
        return Ok(());
    }
    let subs = sql!(
        "SELECT ai.item_id, ai.id as subf0ck_id, ai.slug as subf0ck_slug, ai.dest as subf0ck_dest, ai.mime as subf0ck_mime, ai.order_index
         FROM album_items ai WHERE ai.item_id = ANY(",
        album_ids,
        "::int[]) ORDER BY ai.item_id, ai.order_index ASC"
    )
    .fetch_all(&st.db)
    .await?;
    let sub_ids: Vec<i32> = subs.iter().filter_map(|s| s.i64("subf0ck_id")).map(|n| n as i32).collect();
    let sub_tags = if sub_ids.is_empty() {
        vec![]
    } else {
        sql!(
            "SELECT aita.album_item_id, t.id, t.tag, t.normalized FROM album_items_tags_assign aita JOIN tags t ON t.id = aita.tag_id WHERE aita.album_item_id = ANY(",
            sub_ids,
            "::int[])"
        )
        .fetch_all(&st.db)
        .await
        .unwrap_or_default()
    };
    let mut tags_by_sub: HashMap<i64, Vec<Value>> = HashMap::new();
    for t in sub_tags {
        let k = t.i64("album_item_id").unwrap_or(0);
        tags_by_sub.entry(k).or_default().push(Value::Object(t));
    }
    let mut subs_by_item: HashMap<i64, Vec<Row>> = HashMap::new();
    for s in subs {
        subs_by_item.entry(s.i64("item_id").unwrap_or(0)).or_default().push(s);
    }
    let no_tags: Vec<Value> = vec![];
    let vis = SubVisibility { cfg, session: o.session, eff_mode, safe_ratings: safe, excluded_tags: &f.excluded_tags };

    for row in rows.iter_mut() {
        if !row.bool("is_album") {
            continue;
        }
        let Some(list) = subs_by_item.get(&row.i64("id").unwrap_or(0)) else { continue };
        let username = row.string("username").to_lowercase();
        let owner_or_admin = o
            .session
            .map(|s| (!s.user.is_empty() && !username.is_empty() && s.user.to_lowercase() == username) || s.admin || s.is_moderator)
            .unwrap_or(false);
        let tags_of = |s: &Row| tags_by_sub.get(&s.i64("subf0ck_id").unwrap_or(0)).unwrap_or(&no_tags);
        let mut visible: Vec<&Row> = list.iter().filter(|s| owner_or_admin || vis.visible(tags_of(s))).collect();

        if let Some(tag) = f.tag.as_deref().filter(|t| !t.is_empty() && !f.is_title_search) {
            let terms = tag_terms(tag);
            if !terms.is_empty() {
                let matching: Vec<&Row> = visible.iter().copied().filter(|s| sub_matches_tag(tags_of(s), &terms, &f.strict_params)).collect();
                if let Some(ms) = matching.first() {
                    let slug = ms.val("subf0ck_slug");
                    row.insert("target_subf0ck_slug".into(), if crate::config::truthy(slug) { slug.clone() } else { ms.val("subf0ck_id").clone() });
                    if let Some(dest) = ms.str("subf0ck_dest").filter(|d| !d.is_empty()) {
                        let base = strip_ext(dest);
                        row.insert("thumb".into(), json!(format!("/t/{base}.webp")));
                        row.insert("matching_sub_thumb".into(), json!(format!("/t/{base}.webp")));
                        row.insert("matching_sub_dest".into(), json!(dest));
                        row.insert("dest".into(), json!(dest));
                    }
                    if let Some(m) = ms.str("subf0ck_mime").filter(|m| !m.is_empty()) {
                        row.insert("matching_sub_mime".into(), json!(m));
                        row.insert("mime".into(), json!(m));
                    }
                    visible = matching;
                }
            }
        }
        row.insert("album_count".into(), json!(visible.len()));
        if visible.len() <= 1 {
            row.insert("is_album".into(), json!(false));
        }
    }
    Ok(())
}

/// `dest.replace(/\.[^.]+$/, '')`
pub(crate) fn strip_ext(dest: &str) -> &str {
    match dest.rfind('.') {
        Some(i) if i + 1 < dest.len() => &dest[..i],
        _ => dest,
    }
}
