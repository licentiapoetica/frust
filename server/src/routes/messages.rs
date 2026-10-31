//! Private messages (port of src/inc/routes/messages.mjs) and their encrypted attachments
//! (src/dm_attachment_handler.mjs, registered as bypass routes like in index.mjs).
//!
//! Messages and attachments are end-to-end encrypted in the browser; the server only stores
//! opaque ciphertext. Live delivery: the `private_messages` insert trigger sends
//! `pg_notify('private_message', …)`, which the notifications hub forwards to the recipient.
//!
//! Changes from the old code:
//! * `/api/dm/send` no longer sends its own `pg_notify` on top of the insert trigger (the recipient
//!   got every message event twice).
//! * Deleting a message with several attachments removes all of them (repeated `attachment_ids[]`
//!   fields arrive comma-joined, which the old `Number()` mapping turned into NaN).
//! * Attachment uploads also require `private_messages` (the old handler only checked
//!   `dm_attachments`, so uploads worked with messaging switched off); the body is streamed to
//!   disk with a hard cap instead of being buffered; `iv` must be base64/base64url (it is echoed
//!   in a response header) and the mime hint is reduced to header-safe characters.
//! * Attachment files are addressed by id inside `paths.e`, not by the path stored in the row.
//! * Public key fingerprints are limited to 256 characters.

use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, json_raw, json_resp, redirect, reply};
use crate::media::multipart::{self, MpError, Part};
use crate::session::Session;
use crate::social::helpers::js_parse_int;
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use axum::http::{HeaderValue, Method, header};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::Duration;

/// 50 MB plaintext; the ciphertext is sent as binary, framing etc. ×1.4 like before.
const MAX_BYTES: u64 = 50 * 1024 * 1024;
const MAX_CIPHERTEXT: u64 = (MAX_BYTES as f64 * 1.4) as u64 + 1;

pub fn register(r: &mut Router) {
    // Bypass routes (index.mjs): raw multipart / file bodies
    r.pre(Method::POST, r"^\/api\/dm\/attachment\/upload\/(?<id>\d+)$", None, attachment_upload);
    r.pre(Method::GET, r"^\/api\/dm\/attachment\/(?<id>\d+)$", None, attachment_download);
    r.pre(Method::DELETE, r"^\/api\/dm\/attachment\/(?<id>\d+)$", None, attachment_delete);

    r.exact(Method::POST, "/api/dm/pubkey", None, pubkey_upload);
    r.get(r"\/api\/dm\/pubkey\/(?<userId>\d+)", pubkey_get);
    r.exact(Method::POST, "/api/dm/keyvault", None, keyvault_upload);
    r.exact(Method::GET, "/api/dm/keyvault", None, keyvault_get);
    r.exact(Method::DELETE, "/api/dm/keyvault", None, keyvault_delete);
    r.exact(Method::GET, "/api/dm/conversations", None, conversations);
    r.get(r"\/api\/dm\/thread\/(?<userId>\d+)", thread);
    r.post(r"\/api\/dm\/send\/(?<userId>\d+)", send);
    r.post(r"\/api\/dm\/read\/(?<userId>\d+)", mark_read);
    r.patch(r"\/api\/dm\/message\/(?<msgId>\d+)", edit_message);
    r.delete(r"\/api\/dm\/message\/(?<msgId>\d+)", delete_message);
    r.post(r"\/api\/dm\/conversation\/(?<userId>\d+)\/delete", hide_conversation);
    r.get(r"\/api\/dm\/presence\/(?<userId>\d+)", presence);
    r.exact(Method::GET, "/api/dm/unread", None, unread);
    r.get(r"\/api\/dm\/resolve\/(?<username>[^/]+)", resolve);
    r.exact(Method::GET, "/messages", None, inbox_page);
    r.get(r"\/messages\/(?<username>[^/]+)", conversation_page);
}

/// `json(res, body, code)`: `application/json; charset=utf-8`.
fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn jfail(code: u16, msg: &str) -> Res {
    j(json!({ "success": false, "msg": msg }), code)
}

fn db_error(e: impl std::fmt::Display, what: &str) -> Res {
    tracing::error!("[DM] {what} failed: {e}");
    jfail(500, "DB error")
}

