//! Abyss (doomscroll) page and its JSON feed — port of src/inc/routes/scroller.mjs.

use crate::db::{Row, RowExt, Sql};
use crate::http::{Req, Res, Router, html, html_code, json_resp, redirect, reply};
use crate::session::Session;
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Map, Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/scroller(?:\/(?<id>[a-zA-Z0-9_\/-]+))?\/?$", scroller_redirect);
    r.get(r"^\/abyss(?:\/(?<id>[a-zA-Z0-9_\/-]+))?\/?$", abyss);
    r.get(r"^\/api\/v2\/scroller\/meta\/?$", meta);
    r.get(r"^\/api\/v2\/scroller\/tags\/?$", tag_suggest);
    r.get(r"^\/api\/v2\/scroller\/feed\/?$", feed);
}

const SWF_MIMES: [&str; 2] = ["application/x-shockwave-flash", "application/vnd.adobe.flash.movie"];

fn abyss_disabled(st: &AppState) -> bool {
    st.cfg().get("websrv.abyss_enabled") == &Value::Bool(false)
}

fn private_locked(st: &AppState, req: &Req) -> bool {
    st.cfg().truthy("websrv.private_society") && req.session().is_none()
}

/// JSON answer (`application/json`, not cached).
fn json_no_store(v: &Value, code: u16) -> Response {
    let mut r = json_resp(v, code);
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache"));
    r
}

async fn scroller_redirect(_st: AppState, req: Req) -> Res {
    Ok(match req.param("id").filter(|i| !i.is_empty()) {
        Some(id) => redirect(&format!("/abyss/{id}")),
        None => redirect("/abyss"),
    })
}

/// The filter-pill flags the template's old `@js` block computed.
fn scroller_flags(cfg: &crate::config::Config, session: Option<&Session>) -> Map<String, Value> {
    let member = session.map(|s| !s.user.is_empty() && !s.is_anon).unwrap_or(false);
    let anon = session.map(|s| s.is_anon || s.user == "anonymous" || s.user.starts_with("anon_")).unwrap_or(false);
    let guest = !member && !anon;
    let anon_user = !member;
    let perms = s::anon_permissions(cfg);
    let list = |k: &str, dflt: &[&str]| -> Vec<String> {
        match perms.get(k) {
            Some(v) if crate::config::truthy(v) => v.as_array().map(|a| a.iter().map(|x| crate::tpl::js_to_string(&minijinja::Value::from_serialize(x))).collect()).unwrap_or_default(),
            _ => dflt.iter().map(|s| s.to_string()).collect(),
        }
    };
    let allowed_modes = if guest { vec!["sfw".to_string()] } else { list("allowed_modes", &["sfw", "nsfw", "untagged", "all", "nsfl"]) };
    let has = |m: &str| allowed_modes.iter().any(|x| x == m);
    let enable_nsfl = cfg.enable_nsfl();
    let show_sfw = true;
    let show_nsfw = guest || !anon_user || has("nsfw");
    let show_nsfl = enable_nsfl && (guest || !anon_user || has("nsfl"));
    let show_all = guest || !anon_user || has("all");
    let show_untagged = guest || session.map(|s| s.admin || s.is_moderator).unwrap_or(false) || (anon_user && has("untagged"));
    let active: Vec<&str> = if guest {
        vec!["sfw"]
    } else {
        [(show_sfw, "sfw"), (show_nsfw, "nsfw"), (show_nsfl, "nsfl"), (show_untagged, "untagged")].iter().filter(|(b, _)| *b).map(|(_, m)| *m).collect()
    };
    let single_mode = guest || (anon_user && active.len() == 1 && !show_all);
    let single_mode_val = if guest {
        json!("sfw")
    } else if single_mode {
        active.first().map(|m| json!(m)).unwrap_or(Value::Null)
    } else {
        Value::Null
    };
    let allowed_mimes = list("allowed_mimes", &["image", "video", "audio", "flash", "pdf"]);
    let single_mime = anon_user && allowed_mimes.len() == 1;
    let mut m = Map::new();
    m.insert("_isScrollerGuest".into(), json!(guest));
    m.insert("_scrollerShowSfw".into(), json!(show_sfw));
    m.insert("_scrollerShowNsfw".into(), json!(show_nsfw));
    m.insert("_scrollerShowNsfl".into(), json!(show_nsfl));
    m.insert("_scrollerShowAll".into(), json!(show_all));
    m.insert("_scrollerShowUntagged".into(), json!(show_untagged));
    m.insert("_scrollerIsSingleMode".into(), json!(single_mode));
    m.insert("_scrollerSingleMode".into(), single_mode_val);
    m.insert("_scrollerIsSingleMime".into(), json!(single_mime));
    m.insert("_scrollerSingleMime".into(), if single_mime { json!(allowed_mimes[0]) } else { Value::Null });
    m
}

