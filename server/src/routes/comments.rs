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
use crate::social::polls;
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
    r.get(r"\/api\/polls\/(?<pollId>\d+)\/voters", poll_voters);
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

    // Comments of a private / unavailable item only for those who may see the item
    match crate::f0cklib::may_access_item(&st, session, item_id).await {
        Ok(true) => {}
        Ok(false) => return j(json!({ "success": false, "message": "Item not found" }), 404),
        Err(e) => {
            tracing::error!("{e:#}");
            return db_error();
        }
    }
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

        polls::attach_polls(&st, &mut comments, polls::Viewer::of(&st, session)).await;

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

// ── GET /api/comment/:id ────────────────────────────────────────────────────

async fn get_single_comment(st: AppState, req: Req) -> Res {
    if req.session().is_none() && st.cfg().truthy("main.hide_comments_from_public") {
        return j(json!({ "success": false, "message": "Unauthorized" }), 401);
    }
    // getComment swallowed the query error for ids beyond int4 → "not found".
    let id = param_i32(&req, "id").map(i64::from).unwrap_or(0);
    let comment = sc::get_comment(&st, id, true).await;
    let item_id = comment.as_ref().and_then(|c| c.i64("item_id")).and_then(|i| i32::try_from(i).ok());
    let visible = match item_id {
        Some(i) => crate::f0cklib::may_access_item(&st, req.session(), i).await.unwrap_or(false),
        None => true,
    };
    match comment.filter(|_| visible) {
        Some(c) => {
            let mut one = [c];
            polls::attach_polls(&st, &mut one, polls::Viewer::of(&st, req.session())).await;
            let [c] = one;
            j(json!({ "success": true, "comment": c }), 200)
        }
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
        r#"SELECT "user".id, "user".user, user_options.avatar, user_options.avatar_file, user_options.username_color,
                       user_options.display_name, user_options.banner_file, user_options.banner_position, user_options.banner_size
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
    // Comments on private items only for their owner, on unavailable ones only for admins
    if !session.is_some_and(|s| s.admin) {
        q.push(" AND (COALESCE(i.visibility, 0) = 0");
        if let Some(owner) = crate::settings::session_owner_name(session) {
            q.push(" OR (lower(i.username) = ").bind(owner.to_lowercase()).push(" AND i.visibility != 3)");
        }
        q.push(")");
    }
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
        // the page itself also gets every voter (its no-JavaScript "+N" dialog); the JSON doesn't
        let viewer = if is_json { polls::Viewer::of(st, session) } else { polls::Viewer::page(st, session) };
        polls::attach_polls(st, &mut processed, viewer).await;
    }

    let comments_value = sc::rows_to_value(processed);
    if is_json {
        return Ok(json_resp(&json!({ "success": true, "comments": comments_value, "user": u }), 200));
    }
    // The page's comments for visitors without JavaScript (comments_user-partial.html, hidden when
    // user_comments.js runs): the text rendered like the sidebar frame's, >>id quotes pointing at
    // the comment's item.
    let hosts = site_hosts(&cfg, &req.host_without_port());
    let tr = st.i18n.translator(&crate::routes::index::req_lang(st, req));
    let now = chrono::Utc::now();
    let static_comments: Vec<Value> = comments_value
        .as_array()
        .map(|list| {
            list.iter()
                .map(|c| {
                    let content = c["content"].as_str().unwrap_or("");
                    let files = c["files"].as_array().cloned().unwrap_or_default();
                    let item_key = c["item_slug"].as_str().filter(|s| !s.is_empty()).map(str::to_string).or_else(|| c["item_id"].as_i64().map(|i| i.to_string())).unwrap_or_default();
                    let created = c["created_at"].as_str().and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok());
                    json!({
                        "id": c["id"],
                        "item_id": c["item_id"],
                        "item_key": item_key,
                        "content_html": frame_comment_html(content, &format!("/{item_key}"), &hosts),
                        "attachments_html": frame_attachments_html(&files, content),
                        "poll": c["poll"],
                        "timeago": created.map(|t| crate::util::time_ago(&tr, (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0))).unwrap_or_default(),
                        "iso": c["created_at"],
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let comments_page = page.unwrap_or(1.0).max(1.0) as i64;
    let data = json!({
        "user": u,
        "comments": comments_value,
        "static_comments": static_comments,
        "comments_page": comments_page,
        "has_more": static_comments.len() as i64 >= limit,
        "hidePagination": true,
        "tmp": null,
    });
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
    // A poll comes with the comment (the scripts' `poll` JSON or the plain form's fields) and is
    // checked first: a broken poll is reported instead of posting the comment without it.
    let poll = if polls::enabled(&st) {
        match polls::draft_from_body(&body) {
            Ok(Some(d)) => Some(polls::filter_words(&st, d).await),
            Ok(None) => None,
            Err(m) => return j(json!({ "success": false, "message": m }), 400),
        }
    } else {
        None
    };

    if content.trim().is_empty() && file_ids.is_empty() && poll.is_none() {
        return j(json!({ "success": false, "message": "Empty comment" }), 200);
    }
    if let Some(max) = cfg.get("main.comment_max_length").as_f64() {
        if utf16_len(&content) as f64 > max {
            return j(json!({ "success": false, "message": format!("Comment too long (max {} characters)", crate::util::js_num(max)) }), 400);
        }
    }

    match post_comment_inner(&st, &req, &session, item_id, parent_id, content, video_time, file_ids, poll).await {
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
    poll: Option<polls::Draft>,
) -> anyhow::Result<axum::response::Response> {
    let cfg = st.cfg();
    // NaN / out-of-range ids made the old queries throw (→ 500 "Database error").
    let item_id: i32 = item_id.and_then(|i| i32::try_from(i).ok()).ok_or_else(|| anyhow::anyhow!("invalid item_id"))?;
    let parent_id: Option<i32> = match parent_id {
        Some(p) => Some(i32::try_from(p).map_err(|_| anyhow::anyhow!("invalid parent_id"))?),
        None => None,
    };
    let me = session.id as i32;

    if !crate::f0cklib::may_access_item(st, Some(session), item_id).await? {
        return Ok(json_resp(&json!({ "success": false, "message": "Item not found" }), 404));
    }

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
    // the comment and its poll together, or neither
    let mut tx = st.db.begin().await?;
    let new_comment = q.fetch_optional(&mut *tx).await?.ok_or_else(|| anyhow::anyhow!("insert returned nothing"))?;
    let comment_id = new_comment.i64("id").unwrap_or(0);
    let poll_id = match &poll {
        Some(d) => Some(polls::create(&mut *tx, comment_id as i32, d).await?),
        None => None,
    };
    tx.commit().await?;

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
        "anon_short_fingerprint": short_fp,
        // the poll is per viewer: the pages fetch it (GET /api/polls/:id)
        "poll_id": poll_id,
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
    let poll = match poll_id {
        Some(id) => polls::load_poll(st, id as i32, polls::Viewer::of(st, Some(session))).await.unwrap_or_else(|e| {
            tracing::error!("[POLLS] load after posting: {e:#}");
            None
        }),
        None => None,
    };
    comment.insert("poll".into(), poll.unwrap_or(Value::Null));
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
    if !crate::f0cklib::may_access_item(&st, Some(s), item_id).await.unwrap_or(false) {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    }
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
    let mut polls = polls::load_polls(st, &ids, polls::Viewer::of(st, session)).await.unwrap_or_else(|e| {
        tracing::error!("[ACTIVITY] Failed to fetch polls: {e}");
        HashMap::new()
    });

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
    let hosts = site_hosts(&st.cfg(), &req.host_without_port());
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
            c["content_html"] = json!(frame_comment_html(&content, &format!("/{item_key}"), &hosts) + &frame_attachments_html(&files, &content));
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
/// Everything is escaped; only the tags built here are HTML. Full URLs to one of `hosts` (see
/// [`site_hosts`]) show as their path, like in the JS renderers.
fn frame_comment_html(content: &str, link_base: &str, hosts: &[String]) -> String {
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
                frame_word_html(word, link_base, hosts, &mut out);
                out.push_str(&after[..ws]);
                rest = &after[ws..];
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n");
    let html = bad_apple_links(&html);
    comment_fx(&frame_wrap_tags(&frame_wrap_tags(&html, "spoiler", "spoiler"), "blur", "blur-text"))
}

/// `[label](#bad-apple)` as the link that starts Bad Apple!! (f0ck-core.js), the one bit of Markdown
/// these comments render, like f0ckMotdLinks does for the MOTD; `[](#bad-apple)` is just the
/// apple (comment-fx.css). Runs on the escaped text, so the label is escaped already; one that
/// holds a tag built here (a mention, a link) stays text.
fn bad_apple_links(html: &str) -> String {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"\[([^\]\n<]*)\]\(#bad-apple\)").expect("bad apple regex"));
    RE.replace_all(html, "<a href=\"#bad-apple\">$1</a>").into_owned()
}

/// The comment text effects [rb] [reich] [krebs] [glitch] [badapple], the same in every frontend:
/// server copy of `f0ckCommentFx` (public/s/js/f0ck-core.js); styles in public/s/css/comment-fx.css.
/// [badapple] starts Bad Apple!! when clicked (f0ck-core.js; with JavaScript only).
const COMMENT_FX: [(&str, &str); 5] = [
    ("rb", "class=\"cfx-rb\""),
    ("reich", "class=\"cfx-reich\""),
    ("krebs", "class=\"cfx-krebs\""),
    ("glitch", "class=\"cfx-glitch\" data-glitch=\"0.5\""),
    ("badapple", "class=\"cfx-badapple\" role=\"button\" tabindex=\"0\""),
];

fn comment_fx(html: &str) -> String {
    COMMENT_FX.iter().fold(html.to_string(), |s, (tag, attrs)| wrap_tags_with(&s, tag, attrs))
}

/// Hosts of this site for [`site_path`]: the configured domain and the requested host, lowercase,
/// without `www.` and port.
fn site_hosts(cfg: &crate::config::Config, req_host: &str) -> Vec<String> {
    let mut hosts: Vec<String> = [cfg.domain(), req_host].iter().map(|h| norm_host(h)).filter(|h| !h.is_empty()).collect();
    hosts.dedup();
    hosts
}

fn norm_host(h: &str) -> String {
    let h = h.trim().to_ascii_lowercase();
    let h = h.split(':').next().unwrap_or("");
    h.strip_prefix("www.").unwrap_or(h).to_string()
}

/// A full URL to this site (any of `hosts`) as its path: `https://f0ck.me/123-slug` → `/123-slug`.
/// Server copy of `f0ckSitePath` in public/s/js/f0ck-core.js.
fn site_path(url: &str, hosts: &[String]) -> Option<String> {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &url[8..]
    } else if lower.starts_with("http://") {
        &url[7..]
    } else {
        return None;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, path) = rest.split_at(end);
    let host = norm_host(host);
    if host.is_empty() || !hosts.contains(&host) {
        return None;
    }
    Some(if path.starts_with('/') { path.to_string() } else { format!("/{path}") })
}

fn frame_word_html(word: &str, link_base: &str, hosts: &[String], out: &mut String) {
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
            return frame_word_html(&r[digits..], link_base, hosts, out);
        }
    }
    let external = word.starts_with("http://") || word.starts_with("https://");
    let relative = word.starts_with('/') && word[1..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if external || relative {
        let gif = word.ends_with("#gif");
        let url = word.trim_end_matches("#gif");
        // a full URL to this site counts as a relative one (shown as its path)
        let site = if external { site_path(url, hosts) } else { None };
        let (url, relative, external) = match &site {
            Some(p) => (p.as_str(), true, false),
            None => (url, relative, external),
        };
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
pub(crate) async fn static_item_comments(st: &AppState, page_session: Option<&Session>, real_session: Option<&Session>, item_id: i64, lang: &str, req_host: &str) -> Value {
    let cfg = st.cfg();
    let hosts = site_hosts(&cfg, req_host);
    let legacy = match page_session {
        Some(s) => !s.use_new_layout.unwrap_or(false),
        None => st.settings.default_layout() == "legacy",
    };
    let linear = page_session.and_then(|s| s.comment_display_mode).or_else(|| crate::config::as_int(cfg.get("websrv.default_comment_display_mode"))) == Some(1);
    let sort = if legacy || linear { sc::Sort::Old } else { sc::Sort::New };
    let mut rows = sc::get_comments(st, &item_id.to_string(), sort, false).await;
    polls::attach_polls(st, &mut rows, polls::Viewer::page(st, real_session)).await;
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

    // everything that answers a comment: its direct replies (parent_id) and the >>id quotes;
    // w0bm lists these on the answered comment (↳ >>36 >>37)
    let mut replied_by: HashMap<i64, Vec<i64>> = backlinks.clone();
    for c in &rows {
        if let (Some(id), Some(parent)) = (c.i64("id"), c.i64("parent_id").filter(|p| *p != 0)) {
            let list = replied_by.entry(parent).or_default();
            if parent != id && !list.contains(&id) {
                list.push(id);
            }
        }
    }
    for list in replied_by.values_mut() {
        list.sort_unstable();
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
            // the text as written, for the mods' edit field (the frontends' item pages)
            "content": &content,
            "content_html": frame_comment_html(&content, "", &hosts),
            "attachments_html": frame_attachments_html(&files, &content),
            "poll": c.get("poll").cloned().unwrap_or(Value::Null),
            "timeago": created.map(|t| crate::util::time_ago(&tr, (now - t.with_timezone(&chrono::Utc)).num_seconds().max(0))).unwrap_or_default(),
            "iso": c.str("created_at").unwrap_or(""),
            "backlinks": c.i64("id").and_then(|id| backlinks.get(&id)).cloned().unwrap_or_default(),
            "replied_by": c.i64("id").and_then(|id| replied_by.get(&id)).cloned().unwrap_or_default(),
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
// The poll logic lives in social/polls.rs. Every answer that changes a poll carries the poll as the
// asker now sees it (`poll`), and a live `comments` event `{ type: "poll" | "poll_deleted" }` tells
// the other pages to fetch it again (it is per viewer, so it isn't sent along).

/// A poll, the comment it hangs on and that comment's item; `None` when the poll is gone, its
/// comment deleted, or the item hidden from this visitor.
async fn poll_context(st: &AppState, session: Option<&Session>, poll_id: i32) -> anyhow::Result<Option<Row>> {
    let row = sql!(
        r#"SELECT cp.id, cp.comment_id, c.item_id, c.user_id AS author_id, cp.question,
                  (cp.expires_at IS NOT NULL AND cp.expires_at <= now()) AS is_closed
             FROM comment_polls cp JOIN comments c ON c.id = cp.comment_id
            WHERE cp.id = "#,
        poll_id,
        " AND c.is_deleted = false"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let item_id = row.i64("item_id").and_then(|i| i32::try_from(i).ok()).unwrap_or(0);
    Ok(if crate::f0cklib::may_access_item(st, session, item_id).await? { Some(row) } else { None })
}

async fn notify_poll(st: &AppState, kind: &str, ctx: &Row) {
    notify(st, "comments", &json!({ "type": kind, "poll_id": ctx.val("id"), "comment_id": ctx.val("comment_id"), "item_id": ctx.val("item_id") })).await;
}

fn poll_error(code: u16, message: &str) -> Res {
    j(json!({ "success": false, "message": message }), code)
}

/// `POST /api/polls/attach/:commentId` — a poll for an existing comment (its author or a mod).
/// The comment forms send the poll with the comment itself; this stays for API clients.
async fn poll_attach(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if !polls::enabled(&st) {
        return poll_error(403, "Polls are disabled");
    }
    let Some(comment_id) = param_i32(&req, "commentId") else { return poll_error(404, "Comment not found") };
    let body = req.post_value().await;
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(comment) = sql!("SELECT id, item_id, user_id FROM comments WHERE id = ", comment_id, " AND is_deleted = false").fetch_optional(&st.db).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Comment not found" }), 404));
        };
        if comment.i64("user_id") != Some(session.id) && !is_admin_or_mod(&session) {
            return Ok(json_resp(&json!({ "success": false, "message": "Forbidden" }), 403));
        }
        let draft = match polls::draft_from_body(&body) {
            Ok(Some(d)) => polls::filter_words(&st, d).await,
            Ok(None) => return Ok(json_resp(&json!({ "success": false, "message": "The poll needs a question" }), 400)),
            Err(m) => return Ok(json_resp(&json!({ "success": false, "message": m }), 400)),
        };
        let poll_id = match polls::create(&st.db, comment_id, &draft).await {
            Ok(id) => id,
            Err(e) if crate::db::pg_code(&e).as_deref() == Some("23505") => {
                return Ok(json_resp(&json!({ "success": false, "message": "This comment has a poll already" }), 409));
            }
            Err(e) => return Err(e),
        };
        let poll = polls::load_poll(&st, poll_id as i32, polls::Viewer::of(&st, Some(&session))).await?;
        if let Some(ctx) = poll_context(&st, Some(&session), poll_id as i32).await? {
            notify_poll(&st, "poll", &ctx).await;
        }
        Ok(json_resp(&json!({ "success": true, "poll": poll }), 200))
    }
    .await;
    r.or_else(|e| {
        tracing::error!("[POLLS] attach error: {e:#}");
        poll_error(500, "Database error")
    })
}

/// `GET /api/polls/:id` — the poll as this visitor sees it.
async fn poll_get(st: AppState, req: Req) -> Res {
    if !polls::enabled(&st) {
        return poll_error(403, "Polls are disabled");
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return poll_error(404, "Poll not found") };
    let session = req.session();
    let r: anyhow::Result<Option<Value>> = async {
        if poll_context(&st, session, poll_id).await?.is_none() {
            return Ok(None);
        }
        polls::load_poll(&st, poll_id, polls::Viewer::of(&st, session)).await
    }
    .await;
    match r {
        Ok(Some(poll)) => j(json!({ "success": true, "poll": poll }), 200),
        Ok(None) => poll_error(404, "Poll not found"),
        Err(e) => {
            tracing::error!("[POLLS] get error: {e:#}");
            poll_error(500, "Database error")
        }
    }
}

/// `GET /api/polls/:id/voters?option=<id>` — everyone who picked that option of a public poll.
async fn poll_voters(st: AppState, req: Req) -> Res {
    if !polls::enabled(&st) {
        return poll_error(403, "Polls are disabled");
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return poll_error(404, "Poll not found") };
    let Some(option_id) = req.query("option").and_then(|o| o.parse::<i32>().ok()) else { return poll_error(400, "Pick an option") };
    let session = req.session();
    let r: anyhow::Result<Option<Value>> = async {
        if poll_context(&st, session, poll_id).await?.is_none() {
            return Ok(None);
        }
        polls::option_voters(&st, poll_id, option_id, polls::Viewer::of(&st, session)).await
    }
    .await;
    match r {
        Ok(Some(mut v)) => {
            v["success"] = json!(true);
            j(v, 200)
        }
        Ok(None) => poll_error(404, "No voters to show"),
        Err(e) => {
            tracing::error!("[POLLS] voters error: {e:#}");
            poll_error(500, "Database error")
        }
    }
}

/// `POST /api/polls/:id/vote` — `option_id=<id>` votes (or moves the vote there), `remove=1` takes
/// the vote back (unless `websrv.comment_polls_unvote` is off). Explicit states, so the plain no-JavaScript forms can post them too.
async fn poll_vote(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return poll_error(401, "Log in to vote") };
    if !polls::enabled(&st) {
        return poll_error(403, "Polls are disabled");
    }
    if !polls::Viewer::of(&st, Some(&session)).may_vote {
        return poll_error(403, "Anonymous poll voting is disabled");
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return poll_error(404, "Poll not found") };
    let body = req.post_value().await;
    let remove = h::js_truthy(body.get("remove"));
    let option_id = js_str(body.get("option_id")).as_deref().and_then(js_parse_int).and_then(|o| i32::try_from(o).ok()).filter(|o| *o > 0);
    if !remove && option_id.is_none() {
        return poll_error(400, "Pick an option");
    }
    if remove && !polls::unvote_allowed(&st) {
        return poll_error(403, "Votes can't be taken back");
    }
    let me = session.id as i32;
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(ctx) = poll_context(&st, Some(&session), poll_id).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Poll not found" }), 404));
        };
        if ctx.bool("is_closed") {
            return Ok(json_resp(&json!({ "success": false, "message": "This poll is closed" }), 403));
        }
        if remove {
            sql!("DELETE FROM comment_poll_votes WHERE poll_id = ", poll_id, " AND user_id = ", me).execute(&st.db).await?;
        } else {
            let option_id = option_id.unwrap_or(0);
            // the option must belong to this poll; one statement, so two quick clicks can't
            // both insert (unique (poll_id, user_id))
            let done = sql!(
                "INSERT INTO comment_poll_votes (poll_id, option_id, user_id) SELECT o.poll_id, o.id, ",
                me,
                " FROM comment_poll_options o WHERE o.id = ",
                option_id,
                " AND o.poll_id = ",
                poll_id,
                " ON CONFLICT (poll_id, user_id) DO UPDATE SET option_id = EXCLUDED.option_id, created_at = now()"
            )
            .execute(&st.db)
            .await?;
            if done == 0 {
                return Ok(json_resp(&json!({ "success": false, "message": "Invalid option" }), 400));
            }
        }
        if session.is_anon {
            log_anon_activity(&st, &req, "poll_vote", Some(poll_id as i64), Some(json!({ "option_id": option_id, "remove": remove }))).await;
        }
        notify_poll(&st, "poll", &ctx).await;
        let poll = polls::load_poll(&st, poll_id, polls::Viewer::of(&st, Some(&session))).await?;
        Ok(json_resp(&json!({ "success": true, "poll": poll }), 200))
    }
    .await;
    r.or_else(|e| {
        tracing::error!("[POLLS] vote error: {e:#}");
        poll_error(500, "Database error")
    })
}

/// `POST /api/polls/:id/delete` — by the comment's author or a mod (logged when it's someone else's).
/// A comment that was nothing but the poll is deleted with it (`comment_deleted: true`).
async fn poll_delete(st: AppState, req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return unauthorized() };
    if !polls::enabled(&st) {
        return poll_error(403, "Polls are disabled");
    }
    let Some(poll_id) = param_i32(&req, "pollId") else { return poll_error(404, "Poll not found") };
    let r: anyhow::Result<axum::response::Response> = async {
        let Some(ctx) = poll_context(&st, Some(&session), poll_id).await? else {
            return Ok(json_resp(&json!({ "success": false, "message": "Poll not found" }), 404));
        };
        let own = ctx.i64("author_id") == Some(session.id);
        if !own && !is_admin_or_mod(&session) {
            return Ok(json_resp(&json!({ "success": false, "message": "Forbidden" }), 403));
        }
        let comment_id = ctx.i64("comment_id").unwrap_or(0) as i32;
        // A comment that was only the poll (no text, no files) would stay behind empty: it goes too,
        // the way a deleted comment goes
        let mut tx = st.db.begin().await?;
        sql!("DELETE FROM comment_polls WHERE id = ", poll_id).execute(&mut *tx).await?;
        let emptied = sql!(
            "UPDATE comments SET is_deleted = true, content = '[deleted]' WHERE id = ",
            comment_id,
            " AND btrim(content) = '' AND NOT EXISTS (SELECT 1 FROM comment_files f WHERE f.comment_id = ",
            comment_id,
            ") RETURNING id"
        )
        .fetch_optional(&mut *tx)
        .await?
        .is_some();
        tx.commit().await?;
        if !own {
            crate::audit::log(&st, session.id, "delete_poll", "comment", comment_id.to_string(), json!({ "item_id": ctx.val("item_id"), "question": ctx.val("question"), "comment_deleted": emptied })).await;
        }
        notify_poll(&st, "poll_deleted", &ctx).await;
        if emptied {
            notify(&st, "comments", &json!({ "type": "delete", "item_id": ctx.val("item_id"), "comment_id": comment_id.to_string() })).await;
        }
        Ok(json_resp(&json!({ "success": true, "poll_id": poll_id, "comment_id": comment_id, "item_id": ctx.val("item_id"), "comment_deleted": emptied }), 200))
    }
    .await;
    r.or_else(|e| {
        tracing::error!("[POLLS] delete error: {e:#}");
        poll_error(500, "Database error")
    })
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn greentext_quotes_and_context_links() {
        let h = frame_comment_html(">>7 \n>x\n>foobar", "/10", &[]);
        assert_eq!(h, "<a href=\"/10#c7\" class=\"comment-context-link\">&gt;&gt;7</a> \n<span class=\"greentext\">&gt;x</span>\n<span class=\"greentext\">&gt;foobar</span>");
    }

    #[test]
    fn site_images_links_and_escaping() {
        let h = frame_comment_html("/c/ab.webp#gif hi <b> https://x.org/a?b=1&c @bob, [spoiler]s[/spoiler]", "/3", &[]);
        assert!(h.starts_with("<img class=\"sidebar-comment-img\" src=\"/c/ab.webp\""));
        assert!(h.contains("hi &lt;b&gt; "));
        assert!(h.contains("<a href=\"https://x.org/a?b=1&amp;c\" target=\"_blank\""));
        assert!(h.contains("<a href=\"/user/bob\" class=\"mention\">@bob</a>,"));
        assert!(h.ends_with("<span class=\"spoiler\">s</span>"));
        assert!(!frame_comment_html("//evil.com/x.png javascript:alert(1)", "1", &[]).contains("<img"));
        assert_eq!(frame_comment_html("a\n\nb", "1", &[]), "a\n&nbsp;\nb");
    }

    #[test]
    fn site_links_show_as_path() {
        let hosts = vec!["f0ck.me".to_string(), "localhost".to_string()];
        let h = frame_comment_html("see https://www.F0ck.me/123-some-slug and http://localhost:1337/45?x=1&y#c2 https://f0ck.me", "/1", &hosts);
        assert!(h.contains("<a href=\"/123-some-slug\">/123-some-slug</a>"));
        assert!(h.contains("<a href=\"/45?x=1&amp;y#c2\">/45?x=1&amp;y#c2</a>"));
        assert!(h.ends_with("<a href=\"/\">/</a>"));
        assert!(frame_comment_html("https://f0ck.me/c/ab.webp", "1", &hosts).starts_with("<img class=\"sidebar-comment-img\" src=\"/c/ab.webp\""));
        assert!(frame_comment_html("https://f0ck.me.evil.org/1", "1", &hosts).contains("target=\"_blank\""));
        assert!(frame_comment_html("https://f0ck.me/1", "1", &[]).contains("target=\"_blank\""));
    }

    #[test]
    fn comment_effects() {
        let h = frame_comment_html("[rb]a [glitch]b[/glitch][/rb] [REICH]c[/reich] [krebs]d", "1", &[]);
        assert_eq!(h, "<span class=\"cfx-rb\">a <span class=\"cfx-glitch\" data-glitch=\"0.5\">b</span></span> <span class=\"cfx-reich\">c</span> [krebs]d");
        let a = frame_comment_html("[badapple]click[/BADAPPLE]", "1", &[]);
        assert_eq!(a, "<span class=\"cfx-badapple\" role=\"button\" tabindex=\"0\">click</span>");
        // [label](#bad-apple) links, [](#bad-apple) is just the apple; the label stays escaped
        let l = frame_comment_html("hi [](#bad-apple) [don't <b>](#bad-apple) [x](#nope)", "1", &[]);
        assert_eq!(l, "hi <a href=\"#bad-apple\"></a> <a href=\"#bad-apple\">don&#039;t &lt;b&gt;</a> [x](#nope)");
    }

    #[test]
    fn attachments_skip_inline_files() {
        let files = vec![json!({"dest": "ab.webp", "mime": "image/webp"}), json!({"dest": "x.mp4", "mime": "video/mp4"})];
        let h = frame_attachments_html(&files, "/c/ab.webp#gif");
        assert!(!h.contains("ab.webp"));
        assert!(h.contains("/c/x.mp4"));
    }
}