/// Messaging enabled and a session present; otherwise the error response.
fn gate(st: &AppState, req: &Req) -> Result<Session, Res> {
    if !st.settings.private_messages() {
        return Err(jfail(404, "Not found"));
    }
    req.session.clone().ok_or_else(|| jfail(401, "Unauthorized"))
}

macro_rules! gate {
    ($st:expr, $req:expr) => {
        match gate(&$st, &$req) {
            Ok(s) => s,
            Err(r) => return r,
        }
    };
}

/// Route param that the regex guarantees to be digits, as an `integer` bind.
fn param_i32(req: &Req, name: &str) -> Option<i32> {
    req.param(name).and_then(|s| s.parse::<i32>().ok())
}

/// `X-CSRF-Token` header equal to the session token (the handlers check the header only).
fn csrf_header(req: &Req, s: &Session) -> bool {
    crate::routes::uploads::csrf_header_ok(req, s)
}

/// `body[name]` when it is a string (`typeof x === 'string'`).
fn body_str<'a>(body: &'a Value, name: &str) -> Option<&'a str> {
    body.get(name).and_then(Value::as_str)
}

fn me(s: &Session) -> i32 {
    s.id as i32
}

// ── Public keys ─────────────────────────────────────────────────────────────

async fn pubkey_upload(st: AppState, mut req: Req) -> Res {
    let s = gate!(st, req);
    let body = req.post_value().await;
    let Some(pubkey) = body_str(&body, "pubkey").filter(|p| !p.is_empty() && p.encode_utf16().count() <= 4096) else {
        return jfail(400, "Invalid pubkey");
    };
    // `body.fingerprint || null`
    let fingerprint: Option<String> = match body.get("fingerprint") {
        Some(Value::String(f)) if !f.is_empty() => Some(f.chars().take(256).collect()),
        _ => None,
    };
    match serde_json::from_str::<Value>(pubkey) {
        Ok(Value::Object(k)) => {
            if k.get("kty").and_then(Value::as_str) != Some("EC") || k.get("crv").and_then(Value::as_str) != Some("P-256") {
                return jfail(400, "Invalid key type — expected EC P-256");
            }
        }
        Ok(Value::Null) | Err(_) => return jfail(400, "Invalid JSON"),
        Ok(_) => return jfail(400, "Invalid key type — expected EC P-256"),
    }
    let r = sql!(
        "INSERT INTO user_pubkeys (user_id, pubkey, fingerprint, updated_at) VALUES (", me(&s), ", ", pubkey, ", ", fingerprint,
        ", NOW()) ON CONFLICT (user_id) DO UPDATE SET pubkey = EXCLUDED.pubkey, fingerprint = EXCLUDED.fingerprint, updated_at = NOW()"
    )
    .execute(&st.db)
    .await;
    match r {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "pubkey upsert"),
    }
}

async fn pubkey_get(st: AppState, req: Req) -> Res {
    gate!(st, req);
    let Some(uid) = param_i32(&req, "userId") else { return jfail(404, "No key registered for this user") };
    match sql!("SELECT pubkey, fingerprint FROM user_pubkeys WHERE user_id = ", uid).fetch_optional(&st.db).await {
        Ok(Some(r)) => j(json!({ "success": true, "pubkey": r.val("pubkey"), "fingerprint": r.val("fingerprint") }), 200),
        Ok(None) => jfail(404, "No key registered for this user"),
        Err(e) => db_error(e, "pubkey fetch"),
    }
}

// ── Key vault ───────────────────────────────────────────────────────────────

