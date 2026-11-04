//! The main JSON API (port of src/inc/routes/apiv2/index.mjs): feeds, random, recommendations,
//! suggestions, item reads (here), URL metadata (`index/meta.rs`) and item mutations
//! (`index/items.rs`).
//!
//! `GET /api/v2/scroller/meta` of the old file is not registered: it was written as
//! `group.get(/^\/scroller.../)`, which flummpress joined to `^\/api\/v2^\/scroller...` — a pattern
//! that can never match. The live route is in scroller.mjs.

mod items;
mod meta;

use crate::db::{Row, RowExt, Sql};
use crate::f0cklib::recommend::{self, RecOpts, TagFeedOpts, jnum};
use crate::f0cklib::{self, FeedOpts};
use crate::http::{Group, Guard, Req, Res, Router, json_resp, raw};
use crate::session::Session;
use crate::settings as s;
use crate::social::helpers::{js_number, js_parse_float, js_parse_int, js_str, mode_override, ratings_from_cookie};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Map, Value, json};

pub fn register(r: &mut Router) {
    let mut g = Group::new(r, r"^\/api\/v2");
    g.get_g(r"\/meta\/extract\/item\/(?<id>\d+)$", Guard::LoggedIn, meta::extract_item);
    g.post_g(r"\/meta\/cache$", Guard::LoggedIn, meta::cache);
    g.get(r"\/audio-metadata$", meta::audio_metadata);
    g.get(r"\/meta\/fetch$", meta::fetch);
    g.get(r"$", api_root);
    g.get(r"\/item-page$", item_page);
    g.get(r"\/random(\/user\/.+|\/image|\/video|\/audio)?$", random);
    g.get(r"\/random-pool", random_pool);
    g.get(r"\/recommendations(?:\/(?<type>videos|all))?$", recommendations);
    g.post(r"\/track\/interaction$", track_interaction);
    g.post(r"\/track\/search$", track_search);
    g.get(r"\/favs-feed$", favs_feed);
    g.get(r"\/tag-feed$", tag_feed);
    g.get(r"\/orakel\/user$", orakel_user);
    g.get(r"\/items\/get", items_get);
    g.get(r"\/item\/(?<id>[a-zA-Z0-9_-]{11}|\d+)$", item);
    g.get(r"\/user\/(?<user>[^\/]+)(\/(?<eps>\d+))?$", user_items);
    g.get(r"\/tags\/suggest$", tags_suggest);
    g.get(r"\/users\/suggest$", users_suggest);
    g.get(r"\/items\/suggest$", items_suggest);
    g.put_g(r"\/tags\/rename\/(?<tagname>.*)", Guard::ModAuth, items::tags_rename);
    g.patch_g(r"\/items\/(?<id>\d+)\/title$", Guard::RegisteredUser, items::set_title);
    g.post_g(r"\/admin\/deletepost$", Guard::ModAuth, items::delete_post);
    g.post_g(r"\/admin\/delete-album-item$", Guard::LoggedIn, items::delete_album_item);
    g.post_g(r"\/togglefav$", Guard::LoggedIn, items::toggle_fav);
    g.post_g(r"\/favorites\/import$", Guard::LoggedIn, items::favorites_import);
    g.post_g(r"\/toggle-oc$", Guard::RegisteredUser, items::toggle_oc);
    g.post_g(r"\/item\/visibility$", Guard::RegisteredUser, items::set_visibility);
    g.post_g(r"\/items\/bulk-visibility$", Guard::LoggedIn, items::bulk_visibility);
    g.post_g(r"\/items\/(?<id>[0-9]+)\/rethumb$", Guard::RegisteredUser, items::rethumb);
    g.post_g(r"\/items\/(?<id>[0-9]+)\/expiry$", Guard::RegisteredUser, items::set_expiry);
    // ratingAuth (session required; anonymous sessions need the rate_item permission) is checked
    // in the handler.
    g.route(Method::POST, r"\/item\/(?<id>[0-9]+)\/rating$", None, items::rating);
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

/// `req.url.qs.x || null`
fn qs<'a>(req: &'a Req, k: &str) -> Option<&'a str> {
    req.query(k).filter(|v| !v.is_empty())
}

fn qs_string(req: &Req, k: &str) -> Option<String> {
    qs(req, k).map(str::to_string)
}

/// `req.cookies?.mime !== undefined ? (decodeURIComponent(mime).trim() || null) : null`
fn cookie_mime(req: &Req) -> Option<String> {
    req.cookie("mime").map(|m| crate::util::decode_lossy(m).trim().to_string()).filter(|m| !m.is_empty())
}

