//! Comments, subscriptions toggle, activity feed and comment polls
//! (port of src/inc/routes/comments.mjs).
//!
//! The old handlers answered JSON through `res.reply({ headers, body })`, which ignores `headers`,
//! so every JSON body went out as `text/html`. A JSON document full of user text served as HTML
//! is an XSS vector, so these are sent as `application/json` now (the frontend parses them with
//! `res.json()`, which doesn't care).

use crate::db::{Row, RowExt, Sql};
use crate::http::{Req, Res, Router, html, html_code, json_resp, redirect, reply};
use crate::session::Session;
use crate::settings::{can_anon_do, enable_item_slugs, is_anon_session, is_anonymize_session};
use crate::social::comments as sc;
use crate::social::helpers::{
    self as h, comment_globalfilter_tags, compute_base_mode, js_parse_float, js_parse_int, js_str, log_anon_activity, mode_override, notify,
    ratings_from_cookie, resolve_audit_ip, resolve_mime_sql,
};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, Method, header};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

pub fn register(r: &mut Router) {
    r.get(r"\/api\/comments\/(?<itemid>\d+)", get_item_comments);
    r.get(r"\/api\/comment\/(?<id>\d+)", get_single_comment);
    r.get(r"\/user\/(?<user>[^\/]+)\/comments", user_comments);
    r.exact(Method::POST, "/api/comments", None, post_comment);
    r.post(r"\/api\/subscribe\/(?<itemid>\d+)", subscribe_toggle);
    r.post(r"\/api\/comments\/(?<id>\d+)\/delete", delete_comment);
    r.exact(Method::POST, "/api/comments/attachment/delete", None, delete_attachment);
    r.post(r"\/api\/comments\/(?<id>\d+)\/edit", edit_comment);
    r.post(r"\/api\/comments\/(?<id>\d+)\/pin", pin_comment);
    r.post(r"\/api\/comments\/(?<itemid>\d+)\/lock", lock_thread);
    r.get(r"\/activity\/?", activity);
    r.exact(Method::POST, "/api/v2/user/subscribe-all-uploads", None, subscribe_all_uploads);
    r.post(r"\/api\/polls\/attach\/(?<commentId>\d+)", poll_attach);
    r.get(r"\/api\/polls\/(?<pollId>\d+)", poll_get);
    r.post(r"\/api\/polls\/(?<pollId>\d+)\/vote", poll_vote);
    r.post(r"\/api\/polls\/(?<pollId>\d+)\/delete", poll_delete);
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn unauthorized() -> Res {
    j(json!({ "success": false }), 401)
}

fn forbidden() -> Res {
    j(json!({ "success": false, "message": "Forbidden" }), 403)
}

fn is_admin_or_mod(s: &Session) -> bool {
    s.admin || s.is_moderator
}

/// Route param that the regex guarantees to be digits, as an `integer` bind.
/// Values beyond int4 made the old queries fail (→ the route's error answer).
fn param_i32(req: &Req, name: &str) -> Option<i32> {
    req.param(name).and_then(|s| s.parse::<i32>().ok())
}

fn polls_enabled(st: &AppState) -> bool {
    st.cfg().truthy("websrv.enable_comment_polls")
}

// ── GET /api/comments/:itemid ───────────────────────────────────────────────

async fn get_item_comments(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    if session.is_none() && cfg.truthy("main.hide_comments_from_public") {
        return j(json!({ "success": true, "comments": [], "require_login": true, "user_id": null, "is_admin": false }), 200);
    }
    let db_error = || j(json!({ "success": false, "message": "Database error" }), 500);
    let Some(item_id) = param_i32(&req, "itemid") else { return db_error() };
    let raw_id = req.param("itemid").unwrap_or("").to_string();
    let sort = sc::Sort::from_query(req.query("sort").filter(|s| !s.is_empty()));

    let r: anyhow::Result<Value> = async {
        let item = sql!("SELECT is_comments_locked FROM items WHERE id = ", item_id).fetch_optional(&st.db).await?;
        let is_locked = item.map(|i| i.val("is_comments_locked").clone()).unwrap_or(Value::Bool(false));
        let mut comments = sc::get_comments(&st, &raw_id, sort, false).await;

        let mut is_subscribed = false;
        if let Some(s) = session {
            let sub = sql!("SELECT 1 FROM comment_subscriptions WHERE user_id = ", s.id as i32, " AND item_id = ", item_id, " AND is_subscribed = true")
                .fetch_optional(&st.db)
                .await?;
            is_subscribed = sub.is_some();
        }

        if let Some(s) = session.filter(|_| polls_enabled(&st)) {
            fill_user_votes(&st, s.id, &mut comments).await;
        }

        let out: Vec<Value> = if is_anonymize_session(&cfg, session) {
            comments.into_iter().map(|c| Value::Object(sc::anonymize(c))).collect()
        } else {
            comments.into_iter().map(Value::Object).collect()
        };
        Ok(json!({
            "success": true,
            "comments": out,
            "is_subscribed": is_subscribed,
            "is_locked": is_locked,
            "user_id": session.map(|s| json!(s.id)).unwrap_or(Value::Null),
            "is_admin": session.map(is_admin_or_mod).unwrap_or(false),
        }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => {
            tracing::error!("{e:#}");
            db_error()
        }
    }
}

/// Sets `poll.user_vote_option_id` on comments with polls (errors ignored, like the old code).
async fn fill_user_votes(st: &AppState, user_id: i64, comments: &mut [Row]) {
    let poll_ids: Vec<i32> = comments.iter().filter_map(|c| c.get("poll").and_then(|p| p.get("id")).and_then(crate::config::as_int)).map(|i| i as i32).collect();
    if poll_ids.is_empty() {
        return;
    }
    let Ok(votes) = sql!("SELECT poll_id, option_id FROM comment_poll_votes WHERE poll_id = ANY(", poll_ids, "::int[]) AND user_id = ", user_id as i32)
        .fetch_all(&st.db)
        .await
    else {
        return;
    };
    let map: HashMap<i64, Value> = votes.iter().filter_map(|v| Some((v.i64("poll_id")?, v.val("option_id").clone()))).collect();
    for c in comments.iter_mut() {
        if let Some(Value::Object(p)) = c.get_mut("poll") {
            let id = p.get("id").and_then(crate::config::as_int).unwrap_or(-1);
            p.insert("user_vote_option_id".into(), map.get(&id).cloned().filter(crate::config::truthy).unwrap_or(Value::Null));
        }
    }
}

// ── GET /api/comment/:id ────────────────────────────────────────────────────

async fn get_single_comment(st: AppState, req: Req) -> Res {
    if req.session().is_none() && st.cfg().truthy("main.hide_comments_from_public") {
        return j(json!({ "success": false, "message": "Unauthorized" }), 401);
    }
    // getComment swallowed the query error for ids beyond int4 → "not found".
    let id = param_i32(&req, "id").map(i64::from).unwrap_or(0);
    match sc::get_comment(&st, id, true).await {
        Some(c) => j(json!({ "success": true, "comment": c }), 200),
        None => j(json!({ "success": false, "message": "Comment not found" }), 404),
    }
}

// ── GET /user/:user/comments ────────────────────────────────────────────────

async fn user_comments(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    if is_anonymize_session(&cfg, session) {
        return Ok(redirect(if session.is_some() { "/" } else { "/login" }));
    }
    let Some(user) = crate::util::decode_uri_component(req.param("user").unwrap_or("")) else {
        // decodeURIComponent threw outside the try block in the old code
        return Ok(reply(500, "text/html", "Error"));
    };
    match user_comments_inner(&st, &req, &user).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            Ok(reply(500, "text/html", "Error"))
        }
    }
}

async fn user_comments_inner(st: &AppState, req: &Req, user: &str) -> anyhow::Result<axum::response::Response> {
    let cfg = st.cfg();
    let session = req.session();
    let u = sql!(
        r#"SELECT "user".id, "user".user, user_options.avatar, user_options.avatar_file, user_options.username_color
                FROM "user"
                LEFT JOIN user_options ON "user".id = user_options.user_id
                WHERE "user".user ILIKE "#,
        crate::util::escape_like(user)
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(u) = u else { return Ok(html_code("User not found", 404)) };
    let user_id = u.i64("id").unwrap_or(0) as i32;
    let page = h::js_number(req.query("page").filter(|p| !p.is_empty()).unwrap_or("1"));
    let limit = 20i64;
    let offset = h::sql_count(page.map(|p| (p - 1.0) * limit as f64)).ok_or_else(|| anyhow::anyhow!("invalid page"))?;
    let is_json = req.query("json") == Some("true");

    if session.map(|s| s.user.is_empty()).unwrap_or(true) && cfg.truthy("main.hide_comments_from_public") {
        if is_json {
            return Ok(json_resp(&json!({ "success": false, "require_login": true }), 200));
        }
        return Ok(redirect("/login"));
    }

    let global_tags = comment_globalfilter_tags(st, session);
    let excluded: Vec<i32> = session.map(|s| s.excluded_tags.iter().map(|t| *t as i32).collect()).unwrap_or_default();
    let mode = mode_override(req);
    let ratings = ratings_from_cookie(req);
    let modequery = compute_base_mode(&cfg, Some(mode), ratings.as_deref(), session).replace("items.id", "i.id");

    let mut q = sql!(
        r#"SELECT c.*, i.mime, i.id as item_id, i.slug as item_slug
                FROM comments c
                LEFT JOIN items i ON c.item_id = i.id
                WHERE c.user_id = "#,
        user_id,
        " AND c.is_deleted = false AND i.active = true AND i.is_deleted = false AND "
    );
    q.push(&modequery);
    if is_anon_session(session) && !global_tags.is_empty() {
        q.push(" and not exists (select 1 from tags_assign where item_id = i.id and (").push(&global_tag_sql(&global_tags)).push("))");
    }
    if !excluded.is_empty() {
        q.push(" and not exists (select 1 from tags_assign where item_id = i.id and tag_id = any(").bind(excluded).push("::int[]))");
    }
    q.push(" ORDER BY c.created_at DESC LIMIT ").bind(limit).push(" OFFSET ").bind(offset);
    let comments = q.fetch_all(&st.db).await?;

    let slugs = enable_item_slugs(&cfg);
    let mut processed: Vec<Row> = sc::process_mentions(st, comments)
        .await
        .into_iter()
        .map(|mut c| {
            strip_private_columns(&mut c);
            if !slugs {
                c.insert("item_slug".into(), Value::Null);
            }
            c
        })
        .collect();

    if !processed.is_empty() {
        let ids = sc::comment_ids(&processed);
        sc::attach_files(st, &mut processed, &ids).await;
        let mut polls: HashMap<i64, Value> = HashMap::new();
        if polls_enabled(st) {
            match sc::load_polls(st, &ids, true).await {
                Ok(p) => polls = p,
                Err(e) => tracing::error!("[USER_COMMENTS] Poll fetch error: {e}"),
            }
            if let (Some(s), false) = (session, polls.is_empty()) {
                let poll_ids: Vec<i32> = polls.values().filter_map(|p| p.get("id").and_then(crate::config::as_int)).map(|i| i as i32).collect();
                if let Ok(votes) = sql!("SELECT poll_id, option_id FROM comment_poll_votes WHERE poll_id = ANY(", poll_ids, "::int[]) AND user_id = ", s.id as i32)
                    .fetch_all(&st.db)
                    .await
                {
                    let vm: HashMap<i64, Value> = votes.iter().filter_map(|v| Some((v.i64("poll_id")?, v.val("option_id").clone()))).collect();
                    for p in polls.values_mut() {
                        let id = p.get("id").and_then(crate::config::as_int).unwrap_or(-1);
                        p["user_vote_option_id"] = vm.get(&id).cloned().filter(crate::config::truthy).unwrap_or(Value::Null);
                    }
                }
            }
        }
        for c in processed.iter_mut() {
            let p = c.i64("id").and_then(|id| polls.remove(&id)).unwrap_or(Value::Null);
            c.insert("poll".into(), p);
        }
    }

    let comments_value = sc::rows_to_value(processed);
    if is_json {
        return Ok(json_resp(&json!({ "success": true, "comments": comments_value, "user": u }), 200));
    }
    let data = json!({ "user": u, "comments": comments_value, "hidePagination": true, "tmp": null });
    let view = if req.is_xhr() { "comments_user-partial" } else { "comments_user" };
    Ok(html(crate::tpl::render(st, Some(req), view, data)))
}

/// `tag_id = 1 or tag_id = 2` from integer ids (trusted).
fn global_tag_sql(tags: &[i64]) -> String {
    tags.iter().map(|n| format!("tag_id = {n}")).collect::<Vec<_>>().join(" or ")
}

/// `c.*` includes the poster's stored IP (`comments.ip`); the old code sent it to every visitor.
fn strip_private_columns(c: &mut Row) {
    c.shift_remove("ip");
}

// ── POST /api/comments ──────────────────────────────────────────────────────

const COMMENT_RATE_LIMIT: usize = 25;
const COMMENT_RATE_WINDOW_MS: i64 = 60_000;
static RATE: Lazy<Mutex<HashMap<i64, Vec<i64>>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Sliding-window limiter: 25 comments per minute per user.
fn is_comment_rate_limited(user_id: i64) -> bool {
    let now = crate::util::now_ms();
    let start = now - COMMENT_RATE_WINDOW_MS;
    let mut map = RATE.lock();
    let mut ts: Vec<i64> = map.get(&user_id).map(|v| v.iter().copied().filter(|t| *t > start).collect()).unwrap_or_default();
    if ts.len() >= COMMENT_RATE_LIMIT {
        return true;
    }
    ts.push(now);
    map.insert(user_id, ts);
    if map.len() > 5000 {
        let prune = now - 300_000;
        map.retain(|_, ts| ts.iter().any(|t| *t > prune));
    }
    false
}

fn parse_file_ids(raw: &str) -> Vec<i32> {
    if raw.is_empty() {
        return vec![];
    }
    raw.split(',').filter_map(js_parse_int).filter(|n| *n > 0).filter_map(|n| i32::try_from(n).ok()).collect()
}

static MENTION_ANY_RE: Lazy<fancy_regex::Regex> =
    Lazy::new(|| fancy_regex::Regex::new(r"(?<!\[)@([a-zA-Z0-9_\-\.]+)|\[@([^\]]+)\]").expect("mention regex"));
static SPOILER_RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)\[/?spoiler\]").expect("spoiler regex"));
static LONG_RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)\[(video|audio|youtube|img)\]|!\[|https?://").expect("long regex"));

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.substring(0, n)` on UTF-16 units (kept on char boundaries).
fn utf16_prefix(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            break;
        }
        out.push(c);
    }
    out
}

/// Rating label/class of an item from its first rating tag.
fn rating_of(cfg: &crate::config::Config, tag: Option<i64>) -> (&'static str, &'static str) {
    match tag {
        Some(1) => ("SFW", "sfw"),
        Some(2) => ("NSFW", "nsfw"),
        Some(t) if t == cfg.nsfl_tag_id() => ("NSFL", "nsfl"),
        _ => ("?", "untagged"),
    }
}

/// `content.length > 120 || > 2 lines || attachments || media/links`
fn is_long(content: &str, files: usize) -> bool {
    utf16_len(content) > 120 || content.split('\n').count() > 2 || files > 0 || LONG_RE.is_match(content)
}

async fn post_comment(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let Some(session) = req.session().cloned() else { return j(json!({ "success": false, "message": "Unauthorized" }), 401) };
    if is_anon_session(Some(&session)) && !can_anon_do(&cfg, "comment") {
        return j(json!({ "success": false, "message": "Anonymous commenting is disabled" }), 403);
    }
    // No JavaScript: the image captcha in the comment form stands in for the Cap popup (cap_image.rs)
    if let Some(id) = req.post_str("cap-image-id").await {
        let answer = req.post_str("cap-image-answer").await;
        if matches!(crate::cap_image::check(Some(&id), answer.as_deref(), false), crate::cap::Captcha::Ok) {
            crate::cap::grant(&cfg, &session, crate::cap::Scope::Comment);
        }
    }
    if !crate::cap::take(&cfg, &req, &session, crate::cap::Scope::Comment) {
        return j(json!({ "success": false, "message": "Please complete the captcha.", "captcha_required": "comment" }), 403);
    }
    if !is_admin_or_mod(&session) && is_comment_rate_limited(session.id) {
        return j(json!({ "success": false, "message": "You're posting too fast. Please slow down." }), 429);
    }

    let body = req.post_value().await;
    let item_id = js_str(body.get("item_id")).as_deref().and_then(js_parse_int);
    let parent_id = if h::js_truthy(body.get("parent_id")) { js_str(body.get("parent_id")).as_deref().and_then(js_parse_int).filter(|p| *p != 0) } else { None };
    let raw_content = if h::js_truthy(body.get("content")) { js_str(body.get("content")).unwrap_or_default() } else { String::new() };
    let content = crate::wordfilter::apply_word_filter(&st, &raw_content).await;
    let video_time = js_str(body.get("video_time")).filter(|v| !v.is_empty()).as_deref().and_then(js_parse_float);
    let file_ids = parse_file_ids(&js_str(body.get("file_ids")).unwrap_or_default());
    let has_poll = matches!(js_str(body.get("has_poll")).as_deref(), Some("1" | "true"));

    if content.trim().is_empty() && file_ids.is_empty() && !has_poll {
        return j(json!({ "success": false, "message": "Empty comment" }), 200);
    }
    if let Some(max) = cfg.get("main.comment_max_length").as_f64() {
        if utf16_len(&content) as f64 > max {
            return j(json!({ "success": false, "message": format!("Comment too long (max {} characters)", crate::util::js_num(max)) }), 400);
        }
    }

    match post_comment_inner(&st, &req, &session, item_id, parent_id, content, video_time, file_ids).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false, "message": "Database error" }), 500)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn post_comment_inner(
    st: &AppState,
    req: &Req,
    session: &Session,
    item_id: Option<i64>,
    parent_id: Option<i64>,
    content: String,
    video_time: Option<f64>,
    file_ids: Vec<i32>,
) -> anyhow::Result<axum::response::Response> {
    let cfg = st.cfg();
    // NaN / out-of-range ids made the old queries throw (→ 500 "Database error").
    let item_id: i32 = item_id.and_then(|i| i32::try_from(i).ok()).ok_or_else(|| anyhow::anyhow!("invalid item_id"))?;
    let parent_id: Option<i32> = match parent_id {
        Some(p) => Some(i32::try_from(p).map_err(|_| anyhow::anyhow!("invalid parent_id"))?),
        None => None,
    };
    let me = session.id as i32;

    if !is_admin_or_mod(session) {
        let lock = sql!("SELECT COALESCE(is_comments_locked, false) as is_locked FROM items WHERE id = ", item_id).fetch_optional(&st.db).await?;
        if lock.map(|l| l.bool("is_locked")).unwrap_or(false) {
            return Ok(json_resp(&json!({ "success": false, "message": "This thread is locked" }), 403));
        }
    }

    let audit_ip = resolve_audit_ip(&cfg, req);
    let mut q = Sql::new("INSERT INTO comments (item_id, user_id, parent_id, content, ip");
    if video_time.is_some() {
        q.push(", video_time");
    }
    q.push(") VALUES (").bind(item_id).push(", ").bind(me).push(", ").bind(parent_id).push(", ").bind(content.as_str()).push(", ").bind(audit_ip);
    if let Some(vt) = video_time {
        q.push(", ").bind(vt).push("::numeric");
    }
    q.push(") RETURNING id, created_at, video_time");
    let new_comment = q.fetch_optional(&st.db).await?.ok_or_else(|| anyhow::anyhow!("insert returned nothing"))?;
    let comment_id = new_comment.i64("id").unwrap_or(0);

    if session.is_anon {
        log_anon_activity(st, req, "comment", Some(comment_id), Some(json!({ "item_id": item_id, "parent_id": parent_id }))).await;
    }

    let mut activity_files: Vec<Value> = vec![];
    if !file_ids.is_empty() {
        let r: anyhow::Result<()> = async {
            sql!(
                "UPDATE comment_files SET comment_id = ",
                comment_id as i32,
                " WHERE id = ANY(",
                file_ids.clone(),
                "::int[]) AND user_id = ",
                me,
                " AND comment_id IS NULL"
            )
            .execute(&st.db)
            .await?;
            activity_files = sql!("SELECT id, comment_id, dest, mime, size, original_filename FROM comment_files WHERE comment_id = ", comment_id as i32, " ORDER BY id ASC")
                .fetch_all(&st.db)
                .await?
                .into_iter()
                .map(Value::Object)
                .collect();
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::error!("[COMMENTS] Failed to link files to comment: {e}");
        }
    }

    // Notifications: mention > reply > subscription, one per user.
    let subscribers = sql!("SELECT user_id FROM comment_subscriptions WHERE item_id = ", item_id, " AND is_subscribed = true").fetch_all(&st.db).await?;
    let stripped = SPOILER_RE.replace_all(&content, "");
    let mut names: Vec<String> = Vec::new();
    for caps in MENTION_ANY_RE.captures_iter(&*stripped).flatten() {
        let n = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if !names.contains(&n) {
            names.push(n);
        }
    }
    let lower: Vec<String> = names.iter().map(|n| n.to_lowercase()).collect();
    let mentioned = if lower.is_empty() {
        vec![]
    } else {
        sql!(r#"SELECT id, "user" FROM "user" WHERE login = ANY("#, lower, ")").fetch_all(&st.db).await?
    };
    let parent_author = match parent_id {
        Some(p) => sql!("SELECT user_id FROM comments WHERE id = ", p).fetch_optional(&st.db).await?,
        None => None,
    };

    let mut notes: Vec<(i64, &'static str)> = Vec::new();
    let has = |n: &Vec<(i64, &str)>, id: i64| n.iter().any(|(u, _)| *u == id);
    for u in &mentioned {
        if let Some(id) = u.i64("id").filter(|id| *id != session.id) {
            match notes.iter_mut().find(|(u, _)| *u == id) {
                Some(e) => e.1 = "mention",
                None => notes.push((id, "mention")),
            }
        }
    }
    if let Some(pid) = parent_author.as_ref().and_then(|p| p.i64("user_id")) {
        if pid != session.id && !has(&notes, pid) {
            notes.push((pid, "comment_reply"));
        }
    }
    let uploader = sql!(
        r#"SELECT u.id as uploader_id
                FROM items i
                JOIN "user" u ON (i.username ILIKE u.login OR i.username ILIKE u.user)
                WHERE i.id = "#,
        item_id,
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?
    .and_then(|r| r.i64("uploader_id"));
    for s in &subscribers {
        let Some(uid) = s.i64("user_id") else { continue };
        if uid != session.id && !has(&notes, uid) {
            notes.push((uid, if uploader == Some(uid) { "upload_comment" } else { "subscription" }));
        }
    }

    let non_bundle: Vec<&(i64, &str)> = notes.iter().filter(|(_, t)| *t != "upload_comment").collect();
    if !non_bundle.is_empty() {
        let mut q = Sql::new("INSERT INTO notifications (user_id, type, item_id, reference_id) VALUES ");
        for (i, (uid, t)) in non_bundle.iter().enumerate() {
            if i > 0 {
                q.push(", ");
            }
            q.push("(").bind(*uid as i32).push(", ").bind(*t).push(", ").bind(item_id).push(", ").bind(comment_id as i32).push(")");
        }
        q.execute(&st.db).await?;
    }
    for (uid, _) in notes.iter().filter(|(_, t)| *t == "upload_comment") {
        let updated = sql!(
            "UPDATE notifications SET created_at = NOW(), reference_id = ",
            comment_id as i32,
            " WHERE user_id = ",
            *uid as i32,
            " AND item_id = ",
            item_id,
            " AND type = 'upload_comment' AND is_read = false RETURNING id"
        )
        .fetch_all(&st.db)
        .await?;
        if updated.is_empty() {
            sql!(
                "INSERT INTO notifications (user_id, type, item_id, reference_id) VALUES (",
                *uid as i32,
                ", 'upload_comment', ",
                item_id,
                ", ",
                comment_id as i32,
                ")"
            )
            .execute(&st.db)
            .await?;
        }
    }

    // Live updates
    let nsfl = cfg.nsfl_tag_id() as i32;
    let item_row = sql!(
        r#"SELECT
                    i.slug,
                    i.mime,
                    i.dest,
                    i.has_coverart,
                    COALESCE(i.visibility, 0) as visibility,
                    (SELECT ta.tag_id FROM tags_assign ta
                     WHERE ta.item_id = i.id AND ta.tag_id = ANY("#,
        vec![1i32, 2, nsfl],
        "::int[]) ORDER BY ta.tag_id LIMIT 1) AS rating_tag_id FROM items i WHERE i.id = ",
        item_id
    )
    .fetch_optional(&st.db)
    .await?;
    let iv = |k: &str| item_row.as_ref().map(|r| r.val(k).clone()).unwrap_or(Value::Null);
    let (rating_label, rating_class) = rating_of(&cfg, item_row.as_ref().and_then(|r| r.i64("rating_tag_id")));
    let notify_body = if utf16_len(&content) > 500 { format!("{}…", utf16_prefix(&content, 500)) } else { content.clone() };
    let banner = sql!("SELECT banner_file, banner_position, banner_size, banner_repeat FROM user_options WHERE user_id = ", me)
        .fetch_optional(&st.db)
        .await?
        .unwrap_or_default();
    let pick = |k: &str, fallback: &Option<String>| -> Value {
        match banner.get(k) {
            Some(v) if crate::config::truthy(v) => v.clone(),
            _ => fallback.clone().filter(|s| !s.is_empty()).map(Value::String).unwrap_or(Value::Null),
        }
    };
    let banner_file = pick("banner_file", &session.banner_file);
    let banner_position = pick("banner_position", &session.banner_position);
    let banner_size = pick("banner_size", &session.banner_size);
    let banner_repeat = pick("banner_repeat", &session.banner_repeat);
    let slugs = enable_item_slugs(&cfg);
    let item_slug = if slugs && crate::config::truthy(&iv("slug")) { iv("slug") } else { Value::Null };
    let display_name = if session.is_anon { json!("Anonymous") } else { session.display_name.clone().filter(|d| !d.is_empty()).map(Value::String).unwrap_or(Value::Null) };
    let fp = session.fingerprint.clone().filter(|f| !f.is_empty());
    let short_fp = fp.as_deref().map(|f| f.chars().skip(7).take(8).collect::<String>());

    let opt = |v: &Option<String>| v.clone().map(Value::String).unwrap_or(Value::Null);

    let live = json!({
        "type": "comment",
        "id": comment_id,
        "item_id": item_id,
        "item_slug": item_slug,
        "parent_id": parent_id,
        "body": notify_body,
        "username": session.user,
        "user_id": session.id,
        "avatar": session.avatar,
        "avatar_file": opt(&session.avatar_file),
        "banner_file": banner_file,
        "banner_position": banner_position,
        "banner_size": banner_size,
        "banner_repeat": banner_repeat,
        "created_at": chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
        "username_color": opt(&session.username_color),
        "display_name": display_name,
        "video_time": new_comment.val("video_time"),
        "files": activity_files,
        "is_anon": session.is_anon,
        "anon_fingerprint": fp,
        "anon_short_fingerprint": short_fp,
    });
    notify(st, "comments", &live).await;

    let mime = iv("mime");
    let dest = iv("dest");
    let activity = json!({
        "user_id": session.id,
        "item_id": item_id,
        "item_slug": item_slug,
        "mime": if crate::config::truthy(&mime) { mime } else { Value::Null },
        "dest": if crate::config::truthy(&dest) { dest } else { Value::Null },
        "has_coverart": crate::config::truthy(&iv("has_coverart")),
        "type": "comment",
        "body": notify_body,
        "id": comment_id,
        "item_rating_class": rating_class,
        "item_rating_label": rating_label,
        "avatar": session.avatar,
        "avatar_file": opt(&session.avatar_file),
        "banner_file": banner_file,
        "banner_position": banner_position,
        "banner_size": banner_size,
        "banner_repeat": banner_repeat,
        "username": if session.is_anon { "anonymous".to_string() } else { session.user.clone() },
        "username_color": opt(&session.username_color),
        "display_name": display_name,
        "files": activity_files,
        "is_long": is_long(&content, activity_files.len()),
        "is_anon": session.is_anon,
        "anon_fingerprint": fp,
        "anon_short_fingerprint": short_fp,
    });
    notify(st, "activity", &activity).await;

    let sub = sql!("INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", me, ", ", item_id, ") ON CONFLICT (user_id, item_id) DO NOTHING RETURNING 1")
        .fetch_all(&st.db)
        .await?;

    let mut comment: Map<String, Value> = new_comment;
    comment.insert("user_id".into(), json!(session.id));
    comment.insert("username_color".into(), session.username_color.clone().filter(|c| !c.is_empty()).map(Value::String).unwrap_or(Value::Null));
    comment.insert("content".into(), json!(content));
    comment.insert("files".into(), Value::Array(activity_files));
    comment.insert("banner_file".into(), banner_file);
    comment.insert("banner_position".into(), banner_position);
    comment.insert("banner_size".into(), banner_size);
    comment.insert("banner_repeat".into(), banner_repeat);
    Ok(json_resp(&json!({ "success": true, "comment": comment, "is_new_subscription": !sub.is_empty() }), 200))
}

// ── POST /api/subscribe/:itemid ─────────────────────────────────────────────

async fn subscribe_toggle(st: AppState, mut req: Req) -> Res {
    // `subscribed` ("1" / "0"): set that state instead of toggling (the form without JavaScript,
    // where a repeated post must not undo the first)
    let wanted = match req.post_str("subscribed").await.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    };
    let Some(s) = req.session() else { return unauthorized() };
    let Some(item_id) = param_i32(&req, "itemid") else { return j(json!({ "success": false }), 500) };
    let me = s.id as i32;
    let r: anyhow::Result<bool> = async {
        let existing = sql!("SELECT is_subscribed FROM comment_subscriptions WHERE user_id = ", me, " AND item_id = ", item_id).fetch_optional(&st.db).await?;
        Ok(match existing {
            Some(e) => {
                let subscribed = wanted.unwrap_or(!e.bool("is_subscribed"));
                sql!("UPDATE comment_subscriptions SET is_subscribed = ", subscribed, " WHERE user_id = ", me, " AND item_id = ", item_id).execute(&st.db).await?;
                subscribed
            }
            None => {
                let subscribed = wanted.unwrap_or(true);
                sql!("INSERT INTO comment_subscriptions (user_id, item_id, is_subscribed) VALUES (", me, ", ", item_id, ", ", subscribed, ")").execute(&st.db).await?;
                subscribed
            }
        })
    }
    .await;
    match r {
        Ok(subscribed) => j(json!({ "success": true, "subscribed": subscribed }), 200),
        Err(_) => j(json!({ "success": false }), 500),
    }
}

// ── POST /api/comments/:id/delete ───────────────────────────────────────────

async fn delete_comment(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    let Some(comment_id) = param_i32(&req, "id") else { return j(json!({ "success": false }), 500) };
    let body = req.post_value().await;
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(comment) = sql!("SELECT content, item_id, user_id FROM comments WHERE id = ", comment_id).fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Not found" }), 404));
        };
        let cfg = st.cfg();
        let can_delete_own = crate::settings::allow_comment_deletion(&cfg);
        let is_owner = comment.i64("user_id") == Some(session.id);
        if !is_admin_or_mod(&session) && (!can_delete_own || !is_owner) {
            return Ok(json_resp(&json!({ "success": false, "message": "Forbidden" }), 403));
        }
        let reason = if h::js_truthy(body.get("reason")) {
            js_str(body.get("reason")).unwrap_or_default()
        } else {
            req.query("reason").filter(|r| !r.is_empty()).unwrap_or("No reason provided").to_string()
        };
        crate::audit::log(&st, session.id, "delete_comment", "comment", comment_id.to_string(), json!({ "item_id": comment.val("item_id"), "reason": reason, "old_content": comment.val("content") }))
        .await;
        if session.is_anon {
            log_anon_activity(&st, &req, "comment_delete", Some(comment_id as i64), Some(json!({ "item_id": comment.val("item_id") }))).await;
        }

        let files = sql!("SELECT id, dest, checksum FROM comment_files WHERE comment_id = ", comment_id).fetch_all(&st.db).await?;
        for f in files {
            let fid = f.i64("id").unwrap_or(0) as i32;
            let dest = f.string("dest");
            let other = sql!("SELECT id FROM comment_files WHERE checksum = ", f.string("checksum"), " AND id != ", fid).fetch_all(&st.db).await?;
            let text_refs = sql!("SELECT id FROM comments WHERE content LIKE ", format!("%{dest}%"), " AND id != ", comment_id).fetch_all(&st.db).await?;
            let file_path = crate::http::files::safe_join(&cfg.paths.c, &dest);
            let thumb_path = crate::http::files::safe_join(&cfg.paths.t, &format!("cf_{}.webp", dest.split('.').next().unwrap_or("")));
            let remove = |p: &Option<std::path::PathBuf>| {
                let p = p.clone();
                async move {
                    if let Some(p) = p {
                        let _ = tokio::fs::remove_file(p).await;
                    }
                }
            };
            if other.is_empty() && text_refs.is_empty() {
                remove(&file_path).await;
                remove(&thumb_path).await;
            } else if let Some(fp) = &file_path {
                match tokio::fs::symlink_metadata(fp).await {
                    Ok(m) => {
                        if m.file_type().is_symlink() && text_refs.is_empty() {
                            remove(&file_path).await;
                            remove(&thumb_path).await;
                        }
                    }
                    Err(e) => tracing::error!("[DELETE_COMMENT] Failed to check stats for {dest}: {e}"),
                }
            }
            sql!("DELETE FROM comment_files WHERE id = ", fid).execute(&st.db).await?;
        }

        sql!("UPDATE comments SET is_deleted = true, content = '[deleted]' WHERE id = ", comment_id).execute(&st.db).await?;
        // The old payload carried the id as the route string.
        notify(&st, "comments", &json!({ "type": "delete", "item_id": comment.val("item_id"), "comment_id": comment_id.to_string() })).await;
        Ok(json_resp(&json!({ "success": true }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── POST /api/comments/attachment/delete ────────────────────────────────────

async fn delete_attachment(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if !is_admin_or_mod(&session) {
        return forbidden();
    }
    let body = req.post_value().await;
    let filename = if h::js_truthy(body.get("filename")) { js_str(body.get("filename")).unwrap_or_default() } else { String::new() };
    if filename.is_empty() {
        return j(json!({ "success": false, "message": "Missing filename" }), 200);
    }
    let r: anyhow::Result<axum::response::Response> = async {
        let cfg = st.cfg();
        let clean = filename.split('#').next().unwrap_or("").to_string();
        let Some(file) = sql!("SELECT id, comment_id, dest FROM comment_files WHERE dest = ", &clean).fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Attachment not found" }), 404));
        };
        let file_id = file.i64("id").unwrap_or(0) as i32;
        let file_dest = file.string("dest");
        let mut comment_id = file.i64("comment_id");
        let comment: Option<Row> = match comment_id {
            None => {
                let found = sql!("SELECT id, content, item_id FROM comments WHERE content LIKE ", format!("%{clean}%")).fetch_all(&st.db).await?;
                let first = found.into_iter().next();
                if let Some(f) = &first {
                    comment_id = f.i64("id");
                }
                first
            }
            Some(cid) => sql!("SELECT id, content, item_id FROM comments WHERE id = ", cid as i32).fetch_optional(&st.db).await?,
        };
        let (Some(comment_id), Some(comment)) = (comment_id, comment) else {
            sql!("DELETE FROM comment_files WHERE id = ", file_id).execute(&st.db).await?;
            if let Some(p) = crate::http::files::safe_join(&cfg.paths.c, &file_dest) {
                let _ = tokio::fs::remove_file(p).await;
            }
            return Ok(json_resp(&json!({ "success": true, "message": "Orphaned attachment deleted" }), 200));
        };

        let esc = regex::escape(&clean);
        let domain = r"(?:https?://[^/\s]+)?";
        let md_re = regex::Regex::new(&format!(r"!?\[[^\]]*\]\({domain}/c/{esc}(?:#gif)?\)"))?;
        let bare_re = regex::Regex::new(&format!(r"{domain}/c/{esc}(?:#gif)?"))?;
        let to_update = sql!("SELECT id, content FROM comments WHERE content LIKE ", format!("%{clean}%")).fetch_all(&st.db).await?;
        let mut final_content = comment.val("content").clone();
        for c in to_update {
            let original = c.string("content");
            let step = md_re.replace_all(&original, "[attachment removed]");
            let updated = bare_re.replace_all(&step, "[attachment removed]").into_owned();
            if updated != original {
                let cid = c.i64("id").unwrap_or(0);
                sql!("UPDATE comments SET content = ", &updated, ", updated_at = NOW() WHERE id = ", cid as i32).execute(&st.db).await?;
                if cid == comment_id {
                    final_content = json!(updated);
                }
                notify(&st, "comments", &json!({ "type": "edit", "comment_id": cid, "content": updated })).await;
            }
        }
        sql!("DELETE FROM comment_files WHERE id = ", file_id).execute(&st.db).await?;
        if let Some(p) = crate::http::files::safe_join(&cfg.paths.c, &file_dest) {
            let _ = tokio::fs::remove_file(p).await;
        }
        if let Some(p) = crate::http::files::safe_join(&cfg.paths.t, &format!("cf_{}.webp", filename.split('.').next().unwrap_or(""))) {
            let _ = tokio::fs::remove_file(p).await;
        }
        crate::audit::log(&st, session.id, "delete_attachment", "comment", comment_id.to_string(), json!({ "filename": filename, "old_content": comment.val("content"), "new_content": final_content }))
        .await;
        notify(&st, "comments", &json!({ "type": "edit", "item_id": comment.val("item_id"), "comment_id": comment_id, "content": final_content })).await;
        Ok(json_resp(&json!({ "success": true }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── POST /api/comments/:id/edit ─────────────────────────────────────────────

async fn edit_comment(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if !is_admin_or_mod(&session) {
        return forbidden();
    }
    let body = req.post_value().await;
    let content = match js_str(body.get("content")) {
        Some(c) => crate::wordfilter::apply_word_filter(&st, &c).await,
        None => String::new(),
    };
    if content.trim().is_empty() {
        return j(json!({ "success": false, "message": "Empty content" }), 200);
    }
    let Some(comment_id) = param_i32(&req, "id") else { return j(json!({ "success": false }), 500) };
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(comment) = sql!("SELECT id, user_id, item_id, content FROM comments WHERE id = ", comment_id).fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Not found" }), 404));
        };
        let old = comment.string("content");
        crate::audit::log(&st, session.id, "edit_comment", "comment", comment_id.to_string(), json!({ "item_id": comment.val("item_id"), "old_content": utf16_prefix(&old, 2000), "new_content": utf16_prefix(&content, 2000) }))
        .await;
        sql!("UPDATE comments SET content = ", &content, ", updated_at = NOW() WHERE id = ", comment_id).execute(&st.db).await?;
        notify(&st, "comments", &json!({ "type": "edit", "item_id": comment.val("item_id"), "comment_id": comment_id.to_string(), "content": content })).await;
        Ok(json_resp(&json!({ "success": true }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── POST /api/comments/:id/pin ──────────────────────────────────────────────

async fn pin_comment(st: AppState, req: Req) -> Res {
    let Some(session) = req.session() else { return unauthorized() };
    if !is_admin_or_mod(session) {
        return forbidden();
    }
    let Some(comment_id) = param_i32(&req, "id") else { return j(json!({ "success": false }), 500) };
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(c) = sql!("SELECT id, COALESCE(is_pinned, false) as is_pinned, item_id FROM comments WHERE id = ", comment_id).fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Not found" }), 404));
        };
        let pinned = !c.bool("is_pinned");
        sql!("UPDATE comments SET is_pinned = ", pinned, " WHERE id = ", comment_id).execute(&st.db).await?;
        crate::audit::log(&st, session.id, "pin_comment", "comment", comment_id.to_string(), json!({ "item_id": c.val("item_id"), "is_pinned": pinned })).await;
        Ok(json_resp(&json!({ "success": true, "is_pinned": pinned }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── POST /api/comments/:itemid/lock ─────────────────────────────────────────

async fn lock_thread(st: AppState, req: Req) -> Res {
    let Some(session) = req.session() else { return unauthorized() };
    if !is_admin_or_mod(session) {
        return forbidden();
    }
    let Some(item_id) = param_i32(&req, "itemid") else { return j(json!({ "success": false }), 500) };
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(item) = sql!("SELECT id, COALESCE(is_comments_locked, false) as is_locked FROM items WHERE id = ", item_id).fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Not found" }), 404));
        };
        let locked = !item.bool("is_locked");
        sql!("UPDATE items SET is_comments_locked = ", locked, " WHERE id = ", item_id).execute(&st.db).await?;
        crate::audit::log(&st, session.id, if locked { "lock_thread" } else { "unlock_thread" }, "item", item_id.to_string(), Value::Null).await;
        Ok(json_resp(&json!({ "success": true, "is_locked": locked }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── GET /activity ───────────────────────────────────────────────────────────

async fn activity(st: AppState, req: Req) -> Res {
    match activity_inner(&st, &req).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("{e:#}");
            Ok(reply(500, "text/html", "Error loading activity data"))
        }
    }
}

async fn activity_inner(st: &AppState, req: &Req) -> anyhow::Result<axum::response::Response> {
    let cfg = st.cfg();
    let session = req.session();
    let page_f = h::js_number(req.query("page").filter(|p| !p.is_empty()).unwrap_or("1"));
    let limit_f = h::js_number(req.query("limit").filter(|p| !p.is_empty()).unwrap_or("50")).map(|l| l.min(50.0));
    let offset_f = match req.query("offset") {
        Some(o) => h::js_number(o).map(|o| o.max(0.0)),
        None => match (page_f, limit_f) {
            (Some(p), Some(l)) => Some((p - 1.0) * l),
            _ => None,
        },
    };
    let limit = h::sql_count(limit_f).ok_or_else(|| anyhow::anyhow!("invalid limit"))?;
    let offset = h::sql_count(offset_f).ok_or_else(|| anyhow::anyhow!("invalid offset"))?;

    let mode = mode_override(req);
    let ratings = ratings_from_cookie(req);
    let modequery = compute_base_mode(&cfg, Some(mode), ratings.as_deref(), session).replace("items.id", "i.id");
    let global_tags = comment_globalfilter_tags(st, session);
    let excluded: Vec<i32> = session.map(|s| s.excluded_tags.iter().map(|t| *t as i32).collect()).unwrap_or_default();

    let visibility = match session {
        Some(s) if is_admin_or_mod(s) => Sql::empty(),
        Some(s) => sql!("AND (COALESCE(i.visibility, 0) = 0 OR LOWER(i.username) = ", s.user.to_lowercase(), " OR c.user_id = ", s.id as i32, ")"),
        None => Sql::new("AND COALESCE(i.visibility, 0) = 0"),
    };
    let mime_raw = req.query("mime").filter(|m| !m.is_empty()).or_else(|| req.cookie("mime").filter(|m| !m.is_empty()));
    let mime_sql = resolve_mime_sql(&cfg, mime_raw, session, "i");
    let nsfl = cfg.nsfl_tag_id() as i32;

    let mut q = sql!(
        r#"SELECT
                    c.*,
                    i.mime,
                    i.id as item_id,
                    i.slug as item_slug,
                    i.dest as item_dest,
                    i.has_coverart,
                    (SELECT ta.tag_id FROM tags_assign ta
                     WHERE ta.item_id = i.id AND ta.tag_id = ANY("#,
        vec![1i32, 2, nsfl],
        r#"::int[])
                     ORDER BY ta.tag_id LIMIT 1) AS rating_tag_id,
                    u.user as username,
                    uo.avatar,
                    uo.avatar_file,
                    uo.banner_file,
                    uo.banner_position,
                    uo.banner_size,
                    uo.banner_repeat,
                    uo.username_color,
                    uo.display_name
                FROM comments c
                LEFT JOIN items i ON c.item_id = i.id
                LEFT JOIN "user" u ON c.user_id = u.id
                LEFT JOIN user_options uo ON u.id = uo.user_id
                WHERE c.is_deleted = false
                AND i.active = true
                AND i.is_deleted = false "#,
        @mime_sql,
        " ",
        @visibility,
        " AND "
    );
    q.push(&modequery);
    if is_anon_session(session) && !global_tags.is_empty() {
        q.push(" and not exists (select 1 from tags_assign where item_id = i.id and (").push(&global_tag_sql(&global_tags)).push("))");
    }
    if !excluded.is_empty() {
        q.push(" and not exists (select 1 from tags_assign where item_id = i.id and tag_id = any(").bind(excluded).push("::int[]))");
    }
    q.push(" ORDER BY c.created_at DESC LIMIT ").bind(limit).push(" OFFSET ").bind(offset);
    let comments = q.fetch_all(&st.db).await?;

    let ids = sc::comment_ids(&comments);
    let mut files: HashMap<i64, Vec<Value>> = HashMap::new();
    if !comments.is_empty() {
        match sc::load_files(st, &ids).await {
            Ok(f) => files = f,
            Err(e) => tracing::error!("[ACTIVITY] Failed to fetch comment files: {e}"),
        }
    }
    let mut polls: HashMap<i64, Value> = HashMap::new();
    if !comments.is_empty() && polls_enabled(st) {
        match sc::load_polls(st, &ids, false).await {
            Ok(p) => polls = p,
            Err(e) => tracing::error!("[ACTIVITY] Failed to fetch polls: {e}"),
        }
    }

    let anonymized = is_anonymize_session(&cfg, session);
    let slugs = enable_item_slugs(&cfg);
    let processed: Vec<Value> = comments
        .into_iter()
        .map(|mut c| {
            strip_private_columns(&mut c);
            let (label, class) = rating_of(&cfg, c.i64("rating_tag_id"));
            let content = c.string("content").trim().to_string();
            let id = c.i64("id").unwrap_or(0);
            let f = files.remove(&id).unwrap_or_default();
            let long = is_long(&content, f.len());
            if !slugs {
                c.insert("item_slug".into(), Value::Null);
            }
            if anonymized {
                for (k, v) in [
                    ("username", json!("anonymous")),
                    ("display_name", Value::Null),
                    ("username_color", Value::Null),
                    ("avatar", Value::Null),
                    ("avatar_file", Value::Null),
                    ("banner_file", Value::Null),
                ] {
                    c.insert(k.into(), v);
                }
            }
            c.insert("content".into(), json!(content));
            c.insert("item_rating_class".into(), json!(class));
            c.insert("item_rating_label".into(), json!(label));
            c.insert("files".into(), Value::Array(f));
            c.insert("poll".into(), polls.remove(&id).unwrap_or(Value::Null));
            c.insert("is_long".into(), json!(long));
            Value::Object(c)
        })
        .collect();

    if req.query("json") == Some("true") || req.is_xhr() {
        let has_more = processed.len() as i64 == limit;
        let page = page_f.map(|p| json!(p)).unwrap_or(Value::Null);
        let page = match page_f {
            Some(p) if p.fract() == 0.0 && p.is_finite() => json!(p as i64),
            _ => page,
        };
        let mut r = json_resp(&json!({ "success": true, "comments": processed, "page": page, "hasMore": has_more }), 200);
        // The old handler meant to send these (res.reply dropped them); the feed is per-viewer.
        let hm = r.headers_mut();
        hm.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
        hm.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
        hm.insert(header::EXPIRES, HeaderValue::from_static("0"));
        return Ok(r);
    }

    // Plain HTML: the sidebar of visitors without JavaScript (the <noscript> iframe in
    // snippets/footer.html). With JavaScript the sidebar uses the JSON above and never loads this.
    let tr = st.i18n.translator(&crate::routes::index::req_lang(st, req));
    let now = chrono::Utc::now();
    let comments: Vec<Value> = processed
        .into_iter()
        .map(|mut c| {
            let ago = c["created_at"]
                .as_str()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .map(|t| crate::util::time_ago(&tr, (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0)))
                .unwrap_or_default();
            c["timeago"] = json!(ago);
            let item_key = match &c["item_slug"] {
                Value::String(s) if !s.is_empty() => s.clone(),
                _ => c["item_id"].as_i64().map(|i| i.to_string()).unwrap_or_default(),
            };
            let content = c["content"].as_str().unwrap_or("").to_string();
            let files = c["files"].as_array().cloned().unwrap_or_default();
            c["content_html"] = json!(frame_comment_html(&content, &format!("/{item_key}")) + &frame_attachments_html(&files, &content));
            c
        })
        .collect();
    let mut r = html(crate::tpl::render(st, Some(req), "activity-frame", json!({ "comments": comments, "anonymized": anonymized })));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(r)
}

/// Comment text as HTML for the no-JavaScript sidebar frame: a small server-side version of
/// renderCommentContent in sidebar-activity.js (greentext, >>id links, links, @mentions,
/// same-site images and #gif videos, [spoiler]/[blur]). Markdown and custom emoji stay plain text.
/// Everything is escaped; only the tags built here are HTML.
fn frame_comment_html(content: &str, link_base: &str) -> String {
    use crate::util::escape_html as esc;
    let html = content
        .split('\n')
        .map(|line| {
            let line = line.trim_end_matches('\r');
            let t = line.trim_start();
            let ctx = t.strip_prefix(">>").is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()));
            if t.starts_with('>') && !ctx {
                return format!("<span class=\"greentext\">{}</span>", esc(t));
            }
            if t.is_empty() {
                return "&nbsp;".to_string();
            }
            let mut out = String::new();
            let mut rest = line;
            while !rest.is_empty() {
                let split = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let (word, after) = rest.split_at(split);
                let ws = after.len() - after.trim_start().len();
                frame_word_html(word, link_base, &mut out);
                out.push_str(&after[..ws]);
                rest = &after[ws..];
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n");
    comment_fx(&frame_wrap_tags(&frame_wrap_tags(&html, "spoiler", "spoiler"), "blur", "blur-text"))
}

/// The comment text effects [rb] [reich] [krebs] [glitch], the same in every frontend: server copy
/// of `f0ckCommentFx` (public/s/js/f0ck-core.js); styles in public/s/css/comment-fx.css.
const COMMENT_FX: [(&str, &str); 4] = [
    ("rb", "class=\"cfx-rb\""),
    ("reich", "class=\"cfx-reich\""),
    ("krebs", "class=\"cfx-krebs\""),
    ("glitch", "class=\"cfx-glitch\" data-glitch=\"0.5\""),
];

fn comment_fx(html: &str) -> String {
    COMMENT_FX.iter().fold(html.to_string(), |s, (tag, attrs)| wrap_tags_with(&s, tag, attrs))
}

fn frame_word_html(word: &str, link_base: &str, out: &mut String) {
    use crate::util::escape_html as esc;
    if word.is_empty() {
        return;
    }
    // >>123 (comment on the same item), possibly followed by more text
    if let Some(r) = word.strip_prefix(">>") {
        let digits = r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits > 0 {
            let id = &r[..digits];
            out.push_str(&format!("<a href=\"{}#c{id}\" class=\"comment-context-link\">&gt;&gt;{id}</a>", esc(link_base)));
            return frame_word_html(&r[digits..], link_base, out);
        }
    }
    let external = word.starts_with("http://") || word.starts_with("https://");
    let relative = word.starts_with('/') && word[1..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if external || relative {
        let gif = word.ends_with("#gif");
        let url = word.trim_end_matches("#gif");
        let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let ext = path.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let u = esc(url);
        if relative && matches!(ext, "jpg" | "jpeg" | "png" | "gif" | "webp") {
            out.push_str(&format!("<img class=\"sidebar-comment-img\" src=\"{u}\" alt=\"image\" loading=\"lazy\">"));
        } else if relative && gif && matches!(ext, "mp4" | "webm" | "ogv" | "mov") {
            out.push_str(&format!("<span class=\"video-embed-wrap\"><video src=\"{u}\" class=\"sidebar-comment-img autoplay-gif\" autoplay loop muted playsinline preload=\"auto\"></video></span>"));
        } else if external {
            out.push_str(&format!("<a href=\"{u}\" target=\"_blank\" rel=\"noopener noreferrer\">{u}<i class=\"fa-solid fa-arrow-up-right-from-square external-link-icon\"></i></a>"));
        } else {
            out.push_str(&format!("<a href=\"{u}\">{u}</a>"));
        }
        return;
    }
    if let Some(name) = word.strip_prefix('@') {
        let len = name.len() - name.trim_start_matches(|c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).len();
        if len > 0 {
            let user = &name[..len];
            out.push_str(&format!("<a href=\"/user/{}\" class=\"mention\">@{}</a>", crate::util::encode_uri_component(user), esc(user)));
            out.push_str(&esc(&name[len..]));
            return;
        }
    }
    out.push_str(&esc(word));
}

/// An item's comments as template data for visitors without JavaScript (snippets/comments-static.html,
/// in a <noscript> inside #comments-container; comments.js replaces that box when it runs). Same
/// order and threading as comments.js render(): newest first (oldest first in the legacy layout or
/// linear mode), pinned on top, replies one level deep under their root, oldest first.
pub(crate) async fn static_item_comments(st: &AppState, page_session: Option<&Session>, real_session: Option<&Session>, item_id: i64, lang: &str) -> Value {
    let cfg = st.cfg();
    let legacy = match page_session {
        Some(s) => !s.use_new_layout.unwrap_or(false),
        None => st.settings.default_layout() == "legacy",
    };
    let linear = page_session.and_then(|s| s.comment_display_mode).or_else(|| crate::config::as_int(cfg.get("websrv.default_comment_display_mode"))) == Some(1);
    let sort = if legacy || linear { sc::Sort::Old } else { sc::Sort::New };
    let rows = sc::get_comments(st, &item_id.to_string(), sort, false).await;
    let anonymized = is_anonymize_session(&cfg, real_session);
    let tr = st.i18n.translator(lang);
    let now = chrono::Utc::now();

    // >>id quotes → backlinks on the quoted comment (buildBacklinkMap)
    let mut backlinks: HashMap<i64, Vec<i64>> = HashMap::new();
    for c in &rows {
        let (Some(id), Some(text)) = (c.i64("id"), c.str("content")) else { continue };
        for part in text.split(">>").skip(1) {
            let digits: String = part.chars().take_while(|ch| ch.is_ascii_digit()).collect();
            if let Ok(target) = digits.parse::<i64>() {
                let list = backlinks.entry(target).or_default();
                if target != id && !list.contains(&id) {
                    list.push(id);
                }
            }
        }
    }

    let view = |c: &Row| -> Value {
        let content = c.str("content").unwrap_or("").to_string();
        let files = c.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
        let anon = anonymized || c.get("is_anon").map(crate::config::truthy).unwrap_or(false) || c.str("username").is_some_and(|u| u == "anonymous" || u.starts_with("anon_"));
        let created = c.str("created_at").and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
        let avatar = if anonymized {
            "/a/default.png".to_string()
        } else if let Some(f) = c.str("avatar_file").filter(|f| !f.is_empty()) {
            format!("/a/{f}")
        } else if let Some(a) = c.get("avatar").and_then(|a| crate::config::as_int(a)) {
            format!("/t/{a}.webp")
        } else {
            "/a/default.png".to_string()
        };
        let banner = c.str("banner_file").filter(|b| !b.is_empty() && *b != "null" && !anonymized);
        let pos = c.str("banner_position").filter(|p| !p.is_empty() && *p != "center").unwrap_or("center top");
        let size = c.str("banner_size").filter(|p| !p.is_empty() && *p != "cover").unwrap_or("100% auto");
        json!({
            "id": c.i64("id"),
            "is_pinned": c.get("is_pinned").map(crate::config::truthy).unwrap_or(false),
            "is_anon": anon,
            "username": if anon { Value::Null } else { c.get("username").cloned().unwrap_or(Value::Null) },
            "name": if anon { json!("anonymous") } else { json!(c.str("display_name").filter(|d| !d.is_empty()).or(c.str("username")).unwrap_or("")) },
            "username_color": if anon { Value::Null } else { c.get("username_color").cloned().unwrap_or(Value::Null) },
            "avatar_src": avatar,
            "banner": banner.map(|b| json!({ "file": b, "position": pos, "size": size })),
            "content_html": frame_comment_html(&content, ""),
            "attachments_html": frame_attachments_html(&files, &content),
            "timeago": created.map(|t| crate::util::time_ago(&tr, (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0))).unwrap_or_default(),
            "iso": c.str("created_at").unwrap_or(""),
            "backlinks": c.i64("id").and_then(|id| backlinks.get(&id)).cloned().unwrap_or_default(),
            "replies": [],
        })
    };

    if linear {
        return Value::Array(rows.iter().map(view).collect());
    }
    let by_id: HashMap<i64, &Row> = rows.iter().filter_map(|c| Some((c.i64("id")?, c))).collect();
    let root_of = |c: &Row| -> Option<i64> {
        let mut cur = c;
        let mut hops = 0;
        while let Some(p) = cur.i64("parent_id").filter(|p| *p != 0).and_then(|p| by_id.get(&p)) {
            cur = p;
            hops += 1;
            if hops > 1000 {
                break;
            }
        }
        cur.i64("id").filter(|r| Some(*r) != c.i64("id"))
    };
    let mut roots: Vec<Value> = Vec::new();
    let mut replies: HashMap<i64, Vec<(String, Value)>> = HashMap::new();
    for c in &rows {
        match root_of(c) {
            Some(root) => replies.entry(root).or_default().push((c.str("created_at").unwrap_or("").to_string(), view(c))),
            None => roots.push(view(c)),
        }
    }
    for r in roots.iter_mut() {
        if let Some(mut list) = r["id"].as_i64().and_then(|id| replies.remove(&id)) {
            list.sort_by(|a, b| a.0.cmp(&b.0));
            r["replies"] = Value::Array(list.into_iter().map(|(_, v)| v).collect());
        }
    }
    Value::Array(roots)
}

/// [tag]…[/tag] → <span class="…">…</span>, innermost pair first (so nesting works); unmatched
/// tags stay as text.
fn frame_wrap_tags(html: &str, tag: &str, class: &str) -> String {
    wrap_tags_with(html, tag, &format!("class=\"{class}\""))
}

/// `[tag]…[/tag]` → `<span attrs>…</span>`, innermost first (up to 10).
fn wrap_tags_with(html: &str, tag: &str, attrs: &str) -> String {
    let (open, close) = (format!("[{tag}]"), format!("[/{tag}]"));
    let mut s = html.to_string();
    for _ in 0..10 {
        let lower = s.to_ascii_lowercase();
        let Some(c) = lower.find(&close) else { break };
        let Some(o) = lower[..c].rfind(&open) else { break };
        s = format!("{}<span {attrs}>{}</span>{}", &s[..o], &s[o + open.len()..c], &s[c + close.len()..]);
    }
    s
}

/// Attached files not already shown inline in the text (as renderCommentAttachments).
fn frame_attachments_html(files: &[Value], content: &str) -> String {
    use crate::util::escape_html as esc;
    let items: String = files
        .iter()
        .filter_map(|f| {
            let dest = f["dest"].as_str()?;
            let url = format!("/c/{dest}");
            if content.contains(&url) {
                return None;
            }
            let (mime, u) = (f["mime"].as_str().unwrap_or(""), esc(&url));
            let name = esc(f["original_filename"].as_str().unwrap_or("image"));
            Some(if mime.starts_with("image/") {
                format!("<a href=\"{u}\" class=\"cf-attachment cf-image\"><img src=\"{u}\" class=\"sidebar-comment-img\" alt=\"{name}\" loading=\"lazy\"></a>")
            } else if mime.starts_with("video/") {
                format!("<div class=\"cf-attachment cf-video\"><video src=\"{u}\" class=\"sidebar-comment-img\" controls preload=\"metadata\"></video></div>")
            } else if mime.starts_with("audio/") {
                format!("<div class=\"cf-attachment cf-audio\"><audio src=\"{u}\" controls preload=\"metadata\"></audio></div>")
            } else {
                format!("<a href=\"{u}\" class=\"cf-attachment\">{name}</a>")
            })
        })
        .collect();
    if items.is_empty() { items } else { format!("<div class=\"comment-attachments\">{items}</div>") }
}

// ── POST /api/v2/user/subscribe-all-uploads ─────────────────────────────────

async fn subscribe_all_uploads(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return j(json!({ "success": false, "message": "Unauthorized" }), 401) };
    let r = sql!(
        "INSERT INTO comment_subscriptions (user_id, item_id) SELECT ",
        s.id as i32,
        ", i.id FROM items i WHERE i.username ILIKE ",
        crate::util::escape_like(&s.login),
        " OR i.username ILIKE ",
        crate::util::escape_like(&s.user),
        " ON CONFLICT DO NOTHING"
    )
    .execute(&st.db)
    .await;
    match r {
        Ok(_) => j(json!({ "success": true, "message": "Successfully subscribed to your uploads" }), 200),
        Err(e) => {
            tracing::error!("[API] Failed to subscribe to all uploads: {e}");
            j(json!({ "success": false, "message": "Internal server error" }), 500)
        }
    }
}

// ── Polls ───────────────────────────────────────────────────────────────────

/// `createPollForComment(commentId, pollData)` — `Ok(None)` for invalid poll data.
async fn create_poll_for_comment(st: &AppState, comment_id: i32, data: &Value) -> anyhow::Result<Option<Value>> {
    if !polls_enabled(st) {
        return Ok(None);
    }
    let question = match data.get("question") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(q)) => q.trim().to_string(),
        Some(Value::Bool(false)) => return Ok(None),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => return Ok(None),
        // `question.trim` is not a function → the old route's 500
        Some(_) => anyhow::bail!("question.trim is not a function"),
    };
    if question.is_empty() {
        return Ok(None);
    }
    let Some(Value::Array(options)) = data.get("options") else { return Ok(None) };
    if options.len() < 2 {
        return Ok(None);
    }
    let clean: Vec<String> = options.iter().map(|o| js_str(Some(o)).unwrap_or_else(|| "null".into()).trim().to_string()).filter(|o| !o.is_empty()).collect();
    if clean.len() < 2 || clean.len() > 10 {
        return Ok(None);
    }
    let anonymous = data.get("is_anonymous") != Some(&Value::Bool(false));
    let poll = sql!("INSERT INTO comment_polls (comment_id, question, is_anonymous) VALUES (", comment_id, ", ", &question, ", ", anonymous, ") RETURNING id, is_anonymous")
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("poll insert returned nothing"))?;
    let poll_id = poll.i64("id").unwrap_or(0) as i32;
    for (i, text) in clean.iter().enumerate() {
        sql!("INSERT INTO comment_poll_options (poll_id, text, sort_order) VALUES (", poll_id, ", ", text, ", ", i as i16, ")").execute(&st.db).await?;
    }
    let opts = sql!("SELECT id, text, sort_order FROM comment_poll_options WHERE poll_id = ", poll_id, " ORDER BY sort_order ASC").fetch_all(&st.db).await?;
    let options: Vec<Value> = opts
        .into_iter()
        .map(|o| json!({ "id": o.val("id"), "text": o.val("text"), "sort_order": o.val("sort_order"), "vote_count": 0, "voters": [] }))
        .collect();
    Ok(Some(json!({
        "id": poll_id,
        "question": question,
        "is_anonymous": poll.val("is_anonymous"),
        "options": options,
        "total_votes": 0,
        "user_vote_option_id": null
    })))
}

async fn poll_attach(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if !polls_enabled(&st) {
        return j(json!({ "success": false, "message": "Polls disabled" }), 403);
    }
    let Some(comment_id) = param_i32(&req, "commentId") else { return j(json!({ "success": false, "message": "Database error" }), 500) };
    let body = req.post_value().await;
    // The old handler had no try/catch around these lookups (a DB error left the request hanging).
    let comment = sql!("SELECT id, user_id FROM comments WHERE id = ", comment_id, " AND is_deleted = false LIMIT 1").fetch_optional(&st.db).await?;
    let Some(comment) = comment else { return j(json!({ "success": false, "message": "Comment not found" }), 404) };
    if comment.i64("user_id") != Some(session.id) && !is_admin_or_mod(&session) {
        return forbidden();
    }
    if sql!("SELECT id FROM comment_polls WHERE comment_id = ", comment_id, " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
        return j(json!({ "success": false, "message": "Poll already exists" }), 409);
    }
    let data = match body.get("poll") {
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(v) => v,
            Err(_) => return j(json!({ "success": false, "message": "Invalid poll JSON" }), 400),
        },
        Some(v) => v.clone(),
        None => Value::Null,
    };
    match create_poll_for_comment(&st, comment_id, &data).await {
        Ok(Some(poll)) => j(json!({ "success": true, "poll": poll }), 200),
        Ok(None) => j(json!({ "success": false, "message": "Invalid poll data (need question + 2-10 options)" }), 400),
        Err(e) => {
            tracing::error!("[POLLS] createPollForComment error: {e:#}");
            j(json!({ "success": false, "message": "Database error" }), 500)
        }
    }
}

async fn poll_get(st: AppState, req: Req) -> Res {
    if !polls_enabled(&st) {
        return j(json!({ "success": false }), 403);
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return j(json!({ "success": false }), 500) };
    let r: anyhow::Result<axum::response::Response> = async {
        let p = sql!(
            r#"SELECT
                    cp.id as poll_id, cp.comment_id, cp.question, cp.expires_at, COALESCE(cp.is_anonymous, true) as is_anonymous,
                    json_agg(
                        json_build_object(
                            'id', cpo.id, 'text', cpo.text, 'sort_order', cpo.sort_order,
                            'vote_count', COALESCE(vc.cnt, 0)
                        ) ORDER BY cpo.sort_order ASC, cpo.id ASC
                    ) AS options,
                    COALESCE(SUM(vc.cnt), 0)::int AS total_votes
                FROM comment_polls cp
                JOIN comment_poll_options cpo ON cpo.poll_id = cp.id
                LEFT JOIN (SELECT option_id, COUNT(*) AS cnt FROM comment_poll_votes GROUP BY option_id) vc ON vc.option_id = cpo.id
                WHERE cp.id = "#,
            poll_id,
            " GROUP BY cp.id, cp.comment_id, cp.question, cp.expires_at, cp.is_anonymous"
        )
        .fetch_optional(&st.db)
        .await?;
        let Some(p) = p else { return Ok(json_resp(&json!({ "success": false }), 404)) };
        let mut user_vote = Value::Null;
        if let Some(s) = req.session() {
            if let Some(v) = sql!("SELECT option_id FROM comment_poll_votes WHERE poll_id = ", poll_id, " AND user_id = ", s.id as i32, " LIMIT 1").fetch_optional(&st.db).await? {
                user_vote = v.val("option_id").clone();
            }
        }
        let mut options = p.val("options").clone();
        if !p.bool("is_anonymous") {
            let voters = sc::load_voters(&st, &[poll_id]).await?;
            sc::add_voters(&mut options, &voters);
        }
        Ok(json_resp(
            &json!({
                "success": true,
                "poll": {
                    "id": p.val("poll_id"),
                    "comment_id": p.val("comment_id"),
                    "question": p.val("question"),
                    "expires_at": p.val("expires_at"),
                    "is_anonymous": p.val("is_anonymous"),
                    "options": options,
                    "total_votes": p.i64("total_votes").unwrap_or(0),
                    "user_vote_option_id": user_vote
                }
            }),
            200,
        ))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[POLLS] GET error: {e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn poll_vote(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if is_anon_session(Some(&session)) && !can_anon_do(&cfg, "poll_vote") {
        return j(json!({ "success": false, "message": "Anonymous poll voting is disabled" }), 403);
    }
    if !polls_enabled(&st) {
        return j(json!({ "success": false }), 403);
    }
    let body = req.post_value().await;
    let option_id = js_str(body.get("option_id")).as_deref().and_then(js_parse_int).filter(|o| *o != 0);
    let Some(option_id) = option_id else { return j(json!({ "success": false, "message": "Missing option_id" }), 400) };
    let Some(poll_id) = param_i32(&req, "pollId") else { return j(json!({ "success": false }), 500) };
    let me = session.id as i32;
    let r: anyhow::Result<axum::response::Response> = async {
        let option_id = i32::try_from(option_id)?;
        if sql!("SELECT id FROM comment_poll_options WHERE id = ", option_id, " AND poll_id = ", poll_id, " LIMIT 1").fetch_optional(&st.db).await?.is_none() {
            return Ok(json_resp(&json!({ "success": false, "message": "Invalid option" }), 400));
        }
        let Some(poll) = sql!("SELECT expires_at FROM comment_polls WHERE id = ", poll_id, " LIMIT 1").fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false }), 404));
        };
        if let Some(exp) = poll.str("expires_at").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()) {
            if exp < chrono::Utc::now() {
                return Ok(json_resp(&json!({ "success": false, "message": "Poll has expired" }), 403));
            }
        }
        let existing = sql!("SELECT poll_id FROM comment_poll_votes WHERE poll_id = ", poll_id, " AND user_id = ", me, " LIMIT 1").fetch_optional(&st.db).await?;
        if existing.is_some() {
            sql!("UPDATE comment_poll_votes SET option_id = ", option_id, ", created_at = now() WHERE poll_id = ", poll_id, " AND user_id = ", me).execute(&st.db).await?;
        } else {
            sql!("INSERT INTO comment_poll_votes (poll_id, option_id, user_id) VALUES (", poll_id, ", ", option_id, ", ", me, ")").execute(&st.db).await?;
        }
        if session.is_anon {
            log_anon_activity(&st, &req, "poll_vote", Some(poll_id as i64), Some(json!({ "option_id": option_id }))).await;
        }
        let is_anon = sql!("SELECT COALESCE(is_anonymous, true) as is_anonymous FROM comment_polls WHERE id = ", poll_id, " LIMIT 1")
            .fetch_optional(&st.db)
            .await?
            .map(|r| r.bool("is_anonymous"))
            .unwrap_or(true);
        let rows = sql!(
            "SELECT cpo.id, cpo.text, cpo.sort_order, COALESCE(vc.cnt, 0)::int AS vote_count FROM comment_poll_options cpo LEFT JOIN (SELECT option_id, COUNT(*) AS cnt FROM comment_poll_votes WHERE poll_id = ",
            poll_id,
            " GROUP BY option_id) vc ON vc.option_id = cpo.id WHERE cpo.poll_id = ",
            poll_id,
            " ORDER BY cpo.sort_order ASC"
        )
        .fetch_all(&st.db)
        .await?;
        let total: i64 = rows.iter().filter_map(|r| r.i64("vote_count")).sum();
        let mut options = sc::rows_to_value(rows);
        if !is_anon {
            let voters = sc::load_voters(&st, &[poll_id]).await?;
            sc::add_voters(&mut options, &voters);
        }
        Ok(json_resp(&json!({ "success": true, "is_anonymous": is_anon, "options": options, "total_votes": total, "user_vote_option_id": option_id }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[POLLS] vote error: {e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn poll_delete(st: AppState, req: Req) -> Res {
    let Some(session) = req.session() else { return unauthorized() };
    if !polls_enabled(&st) {
        return j(json!({ "success": false }), 403);
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return j(json!({ "success": false }), 500) };
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(poll) = sql!("SELECT cp.id, cp.comment_id, c.user_id FROM comment_polls cp JOIN comments c ON c.id = cp.comment_id WHERE cp.id = ", poll_id, " LIMIT 1")
            .fetch_optional(&st.db)
            .await?
        else {
            return Ok(json_resp(&json!({ "success": false }), 404));
        };
        if poll.i64("user_id") != Some(session.id) && !is_admin_or_mod(session) {
            return Ok(json_resp(&json!({ "success": false, "message": "Forbidden" }), 403));
        }
        sql!("DELETE FROM comment_polls WHERE id = ", poll_id).execute(&st.db).await?;
        notify(&st, "comments", &json!({ "type": "poll_deleted", "poll_id": poll_id.to_string(), "comment_id": poll.val("comment_id") })).await;
        Ok(json_resp(&json!({ "success": true }), 200))
    }
    .await;
    match r {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[POLLS] delete error: {e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn greentext_quotes_and_context_links() {
        let h = frame_comment_html(">>7 \n>x\n>foobar", "/10");
        assert_eq!(h, "<a href=\"/10#c7\" class=\"comment-context-link\">&gt;&gt;7</a> \n<span class=\"greentext\">&gt;x</span>\n<span class=\"greentext\">&gt;foobar</span>");
    }

    #[test]
    fn site_images_links_and_escaping() {
        let h = frame_comment_html("/c/ab.webp#gif hi <b> https://x.org/a?b=1&c @bob, [spoiler]s[/spoiler]", "/3");
        assert!(h.starts_with("<img class=\"sidebar-comment-img\" src=\"/c/ab.webp\""));
        assert!(h.contains("hi &lt;b&gt; "));
        assert!(h.contains("<a href=\"https://x.org/a?b=1&amp;c\" target=\"_blank\""));
        assert!(h.contains("<a href=\"/user/bob\" class=\"mention\">@bob</a>,"));
        assert!(h.ends_with("<span class=\"spoiler\">s</span>"));
        assert!(!frame_comment_html("//evil.com/x.png javascript:alert(1)", "1").contains("<img"));
        assert_eq!(frame_comment_html("a\n\nb", "1"), "a\n&nbsp;\nb");
    }

    #[test]
    fn comment_effects() {
        let h = frame_comment_html("[rb]a [glitch]b[/glitch][/rb] [REICH]c[/reich] [krebs]d", "1");
        assert_eq!(h, "<span class=\"cfx-rb\">a <span class=\"cfx-glitch\" data-glitch=\"0.5\">b</span></span> <span class=\"cfx-reich\">c</span> [krebs]d");
    }

    #[test]
    fn attachments_skip_inline_files() {
        let files = vec![json!({"dest": "ab.webp", "mime": "image/webp"}), json!({"dest": "x.mp4", "mime": "video/mp4"})];
        let h = frame_attachments_html(&files, "/c/ab.webp#gif");
        assert!(!h.contains("ab.webp"));
        assert!(h.contains("/c/x.mp4"));
    }
}