async fn keyvault_upload(st: AppState, mut req: Req) -> Res {
    let s = gate!(st, req);
    let body = req.post_value().await;
    let (Some(salt), Some(iv), Some(ct)) = (body_str(&body, "salt"), body_str(&body, "iv"), body_str(&body, "ciphertext")) else {
        return jfail(400, "Missing salt, iv or ciphertext");
    };
    if salt.is_empty() || iv.is_empty() || ct.is_empty() {
        return jfail(400, "Missing salt, iv or ciphertext");
    }
    let len = |x: &str| x.encode_utf16().count();
    if len(salt) > 32 || len(iv) > 24 || len(ct) > 1024 {
        return jfail(400, "Payload out of bounds");
    }
    let r = sql!(
        "INSERT INTO user_dm_keyvault (user_id, salt, iv, ciphertext, updated_at) VALUES (", me(&s), ", ", salt, ", ", iv, ", ", ct,
        ", NOW()) ON CONFLICT (user_id) DO UPDATE SET salt = EXCLUDED.salt, iv = EXCLUDED.iv, ciphertext = EXCLUDED.ciphertext, updated_at = NOW()"
    )
    .execute(&st.db)
    .await;
    match r {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "keyvault upsert"),
    }
}

async fn keyvault_get(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    match sql!("SELECT salt, iv, ciphertext, version FROM user_dm_keyvault WHERE user_id = ", me(&s)).fetch_optional(&st.db).await {
        Ok(Some(r)) => j(json!({ "success": true, "vault": r }), 200),
        Ok(None) => jfail(404, "No vault found"),
        Err(e) => db_error(e, "keyvault fetch"),
    }
}

async fn keyvault_delete(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    match sql!("DELETE FROM user_dm_keyvault WHERE user_id = ", me(&s)).execute(&st.db).await {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "keyvault delete"),
    }
}

// ── Conversations ───────────────────────────────────────────────────────────

async fn conversations(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    let id = me(&s);
    let q = sql!(
        "WITH convos AS (
            SELECT CASE WHEN sender_id = ", id, " THEN recipient_id ELSE sender_id END AS other_id, MAX(id) AS last_msg_id
            FROM private_messages WHERE sender_id = ", id, " OR recipient_id = ", id, " GROUP BY other_id
        ),
        unread_counts AS (
            SELECT sender_id AS other_id, COUNT(*) AS unread FROM private_messages
            WHERE recipient_id = ", id, " AND is_read = false GROUP BY sender_id
        )
        SELECT u.id AS user_id, u.user AS username, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name,
               pm.created_at AS last_message_at, COALESCE(uc.unread, 0) AS unread_count
        FROM convos c
        JOIN \"user\" u ON u.id = c.other_id
        LEFT JOIN user_options uo ON uo.user_id = u.id
        JOIN private_messages pm ON pm.id = c.last_msg_id
        LEFT JOIN unread_counts uc ON uc.other_id = c.other_id
        LEFT JOIN user_conversation_states ucs ON ucs.user_id = ", id, " AND ucs.other_id = c.other_id
        WHERE (ucs.is_hidden IS NULL OR ucs.is_hidden = false)
        ORDER BY pm.created_at DESC"
    );
    match q.fetch_all(&st.db).await {
        Ok(rows) => j(json!({ "success": true, "conversations": rows }), 200),
        Err(e) => db_error(e, "conversations"),
    }
}

/// `req.url.qs.x ? parseInt(x, 10) : null`, where NaN/0 count as absent.
fn qs_id(req: &Req, name: &str) -> Option<i64> {
    req.query(name).and_then(js_parse_int).filter(|n| *n != 0)
}

async fn thread(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    let Some(other_id) = param_i32(&req, "userId") else { return jfail(404, "User not found") };
    let before = qs_id(&req, "before");
    let after = qs_id(&req, "after");
    let limit: usize = 50;

    let Some(other) = sql!(r#"SELECT id, "user" FROM "user" WHERE id = "#, other_id, " LIMIT 1").fetch_optional(&st.db).await? else {
        return jfail(404, "User not found");
    };
    let r: anyhow::Result<Vec<Row>> = async {
        sql!(
            "INSERT INTO user_conversation_states (user_id, other_id, is_hidden) VALUES (", me(&s), ", ", other_id,
            ", false) ON CONFLICT (user_id, other_id) DO UPDATE SET is_hidden = false"
        )
        .execute(&st.db)
        .await?;
        let mut q = sql!(
            "SELECT pm.id, pm.sender_id, pm.recipient_id, pm.ciphertext, pm.iv, pm.is_read, pm.created_at, pm.edited_at
             FROM private_messages pm
             WHERE ((pm.sender_id = ", me(&s), " AND pm.recipient_id = ", other_id, ") OR (pm.sender_id = ", other_id, " AND pm.recipient_id = ", me(&s), "))"
        );
        if let Some(a) = after {
            q.push(" AND pm.id > ").bind(a);
        }
        if let Some(b) = before {
            q.push(" AND pm.id < ").bind(b);
        }
        if after.is_some() {
            q.push(" ORDER BY pm.id ASC LIMIT 200");
        } else {
            q.push(&format!(" ORDER BY pm.id DESC LIMIT {}", limit + 1));
        }
        q.fetch_all(&st.db).await
    }
    .await;
    let mut messages = match r {
        Ok(m) => m,
        Err(e) => return db_error(e, "thread fetch"),
    };
    let has_more = after.is_none() && messages.len() > limit;
    if has_more {
        messages.pop();
    }
    if after.is_none() {
        messages.reverse();
    }
    j(json!({ "success": true, "messages": messages, "hasMore": has_more, "other": other }), 200)
}