/// `typeof qs.mime !== 'undefined' ? (qs.mime || null) : (cookieMime || fallback)`
fn effective_mime(req: &Req, fallback: Option<&str>) -> Option<String> {
    match req.query("mime") {
        Some(m) => Some(m.to_string()).filter(|m| !m.is_empty()),
        None => cookie_mime(req).or_else(|| fallback.map(str::to_string)),
    }
}

/// Valid `ratings` cookie values, `None` when missing or empty.
fn ratings(req: &Req) -> Option<Vec<String>> {
    ratings_from_cookie(req).filter(|r| !r.is_empty())
}

/// `+(qs.x || dflt)` as a float (NaN for garbage, like JS).
fn qs_num(req: &Req, k: &str, dflt: f64) -> f64 {
    match qs(req, k) {
        Some(v) => js_number(v).unwrap_or(f64::NAN),
        None => dflt,
    }
}

fn excluded(session: Option<&Session>) -> Vec<i64> {
    session.map(|s| s.excluded_tags.clone()).unwrap_or_default()
}

fn anonymize(st: &AppState, req: &Req) -> bool {
    s::is_anonymize_session(&st.cfg(), req.session())
}

/// The options shared by item-page / random / random-pool.
fn feed_opts(req: &Req, mime: Option<String>) -> FeedOpts<'_> {
    let session = req.session();
    FeedOpts {
        user: qs_string(req, "user"),
        tag: qs_string(req, "tag"),
        hall: qs_string(req, "hall"),
        user_hall: qs_string(req, "userHall"),
        user_hall_owner: qs_string(req, "userHallOwner"),
        mime,
        fav: req.query("fav") == Some("true"),
        mode: Some(req.mode),
        ratings: ratings(req),
        strict: req.query("strict") == Some("1"),
        session,
        exclude: excluded(session),
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin),
        ..Default::default()
    }
}

fn is_staff(session: Option<&Session>) -> bool {
    session.map(|s| s.admin || s.is_moderator).unwrap_or(false)
}

// ── GET /api/v2 ─────────────────────────────────────────────────────────────

async fn api_root(_st: AppState, _req: Req) -> Res {
    Ok(raw(200, "api lol"))
}

// ── GET /api/v2/item-page ───────────────────────────────────────────────────

async fn item_page(st: AppState, req: Req) -> Res {
    let Some(raw_id) = qs(&req, "id").or_else(|| qs(&req, "slug")).or_else(|| qs(&req, "item")).map(str::to_string) else {
        return j(json!({ "success": false, "page": 1 }), 200);
    };
    let mut o = feed_opts(&req, effective_mime(&req, None));
    o.target_item_id = Some(raw_id);
    o.tagger = qs_string(&req, "tagger");
    match f0cklib::get_item_page(&st, &o).await {
        Ok(page) => j(json!({ "success": true, "page": page }), 200),
        Err(e) => {
            tracing::debug!("[ITEM-PAGE] {e}");
            j(json!({ "success": false, "page": 1 }), 200)
        }
    }
}

// ── GET /api/v2/random ──────────────────────────────────────────────────────

static YT_SRC_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?/?\?(?:\S*?&v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").unwrap()
});

async fn random(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let parts: Vec<&str> = req.path.split('/').collect();
    let p4 = parts.get(4).copied();
    let path_user = if p4 == Some("user") { parts.get(5).map(|u| u.to_string()) } else { None };
    let user = qs_string(&req, "user").or(path_user).filter(|u| !u.is_empty());
    let path_mime = p4.filter(|m| ["audio", "image", "video", "%"].contains(m));
    let mut o = feed_opts(&req, effective_mime(&req, path_mime));
    o.user = user;
    let data = f0cklib::get_random(&st, &o).await?;
    let itemid = data.get("itemid").filter(|v| crate::config::truthy(v)).and_then(Value::as_i64);
    let none = || j(json!({ "success": false, "items": [] }), 200);
    let Some(itemid) = itemid else { return none() };
    let Some(item) = sql!(r#"SELECT * FROM "items" WHERE id = "#, itemid as i32, " AND active = true LIMIT 1").fetch_optional(&st.db).await? else {
        return none();
    };
    let is_yt = item.str("mime") == Some("video/youtube");
    let mut yt_dest = item.string("dest");
    if is_yt && !yt_dest.starts_with("yt:") {
        if let Some(c) = YT_SRC_RE.captures(item.str("src").unwrap_or("")) {
            yt_dest = format!("yt:{}", &c[1]);
        }
    }
    let images = cfg.str_or("websrv.paths.images", "");
    let (relative, direct) = if is_yt {
        (yt_dest.clone(), yt_dest)
    } else {
        let d = item.string("dest");
        (format!("{images}/{d}"), format!("{}{images}/{d}", cfg.url_full()))
    };
    let slug = if s::enable_item_slugs(&cfg) && crate::config::truthy(item.val("slug")) { item.val("slug").clone() } else { Value::Null };
    let mut out: Row = item;
    out.shift_remove("username");
    out.shift_remove("src");

    out.insert("slug".into(), slug);
    out.insert("dest".into(), json!(relative));
    out.insert("url".into(), json!(direct));
    out.insert("direct_url".into(), json!(direct));
    out.insert("page".into(), json!(1));
    j(json!({ "success": true, "items": out }), 200)
}