async fn abyss(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if abyss_disabled(&st) {
        return Ok(html_code(crate::tpl::render(&st, Some(&req), "error", json!({ "message": "Not found", "tmp": null })), 404));
    }
    if private_locked(&st, &req) {
        return Ok(reply(502, "text/html", "<html><body>502 Bad Gateway</body></html>"));
    }
    let domain = cfg.str_or("main.url.domain", "").to_string();
    let id = req.param("id").filter(|i| !i.is_empty()).map(str::to_string);
    let mut page_meta = json!({
        "title": "doomscroll",
        "description": "Scroll through content endlessly",
        "url": format!("https://{domain}/abyss"),
    });
    if let Some(id) = id.as_deref() {
        let tid = id.trim();
        if !tid.is_empty() && tid.bytes().all(|b| b.is_ascii_digit()) {
            if let Ok(n) = tid.parse::<i32>() {
                // Only public items get a preview (the old query also matched private uploads).
                let found = sql!("select i.id from items i where i.id = ", n, " and i.active = true and coalesce(i.visibility, 0) = 0 limit 1")
                    .fetch_optional(&st.db)
                    .await;
                match found {
                    Ok(Some(_)) => {
                        let tag_ids: Vec<i32> = sql!("select tag_id from tags_assign where item_id = ", n).fetch_scalar_list(&st.db).await.unwrap_or_default();
                        let nsfl = match cfg.int("nsfl_tag_id") {
                            Some(v) if v != 0 => v as i32,
                            _ => 3,
                        };
                        let blurred = tag_ids.contains(&2) || tag_ids.contains(&nsfl);
                        page_meta["title"] = json!(id);
                        page_meta["description"] = json!(cfg.str_or("websrv.description", "The webs dumpster"));
                        page_meta["url"] = json!(format!("https://{domain}/abyss/{id}"));
                        page_meta["image"] = json!(format!("https://{domain}/t/{id}{}.webp", if blurred { "_blur" } else { "" }));
                    }
                    Ok(None) => {}
                    Err(e) => tracing::error!("[SCROLLER] Failed to fetch meta for ID: {id} {e}"),
                }
            }
        } else if let Some((board, thread)) = chan_ref(tid) {
            page_meta["title"] = json!(format!("/{board}/{thread} — Abyss"));
            page_meta["description"] = json!(format!("Watch /{board}/ thread {thread} in Abyss"));
            page_meta["url"] = json!(format!("https://{domain}/abyss/{board}/{thread}"));
        }
    }
    let mut data = json!({
        "tmp": null,
        "session": req.session_value(),
        "enable_nsfl": cfg.enable_nsfl(),
        "enable_swf": cfg.truthy("websrv.enable_swf"),
        "page_meta": page_meta,
    });
    if let Value::Object(m) = &mut data {
        m.extend(scroller_flags(&cfg, req.session()));
    }
    Ok(html(crate::tpl::render(&st, Some(&req), "scroller", data)))
}

/// `^([a-z0-9]+)\/(\d+)$` (case-insensitive)
fn chan_ref(id: &str) -> Option<(&str, &str)> {
    let (board, thread) = id.split_once('/')?;
    let ok = !board.is_empty() && board.bytes().all(|b| b.is_ascii_alphanumeric()) && !thread.is_empty() && thread.bytes().all(|b| b.is_ascii_digit());
    ok.then_some((board, thread))
}

/// `?ids=1,2,3` / `?exclude=…` → positive integers (parseInt semantics), capped.
fn id_list(raw: &str, cap: usize) -> Vec<i32> {
    raw.split(',')
        .filter_map(|n| crate::social::helpers::js_parse_int(n))
        .filter(|&n| n > 0 && n <= i32::MAX as i64)
        .map(|n| n as i32)
        .take(cap)
        .collect()
}