/// Ciphertext + IV from the body, with the old bounds; `Err` is the response.
fn cipher_fields(body: &Value) -> Result<(String, String), Res> {
    let (Some(ct), Some(iv)) = (body_str(body, "ciphertext"), body_str(body, "iv")) else {
        return Err(jfail(400, "Missing ciphertext or iv"));
    };
    if ct.is_empty() || iv.is_empty() {
        return Err(jfail(400, "Missing ciphertext or iv"));
    }
    if ct.encode_utf16().count() > 65536 || iv.encode_utf16().count() > 32 {
        return Err(jfail(413, "Payload too large"));
    }
    Ok((ct.to_string(), iv.to_string()))
}

async fn send(st: AppState, mut req: Req) -> Res {
    let s = gate!(st, req);
    let Some(recipient) = param_i32(&req, "userId") else { return jfail(404, "Recipient not found") };
    if recipient as i64 == s.id {
        return jfail(400, "Can't DM yourself");
    }
    let body = req.post_value().await;
    let (ct, iv) = match cipher_fields(&body) {
        Ok(x) => x,
        Err(r) => return r,
    };
    if sql!(r#"SELECT id FROM "user" WHERE id = "#, recipient, " LIMIT 1").fetch_optional(&st.db).await?.is_none() {
        return jfail(404, "Recipient not found");
    }
    let r: anyhow::Result<Row> = async {
        let mut tx = st.db.begin().await?;
        let m = sql!(
            "INSERT INTO private_messages (sender_id, recipient_id, ciphertext, iv) VALUES (", me(&s), ", ", recipient, ", ", ct, ", ", iv,
            ") RETURNING id, created_at"
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("insert returned nothing"))?;
        sql!(
            "INSERT INTO user_conversation_states (user_id, other_id, is_hidden) VALUES (", me(&s), ", ", recipient, ", false), (",
            recipient, ", ", me(&s), ", false) ON CONFLICT (user_id, other_id) DO UPDATE SET is_hidden = false"
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(m)
    }
    .await;
    // The insert trigger (notify_private_message) wakes the SSE hub for the recipient.
    match r {
        Ok(m) => j(json!({ "success": true, "id": m.val("id"), "created_at": m.val("created_at") }), 200),
        Err(e) => db_error(e, "send"),
    }
}

async fn mark_read(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    let Some(other_id) = param_i32(&req, "userId") else { return jfail(500, "DB error") };
    match sql!("UPDATE private_messages SET is_read = true WHERE recipient_id = ", me(&s), " AND sender_id = ", other_id, " AND is_read = false")
        .execute(&st.db)
        .await
    {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "read mark"),
    }
}

async fn edit_message(st: AppState, mut req: Req) -> Res {
    let s = gate!(st, req);
    if !csrf_header(&req, &s) {
        return jfail(403, "CSRF mismatch");
    }
    let Some(msg_id) = req.param("msgId").and_then(|m| m.parse::<i64>().ok()) else { return jfail(404, "Not found or not yours") };
    let body = req.post_value().await;
    let (ct, iv) = match cipher_fields(&body) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let r = sql!(
        "UPDATE private_messages SET ciphertext = ", ct, ", iv = ", iv, ", edited_at = NOW() WHERE id = ", msg_id, " AND sender_id = ", me(&s),
        " RETURNING id, edited_at"
    )
    .fetch_optional(&st.db)
    .await;
    match r {
        Ok(Some(row)) => j(json!({ "success": true, "id": row.val("id"), "edited_at": row.val("edited_at") }), 200),
        Ok(None) => jfail(404, "Not found or not yours"),
        Err(e) => db_error(e, "edit message"),
    }
}