// ── GET /api/v2/random-pool ─────────────────────────────────────────────────

async fn random_pool(st: AppState, req: Req) -> Res {
    let o = feed_opts(&req, effective_mime(&req, None));
    let t = std::time::Instant::now();
    let mut pool = f0cklib::get_random_pool(&st, &o).await?;
    tracing::info!(
        "[RANDOM-POOL] {} items in {}ms (mode={} tag={:?} hall={:?} user={:?})",
        pool.get("total").cloned().unwrap_or(serde_json::Value::Null),
        t.elapsed().as_millis(),
        req.mode,
        o.tag,
        o.hall,
        o.user
    );
    let mut nsfp = st.settings.nsfp_ids();
    nsfp.sort();
    if let Value::Object(m) = &mut pool {
        m.insert("nsfp_version".into(), json!(nsfp.iter().map(i64::to_string).collect::<Vec<_>>().join(",")));
    }
    j(pool, 200)
}

// ── GET /api/v2/recommendations ─────────────────────────────────────────────

fn rec_opts(req: &Req) -> RecOpts<'_> {
    let session = req.session();
    RecOpts {
        mode: Some(mode_override(req)),
        ratings: ratings(req),
        session,
        exclude: excluded(session),
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin).unwrap_or(false),
        ..Default::default()
    }
}

async fn recommendations(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let mut o = rec_opts(&req);
    o.limit = Some(qs_num(&req, "limit", 20.0).min(50.0));
    let videos = req.param("type") == Some("videos");
    // cookie: `req.cookies?.mime ? ... : null` (an empty cookie counts as missing)
    o.mime = effective_mime(&req, if videos { Some("video") } else { None });
    o.exclude_ids = qs(&req, "exclude_ids")
        .map(|v| {
            v.split(',')
                .filter_map(js_number)
                .filter(|n| n.fract() == 0.0 && *n > 0.0 && *n < i32::MAX as f64)
                .map(|n| n as i64)
                .take(150)
                .collect()
        })
        .unwrap_or_default();
    o.session_tags = req.query("session_tags").unwrap_or("").to_string();
    o.session_creators = req.query("session_creators").unwrap_or("").to_string();
    o.continuation = matches!(req.query("continuation"), Some("1" | "true"));
    o.prefer_personalized = match req.query("prefer_personalized") {
        Some("true" | "1") => Some(true),
        Some("false" | "0") => Some(false),
        _ => None,
    };
    let items = match recommend::get_personalized_recommendations(&st, &o).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("[RECOMMENDATIONS] Failed to fetch personalized recommendations: {e}");
            return j(json!({ "success": false, "error": "Failed to load recommendations" }), 500);
        }
    };
    let anon = anonymize(&st, &req);
    let slugs = s::enable_item_slugs(&cfg);
    let out: Vec<Value> = items
        .into_iter()
        .map(|it| {
            let g = |k: &str| it.get(k).cloned().unwrap_or(Value::Null);
            let slug = g("slug");
            json!({
                "id": g("id"),
                "title": g("title"),
                "slug": if slugs && crate::config::truthy(&slug) { slug } else { Value::Null },
                "dest": g("dest"),
                "mime": g("mime"),
                "stamp": g("stamp"),
                "username": if anon { json!("anonymous") } else { g("username") },
                "display_name": if anon { Value::Null } else { g("display_name") },
                "username_color": if anon { Value::Null } else { g("username_color") },
                "has_coverart": g("has_coverart"),
                "rating_class": g("rating_class"),

                "personalized": crate::config::truthy(&g("personalized")),
                "tags": it.get("tags").cloned().unwrap_or(json!([])),
                "score": g("score"),
                "rank_score": g("rank_score"),
            })
        })
        .collect();
    j(json!({ "success": true, "items": out }), 200)
}

// ── POST /api/v2/track/* ────────────────────────────────────────────────────