/// Rows hidden from this viewer: private uploads of others (moderators see everything).
fn visibility_sql(session: Option<&Session>) -> Sql {
    let mut q = Sql::empty();
    match session {
        Some(s) if s.admin || s.is_moderator => {}
        Some(s) if !s.user.is_empty() => {
            q.push(" AND (COALESCE(items.visibility, 0) = 0 OR LOWER(items.username) = ").bind(s.user.to_lowercase()).push(") ");
        }
        _ => {
            q.push(" AND COALESCE(items.visibility, 0) = 0 ");
        }
    }
    q
}

async fn meta(st: AppState, req: Req) -> Res {
    if abyss_disabled(&st) {
        return Ok(json_no_store(&json!({ "success": false }), 404));
    }
    if private_locked(&st, &req) {
        return Ok(json_no_store(&json!({}), 502));
    }
    let ids = id_list(req.query("ids").unwrap_or(""), 50);
    if ids.is_empty() {
        return Ok(json_no_store(&json!({}), 200));
    }
    let sid = req.session().map(|s| s.id as i32).filter(|&i| i != 0);
    let mut q = Sql::new(
        "SELECT items.id,
          (SELECT string_agg(t.tag, ', ' ORDER BY ta2.tag_id) FROM tags_assign ta2 JOIN tags t ON t.id = ta2.tag_id
           WHERE ta2.item_id = items.id AND ta2.tag_id > 2 LIMIT 5) AS tag_list,
          (SELECT COUNT(*) FROM favorites WHERE favorites.item_id = items.id) AS fav_count,
          (SELECT COUNT(*) FROM comments WHERE comments.item_id = items.id AND comments.is_deleted = false) AS comment_count, ",
    );
    match sid {
        Some(uid) => q.push("EXISTS (SELECT 1 FROM favorites WHERE favorites.item_id = items.id AND favorites.user_id = ").bind(uid).push(")"),
        None => q.push("false"),
    };
    // Only items the feed could have shown (the old endpoint answered for any id, including
    // deleted and private uploads).
    q.push(" AS is_faved FROM items WHERE items.id = ANY(").bind(ids).push("::int[]) AND items.active = true ");
    q.append(&visibility_sql(req.session()));
    let rows = match q.fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[SCROLLER] meta error: {e}");
            return Ok(json_resp(&json!({}), 500));
        }
    };
    let mut out = Map::new();
    for r in rows {
        out.insert(
            r.string("id"),
            json!({
                "tags": r.str("tag_list").unwrap_or(""),
                "fav_count": r.i64("fav_count").unwrap_or(0),
                "comment_count": r.i64("comment_count").unwrap_or(0),
                "is_faved": r.bool("is_faved"),
            }),
        );
    }
    Ok(json_no_store(&Value::Object(out), 200))
}

async fn tag_suggest(st: AppState, req: Req) -> Res {
    if abyss_disabled(&st) {
        return Ok(json_resp(&json!([]), 404));
    }
    if private_locked(&st, &req) {
        return Ok(json_resp(&json!([]), 502));
    }
    let q = req.query("q").unwrap_or("").trim().to_string();
    if q.is_empty() {
        return Ok(json_resp(&json!([]), 200));
    }
    let slug = format!("%{}%", crate::util::slugify(&q));
    let rows = sql!(
        "SELECT t.tag, t.normalized, COUNT(DISTINCT items.id) as uses
        FROM tags t
        JOIN tags_assign ta ON ta.tag_id = t.id
        JOIN items ON items.id = ta.item_id
        WHERE t.id > 2
          AND items.active = true
          AND items.is_deleted = false
          AND lower(t.normalized) ILIKE ",
        slug,
        " GROUP BY t.tag, t.normalized ORDER BY uses DESC LIMIT 10"
    )
    .fetch_all(&st.db)
    .await
    .unwrap_or_default();
    let out: Vec<Value> = rows
        .iter()
        .map(|r| json!({ "tag": r.val("tag"), "normalized": r.val("normalized"), "uses": r.i64("uses").unwrap_or(0) }))
        .collect();
    Ok(json_resp(&Value::Array(out), 200))
}

