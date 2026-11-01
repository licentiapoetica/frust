//! `getf0ck` — a single item with neighbours, tags, favourites, reposts and album sub-items.

use super::feed::strip_ext;
use super::*;
use crate::settings::enable_item_slugs;
use crate::util::{LinkEnv, encode_uri_component, gen_link};
use std::path::Path;

fn not_found() -> Value {
    json!({ "success": false, "message": "404 - upload not found" })
}

/// "Sorry, this post is currently not visible." with OG data for link previews.
/// `slug`: `Some(..)` adds the `slug` key (some branches of the old code included it).
fn not_visible(cfg: &Config, itemid: i64, slug: Option<Value>, blur: bool, hall_slug: Option<&str>) -> Value {
    let domain = cfg.str_or("main.url.domain", "");
    let thumbs = cfg.str_or("websrv.paths.thumbnails", "");
    let og_url = match hall_slug.filter(|h| !h.is_empty()) {
        Some(h) => format!("https://{domain}/h/{}/{itemid}", encode_uri_component(h)),
        None => format!("https://{domain}/{itemid}"),
    };
    let mut item = json!({ "id": itemid });
    if let Some(s) = slug {
        item["slug"] = s;
    }
    item["og_thumbnail"] = json!(format!("{thumbs}/{itemid}{}.webp", if blur { "_blur" } else { "" }));
    item["og_url"] = json!(og_url);
    item["og_description"] = json!("Content not visible in current mode");
    json!({ "success": false, "message": "Sorry, this post is currently not visible.", "item": item })
}

fn nonempty_file(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false)
}

async fn quiet(cmd: &str, args: &[&str]) {
    let opts = crate::proc::Opts { quiet: true, ignore_exit_code: true, ..Default::default() };
    if let Err(e) = crate::proc::spawn_with(cmd, args, opts).await {
        tracing::debug!("[GETF0CK] {cmd}: {e}");
    }
}

/// Extract the cover art of an audio file to `ca`, and a 256px square thumbnail to `t` on success.
async fn extract_coverart(audio: &Path, ca: &Path, t: &Path) -> bool {
    let (a, c, tt) = (audio.to_string_lossy(), ca.to_string_lossy(), t.to_string_lossy());
    quiet("ffmpeg", &["-y", "-i", &a, "-an", "-vcodec", "webp", "-frames:v", "1", &c]).await;
    if nonempty_file(ca) {
        let src = format!("{c}[0]");
        quiet("magick", &[&src, "-resize", "256x256^", "-gravity", "center", "-crop", "256x256+0+0", "+repage", &tt]).await;
        true
    } else {
        let _ = std::fs::remove_file(ca);
        false
    }
}

/// `queue.findallrepostphash(phash, excludeId)` — items whose perceptual hash is close.
/// TODO(port): move to the queue port once it exists.
pub async fn find_all_repost_phash(st: &AppState, phash: &str, exclude_id: Option<i64>) -> anyhow::Result<Vec<Row>> {
    let hashes: Vec<&str> = phash.split('_').filter(|s| !s.is_empty() && !s.starts_with("00000000")).collect();
    if hashes.is_empty() {
        return Ok(vec![]);
    }
    let h = |i: usize| hashes.get(i).copied().unwrap_or("").to_string();
    let mut q = Sql::new(
        "SELECT id, username, stamp FROM items WHERE is_deleted = false AND phash IS NOT NULL AND phash != '' AND phash != 'ERROR' AND phash != 'MISSING' AND phash NOT LIKE '00000000%' ",
    );
    if let Some(x) = exclude_id.filter(|&x| x != 0) {
        q.push("AND id != ").bind(x).push(" ");
    }
    q.push("AND (");
    for i in 0..3 {
        if i > 0 {
            q.push(" + ");
        }
        let part = i + 1;
        q.push(&format!("CASE WHEN split_part(phash, '_', {part}) != '' AND "))
            .bind(h(i))
            .push(&format!(" != '' THEN bit_count(('x' || split_part(phash, '_', {part}))::bit(1024) # ('x' || "))
            .bind(h(i))
            .push(")::bit(1024)) <= 15 ELSE false END::int");
    }
    q.push(" >= 1) ORDER BY id ASC");
    let rows = q.fetch_all(&st.db).await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let mut m = Row::new();
            m.insert("id".into(), r.val("id").clone());
            m.insert("username".into(), r.val("username").clone());
            m.insert("stamp".into(), r.val("stamp").clone());
            m
        })
        .collect())
}