/// Attachment ids from `attachment_ids[]` / `attachment_ids` (array, single value or the
/// comma-joined string the urlencoded parser makes of repeated fields).
fn attachment_ids(body: &Value) -> Vec<i64> {
    let raw = body.get("attachment_ids[]").or_else(|| body.get("attachment_ids"));
    let items: Vec<String> = match raw {
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).collect(),
        Some(Value::String(s)) => s.split(',').map(str::to_string).collect(),
        Some(Value::Number(n)) => vec![n.to_string()],
        _ => vec![],
    };
    items.iter().filter_map(|x| x.trim().parse::<i64>().ok()).filter(|n| *n > 0).collect()
}

async fn delete_message(st: AppState, mut req: Req) -> Res {
    let s = gate!(st, req);
    if !csrf_header(&req, &s) {
        return jfail(403, "CSRF mismatch");
    }
    let Some(msg_id) = req.param("msgId").and_then(|m| m.parse::<i64>().ok()) else { return jfail(404, "Not found or not yours") };
    let body = req.post_value().await;
    let ids = attachment_ids(&body);
    if sql!("SELECT id FROM private_messages WHERE id = ", msg_id, " AND sender_id = ", me(&s), " LIMIT 1").fetch_optional(&st.db).await?.is_none() {
        return jfail(404, "Not found or not yours");
    }
    let r: anyhow::Result<()> = async {
        if !ids.is_empty() {
            let own: Vec<i64> = sql!("SELECT id FROM dm_attachments WHERE id = ANY(", ids, ") AND sender_id = ", me(&s))
                .fetch_scalar_list(&st.db)
                .await?;
            for id in &own {
                let _ = tokio::fs::remove_file(attachment_path(&st, *id)).await;
            }
            if !own.is_empty() {
                sql!("DELETE FROM dm_attachments WHERE id = ANY(", own, ")").execute(&st.db).await?;
            }
        }
        sql!("DELETE FROM private_messages WHERE id = ", msg_id, " AND sender_id = ", me(&s)).execute(&st.db).await?;
        Ok(())
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "delete message"),
    }
}

async fn hide_conversation(st: AppState, req: Req) -> Res {
    let s = gate!(st, req);
    let Some(other_id) = param_i32(&req, "userId") else { return jfail(500, "DB error") };
    let r = sql!(
        "INSERT INTO user_conversation_states (user_id, other_id, is_hidden) VALUES (", me(&s), ", ", other_id,
        ", true) ON CONFLICT (user_id, other_id) DO UPDATE SET is_hidden = true"
    )
    .execute(&st.db)
    .await;
    match r {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => db_error(e, "hide conversation"),
    }
}