async fn track_interaction(st: AppState, mut req: Req) -> Res {
    let p = req.post_value().await;
    let raw_id = p.get("item_id").filter(|v| crate::config::truthy(v)).or_else(|| p.get("itemId"));
    let item_id = js_str(raw_id).and_then(|v| js_parse_int(&v)).unwrap_or(0);
    if item_id == 0 {
        return j(json!({ "success": false, "error": "Invalid item_id" }), 400);
    }
    let pf = |k: &str| js_str(p.get(k)).and_then(|v| js_parse_float(&v)).unwrap_or(0.0);
    let duration = pf("duration");
    let percent = pf("percent");
    let kind = p.get("type").filter(|v| crate::config::truthy(v)).and_then(|v| js_str(Some(v))).unwrap_or_else(|| "dwell".into());
    let delta = if kind == "skip" || (duration < 2.5 && percent < 15.0) {
        -1.5
    } else if kind == "click_suggestion" {
        2.5
    } else if kind == "finish" || percent >= 75.0 || duration >= 20.0 {
        3.0
    } else if duration >= 8.0 || percent >= 35.0 {
        1.5
    } else {
        1.0
    };
    if let Some(uid) = req.session().map(|s| s.id).filter(|&i| i != 0) {
        recommend::spawn_update_user_affinity(&st, uid, item_id, delta);
    }
    j(json!({ "success": true, "delta": jnum(delta) }), 200)
}

async fn track_search(st: AppState, mut req: Req) -> Res {
    let p = req.post_value().await;
    let pick = |v: Option<&Value>| v.filter(|v| crate::config::truthy(v)).cloned();
    let tag = pick(p.get("tag"))
        .or_else(|| pick(p.get("q")))
        .or_else(|| qs(&req, "tag").map(|t| json!(t)))
        .or_else(|| qs(&req, "q").map(|t| json!(t)));
    let Some(tag) = tag.as_ref().and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) else {
        return j(json!({ "success": false, "error": "Invalid tag parameter" }), 400);
    };
    if let Some(uid) = req.session().map(|s| s.id).filter(|&i| i != 0) {
        recommend::spawn_update_user_tag_affinity(&st, uid, tag, 2.0);
    }
    j(json!({ "success": true }), 200)
}

// ── GET /api/v2/favs-feed ───────────────────────────────────────────────────

/// `limit`/`offset` from the query. NaN or non-positive limits fall back to 20 and a NaN offset
/// to 0 (the old code passed NaN on to the feed functions).
fn limit_offset(req: &Req) -> (i64, i64) {
    let l = qs_num(req, "limit", 20.0).min(50.0);
    let limit = if l.is_finite() && l >= 1.0 { l.trunc() as i64 } else { 20 };
    let o = qs_num(req, "offset", 0.0).max(0.0);
    let offset = if o.is_finite() { o.trunc() as i64 } else { 0 };
    (limit, offset)
}

fn id_matches(it: &Value, id: &str) -> bool {
    let sid = match &it["id"] {
        Value::String(s) => s.clone(),
        v => v.to_string(),
    };
    sid == id || it["slug"].as_str() == Some(id)
}