/// `getf0ck({...})`
pub async fn getf0ck(st: &AppState, o: &FeedOpts<'_>) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    let session = o.session;
    if o.fav {
        if let Some(u) = o.user.as_deref().filter(|u| !u.is_empty()) {
            let (is_private, allowed) = check_favorites_access(st, u, o).await?;
            if is_private && !allowed {
                return Ok(json!({ "success": false, "is_private": true, "message": "private favorites" }));
            }
        }
    }
    let clean_ids: Option<Vec<i32>> = o.ids.as_ref().map(|ids| ids.iter().filter(|&&n| n > 0).map(|&n| n as i32).collect());
    let ids_filter = match clean_ids.as_ref().filter(|i| !i.is_empty()) {
        Some(ids) => sql!("and items.id = ANY(", ids.clone(), "::int[])"),
        None => Sql::empty(),
    };

    let user = o.user.as_deref().filter(|u| !u.is_empty()).map(crate::http::decode_uri);
    let user_like = user.as_deref().map(escape_like);
    let (decoded_tag, is_title_search, title_query, tag) = split_title_tag(o.tag.as_deref());

    // hall: the hall object when it exists, the raw slug otherwise
    let raw_hall = o.hall.clone().filter(|h| !h.is_empty());
    let hall_value: Value = match raw_hall.as_deref() {
        Some(h) => hall_obj(st, h).await?.unwrap_or_else(|| json!(h)),
        None => json!(o.hall),
    };
    let hall_slug: Option<String> = match &hall_value {
        Value::Object(m) => m.get("slug").and_then(Value::as_str).map(str::to_string),
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    let user_hall_slug = o.user_hall.clone();
    let user_hall_owner = o.user_hall_owner.clone();
    let user_hall_obj = match (user_hall_slug.as_deref().filter(|s| !s.is_empty()), user_hall_owner.as_deref().filter(|s| !s.is_empty())) {
        (Some(slug), Some(owner)) => user_hall_row(st, owner, slug).await?,
        _ => None,
    };
    let mime = o.mime.clone().unwrap_or_default();
    let Some(raw_id) = o.itemid.clone().filter(|i| !i.is_empty()) else {
        return Ok(not_found());
    };

    let is_numeric = raw_id.bytes().all(|b| b.is_ascii_digit());
    let mut item_lookup = if is_numeric {
        match raw_id.parse::<i64>() {
            Ok(n) => sql!("items.id = ", n),
            Err(_) => return Ok(not_found()),
        }
    } else {
        sql!("items.slug = ", raw_id.as_str())
    };
    let mut requested_sub: Option<String> = o.subf0ck.clone().filter(|s| !s.is_empty());
    if !is_numeric {
        let found = sql!("SELECT id FROM items WHERE slug = ", raw_id.as_str(), " LIMIT 1").fetch_optional(&st.db).await?;
        if found.is_none() {
            if let Some(sub) = sql!("SELECT item_id, slug FROM album_items WHERE slug = ", raw_id.as_str(), " LIMIT 1").fetch_optional(&st.db).await? {
                requested_sub = sub.str("slug").map(str::to_string);
                item_lookup = sql!("items.id = ", sub.i64("item_id").unwrap_or(0) as i32);
            }
        }
    }

    let (mime_parts, mime_sql) = resolve_mime_sql(&cfg, Some(&mime), session, "items");
    let excluded_tags = o.exclude.clone();
    let strict_p = strict_params(o.strict, tag.as_deref());
    let is_strict = !strict_p.is_empty();

    let user_hall_val = match (&user_hall_obj, &user_hall_slug) {
        (Some(u), _) => Value::Object(u.clone()),
        (None, Some(s)) => json!(s),
        _ => Value::Null,
    };
    let tmp = json!({
        "user": user,
        "tag": if is_title_search { json!(decoded_tag) } else { json!(tag) },
        "hall": hall_value,
        "mime": mime,
        "itemid": raw_id,
        "strict": o.strict,
        "userHall": user_hall_val,
        "userHallOwner": user_hall_owner,
    });

    let eff_mode = o.mode.unwrap_or(0);
    let nsfl = nsfl_id(&cfg);
    let safe = safe_ratings(&cfg, o.ratings.as_ref(), session, false);
    let item_mode_query = compute_base_mode(&cfg, o.mode, o.ratings.as_ref(), session);

    let mut tag_filter = Sql::empty();
    let mut title_filter = Sql::empty();
    if is_title_search && title_query.as_deref().map(|q| !q.is_empty()).unwrap_or(false) {
        title_filter = sql!("and items.title ILIKE ", format!("%{}%", title_query.as_deref().unwrap_or("")), " and items.title IS NOT NULL");
    } else if let Some(t) = tag.as_deref().filter(|t| !t.is_empty()) {
        tag_filter = tag_filter_sql(t, is_strict, None, false);
    }
    let hall_filter = match hall_slug.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => sql!(
            "and items.id in (select item_id from halls_assign join halls on halls.id = halls_assign.hall_id where halls.slug = ",
            h,
            ")"
        ),
        None => Sql::empty(),
    };
    let user_hall_filter = match &user_hall_obj {
        Some(u) => sql!("and items.id in (select uha.item_id from user_halls_assign uha where uha.hall_id = ", u.i64("id").unwrap_or(0) as i32, ")"),
        None => Sql::empty(),
    };

    let fav_user = o.fav && user.is_some();
    let conditions = || -> Sql {
        let mut q = Sql::new(&item_mode_query);
        q.push(" and items.active = true ");
        q.append(&visibility_filter(session));
        q.push(" and (items.expires_at IS NULL OR items.expires_at > ").bind(crate::util::now_secs()).push(") ");
        q.append(&tag_filter).push(" ");
        q.append(&title_filter).push(" ");
        q.append(&hall_filter).push(" ");
        q.append(&user_hall_filter).push(" ");
        if let Some(like) = &user_like {
            if o.fav {
                q.push(r#"and "user"."user" ilike "#).bind(like.as_str()).push(" ");
            } else {
                q.push("and items.username ilike ").bind(like.as_str()).push(" ");
            }
        }
        q.append(&ids_filter).push(" ");
        q.append(&mime_sql).push(" ");
        q.append(&global_filter_sql(st, &cfg, session)).push(" ");
        q.append(&exclude_sql(&excluded_tags));
        q
    };

    // 1. The item itself
    let uid = o.user_id();
    let mut q = Sql::new(
        r#"select items.*, items.username as username, uo.username_color as author_color, uo.display_name as author_display_name,
        uo.avatar as author_avatar, uo.avatar_file as author_avatar_file, uo.banner_file as author_banner_file,
        uo.banner_position as author_banner_position, uo.banner_size as author_banner_size, uo.description as author_description,
        author_u.id as author_id, items.is_pinned, "#,
    );
    match uid {
        Some(_) => q.push("coalesce(uvv.view_count, 0) as my_views"),
        None => q.push("0 as my_views"),
    };
    q.push(r#" from items left join "user" author_u on author_u."user" = items.username left join "user_options" uo on uo.user_id = author_u.id "#);
    if let Some(u) = uid {
        q.push("left join user_video_views uvv on uvv.video_id = items.id and uvv.user_id = ").bind(u).push(" ");
    }
    q.push("where ").append(&item_lookup).push(" and items.active = true limit 1");
    let Some(mut actitem) = q.fetch_optional(&st.db).await? else {
        return Ok(not_found());
    };
    let itemid = actitem.i64("id").unwrap_or(0);
    let visibility = actitem.i64("visibility").unwrap_or(0);

    let owner_name = session_owner_name(session);
    let item_user = actitem.string("username");
    let is_owner_or_admin = session
        .map(|s| owner_name.as_deref().map(|n| n.to_lowercase() == item_user.to_lowercase()).unwrap_or(false) || s.admin || s.is_moderator)
        .unwrap_or(false);
    if visibility == 2 && !is_owner_or_admin {
        return Ok(json!({ "success": false, "is_private": true, "message": "403 - private upload" }));
    }
    if visibility == 3 && !session.map(|s| s.admin).unwrap_or(false) {
        return Ok(not_found());
    }
    // Numeric ids must not enumerate unlisted/private items.
    if is_numeric && (visibility == 1 || visibility == 2) && !is_owner_or_admin {
        return Ok(not_found());
    }

    if let Some(u) = uid {
        let db = st.db.clone();
        tokio::spawn(async move {
            let r = sql!(
                "insert into user_video_views (user_id, video_id, view_count, last_viewed) values (",
                u,
                ", ",
                itemid as i32,
                ", 1, now()) on conflict (user_id, video_id) do update set view_count = user_video_views.view_count + 1, last_viewed = now()"
            )
            .execute(&db)
            .await;
            if let Err(e) = r {
                tracing::error!("Failed to track view: {e}");
            }
        });
    }

    // Guests and anonymous users: nsfp / rating restrictions for public items
    let public_visitor = is_guest(session) || is_anon_session(session);
    if public_visitor && !is_owner_or_admin && visibility == 0 {
        let mut blocked = false;
        if let Some(gf) = global_filter(st, &cfg, session) {
            let mut q = sql!("select 1 from tags_assign where item_id = ", itemid as i32, " and (");
            q.push(&gf).push(") limit 1");
            blocked = q.fetch_optional(&st.db).await?.is_some();
        }
        if !blocked && session.is_none() && !cfg.truthy("websrv.public_untagged") {
            let rating = sql!("select 1 from tags_assign where item_id = ", itemid as i32, " and tag_id in (1, 2, ", nsfl, ") limit 1")
                .fetch_optional(&st.db)
                .await?;
            blocked = rating.is_none();
        }
        if blocked {
            return Ok(not_visible(&cfg, itemid, None, true, hall_slug.as_deref()));
        }
    }

    // 2. Neighbours
    let nsfl_cfg = {
        let v = cfg.get("nsfl_tag_id");
        if crate::config::truthy(v) { crate::config::as_int(v).unwrap_or(3) } else { 3 }
    };
    let use_tags_driver = session.is_some()
        && !is_anon_session(session)
        && (eff_mode == 1 || eff_mode == 4)
        && !o.fav
        && tag.as_deref().map(str::is_empty).unwrap_or(true)
        && user.is_none()
        && raw_hall.is_none()
        && clean_ids.as_ref().map(|i| i.is_empty()).unwrap_or(true);
    let neighbour = |where_clause: Sql, order_by: Sql, limit: i64| -> Sql {
        if use_tags_driver {
            let modequery = get_mode(&cfg, eff_mode);
            let tag_id = if eff_mode == 4 { nsfl_cfg } else if eff_mode == 1 { 2 } else { 1 };
            let use_tag_id = !mime_parts.iter().any(|m| m == "audio");
            let nsfp = st.settings.nsfp_ids();
            let check_filter = is_anon_session(session) && !nsfp.is_empty();
            let mut q = Sql::new("SELECT ta.item_id as id, items.slug FROM tags_assign ta INNER JOIN items ON items.id = ta.item_id ");
            if check_filter {
                q.push("LEFT JOIN tags_assign filter_ta ON filter_ta.item_id = ta.item_id AND filter_ta.tag_id IN ").bind_list(&nsfp).push(" ");
            }
            q.push("WHERE ");
            if use_tag_id {
                q.push("ta.tag_id = ").bind(tag_id);
            } else {
                q.push(&modequery);
            }
            q.push(" AND items.active = true AND coalesce(items.visibility, 0) = 0 ");
            q.append(&mime_sql).push(" ");
            if check_filter {
                q.push("AND filter_ta.tag_id IS NULL ");
            }
            if !excluded_tags.is_empty() {
                let ids: Vec<i32> = excluded_tags.iter().map(|&n| n as i32).collect();
                q.push("and not exists (select 1 from tags_assign where item_id = ta.item_id and tag_id = any(").bind(ids).push("::int[])) ");
            }
            q.append(&where_clause).push(" ").append(&order_by).push(" LIMIT ").bind(limit);
            return q;
        }
        let mut q = Sql::new("select items.id, items.slug from items left join tags_assign on tags_assign.item_id = items.id left join tags on tags.id = tags_assign.tag_id ");
        if fav_user {
            q.push(r#"inner join favorites on favorites.item_id = items.id inner join "user" on "user".id = favorites.user_id "#);
        } else {
            q.push(r#"left join favorites on favorites.item_id = items.id left join "user" on "user".id = favorites.user_id "#);
        }
        q.push("where ").append(&conditions()).push(" ").append(&where_clause);
        q.push(" group by items.id, items.slug ").append(&order_by).push(" limit ").bind(limit);
        q
    };
    let id32 = itemid as i32;
    let (next_item, prev_item, start_item, end_item, cheat_items) = if o.random {
        let next = neighbour(sql!("and items.id != ", id32), Sql::new("order by random()"), 1).fetch_all(&st.db).await?;
        (next.clone(), next, vec![], vec![], vec![])
    } else {
        let qs = [
            neighbour(sql!("and items.id > ", id32), Sql::new("order by items.id asc"), 1),
            neighbour(sql!("and items.id < ", id32), Sql::new("order by items.id desc"), 1),
            neighbour(Sql::empty(), Sql::new("order by items.id asc"), 1),
            neighbour(Sql::empty(), Sql::new("order by items.id desc"), 1),
            neighbour(sql!("and items.id != ", id32), sql!("order by abs(items.id - ", id32, ") asc"), 7),
        ];
        let (a, b, c, d, e) = tokio::try_join!(
            qs[0].fetch_all(&st.db),
            qs[1].fetch_all(&st.db),
            qs[2].fetch_all(&st.db),
            qs[3].fetch_all(&st.db),
            qs[4].fetch_all(&st.db),
        )?;
        (a, b, c, d, e)
    };
    let mut cheat: Vec<i64> = std::iter::once(itemid).chain(cheat_items.iter().filter_map(|r| r.i64("id"))).collect();
    cheat.sort();

    // 3. Metadata
    let checksum = actitem.string("checksum");
    let repost_base = if checksum.is_empty() {
        None
    } else if let Some((base, _)) = checksum.split_once("_bypass_") {
        Some(base.to_string())
    } else {
        Some(checksum.clone())
    };
    let has_bypass = checksum.contains("_bypass_");
    let tags_fut = crate::util::get_tags(st, itemid, session, None);
    let halls_q = sql!("select h.name, h.slug from halls h join halls_assign ha on ha.hall_id = h.id where ha.item_id = ", id32);
    let halls_fut = halls_q.fetch_all(&st.db);
    let user_halls_fut = async {
        match uid {
            Some(u) => {
                sql!(
                    "select uh.name, uh.slug from user_halls uh join user_halls_assign uha on uha.hall_id = uh.id where uha.item_id = ",
                    id32,
                    " and uh.user_id = ",
                    u
                )
                .fetch_all(&st.db)
                .await
            }
            None => Ok(vec![]),
        }
    };
    let favs_q = sql!(
        r#"select "favorites".user_id, "user".user, "user".login, "user_options".avatar, "user_options".avatar_file, "user_options".username_color, "user_options".display_name, "user_options".hide_fav_badge, "anon_identities".fingerprint as anon_fingerprint
        from "favorites"
        left join "user" on "user".id = "favorites".user_id
        left join "user_options" on "user_options".user_id = "favorites".user_id
        left join "anon_identities" on "anon_identities".user_id = "favorites".user_id
        where "favorites".item_id = "#,
        id32
    )
;
    let favs_fut = favs_q.fetch_all(&st.db);
    let reposts_fut = async {
        match &repost_base {
            Some(base) => {
                let mut q = sql!("SELECT id, slug, username, stamp FROM items WHERE active = true AND id != ", id32, " AND ");
                if has_bypass {
                    q.push("(checksum = ").bind(base.as_str()).push(" OR checksum LIKE ").bind(format!("{base}_bypass_%")).push(")");
                } else {
                    q.push("checksum LIKE ").bind(format!("{base}_bypass_%"));
                }
                q.push(" ORDER BY id ASC");
                q.fetch_all(&st.db).await
            }
            None => Ok(vec![]),
        }
    };
    let phash = actitem.string("phash");
    let phash_fut = async {
        if !phash.is_empty() && phash != "ERROR" && phash != "MISSING" {
            find_all_repost_phash(st, &phash, Some(itemid)).await.unwrap_or_default()
        } else {
            vec![]
        }
    };
    let (tags, item_halls, user_halls, mut favorites, repost_rows, phash_matches) =
        tokio::try_join!(tags_fut, halls_fut, user_halls_fut, favs_fut, reposts_fut, async { Ok(phash_fut.await) })?;

    let mut link = gen_link(&LinkEnv {
        tag: tag.as_deref().filter(|t| !t.is_empty()),
        hall: hall_slug.as_deref().filter(|h| !h.is_empty()),
        user: user.as_deref(),
        kind: Some(if o.fav { "favs" } else { "uploads" }),
        path: Some(""),
        strict: o.strict,
        tagger: None,
    });
    if is_title_search {
        if let Some(tq) = title_query.as_deref().filter(|q| !q.is_empty()) {
            link["main"] = json!(format!("/tag/title:{}/", encode_uri_component(tq)));
            link["mainDisplay"] = json!(format!("/tag/title:{tq}/"));
            link["path"] = json!("");
            link["suffix"] = json!("");
        }
    }
    if let (Some(uh), Some(owner)) = (&user_hall_obj, user_hall_owner.as_deref().filter(|o| !o.is_empty())) {
        let owner_name = uh.str("owner_name").filter(|s| !s.is_empty()).unwrap_or(owner).to_string();
        let slug = uh.string("slug");
        link["main"] = json!(format!("/user/{}/hall/{}/", encode_uri_component(&owner_name), encode_uri_component(&slug)));
        link["mainDisplay"] = json!(format!("/user/{owner_name}/hall/{slug}/"));
        link["path"] = json!("");
        link["suffix"] = json!("");
    }

    for f in favorites.iter_mut() {
        let u = f.string("user");
        if f.bool("anon_fingerprint") || u == "anonymous" || u.starts_with("anon_") {
            f.insert("is_anon".into(), json!(true));
            f.insert("user".into(), json!("anonymous"));
            f.insert("login".into(), json!("anonymous"));
            f.insert("display_name".into(), json!("Anonymous"));
            f.insert("avatar".into(), Value::Null);
            f.insert("avatar_file".into(), Value::Null);
            f.insert("username_color".into(), Value::Null);
            f.insert("hide_fav_badge".into(), json!(true));
        }
    }

    // Reposts: checksum matches + perceptual-hash matches
    let mut reposts: Vec<Value> = repost_rows
        .iter()
        .map(|r| json!({ "id": r.val("id"), "slug": r.val("slug"), "username": r.val("username"), "stamp": r.val("stamp"), "match_type": "checksum" }))
        .collect();
    if !phash_matches.is_empty() {
        let mut seen: std::collections::HashSet<i64> = reposts.iter().filter_map(|r| crate::config::as_int(&r["id"])).collect();
        for pm in &phash_matches {
            let id = pm.i64("id").unwrap_or(0);
            if seen.insert(id) {
                // findallrepostphash returns no slug, so the old objects had `slug: undefined`
                reposts.push(json!({ "id": pm.val("id"), "username": pm.val("username"), "stamp": pm.val("stamp"), "match_type": "phash" }));
            }
        }
        reposts.sort_by_key(|r| crate::config::as_int(&r["id"]).unwrap_or(0));
    }

    // Cover art for audio (extracted on demand)
    let thumbs = cfg.str_or("websrv.paths.thumbnails", "").to_string();
    let images = cfg.str_or("websrv.paths.images", "").to_string();
    let coverarts = cfg.str_or("websrv.paths.coverarts", "").to_string();
    let mut has_coverart = false;
    if actitem.str("mime").map(|m| m.starts_with("audio/")).unwrap_or(false) {
        let ca = cfg.paths.ca.join(format!("{itemid}.webp"));
        if nonempty_file(&ca) {
            has_coverart = true;
        } else if let Some(audio) = crate::http::files::safe_join(&cfg.paths.b, &actitem.string("dest")) {
            if audio.exists() {
                has_coverart = extract_coverart(&audio, &ca, &cfg.paths.t.join(format!("{itemid}.webp"))).await;
            }
        }
    }
    let coverart_url = has_coverart.then(|| format!("{coverarts}/{itemid}.webp"));

    let mut album: Vec<Value> = vec![];
    let mut effective_tags = tags.clone();
    if actitem.bool("is_album") {
        let ctx = AlbumCtx {
            st,
            cfg: &cfg,
            o,
            itemid,
            tags: &tags,
            tag: tag.as_deref().filter(|t| !t.is_empty() && !is_title_search),
            strict_params: &strict_p,
            excluded_tags: &excluded_tags,
            eff_mode,
            safe: safe.clone(),
            has_coverart,
            hall_slug: hall_slug.as_deref(),
        };
        match build_album(&ctx, &mut actitem, &mut requested_sub).await {
            Ok(AlbumOutcome::NotVisible(v)) => return Ok(v),
            Ok(AlbumOutcome::Done { album: a, effective_tags: et }) => {
                album = a;
                if let Some(et) = et {
                    effective_tags = et;
                }
            }
            Err(e) => tracing::error!("[GETF0CK] Failed to fetch album items: {e}"),
        }
    }

    let r = Rating::of(&cfg, &effective_tags);
    let (is_nsfl, is_nsfw, is_sfw) = (r.nsfl, r.nsfw, r.sfw);
    let is_untagged = r.untagged();
    if session.is_none() && !is_owner_or_admin && visibility == 0 {
        let blocked = (is_nsfw && !cfg.truthy("websrv.public_nsfw")) || is_nsfl || (is_untagged && !cfg.truthy("websrv.public_untagged"));
        if blocked {
            return Ok(not_visible(&cfg, itemid, None, is_nsfw, hall_slug.as_deref()));
        }
    } else if is_anon_session(session) && !is_owner_or_admin && visibility == 0 {
        let allowed = anon_allowed_modes(&cfg);
        let has = |m: &str| allowed.iter().any(|x| x == m);
        let blocked = (is_nsfw && !has("nsfw"))
            || (is_nsfl && !has("nsfl"))
            || (is_untagged && !has("untagged") && eff_mode != 2)
            || (is_sfw && !has("sfw"));
        if blocked {
            return Ok(not_visible(&cfg, itemid, None, is_nsfw, hall_slug.as_deref()));
        }
    }

    // Mode mismatch (0 sfw, 1 nsfw, 2 untagged, 3 all, 4 nsfl)
    if !o.bypass_filter {
        let untagged_disallowed = (session.is_none() && !cfg.truthy("websrv.public_untagged"))
            || (session.is_some() && is_anon_session(session) && !anon_allowed_modes(&cfg).iter().any(|m| m == "untagged"));
        let blocked = match safe.as_ref().filter(|s| !s.is_empty()) {
            Some(s) => !s.iter().any(|x| x == r.name()),
            None => match eff_mode {
                3 => false,
                0 => is_nsfw || is_nsfl || (is_untagged && untagged_disallowed),
                1 => !is_nsfw,
                4 => !cfg.enable_nsfl() || !is_nsfl,
                2 => !is_untagged,
                _ => false,
            },
        };
        if blocked {
            return Ok(not_visible(&cfg, itemid, Some(slug_or_null(&actitem)), is_nsfw, hall_slug.as_deref()));
        }
    }

    let slugs = enable_item_slugs(&cfg);
    if slugs && !actitem.bool("slug") {
        let slug = crate::util::generate_slug(11);
        actitem.insert("slug".into(), json!(slug));
        let db = st.db.clone();
        tokio::spawn(async move {
            if let Err(e) = sql!("UPDATE items SET slug = ", slug, " WHERE id = ", id32, " AND (slug IS NULL OR slug = '')").execute(&db).await {
                tracing::error!("[AUTO_SLUG] Failed DB update: {e}");
            }
        });
    }

    let domain = cfg.str_or("main.url.domain", "").to_string();
    let username = actitem.string("username");
    let src = actitem.string("src");
    let mime_now = actitem.string("mime");
    let dest = actitem.string("dest");
    let dest_url = if mime_now != "video/youtube" {
        format!("{images}/{dest}")
    } else if dest.starts_with("yt:") {
        dest.clone()
    } else {
        static YT: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
            regex::Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?/?\?(?:\S*?&?v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").unwrap()
        });
        YT.captures(&src).map(|c| format!("yt:{}", &c[1])).unwrap_or_else(|| dest.clone())
    };
    let og_description = {
        let label = if is_nsfl { "NSFL" } else if is_nsfw { "NSFW" } else if is_sfw { "SFW" } else { "Unrated" };
        let title = actitem.string("title");
        let title_part = if title.is_empty() { String::new() } else { format!(" · \"{title}\"") };
        format!("{label}{title_part} · uploaded by {username}")
    };
    let og_url = match hall_slug.as_deref().filter(|h| !h.is_empty()) {
        Some(h) => format!("https://{domain}/h/{}/{itemid}", encode_uri_component(h)),
        None => format!("https://{domain}/{itemid}"),
    };
    let stamp = actitem.i64("stamp").unwrap_or(0);
    let lang = o.lang.clone().unwrap_or_else(|| "en".into());
    let timeago = crate::util::time_ago(&st.i18n.translator(&lang), crate::util::now_secs() - stamp);
    let timefull = chrono::DateTime::from_timestamp(stamp, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap_or_default();
    let or_null = |v: &Value| if crate::config::truthy(v) { v.clone() } else { Value::Null };
    let is_repost = checksum.contains("_bypass_");
    let show_repost_row = (session.is_some() || cfg.truthy("websrv.expose_repost_links_to_guests") || cfg.truthy("websrv.expose_repost_links"))
        && (is_repost || !reposts.is_empty());
    let slug_val = actitem.val("slug").clone();
    let has_slug = slugs && crate::config::truthy(&slug_val);
    let pick = |rows: &Vec<Row>, fallback: Value| -> Value {
        match rows.first() {
            Some(r) if slugs && r.bool("slug") => r.val("slug").clone(),
            Some(r) if r.bool("id") => r.val("id").clone(),
            _ => fallback,
        }
    };
    let channel = if actitem.str("usernetwork") == Some("Telegram") && actitem.str("userchannel") != cfg.str("websrv.domain") {
        json!("anonymous")
    } else {
        actitem.val("userchannel").clone()
    };
    let album_json = serde_json::to_string(&album).unwrap_or_else(|_| "[]".into());
    let album_count = if !album.is_empty() {
        json!(album.len())
    } else if actitem.bool("album_count") {
        actitem.val("album_count").clone()
    } else {
        json!(0)
    };
    let phrases = cfg.get("websrv.phrases").as_array().cloned().unwrap_or_default();
    let phrase = if phrases.is_empty() {
        Value::Null
    } else {
        use rand::RngExt;
        phrases[rand::rng().random_range(0..phrases.len())].clone()
    };
    let src_short = url::Url::parse(&src).ok().and_then(|u| u.host_str().map(|h| h.trim_start_matches('[').trim_end_matches(']').to_string()));
    let page_id = if has_slug { slug_val.clone() } else { json!(itemid) };

    let mut item_v = json!({
            "id": itemid,
            "slug": if has_slug { slug_val.clone() } else { Value::Null },
            "visibility": actitem.val("visibility"),
            "username": actitem.val("username"),
            "author_id": actitem.val("author_id"),
            "author_color": actitem.val("author_color"),
            "author_display_name": or_null(actitem.val("author_display_name")),
            "author_avatar": actitem.val("author_avatar"),
            "author_avatar_file": actitem.val("author_avatar_file"),
            "author_banner_file": actitem.val("author_banner_file"),
            "author_banner_position": actitem.val("author_banner_position"),
            "author_banner_size": actitem.val("author_banner_size"),
            "author_description": actitem.val("author_description"),
            "title": or_null(actitem.val("title")),
            "src": { "long": actitem.val("src"), "short": src_short },
            "thumbnail": format!("{thumbs}/{itemid}.webp"),
            "og_thumbnail": format!("{thumbs}/{itemid}{}.webp", if is_nsfw || is_nsfl { "_blur" } else { "" }),
            "og_url": og_url,
            "og_description": og_description,
            "coverart": coverart_url,
            "has_coverart": has_coverart,
            "is_audio": mime_now.starts_with("audio/"),
            "dest": dest_url,
            "mime": actitem.val("mime"),
            "size": crate::util::format_size(actitem.val("size")),
            "checksum": actitem.val("checksum"),
            "timestamp": { "timeago": timeago, "timefull": timefull },
            "favorites": favorites.into_iter().map(Value::Object).collect::<Vec<_>>(),
            "tags": effective_tags,
            "halls": item_halls.into_iter().map(Value::Object).collect::<Vec<_>>(),
            "user_halls": user_halls.into_iter().map(Value::Object).collect::<Vec<_>>(),
    });
    let item_b = json!({
            "is_nsfw": is_nsfw,
            "is_nsfl": is_nsfl,
            "is_sfw": is_sfw,
            "is_pinned": actitem.bool("is_pinned"),
            "is_comments_locked": actitem.bool("is_comments_locked"),
            "is_oc": actitem.bool("is_oc"),
            "is_repost": is_repost,
            "reposts": reposts,
            "show_repost_row": show_repost_row,
            "width": or_null(actitem.val("width")),
            "height": or_null(actitem.val("height")),
            "original_filename": or_null(actitem.val("original_filename")),
            "expires_at": or_null(actitem.val("expires_at")),
            "expires_in": crate::util::expires_in(actitem.val("expires_at")).filter(|_| crate::config::truthy(actitem.val("expires_at"))),
            "is_album": actitem.bool("is_album") && album.len() > 1,
            "album_count": album_count,
            "album": album,
            "album_json": album_json,
            "requested_subf0ck_slug": requested_sub,
    });
    if let (Value::Object(a), Value::Object(b)) = (&mut item_v, item_b) {
        a.extend(b);
    }
    Ok(json!({
        "success": true,
        "user": {
            "name": actitem.val("username"),
            "id": actitem.val("author_id"),
            "color": actitem.val("author_color"),
            "channel": channel,
            "network": actitem.val("usernetwork"),
        },
        "item": item_v,
        "title": format!("{} - {}", if has_slug { crate::tpl::js_to_string(&minijinja::Value::from_serialize(&slug_val)) } else { itemid.to_string() }, cfg.str_or("websrv.domain", "")),
        "pagination": {
            "end": pick(&end_item, json!(itemid)),
            "start": pick(&start_item, json!(itemid)),
            "next": pick(&next_item, Value::Null),
            "prev": pick(&prev_item, Value::Null),
            "page": page_id,
            "cheat": cheat,
        },
        "phrase": phrase,
        "link": link,
        "tmp": tmp,
    }))
}

fn slug_or_null(actitem: &Row) -> Value {
    if actitem.bool("slug") { actitem.val("slug").clone() } else { Value::Null }
}

struct AlbumCtx<'a> {
    st: &'a AppState,
    cfg: &'a Config,
    o: &'a FeedOpts<'a>,
    itemid: i64,
    tags: &'a [Value],
    /// Tag filter of the feed (not for title searches).
    tag: Option<&'a str>,
    strict_params: &'a [String],
    excluded_tags: &'a [i64],
    eff_mode: i64,
    safe: Option<Vec<String>>,
    has_coverart: bool,
    hall_slug: Option<&'a str>,
}

enum AlbumOutcome {
    NotVisible(Value),
    Done { album: Vec<Value>, effective_tags: Option<Vec<Value>> },
}

async fn build_album(c: &AlbumCtx<'_>, actitem: &mut Row, requested_sub: &mut Option<String>) -> anyhow::Result<AlbumOutcome> {
    let (st, cfg) = (c.st, c.cfg);
    let itemid = c.itemid;
    let mut album_rows = sql!(
        "SELECT id, dest, mime, size, width, height, order_index, slug, checksum FROM album_items WHERE item_id = ",
        itemid as i32,
        " ORDER BY order_index ASC"
    )
    .fetch_all(&st.db)
    .await?;
    if album_rows.is_empty() {
        return Ok(AlbumOutcome::Done { album: vec![], effective_tags: None });
    }
    let ids: Vec<i32> = album_rows.iter().filter_map(|r| r.i64("id")).map(|n| n as i32).collect();
    let mut q = Sql::new("SELECT aita.album_item_id, t.id, t.tag, t.normalized FROM album_items_tags_assign aita JOIN tags t ON t.id = aita.tag_id WHERE aita.album_item_id IN ");
    q.bind_list(&ids)
        .push(" ORDER BY (case when t.id = 1 then 0 when t.id = 2 then 1 when t.normalized = 'nsfl' then 2 else 3 end) asc, t.id asc");
    let sub_tag_rows = q.fetch_all(&st.db).await.unwrap_or_default();
    let mut tags_by_sub: std::collections::HashMap<i64, Vec<Value>> = std::collections::HashMap::new();
    for mut t in sub_tag_rows {
        let badge = crate::util::get_badge(cfg, t.str("tag").unwrap_or(""), t.str("normalized").unwrap_or(""));
        t.insert("badge".into(), json!(badge));
        tags_by_sub.entry(t.i64("album_item_id").unwrap_or(0)).or_default().push(Value::Object(t));
    }
    let empty: Vec<Value> = vec![];
    let tags_of = |r: &Row| -> &Vec<Value> { tags_by_sub.get(&r.i64("id").unwrap_or(0)).unwrap_or(&empty) };

    if let Some(tag) = c.tag {
        let terms = tag_terms(tag);
        if !terms.is_empty() && !tags_by_sub.is_empty() {
            let matching: Vec<Row> = album_rows.iter().filter(|r| sub_matches_tag(tags_of(r), &terms, c.strict_params)).cloned().collect();
            if !matching.is_empty() {
                album_rows = matching;
            }
        }
    }

    let parent_rating: Vec<Value> = c.tags.iter().filter(|t| is_rating_tag(cfg, t)).cloned().collect();
    let with_parent = |sub: &[Value]| -> Vec<Value> {
        if sub.iter().any(|t| is_rating_tag(cfg, t)) {
            sub.to_vec()
        } else {
            parent_rating.iter().chain(sub.iter()).cloned().collect()
        }
    };
    let vis = SubVisibility { cfg, session: c.o.session, eff_mode: c.eff_mode, safe_ratings: c.safe.clone(), excluded_tags: c.excluded_tags };
    album_rows.retain(|r| c.o.bypass_filter || vis.visible(&with_parent(tags_of(r))));

    let not_visible_v = || not_visible(cfg, itemid, Some(slug_or_null(actitem)), true, c.hall_slug);
    if album_rows.is_empty() {
        return Ok(AlbumOutcome::NotVisible(not_visible_v()));
    }

    let thumbs = cfg.str_or("websrv.paths.thumbnails", "");
    let images = cfg.str_or("websrv.paths.images", "");
    let coverarts = cfg.str_or("websrv.paths.coverarts", "");
    let mut album: Vec<Value> = Vec::with_capacity(album_rows.len());
    for (order, r) in album_rows.iter().enumerate() {
        let mime = r.string("mime");
        let dest = r.string("dest");
        let is_youtube = mime == "video/youtube";
        let base = strip_ext(&dest).to_string();
        let is_audio = mime.starts_with("audio/");
        let mut sub_cover: Option<String> = None;
        let mut sub_thumb: Option<String> =
            Some(if is_youtube { format!("{thumbs}/{}.webp", r.i64("id").unwrap_or(0)) } else { format!("{thumbs}/{base}.webp") });
        let yt_id = if is_youtube { dest.replacen("yt:", "", 1) } else { String::new() };

        if is_audio {
            let ca_file = cfg.paths.ca.join(format!("{base}.webp"));
            let t_file = cfg.paths.t.join(format!("{base}.webp"));
            let mut ca_exists = !base.contains('/') && !base.contains("..") && nonempty_file(&ca_file);
            if !ca_exists && order == 0 && c.has_coverart && nonempty_file(&cfg.paths.ca.join(format!("{itemid}.webp"))) {
                sub_cover = Some(format!("{coverarts}/{itemid}.webp"));
                sub_thumb = Some(format!("{thumbs}/{itemid}.webp"));
                ca_exists = true;
            }
            if !ca_exists && !base.contains('/') && !base.contains("..") {
                if let Some(audio) = crate::http::files::safe_join(&cfg.paths.b, &dest) {
                    if audio.exists() {
                        ca_exists = extract_coverart(&audio, &ca_file, &t_file).await;
                    }
                }
            }
            if ca_exists {
                if sub_cover.is_none() {
                    sub_cover = Some(format!("{coverarts}/{base}.webp"));
                }
                if !nonempty_file(&t_file) {
                    sub_thumb = sub_cover.clone();
                }
            } else {
                sub_cover = None;
                sub_thumb = None;
            }
        }

        let mut size = r.val("size").clone();
        if !crate::config::truthy(&size) || crate::config::as_f64(&size).map(|n| n <= 0.0).unwrap_or(true) {
            if let Some(p) = crate::http::files::safe_join(&cfg.paths.b, &dest) {
                if let Ok(m) = std::fs::metadata(p) {
                    size = json!(m.len());
                }
            }
        }
        let combined = with_parent(tags_of(r));
        let slug = r.val("slug").clone();
        let subf0ck_id = if crate::config::truthy(&slug) { slug.clone() } else { r.val("id").clone() };
        album.push(json!({
            "id": r.val("id"),
            "slug": slug,
            "subf0ck_id": subf0ck_id,
            "dest": if is_youtube { dest.clone() } else { format!("{images}/{dest}") },
            "src": if is_youtube { format!("https://www.youtube.com/embed/{yt_id}?enablejsapi=1") } else { format!("{images}/{dest}") },
            "filename": dest,
            "mime": r.val("mime"),
            "size": crate::util::format_size(&size),
            "width": r.val("width"),
            "height": r.val("height"),
            "checksum": r.val("checksum"),
            "order_index": order,
            "display_index": order + 1,
            "is_first": order == 0,
            "is_video": mime.starts_with("video/"),
            "is_youtube": is_youtube,
            "is_audio": is_audio,
            "is_image": mime.starts_with("image/"),
            "has_coverart": is_audio && sub_cover.is_some(),
            "coverart": if is_audio { json!(sub_cover) } else { Value::Null },
            "thumb": if is_youtube { json!(format!("https://img.youtube.com/vi/{yt_id}/mqdefault.jpg")) } else { json!(sub_thumb) },
            "tags": combined,
        }));
    }

    let mut req_idx = 0;
    if let Some(req) = requested_sub.as_deref() {
        let s = |v: &Value| crate::tpl::js_to_string(&minijinja::Value::from_serialize(v));
        let found = album.iter().position(|a| {
            let slug = if a["slug"].is_null() { String::new() } else { s(&a["slug"]) };
            let sid = if a["subf0ck_id"].is_null() { String::new() } else { s(&a["subf0ck_id"]) };
            slug == req || sid == req || s(&a["id"]) == req
        });
        match found {
            Some(i) => req_idx = i,
            None => return Ok(AlbumOutcome::NotVisible(not_visible_v())),
        }
    }

    let mut effective_tags = None;
    if let Some(target) = album.get(req_idx).cloned() {
        for (i, a) in album.iter_mut().enumerate() {
            a["is_first"] = json!(i == req_idx);
        }
        *requested_sub = if requested_sub.is_some() || c.tag.is_some() {
            let v = if crate::config::truthy(&target["slug"]) { &target["slug"] } else { &target["id"] };
            Some(crate::tpl::js_to_string(&minijinja::Value::from_serialize(v)))
        } else {
            None
        };
        let filename = target["filename"].as_str().filter(|f| !f.is_empty()).map(str::to_string).unwrap_or_else(|| {
            target["dest"].as_str().unwrap_or("").trim_start_matches("/b/").to_string()
        });
        actitem.insert("dest".into(), json!(filename));
        actitem.insert("mime".into(), target["mime"].clone());
        actitem.insert("width".into(), target["width"].clone());
        actitem.insert("height".into(), target["height"].clone());
        if crate::config::truthy(&target["size"]) {
            actitem.insert("size".into(), target["size"].clone());
        }
        if crate::config::truthy(&target["coverart"]) {
            actitem.insert("coverart".into(), target["coverart"].clone());
        }
        let ttags = target["tags"].as_array().cloned().unwrap_or_default();
        effective_tags = Some(if !ttags.is_empty() {
            with_parent(&ttags)
        } else if req_idx == 0 {
            c.tags.to_vec()
        } else {
            parent_rating.clone()
        });
    }
    Ok(AlbumOutcome::Done { album, effective_tags })
}