/// `SELECT` columns of a feed row.
fn select_cols(nsfl: i64, sid: Option<i32>) -> Sql {
    let mut q = Sql::new(&format!(
        "items.id, items.mime, items.dest, items.username, items.stamp, items.src, items.is_oc,
      uo.display_name, uo.avatar, uo.avatar_file, uo.username_color,
      (SELECT string_agg(t.tag, ', ' ORDER BY ta2.tag_id) FROM tags_assign ta2 JOIN tags t ON t.id = ta2.tag_id
        WHERE ta2.item_id = items.id AND ta2.tag_id > 2 LIMIT 5) AS tag_list,
      (SELECT ta3.tag_id FROM tags_assign ta3 WHERE ta3.item_id = items.id AND ta3.tag_id IN (1,2,{nsfl}) ORDER BY ta3.tag_id LIMIT 1) AS rating_tag_id,
      (SELECT COUNT(*) FROM favorites WHERE favorites.item_id = items.id) AS fav_count,
      (SELECT COUNT(*) FROM comments WHERE comments.item_id = items.id AND comments.is_deleted = false) AS comment_count, "
    ));
    match sid {
        Some(uid) => q.push("EXISTS (SELECT 1 FROM favorites WHERE favorites.item_id = items.id AND favorites.user_id = ").bind(uid).push(")"),
        None => q.push("false"),
    };
    q.push(" AS is_faved ");
    q
}

fn yt_id(src: &str) -> Option<String> {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?/?\?(?:\S*?&?v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").expect("yt regex")
    });
    RE.captures(src).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
}

async fn feed(st: AppState, req: Req) -> Res {
    if abyss_disabled(&st) {
        return Ok(json_resp(&json!({ "success": false, "items": [] }), 404));
    }
    if private_locked(&st, &req) {
        return Ok(json_resp(&json!({ "success": false, "items": [] }), 502));
    }
    match feed_inner(&st, &req).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[SCROLLER] Feed error: {e}");
            Ok(json_resp(&json!({ "success": false, "items": [], "error": "Feed error" }), 500))
        }
    }
}