async fn favs_feed(st: AppState, req: Req) -> Res {
    let Some(user) = req.query("user").map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) else {
        return j(json!({ "success": false, "error": "user is required", "items": [], "total": 0 }), 400);
    };
    let (limit, offset) = limit_offset(&req);
    let order = if req.query("order") == Some("asc") { "asc" } else { "desc" };
    let focus_id = req.query("focus_id").map(|f| f.trim().to_string()).filter(|f| !f.is_empty());
    let session = req.session();
    let lang = crate::routes::index::req_lang(&st, &req);
    let base = FeedOpts {
        user: Some(user.clone()),
        fav: true,
        mode: Some(mode_override(&req)),
        ratings: ratings(&req),
        session,
        exclude: excluded(session),
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin),
        limit: Some(limit),
        lang: Some(lang),
        ..Default::default()
    };
    let r = async {
        let mut o = base.clone();
        o.page = Some(offset / limit + 1);
        let result = f0cklib::getf0cks(&st, &o).await?;
        if !result.get("success").map(crate::config::truthy).unwrap_or(false) {
            return Ok::<_, anyhow::Error>(json!({ "success": false, "error": "not found", "items": [], "total": 0 }));
        }
        let mut items: Vec<Value> = result.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
        let total = result.get("total").and_then(crate::config::as_int).unwrap_or(0);
        let mut eff_offset = offset;
        if let Some(fid) = focus_id.as_deref().filter(|f| *f != "favs") {
            if !items.is_empty() && !items.iter().any(|it| id_matches(it, fid)) {
                let mut po = base.clone();
                po.lang = None;
                po.target_item_id = Some(fid.to_string());
                let page = f0cklib::get_item_page(&st, &po).await?;
                if page > 0 {
                    eff_offset = (page - 1) * limit;
                    let mut o2 = base.clone();
                    o2.page = Some(page);
                    o2.total = Some(total);
                    let r2 = f0cklib::getf0cks(&st, &o2).await?;
                    if r2.get("success").map(crate::config::truthy).unwrap_or(false) {
                        items = r2.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
                    }
                }
            }
        }
        if anonymize(&st, &req) {
            for it in items.iter_mut() {
                if let Value::Object(m) = it {
                    m.insert("username".into(), json!("anonymous"));
                    m.insert("display_name".into(), json!("anonymous"));
                    m.insert("username_color".into(), Value::Null);
                }
            }
        }
        if order == "asc" {
            items.reverse();
        }
        let has_more = eff_offset + (items.len() as i64) < total;
        Ok(json!({ "success": true, "user": user, "order": order, "total": total, "offset": eff_offset, "limit": limit, "items": items, "hasMore": has_more }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => {
            tracing::error!("[FAVS-FEED] Error: {e}");
            j(json!({ "success": false, "error": "Failed to load favs feed", "items": [], "total": 0 }), 500)
        }
    }
}

// ── GET /api/v2/tag-feed ────────────────────────────────────────────────────

async fn tag_feed(st: AppState, req: Req) -> Res {
    let Some(tag) = req.query("tag").map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else {
        return j(json!({ "success": false, "error": "Tag is required", "items": [], "total": 0 }), 400);
    };
    let (limit, offset) = limit_offset(&req);
    let order = if req.query("order") == Some("asc") { "asc" } else { "desc" };
    let session = req.session();
    let strict = matches!(req.query("strict"), Some("1" | "true")) || session.and_then(|s| s.strict_mode).unwrap_or(false);
    let mut rec = rec_opts(&req);
    rec.mime = effective_mime(&req, None);
    let o = TagFeedOpts {
        tag: tag.clone(),
        order: order.into(),
        offset: offset as f64,
        limit: Some(limit as f64),
        focus_id: req.query("focus_id").map(|f| f.trim().to_string()).filter(|f| !f.is_empty()),
        strict,
        rec,
    };
    let result = match recommend::get_tag_feed_items(&st, &o).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[TAG-FEED] Error loading tag feed items: {e}");
            return j(json!({ "success": false, "error": "Failed to load tag feed", "items": [], "total": 0 }), 500);
        }
    };
    let total = result.get("total").and_then(Value::as_i64).unwrap_or(0);
    let eff_offset = result.get("offset").and_then(Value::as_i64).unwrap_or(offset);
    let mut items: Vec<Value> = result.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let n = items.len() as i64;
    if anonymize(&st, &req) {
        for it in items.iter_mut() {
            if let Value::Object(m) = it {
                m.insert("username".into(), json!("anonymous"));
                m.insert("display_name".into(), json!("anonymous"));
                m.insert("username_color".into(), Value::Null);
                m.insert("avatar".into(), json!("/a/default.png"));
                m.insert("avatar_file".into(), Value::Null);
            }
        }
    }
    j(
        json!({
            "success": true, "tag": tag, "order": order, "total": total, "offset": eff_offset,
            "limit": result.get("limit").cloned().unwrap_or(json!(limit)), "items": items, "hasMore": eff_offset + n < total,
        }),
        200,
    )
}

// ── GET /api/v2/orakel/user ─────────────────────────────────────────────────