async fn presence(st: AppState, req: Req) -> Res {
    if !st.settings.private_messages() {
        return j(json!({ "success": false }), 404);
    }
    if req.session.is_none() {
        return j(json!({ "success": false }), 401);
    }
    let Some(uid) = param_i32(&req, "userId") else { return jfail(404, "User not found") };
    match sql!(r#"SELECT last_seen FROM "user" WHERE id = "#, uid, " AND banned = false LIMIT 1").fetch_optional(&st.db).await {
        Ok(Some(r)) => {
            let last_seen = r.i64("last_seen").unwrap_or(0);
            let online = crate::util::now_secs() - last_seen < 300;
            j(json!({ "success": true, "online": online, "last_seen": last_seen }), 200)
        }
        Ok(None) => jfail(404, "User not found"),
        Err(e) => {
            tracing::error!("[DM] presence failed: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn unread(st: AppState, req: Req) -> Res {
    if !st.settings.private_messages() {
        return j(json!({ "success": true, "count": 0 }), 200);
    }
    let Some(s) = req.session() else { return j(json!({ "success": false, "count": 0 }), 401) };
    match sql!("SELECT COUNT(*) FROM private_messages WHERE recipient_id = ", s.id as i32, " AND is_read = false").fetch_scalar::<i64, _>(&st.db).await {
        Ok(n) => j(json!({ "success": true, "count": n.unwrap_or(0) }), 200),
        Err(_) => j(json!({ "success": false, "count": 0 }), 500),
    }
}

async fn resolve(st: AppState, req: Req) -> Res {
    gate!(st, req);
    let Some(uname) = req.param("username").and_then(crate::util::decode_uri_component) else { return jfail(404, "User not found") };
    match sql!(r#"SELECT id, "user" FROM "user" WHERE login = "#, uname.to_lowercase(), " LIMIT 1").fetch_optional(&st.db).await {
        Ok(Some(r)) => j(json!({ "success": true, "user_id": r.val("id"), "username": r.val("user") }), 200),
        Ok(None) => jfail(404, "User not found"),
        Err(_) => jfail(500, "DB error"),
    }
}

// ── Pages ───────────────────────────────────────────────────────────────────

fn not_found_page(msg: &str) -> Res {
    Ok(reply(404, "text/html", msg.to_string()))
}

fn page_data(st: &AppState, req: &Req) -> serde_json::Map<String, Value> {
    let cfg = st.cfg();
    let mut m = serde_json::Map::new();
    m.insert("session".into(), req.session_value());
    m.insert("hidePagination".into(), Value::Bool(true));
    m.insert("link".into(), json!({ "main": "/messages", "path": "/" }));
    m.insert("domain".into(), cfg.get("main.url.domain").clone());
    m
}

async fn inbox_page(st: AppState, req: Req) -> Res {
    if !st.settings.private_messages() {
        return not_found_page("Not found");
    }
    if req.session.is_none() {
        return Ok(redirect("/login"));
    }
    let body = crate::tpl::render(&st, Some(&req), "messages", Value::Object(page_data(&st, &req)));
    Ok(crate::http::html(body))
}

async fn conversation_page(st: AppState, req: Req) -> Res {
    if !st.settings.private_messages() {
        return not_found_page("Not found");
    }
    if req.session.is_none() {
        return Ok(redirect("/login"));
    }
    let Some(uname) = req.param("username").and_then(crate::util::decode_uri_component) else { return not_found_page("User not found") };
    let row = sql!(
        r#"SELECT u.id, u.user, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name
           FROM "user" u LEFT JOIN user_options uo ON uo.user_id = u.id WHERE u.login = "#,
        uname.to_lowercase(),
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(other) = row else { return not_found_page("User not found") };
    let mut data = page_data(&st, &req);
    data.insert("other".into(), Value::Object(other));
    let body = crate::tpl::render(&st, Some(&req), "messages-conversation", Value::Object(data));
    Ok(crate::http::html(body))
}

// ── Encrypted attachments (dm_attachment_handler.mjs) ───────────────────────

/// `sendJson(res, data, code)`: `application/json` without charset.
fn send_json(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

fn afail(code: u16, msg: &str) -> Res {
    send_json(json!({ "success": false, "msg": msg }), code)
}

fn attachment_dir(st: &AppState) -> PathBuf {
    st.cfg().paths.e.clone()
}

fn attachment_path(st: &AppState, id: i64) -> PathBuf {
    attachment_dir(st).join(id.to_string())
}

/// base64 / base64url text (or the `unencrypted` marker the client uses in plaintext mode).
fn valid_iv(iv: &str) -> bool {
    !iv.is_empty() && iv.len() <= 32 && iv.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_'))
}

/// Visible ASCII only (the value goes back out in a response header).
fn header_safe(s: &str, max: usize) -> String {
    s.chars().filter(|c| c.is_ascii_graphic() || *c == ' ').take(max).collect::<String>().trim().to_string()
}

async fn attachment_upload(st: AppState, mut req: Req) -> Res {
    if !st.settings.dm_attachments() || !st.settings.private_messages() {
        return afail(404, "Not found");
    }
    let Some(s) = req.session.clone() else { return afail(401, "Unauthorized") };
    if !csrf_header(&req, &s) {
        return afail(403, "CSRF mismatch");
    }
    let rid = req.param("id").and_then(|r| r.parse::<i32>().ok()).unwrap_or(0);
    if rid == 0 || rid as i64 == s.id {
        return afail(400, "Invalid recipient");
    }
    if sql!(r#"SELECT id FROM "user" WHERE id = "#, rid, " LIMIT 1").fetch_optional(&st.db).await?.is_none() {
        return afail(404, "Recipient not found");
    }
    let ct = req.content_type().to_string();
    let Some(boundary) = multipart::boundary(&ct).filter(|_| ct.contains("multipart/form-data")) else {
        return afail(400, "Expected multipart/form-data");
    };
    let tmp = st.cfg().paths.tmp.clone();
    let body = req.take_body().unwrap_or_else(Body::empty);
    let limits = multipart::Limits { max_total: MAX_CIPHERTEXT + 4096, max_text: 64 * 1024, max_parts: 16 };
    let form = match multipart::parse(body, &boundary, crate::routes::uploads::declared_len(&req), &tmp, limits).await {
        Ok(f) => f,
        Err(MpError::TooLarge) => return afail(413, "Attachment too large (max 50 MB)"),
        Err(e) => {
            tracing::warn!("[DM_ATT] Malformed upload: {e}");
            return afail(400, "No file part found");
        }
    };
    let Some(file) = form.entries.iter().find_map(|(_, p)| if let Part::File(f) = p { Some(f.clone()) } else { None }) else {
        return afail(400, "No file part found");
    };
    let field = |n: &str| form.text(n).unwrap_or("").to_string();
    let iv = field("iv").trim().to_string();
    if !valid_iv(&iv) {
        return afail(400, "Missing or invalid iv");
    }
    if file.size > MAX_CIPHERTEXT {
        return afail(413, "Attachment too large (max 50 MB)");
    }
    let original_name: String = field("original_name").chars().take(255).collect();
    let mime_hint = header_safe(&field("mime_hint"), 100);
    let size_bytes = js_parse_int(&field("size_bytes")).unwrap_or(0).clamp(0, i32::MAX as i64) as i32;
    let days = crate::settings::dm_attachment_expiry_days(&st.cfg()) as i32;

    let dir = attachment_dir(&st);
    let r: anyhow::Result<i64> = async {
        tokio::fs::create_dir_all(&dir).await?;
        let row = sql!(
            "INSERT INTO dm_attachments (sender_id, recipient_id, iv, file_path, original_name, mime_hint, size_bytes, expires_at) VALUES (",
            me(&s), ", ", rid, ", ", iv.as_str(), ", '', ", original_name.as_str(), ", ", mime_hint.as_str(), ", ", size_bytes,
            ", now() + make_interval(days => ", days, ")) RETURNING id"
        )
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("insert returned nothing"))?;
        let id = row.i64("id").unwrap_or(0);
        let path = attachment_path(&st, id);
        if let Err(e) = crate::media::move_file(&file.path, &path).await {
            let _ = sql!("DELETE FROM dm_attachments WHERE id = ", id).execute(&st.db).await;
            return Err(e.into());
        }
        sql!("UPDATE dm_attachments SET file_path = ", path.to_string_lossy().into_owned(), " WHERE id = ", id).execute(&st.db).await?;
        Ok(id)
    }
    .await;
    drop(form);
    match r {
        Ok(id) => send_json(json!({ "success": true, "id": id.to_string() }), 200),
        Err(e) => {
            tracing::error!("[DM_ATT] Upload error: {e:#}");
            afail(500, "Server error")
        }
    }
}

async fn attachment_download(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return afail(401, "Unauthorized") };
    let id = req.param("id").and_then(|i| i.parse::<i64>().ok()).unwrap_or(0);
    if id == 0 {
        return afail(400, "Invalid id");
    }
    let Some(att) = sql!("SELECT id, sender_id, recipient_id, iv, original_name, mime_hint FROM dm_attachments WHERE id = ", id, " LIMIT 1")
        .fetch_optional(&st.db)
        .await?
    else {
        return afail(404, "Not found");
    };
    let (sender, recipient) = (att.i64("sender_id").unwrap_or(-1), att.i64("recipient_id").unwrap_or(-1));
    if sender != s.id && recipient != s.id && !s.admin {
        return afail(403, "Forbidden");
    }
    let path = attachment_path(&st, id);
    let (file, len) = match tokio::fs::File::open(&path).await {
        Ok(f) => match f.metadata().await {
            Ok(m) if m.is_file() => (f, m.len()),
            _ => return afail(404, "File not found on disk"),
        },
        Err(e) => {
            tracing::error!("[DM_ATT] Download error: {e}");
            return afail(404, "File not found on disk");
        }
    };
    let name = att.str("original_name").filter(|n| !n.is_empty()).unwrap_or("attachment");
    let mut r = axum::response::Response::new(Body::from_stream(tokio_util::io::ReaderStream::new(file)));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    let set = |h: &mut axum::http::HeaderMap, k: &'static str, v: &str| {
        if let Ok(v) = HeaderValue::from_str(v) {
            h.insert(k, v);
        }
    };
    set(h, "x-dm-iv", &header_safe(att.str("iv").unwrap_or(""), 64));
    set(h, "x-original-name", &crate::util::encode_uri_component(name));
    set(h, "x-mime-hint", &header_safe(att.str("mime_hint").unwrap_or(""), 100));
    Ok(r)
}

async fn attachment_delete(st: AppState, req: Req) -> Res {
    let Some(s) = req.session.clone() else { return afail(401, "Unauthorized") };
    if !csrf_header(&req, &s) {
        return afail(403, "CSRF mismatch");
    }
    let id = req.param("id").and_then(|i| i.parse::<i64>().ok()).unwrap_or(0);
    if id == 0 {
        return afail(400, "Invalid id");
    }
    let Some(att) = sql!("SELECT id, sender_id FROM dm_attachments WHERE id = ", id, " LIMIT 1").fetch_optional(&st.db).await? else {
        return afail(404, "Not found");
    };
    if att.i64("sender_id") != Some(s.id) && !s.admin {
        return afail(403, "Forbidden");
    }
    let _ = tokio::fs::remove_file(attachment_path(&st, id)).await;
    match sql!("DELETE FROM dm_attachments WHERE id = ", id).execute(&st.db).await {
        Ok(_) => send_json(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[DM_ATT] Delete error: {e}");
            afail(500, "Server error")
        }
    }
}

/// Remove expired attachments (rows and files).
pub async fn cleanup_expired_attachments(st: &AppState) {
    let r: anyhow::Result<usize> = async {
        let ids: Vec<i64> = sql!("SELECT id FROM dm_attachments WHERE expires_at < now()").fetch_scalar_list(&st.db).await?;
        if ids.is_empty() {
            return Ok(0);
        }
        for id in &ids {
            let _ = tokio::fs::remove_file(attachment_path(st, *id)).await;
        }
        sql!("DELETE FROM dm_attachments WHERE id = ANY(", ids.clone(), ")").execute(&st.db).await?;
        Ok(ids.len())
    }
    .await;
    match r {
        Ok(0) => {}
        Ok(n) => tracing::info!("[DM_ATT] Cleanup: removed {n} expired attachment(s)"),
        Err(e) => tracing::error!("[DM_ATT] Cleanup error: {e}"),
    }
}

/// Expired attachment cleanup: at startup, then every 6 hours (the old setInterval).
pub fn spawn_cleanup(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        let _ = tokio::fs::create_dir_all(attachment_dir(&st)).await;
        loop {
            cleanup_expired_attachments(&st).await;
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_from_body() {
        assert_eq!(attachment_ids(&json!({ "attachment_ids[]": "3,4, 5" })), vec![3, 4, 5]);
        assert_eq!(attachment_ids(&json!({ "attachment_ids": ["7", 8, "x", "-1"] })), vec![7, 8]);
        assert_eq!(attachment_ids(&json!({})), Vec::<i64>::new());
    }

    #[test]
    fn iv_check() {
        assert!(valid_iv("AbC-_09+/="));
        assert!(valid_iv("unencrypted"));
        assert!(!valid_iv(""));
        assert!(!valid_iv("a\r\nb"));
        assert!(!valid_iv(&"a".repeat(33)));
    }
}