async fn feed_inner(st: &AppState, req: &Req) -> anyhow::Result<Response> {
    use crate::social::helpers::{js_number, js_parse_int};
    let cfg = st.cfg();
    let session = req.session();
    let num = |k: &str| req.query(k).and_then(js_number).filter(|f| f.is_finite());
    let mode = match req.query("mode") {
        Some(m) => js_number(m).filter(|f| f.is_finite()).map(|f| f as i64).unwrap_or(0),
        None => req.mode,
    };
    // `Math.min(+qs.limit || 12, 30)`; values below 1 made the old query fail
    let limit = num("limit").filter(|&f| f != 0.0).map(|f| f.trunc() as i64).unwrap_or(12).clamp(1, 30);
    let after = num("after").filter(|&f| f != 0.0).map(|f| f.trunc() as i64);
    let mime = req.query("mime").filter(|m| !m.is_empty());
    let tag_filter = req.query("tag").filter(|t| !t.is_empty()).map(|t| t.trim().to_string());
    let orderby = match req.query("orderby") {
        Some("newest") => "newest",
        Some("oldest") => "oldest",
        _ => "random",
    };
    let excluded: Vec<i32> = session.map(|s| s.excluded_tags.iter().map(|&t| t as i32).collect()).unwrap_or_default();
    let exclude_ids = req.query("exclude").filter(|e| !e.is_empty()).map(|e| id_list(e, 500)).unwrap_or_default();
    let modequery = crate::f0cklib::compute_base_mode(&cfg, Some(mode), None, session);
    let nsfp: Vec<i32> = st.settings.nsfp_ids().into_iter().map(|i| i as i32).collect();
    let anchor = req.query("anchor").and_then(js_parse_int).filter(|&a| a != 0).and_then(|a| i32::try_from(a).ok());

    let mimes = cfg.mimes();
    let archive_mimes: Vec<String> = mimes
        .iter()
        .filter(|(m, ext)| m.starts_with("application/") && !matches!(ext.as_str(), Some("swf" | "pdf")))
        .map(|(m, _)| m.clone())
        .collect();
    let mut common = Sql::empty();
    if !cfg.truthy("websrv.enable_swf") {
        common.push(" AND items.mime != ALL(").bind(SWF_MIMES.iter().map(|s| s.to_string()).collect::<Vec<_>>()).push(") ");
    }
    common.push(" AND items.mime != 'application/pdf' AND items.mime != ALL(").bind(archive_mimes.clone()).push(") ");
    let mut nsfp_sql = Sql::empty();
    if (session.is_none() || s::is_anon_session(session)) && !nsfp.is_empty() {
        nsfp_sql.push(" AND NOT EXISTS (SELECT 1 FROM tags_assign WHERE item_id = items.id AND tag_id = ANY(").bind(nsfp).push(")) ");
    }

    let mut mime_sql = Sql::empty();
    let parts: Vec<&str> = mime.unwrap_or("").split(',').filter(|m| matches!(*m, "video" | "audio" | "image")).collect();
    if !parts.is_empty() {
        mime_sql.push(" AND (");
        for (i, m) in parts.iter().enumerate() {
            if i > 0 {
                mime_sql.push(" OR ");
            }
            mime_sql.push("items.mime ilike ").bind(format!("{m}/%"));
        }
        mime_sql.push(") ");
    }
    let mut tag_sql = Sql::empty();
    if let Some(tf) = &tag_filter {
        let terms: Vec<String> = tf.split(',').map(str::trim).filter(|t| !t.is_empty()).map(|t| format!("%{}%", crate::util::slugify(t))).collect();
        if !terms.is_empty() {
            tag_sql.push(" AND items.id IN (SELECT ta.item_id FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ");
            for (i, s) in terms.into_iter().enumerate() {
                if i > 0 {
                    tag_sql.push(" OR ");
                }
                tag_sql.push("lower(t.normalized) ILIKE ").bind(s);
            }
            tag_sql.push(") ");
        }
    }
    let mut cursor = Sql::empty();
    if let Some(a) = after.and_then(|a| i32::try_from(a).ok()) {
        match orderby {
            "newest" => {
                cursor.push(" AND items.id < ").bind(a);
            }
            "oldest" => {
                cursor.push(" AND items.id > ").bind(a);
            }
            _ => {}
        }
    }
    let mut exclude_sql = Sql::empty();
    if orderby == "random" && !exclude_ids.is_empty() {
        exclude_sql.push(" AND items.id != ALL(").bind(exclude_ids).push("::int[]) ");
    }
    let mut excl_tags = Sql::empty();
    if !excluded.is_empty() {
        excl_tags.push(" AND NOT EXISTS (SELECT 1 FROM tags_assign WHERE item_id = items.id AND tag_id = ANY(").bind(excluded).push("::int[])) ");
    }
    let order = match orderby {
        "newest" => " ORDER BY items.id DESC ",
        "oldest" => " ORDER BY items.id ASC ",
        _ => " ORDER BY random() ",
    };
    let nsfl = match cfg.int("nsfl_tag_id") {
        Some(v) if v != 0 => v,
        _ => 3,
    };
    let sid = session.map(|s| s.id as i32).filter(|&i| i != 0);
    let vis = visibility_sql(session);
    const FROM: &str = r#" FROM items LEFT JOIN "user" author_u ON author_u.user = items.username LEFT JOIN user_options uo ON uo.user_id = author_u.id "#;

    let rows: Vec<Row> = if let Some(anchor) = anchor {
        let mut q = Sql::new("SELECT ");
        q.append(&select_cols(nsfl, sid)).push(FROM).push(" WHERE items.id = ").bind(anchor).push(" AND items.active = true AND ").push(&modequery);
        // The anchor is a deep link: it must respect visibility like every other row.
        q.append(&vis).append(&common).append(&nsfp_sql);
        let anchor_rows = q.fetch_all(&st.db).await?;
        if anchor_rows.is_empty() {
            Vec::new()
        } else {
            let mut q = Sql::new("SELECT ");
            q.append(&select_cols(nsfl, sid)).push(FROM).push(" WHERE ").push(&modequery).push(" AND items.active = true ");
            q.append(&vis).append(&common).push(" AND items.id != ").bind(anchor).push(" ");
            q.append(&exclude_sql).append(&mime_sql).append(&tag_sql).append(&nsfp_sql).append(&excl_tags);
            q.push(" ORDER BY random() LIMIT ").bind(limit - 1);
            let rest = q.fetch_all(&st.db).await?;
            anchor_rows.into_iter().chain(rest).collect()
        }
    } else {
        let mut q = Sql::new("SELECT ");
        q.append(&select_cols(nsfl, sid)).push(FROM).push(" WHERE ").push(&modequery).push(" AND items.active = true ");
        q.append(&vis).append(&common).append(&cursor).append(&exclude_sql).append(&mime_sql).append(&tag_sql).append(&nsfp_sql).append(&excl_tags);
        q.push(order).push(" LIMIT ").bind(limit);
        q.fetch_all(&st.db).await?
    };

    let images = cfg.str_or("websrv.paths.images", "").to_string();
    let thumbs = cfg.str_or("websrv.paths.thumbnails", "").to_string();
    let tr = st.i18n.translator(&super::index::req_lang(st, req));
    let now = crate::util::now_secs();
    let anonymized = s::is_anonymize_session(&cfg, session);
    let mut items: Vec<Value> = Vec::with_capacity(rows.len());
    for row in &rows {
        let mime = row.str("mime").unwrap_or("");
        let is_youtube = mime == "video/youtube";
        let is_video = mime.starts_with("video") && !is_youtube;
        let mut dest = row.val("dest").clone();
        if is_youtube {
            let ok = row.str("dest").map(|d| d.starts_with("yt:")).unwrap_or(false);
            if !ok {
                if let Some(id) = row.str("src").and_then(yt_id) {
                    dest = json!(format!("yt:{id}"));
                }
            }
        } else if let Some(d) = row.str("dest").filter(|d| !d.is_empty()) {
            dest = json!(format!("{images}/{d}"));
        }
        let id = row.val("id").clone();
        let id_s = row.string("id");
        let rating = row.i64("rating_tag_id");
        let (label, class) = match rating {
            Some(1) => ("SFW", "sfw"),
            Some(2) => ("NSFW", "nsfw"),
            Some(n) if n == nsfl => ("NSFL", "nsfl"),
            _ => ("?", "untagged"),
        };
        let avatar = if let Some(f) = row.str("avatar_file").filter(|f| !f.is_empty()) {
            format!("/a/{f}")
        } else if row.bool("avatar") {
            format!("/t/{}.webp", row.string("avatar"))
        } else {
            "/a/default.png".into()
        };
        let stamp = row.i64("stamp").unwrap_or(0);
        let src_host = row
            .str("src")
            .filter(|s| !s.is_empty())
            .map(|s| url::Url::parse(s).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default())
            .unwrap_or_default();
        let username = row.val("username").clone();
        let display = if row.bool("display_name") { row.val("display_name").clone() } else { username.clone() };
        let color = if row.bool("username_color") { row.val("username_color").clone() } else { Value::Null };
        let mut item = json!({
            "id": id,
            "mime": row.val("mime"),
            "dest": dest,
            "thumbnail": format!("{thumbs}/{id_s}.webp"),
            "username": username,
            "display_name": display,
            "avatar": avatar,
            "username_color": color,
            "stamp": row.val("stamp"),
            "timeago": crate::util::time_ago(&tr, now - stamp),
            "tags": row.str("tag_list").unwrap_or(""),
            "is_oc": row.bool("is_oc"),
            "is_faved": row.bool("is_faved"),
            "fav_count": row.i64("fav_count").unwrap_or(0),
            "comment_count": row.i64("comment_count").unwrap_or(0),
            "is_swf": SWF_MIMES.contains(&mime),
            "is_archive": !mime.is_empty() && archive_mimes.iter().any(|m| m == mime),
            "is_video": is_video,
            "is_youtube": is_youtube,
            "is_audio": mime.starts_with("audio"),
            "is_image": mime.starts_with("image"),
            "rating_label": label,
            "rating_class": class,
            "src_host": src_host,
        });
        if anonymized {
            item["username"] = json!("anonymous");
            item["display_name"] = json!("anonymous");
            item["avatar"] = json!("/a/default.png");
            item["username_color"] = Value::Null;
            item["src_host"] = json!("");
        }
        items.push(item);
    }
    let next_cursor = items.last().map(|i| i["id"].clone()).unwrap_or(Value::Null);
    Ok(json_no_store(&json!({ "success": true, "items": items, "nextCursor": next_cursor }), 200))
}