async fn orakel_user(st: AppState, req: Req) -> Res {
    if anonymize(&st, &req) {
        return j(json!({ "success": true, "username": "anonymous", "display_name": "Anonymous", "id": 0 }), 200);
    }
    let since = (crate::util::now_secs() - 604800) as i32;
    let r = async {
        let mut row = sql!(
            r#"SELECT "user"."user", "user".id, uo.display_name FROM "user" LEFT JOIN user_options uo ON uo.user_id = "user".id
            WHERE "user".last_seen > "#,
            since,
            r#" AND "user".banned = false ORDER BY RANDOM() LIMIT 1"#
        )
        .fetch_optional(&st.db)
        .await?;
        if row.is_none() {
            row = sql!(r#"SELECT "user"."user", "user".id, uo.display_name FROM "user" LEFT JOIN user_options uo ON uo.user_id = "user".id WHERE "user".banned = false ORDER BY RANDOM() LIMIT 1"#)
                .fetch_optional(&st.db)
                .await?;
        }
        Ok::<_, anyhow::Error>(row)
    }
    .await;
    match r {
        Ok(row) => {
            let row = row.unwrap_or_default();
            let or = |k: &str, d: Value| if crate::config::truthy(row.val(k)) { row.val(k).clone() } else { d };
            j(json!({ "success": true, "username": or("user", json!("Anonymous")), "display_name": or("display_name", Value::Null), "id": or("id", json!(0)) }), 200)
        }
        Err(e) => {
            tracing::error!("[API ORAKEL USER ERROR] {e}");
            j(json!({ "success": false, "msg": "Error fetching random user" }), 500)
        }
    }
}

// ── GET /api/v2/items/get ───────────────────────────────────────────────────

/// `+qs.x` as an integer mode; NaN / fractions become -1 (the default "sfw" branch, like JS).
fn mode_num(v: Option<&str>) -> i64 {
    let n = v.and_then(js_number).unwrap_or(f64::NAN);
    if n.is_finite() && n.fract() == 0.0 { n as i64 } else { -1 }
}

fn parse_id_param(v: &str) -> anyhow::Result<i32> {
    js_number(v).filter(|n| n.fract() == 0.0 && n.abs() < i32::MAX as f64).map(|n| n as i32).ok_or_else(|| anyhow::anyhow!("invalid id {v:?}"))
}

async fn items_get(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    let older = req.query("older").filter(|v| !v.is_empty());
    let newer = req.query("newer").filter(|v| !v.is_empty());
    let mode = mode_num(req.query("mode"));
    let excluded: Vec<i32> = excluded(session).into_iter().map(|n| n as i32).collect();
    let visibility = if is_staff(session) {
        Sql::empty()
    } else {
        match session.map(|s| s.user.to_lowercase()).filter(|u| !u.is_empty()) {
            Some(u) => sql!(r#"and (coalesce("items".visibility, 0) = 0 or lower("items".username) = "#, u, ")"),
            None => Sql::new(r#"and coalesce("items".visibility, 0) = 0"#),
        }
    };
    let newest = sql!(r#"select max(id) as id from "items""#).fetch_scalar::<i32, _>(&st.db).await?.unwrap_or(0) as i64;
    let oldest = sql!(r#"select min(id) as id from "items""#).fetch_scalar::<i32, _>(&st.db).await?.unwrap_or(0) as i64;
    let modequery = f0cklib::compute_base_mode(&cfg, Some(mode), None, session);

    let mut q = sql!(
        r#"select "items".id, "items".mime, coalesce("tags_assign".tag_id, 0) as tag_id from "items"
        left join "tags_assign" on "tags_assign".item_id = "items".id and ("tags_assign".tag_id = 1 or "tags_assign".tag_id = 2 or "tags_assign".tag_id = "#,
        cfg.nsfl_tag_id() as i32,
        ") where "
    );
    q.push(&modequery).push(" and active = true ").append(&visibility).push(" ");
    if s::is_anon_session(session) {
        if let Some(gf) = f0cklib::global_filter(&st, &cfg, session) {
            q.push(r#"and not exists (select 1 from tags_assign where item_id = "items".id and ("#).push(&gf).push(")) ");
        }
    }
    if !excluded.is_empty() {
        q.push(r#"and not exists (select 1 from tags_assign where item_id = "items".id and tag_id = any("#).bind(excluded).push("::int[])) ");
    }
    if let Some(o) = older {
        q.push("and id <= ").bind(parse_id_param(o)?).push(" ");
    } else if let Some(n) = newer {
        q.push("and id >= ").bind(parse_id_param(n)?).push(" ");
    }
    q.push(if newer.is_some() { "order by id asc" } else { "order by id desc" }).push(" limit 150");
    let mut rows = q.fetch_all(&st.db).await?;
    rows.sort_by_key(|r| std::cmp::Reverse(r.i64("id").unwrap_or(0)));
    let first = rows.first().and_then(|r| r.i64("id"));
    let last = rows.last().and_then(|r| r.i64("id"));
    j(json!({ "atEnd": first == Some(newest), "atStart": last == Some(oldest), "success": true, "items": rows }), 200)
}

// ── GET /api/v2/item/:id ────────────────────────────────────────────────────

async fn item(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let raw_id = req.param("id").unwrap_or("");
    let none = || j(json!({ "success": false, "msg": "no items found" }), 200);
    let id: Option<i64> = if raw_id.bytes().all(|b| b.is_ascii_digit()) {
        raw_id.parse::<i64>().ok().filter(|&n| n != 0 && n <= i32::MAX as i64)
    } else {
        sql!("SELECT id FROM items WHERE slug = ", raw_id, " LIMIT 1").fetch_scalar::<i32, _>(&st.db).await?.map(i64::from).filter(|&n| n != 0)
    };
    let Some(id) = id else { return none() };
    let Some(actitem) = sql!(r#"select * from "items" where id = "#, id as i32, " and active = true limit 1").fetch_optional(&st.db).await? else {
        return none();
    };
    let session = req.session();
    let item_user = actitem.string("username").to_lowercase();
    let owner_or_admin = session.map(|s| s.admin || s.is_moderator || (!s.user.is_empty() && s.user.to_lowercase() == item_user)).unwrap_or(false);
    let visibility = actitem.i64("visibility").unwrap_or(0);
    if visibility > 0 && !owner_or_admin {
        return none();
    }
    let public_visitor = s::is_anon_session(session);
    let gf = if public_visitor { f0cklib::global_filter(&st, &cfg, session) } else { None };
    if public_visitor && !owner_or_admin && visibility == 0 {
        if let Some(gf) = &gf {
            let mut q = sql!("select 1 from tags_assign where item_id = ", id as i32, " and (");
            q.push(gf).push(") limit 1");
            if q.fetch_optional(&st.db).await?.is_some() {
                return none();
            }
        }
    }

    // The old code read `req.query.mode` / `req.query.ratings`, which don't exist in flummpress
    // (every request for a visible item crashed); the query string is `req.url.qs`.
    let eff_mode = match req.query("mode") {
        Some(m) => mode_num(Some(m)),
        None => req.mode,
    };
    let ratings_arr: Option<Vec<String>> = if eff_mode == 3 || eff_mode == 2 {
        None
    } else {
        let raw = req.cookie("ratings").filter(|r| !r.is_empty()).or_else(|| req.query("ratings").filter(|r| !r.is_empty()));
        raw.map(|r| crate::util::decode_lossy(r).split(['|', ',']).filter(|x| matches!(*x, "sfw" | "nsfw" | "nsfl" | "untagged")).map(str::to_string).collect())
    };

    let nsfl = cfg.nsfl_tag_id();
    let mut guest_filter = Sql::empty();
    if public_visitor {
        let is_anon = session.is_some();
        let modes = if is_anon { s::anon_allowed_modes(&cfg) } else { vec![] };
        let allow_nsfw = if is_anon { modes.iter().any(|m| m == "nsfw") } else { cfg.truthy("websrv.public_nsfw") };
        let allow_nsfl = is_anon && modes.iter().any(|m| m == "nsfl");
        let mut blocked: Vec<i32> = vec![];
        if !allow_nsfw {
            blocked.push(2);
        }
        if !allow_nsfl {
            blocked.push(nsfl as i32);
        }
        if !blocked.is_empty() {
            guest_filter = sql!("and not exists (select 1 from tags_assign where item_id = items.id and tag_id = any(", blocked, "::int[]))");
        }
    }
    let global_cond = match &gf {
        Some(gf) if public_visitor => {
            let mut q = Sql::new("and not exists (select 1 from tags_assign where item_id = items.id and (");
            q.push(gf).push("))");
            q
        }
        _ => Sql::empty(),
    };
    let mode_cond = match eff_mode {
        2 => Sql::new("and not exists (select 1 from tags_assign where item_id = items.id)"),
        0 => Sql::new("and (id in (select item_id from tags_assign where tag_id = 1) or not exists (select 1 from tags_assign where item_id = items.id))"),
        1 => Sql::new("and id in (select item_id from tags_assign where tag_id = 2)"),
        4 => sql!("and id in (select item_id from tags_assign where tag_id = ", nsfl as i32, ")"),
        _ => match ratings_arr.as_ref().filter(|r| !r.is_empty()).and_then(|r| crate::util::get_multi_rating_mode(&cfg, r)) {
            Some(m) => {
                let mut q = Sql::new("and ");
                q.push(&m);
                q
            }
            None => Sql::empty(),
        },
    };
    let vis = if owner_or_admin {
        Sql::empty()
    } else {
        match session.map(|s| s.user.to_lowercase()).filter(|u| !u.is_empty()) {
            Some(u) => sql!("and (coalesce(visibility, 0) = 0 or lower(username) = ", u, ")"),
            None => Sql::new("and coalesce(visibility, 0) = 0"),
        }
    };
    let neighbour = |cmp: &str, order: &str| {
        let mut q = sql!(r#"select id from "items" where id "#);
        q.push(cmp).bind(id as i32).push(" and active = true ");
        for f in [&vis, &guest_filter, &global_cond, &mode_cond] {
            q.append(f).push(" ");
        }
        q.push("order by id ").push(order).push(" limit 1");
        q
    };
    let next = neighbour("> ", "").fetch_scalar::<i32, _>(&st.db).await?;
    let prev = neighbour("< ", "desc").fetch_scalar::<i32, _>(&st.db).await?;
    let mut rows: Row = actitem;
    rows.insert("next".into(), json!(next));
    rows.insert("prev".into(), json!(prev));
    if anonymize(&st, &req) {
        rows.insert("username".into(), json!("anonymous"));
        rows.insert("display_name".into(), Value::Null);
        rows.insert("user".into(), json!("anonymous"));
        rows.insert("src".into(), Value::Null);
    }
    j(json!({ "success": true, "rows": rows }), 200)
}

// ── GET /api/v2/user/:user(/:eps) ───────────────────────────────────────────

/// The old handler never awaited its query (`rows.length` of a pending query is undefined), so
/// it always answered `{ success: false, items: [] }`. Kept that way: actually running it would
/// publish every upload of a user, unlisted/private ones included, with their source URLs.
async fn user_items(st: AppState, req: Req) -> Res {
    if anonymize(&st, &req) {
        return j(json!({ "success": false, "msg": "access denied" }), 200);
    }
    j(json!({ "success": false, "items": [] }), 200)
}

// ── suggestions ─────────────────────────────────────────────────────────────

async fn tags_suggest(st: AppState, req: Req) -> Res {
    let mut reply = Map::new();
    reply.insert("success".into(), json!(false));
    reply.insert("suggestions".into(), json!({}));
    // JS: `searchString?.length <= 0` — only an empty `q=` is "too short"; a missing q queries
    // with NULL (no rows).
    let q = req.query("q");
    if q.is_none() {
        // postgres.js refused the undefined parameter
        reply.insert("error".into(), json!("Tag suggestion error"));
        return j(Value::Object(reply), 200);
    }
    if q == Some("") {
        reply.insert("error".into(), json!("too short lol"));
        return j(Value::Object(reply), 200);
    }
    let res = sql!(
        r#"select tag, count(tags_assign.tag_id) as tagged from "tags" left join "tags_assign" on "tags_assign".tag_id = "tags".id
        where normalized like '%' || slugify("#,
        q.map(str::to_string),
        r#") || '%' group by "tags".id having count(tags_assign.tag_id) > 0 order by tagged desc limit 15"#
    )
    .fetch_all(&st.db)
    .await;
    match res {
        Ok(rows) => {
            let rows = Value::Array(rows.into_iter().map(Value::Object).collect());
            reply.insert("success".into(), json!(true));
            let sug = f0cklib::search::search(&rows, q.unwrap_or("")).map(Value::Array).unwrap_or(Value::Bool(false));
            reply.insert("suggestions".into(), sug);
        }
        Err(e) => {
            tracing::debug!("[TAGS-SUGGEST] {e}");
            reply.insert("error".into(), json!("Tag suggestion error"));
        }
    }
    j(Value::Object(reply), 200)
}

async fn users_suggest(st: AppState, req: Req) -> Res {
    if anonymize(&st, &req) {
        return j(json!({ "success": true, "suggestions": [] }), 200);
    }
    let Some(q) = qs(&req, "q") else {
        return j(json!({ "success": false, "suggestions": [] }), 200);
    };
    let pat = format!("{q}%");
    match sql!(
        r#"SELECT "user".user, "user_options".display_name, "user_options".avatar, "user_options".avatar_file
        FROM "user" LEFT JOIN "user_options" ON "user".id = "user_options".user_id
        WHERE "user".user ILIKE "#,
        pat.clone(),
        r#" OR "user_options".display_name ILIKE "#,
        pat,
        r#" ORDER BY "user".user ASC LIMIT 10"#
    )
    .fetch_all(&st.db)
    .await
    {
        Ok(rows) => j(json!({ "success": true, "suggestions": rows }), 200),
        Err(_) => j(json!({ "success": false, "error": "User suggestion error", "suggestions": [] }), 200),
    }
}

async fn items_suggest(st: AppState, req: Req) -> Res {
    let Some(q) = qs(&req, "q") else {
        return j(json!({ "success": false, "suggestions": [] }), 200);
    };
    let pat = format!("%{q}%");
    // Only public items: the old query also suggested titles of unlisted/private uploads.
    match sql!(
        "SELECT id, title FROM items WHERE title IS NOT NULL AND active = true AND COALESCE(visibility, 0) = 0 AND title ILIKE ",
        pat,
        " ORDER BY id DESC LIMIT 8"
    )
    .fetch_all(&st.db)
    .await
    {
        Ok(rows) => j(json!({ "success": true, "suggestions": rows }), 200),
        Err(_) => j(json!({ "success": false, "error": "Item title suggestion error", "suggestions": [] }), 200),
    }
}
