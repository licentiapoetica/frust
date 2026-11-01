//! Notifications and the live event stream (port of src/inc/routes/notifications.mjs).
//!
//! `GET /api/notifications/ws` is a WebSocket carrying the live events as JSON text frames
//! (`{"type": ..., "data": ...}`); the server only sends, apart from ping/pong. Live events come
//! from Postgres `NOTIFY` (triggers and `pg_notify` calls across the app) via one `PgListener` task
//! ([`spawn_listener`]) and are fanned out to the connected clients held in [`HUB`].
//!
//! The old JSON handlers answered through `res.reply({ headers, body })`, which ignores `headers`,
//! so their JSON went out as `text/html`; they are sent as `application/json` now.

use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, html, json_resp};
use crate::session::Session;
use crate::settings::{enable_item_slugs, hw_fingerprint_enabled, is_anonymize_session};
use crate::sql;
use crate::state::AppState;
use axum::extract::FromRequestParts;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::Method;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use parking_lot::{Mutex, RwLock};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

pub fn register(r: &mut Router) {
    r.exact(Method::GET, "/api/notifications", None, unread);
    r.exact(Method::POST, "/api/notifications/read", None, read_all);
    r.post(r"\/api\/notifications\/(?<id>\d+)\/read", read_one);
    r.post(r"\/api\/notifications\/item\/(?<itemId>[a-zA-Z0-9_-]{11}|\d+)\/read", read_item);
    r.exact(Method::GET, "/api/notifications/ws", None, socket);
    r.exact(Method::GET, "/api/notifications/active", None, active);
    r.exact(Method::GET, "/notifications", None, page);
    r.exact(Method::GET, "/ajax/notifications", None, ajax_history);
}

const USER_TYPES: [&str; 4] = ["comment_reply", "subscription", "mention", "upload_comment"];
const SYSTEM_TYPES: [&str; 10] =
    ["approve", "deny", "item_deleted", "upload_success", "upload_error", "admin_pending", "report", "warning", "invite_approved", "invite_denied"];
const ADMIN_TYPES: [&str; 1] = ["invite_request"];

fn type_filter(tab: Option<&str>) -> Option<Vec<String>> {
    let l: &[&str] = match tab? {
        "admin" => &ADMIN_TYPES,
        "system" => &SYSTEM_TYPES,
        "user" => &USER_TYPES,
        _ => return None,
    };
    Some(l.iter().map(|s| s.to_string()).collect())
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

/// Shared SELECT list/joins of the notification queries.
fn select_head(nsfl: i32) -> crate::db::Sql {
    sql!(
        r#"SELECT n.id, n.type, n.item_id, i.slug as item_slug, n.reference_id, n.created_at, n.is_read, n.data,
               COALESCE(u.user, 'System') as from_user,
               COALESCE(uo.display_name, '') as from_display_name,
               COALESCE(u.id, 0) as from_user_id,
               uo.username_color,
               i.dest, i.mime,
               CASE (SELECT ta.tag_id FROM tags_assign ta WHERE ta.item_id = n.item_id AND ta.tag_id IN (1, 2, "#,
        nsfl,
        ") LIMIT 1) WHEN 1 THEN 'sfw' WHEN 2 THEN 'nsfw' WHEN ",
        nsfl,
        r#" THEN 'nsfl' ELSE NULL END as item_mode
        FROM notifications n
        LEFT JOIN comments c ON n.reference_id = c.id
        LEFT JOIN "user" u ON c.user_id = u.id
        LEFT JOIN user_options uo ON u.id = uo.user_id
        LEFT JOIN items i ON n.item_id = i.id"#
    )
}

/// `reason` from `data.reason`, slugs hidden when item slugs are off.
fn process(st: &AppState, mut n: Row) -> Value {
    let data = match n.val("data") {
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
        v => v.clone(),
    };
    let reason = data.get("reason").filter(|r| crate::config::truthy(r)).cloned().unwrap_or(json!("No reason provided"));
    if !enable_item_slugs(&st.cfg()) {
        n.insert("item_slug".into(), Value::Null);
    }
    n.insert("reason".into(), reason);
    Value::Object(n)
}

// ── GET /api/notifications ──────────────────────────────────────────────────

async fn unread(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return j(json!({ "success": false }), 401) };
    let nsfl = st.cfg().nsfl_tag_id() as i32;
    let dnd = s.do_not_disturb == Some(true);
    let sys = s.receive_system_notifications != Some(false);
    let usr = s.receive_user_notifications != Some(false);
    let q = sql!(
        @select_head(nsfl),
        r#" LEFT JOIN reports r ON n.type = 'report' AND n.reference_id = r.id
        WHERE n.user_id = "#,
        s.id as i32,
        r#" AND n.is_read = false
        AND (n.type IN ('admin_pending', 'deny', 'item_deleted', 'report', 'approve', 'warning', 'invite_request', 'invite_approved', 'invite_denied')
             OR ("#,
        !dnd,
        r#" AND (
                 (n.type IN ('upload_success', 'upload_error') AND "#,
        sys,
        r#") OR (n.type IN ('comment', 'comment_reply', 'mention', 'subscription', 'upload_comment') AND "#,
        usr,
        r#")
             ))
        )
        AND (n.item_id IS NULL OR (i.active = true AND i.is_deleted = false) OR n.type IN ('admin_pending', 'deny', 'item_deleted', 'report', 'warning', 'invite_request', 'invite_approved', 'invite_denied'))
        AND (n.type != 'report' OR r.status = 'pending')
        AND (n.type != 'admin_pending' OR (i.active = false AND i.is_deleted = false))
        ORDER BY n.created_at DESC
        LIMIT 1000"#
    );
    match q.fetch_all(&st.db).await {
        Ok(rows) => {
            let list: Vec<Value> = rows.into_iter().map(|n| process(&st, n)).collect();
            j(json!({ "success": true, "notifications": list }), 200)
        }
        Err(e) => {
            tracing::error!("{e:#}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── Mark read ───────────────────────────────────────────────────────────────

async fn read_all(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return j(json!({ "success": false }), 401) };
    let tab = req.query("tab").filter(|t| !t.is_empty()).map(str::to_string);
    let r = match type_filter(tab.as_deref()) {
        Some(types) => sql!("UPDATE notifications SET is_read = true WHERE user_id = ", s.id as i32, " AND type = ANY(", types, ")").execute(&st.db).await,
        None => sql!("UPDATE notifications SET is_read = true WHERE user_id = ", s.id as i32).execute(&st.db).await,
    };
    match r {
        Ok(_) => j(json!({ "success": true, "tab": tab }), 200),
        Err(_) => j(json!({ "success": false }), 500),
    }
}

async fn read_one(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return j(json!({ "success": false }), 401) };
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return j(json!({ "success": false }), 500) };
    match sql!("UPDATE notifications SET is_read = true WHERE id = ", id, " AND user_id = ", s.id as i32).execute(&st.db).await {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(_) => j(json!({ "success": false }), 500),
    }
}

async fn read_item(st: AppState, req: Req) -> Res {
    let Some(s) = req.session().filter(|s| s.id != 0) else { return j(json!({ "success": false }), 401) };
    let raw = req.param("itemId").unwrap_or("").to_string();
    let Some(item_id) = crate::social::helpers::resolve_numeric_item_id(&st, &raw).await.and_then(|i| i32::try_from(i).ok()) else {
        return j(json!({ "success": false }), 404);
    };
    // System notifications need an explicit acknowledgement
    let system: Vec<String> = ["item_deleted", "deny", "admin_pending", "report", "warning"].iter().map(|s| s.to_string()).collect();
    match sql!("UPDATE notifications SET is_read = true WHERE user_id = ", s.id as i32, " AND item_id = ", item_id, " AND NOT (type = ANY(", system, "))")
        .execute(&st.db)
        .await
    {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(_) => j(json!({ "success": false }), 500),
    }
}

// ── History ─────────────────────────────────────────────────────────────────

async fn history(st: &AppState, user_id: i64, page: i64, limit: i64, tab: Option<&str>) -> anyhow::Result<(Vec<Value>, bool)> {
    let nsfl = st.cfg().nsfl_tag_id() as i32;
    let mut q = select_head(nsfl);
    q.push(" WHERE n.user_id = ").bind(user_id as i32);
    if let Some(types) = type_filter(tab) {
        q.push(" AND n.type = ANY(").bind(types).push(")");
    }
    q.push(" AND (n.type IN ('admin_pending', 'deny', 'item_deleted', 'report', 'warning') OR i.id IS NULL OR (i.active = true AND i.is_deleted = false))");
    q.push(" ORDER BY n.created_at DESC LIMIT ").bind(limit + 1).push(" OFFSET ").bind((page - 1) * limit);
    let mut rows = q.fetch_all(&st.db).await?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.pop();
    }
    Ok((rows.into_iter().map(|n| process(st, n)).collect(), has_more))
}

fn comments_disabled(st: &AppState) -> bool {
    st.cfg().get("enable_comments") == &Value::Bool(false)
}

async fn page(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return Ok(crate::http::redirect("/login")) };
    let tab = if comments_disabled(&st) { "system".to_string() } else { req.query("tab").filter(|t| !t.is_empty()).unwrap_or("user").to_string() };
    let (list, has_more) = history(&st, s.id, 1, 50, Some(&tab)).await?;
    let cfg = st.cfg();
    let data = json!({
        "notifications": list,
        "hasMore": has_more,
        "page": 1,
        "hidePagination": true,
        "pagination": { "page": 1, "next": if has_more { json!(2) } else { Value::Null } },
        "link": { "main": "/notifications", "path": "/" },
        "activeTab": tab,
        "domain": cfg.str_or("main.url.domain", ""),
        "active_mode": s.mode.unwrap_or(0),
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "notifications", data)))
}

async fn ajax_history(st: AppState, req: Req) -> Res {
    let Some(s) = req.session() else { return j(json!({ "success": false }), 401) };
    let page = req.query("page").and_then(crate::social::helpers::js_parse_int).filter(|p| *p != 0).unwrap_or(1).clamp(1, 1_000_000);
    let tab = if comments_disabled(&st) { Some("system".to_string()) } else { req.query("tab").filter(|t| !t.is_empty()).map(str::to_string) };
    let (list, has_more) = history(&st, s.id, page, 50, tab.as_deref()).await?;
    let data = json!({ "notifications": list, "hasMore": has_more, "page": page, "active_mode": s.mode.unwrap_or(0) });
    let body = crate::tpl::render(&st, Some(&req), "snippets/notifications-list", data);
    j(
        json!({
            "success": true,
            "html": body,
            "hasMore": has_more,
            "currentPage": page,
            "nextPage": if has_more { json!(page + 1) } else { Value::Null },
        }),
        200,
    )
}

async fn active(_st: AppState, _req: Req) -> Res {
    // The old code recorded the focused tab per session, but never read it back.
    j(json!({ "success": true }), 200)
}

// ── Live clients ────────────────────────────────────────────────────────────

/// Per-tab limit for one session cookie; the oldest connection is closed beyond it.
const MAX_TABS_PER_SESSION: usize = 10;
/// Queued events per client; a client that falls this far behind loses events.
const CLIENT_QUEUE: usize = 512;
/// Ping interval; a client that answers nothing (no pong, no frame) for `IDLE_LIMIT` is dropped.
const PING_EVERY: Duration = Duration::from_secs(30);
const IDLE_LIMIT: Duration = Duration::from_secs(75);
/// A client whose socket doesn't take a frame within this time is dropped.
const SEND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy)]
struct Prefs {
    receive_system: bool,
    receive_user: bool,
    do_not_disturb: bool,
}

pub struct Client {
    id: u64,
    user_id: Option<i64>,
    session: Option<Session>,
    fingerprint: Option<String>,
    hw_fingerprint: Option<String>,
    username: Option<String>,
    display_name: Option<String>,
    avatar_file: Option<String>,
    avatar: Option<i64>,
    username_color: Option<String>,
    admin: bool,
    is_moderator: bool,
    prefs: Mutex<Prefs>,
    /// sha256 of the session cookie, or `guest-<tabId>`.
    session_key: String,
    is_guest: bool,
    tab_id: String,
    ip: String,
    ip_hash: String,
    /// Serialized events waiting for the socket.
    tx: mpsc::Sender<String>,
    close: Notify,
}

impl Client {
    fn send(&self, v: &Value) {
        let _ = self.tx.try_send(crate::util::json_string(v));
    }

    fn close(&self) {
        self.close.notify_one();
    }

    fn close_later(self: &Arc<Self>) {
        let c = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(1)).await;
            c.close();
        });
    }
}

pub struct Hub {
    clients: RwLock<HashMap<u64, Arc<Client>>>,
    next_id: AtomicU64,
}

pub static HUB: Lazy<Hub> = Lazy::new(|| Hub { clients: RwLock::new(HashMap::new()), next_id: AtomicU64::new(1) });

impl Hub {
    fn snapshot(&self) -> Vec<Arc<Client>> {
        let mut v: Vec<Arc<Client>> = self.clients.read().values().cloned().collect();
        v.sort_by_key(|c| c.id);
        v
    }

    pub fn len(&self) -> usize {
        self.clients.read().len()
    }

    /// Send an event to every connected client.
    pub fn broadcast(&self, v: &Value) {
        for c in self.snapshot() {
            c.send(v);
        }
    }

    /// Deduplicated online-user list to everyone.
    fn broadcast_presence(&self) {
        let clients = self.snapshot();
        let mut seen = std::collections::HashSet::new();
        let mut users = vec![];
        let mut guests = std::collections::HashSet::new();
        for c in &clients {
            match c.user_id {
                Some(uid) => {
                    if seen.insert(uid) {
                        users.push(json!({
                            "id": uid,
                            "username": c.username,
                            "display_name": c.display_name,
                            "avatar_file": c.avatar_file,
                            "avatar": c.avatar,
                            "username_color": c.username_color,
                        }));
                    }
                }
                None => {
                    if !c.ip.is_empty() {
                        guests.insert(c.ip.clone());
                    }
                }
            }
        }
        let ev = json!({ "type": "global_chat_presence", "data": { "users": users, "guestCount": guests.len() } });
        for c in &clients {
            c.send(&ev);
        }
    }
}

/// `new Date(x).toLocaleString()` for ban expiry, `'Permanent'` when unset.
fn fmt_expires(v: Option<&Value>) -> Value {
    match v {
        None | Some(Value::Null) => json!("Permanent"),
        Some(Value::String(s)) if s.is_empty() => json!("Permanent"),
        Some(Value::String(s)) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(d) => json!(crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None)),
            Err(_) => json!(s),
        },
        Some(Value::Number(n)) => match n.as_i64().and_then(|ms| chrono::DateTime::from_timestamp_millis(ms)) {
            Some(d) => json!(crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None)),
            None => json!(n),
        },
        Some(v) => v.clone(),
    }
}

fn str_set(data: &Value, one: &str, many: &str) -> Vec<String> {
    let mut out = vec![];
    if let Some(s) = data.get(one).filter(|v| crate::config::truthy(v)).and_then(|v| crate::social::helpers::js_str(Some(v))) {
        out.push(s);
    }
    if let Some(a) = data.get(many).and_then(Value::as_array) {
        out.extend(a.iter().filter_map(|v| crate::social::helpers::js_str(Some(v))));
    }
    out
}

/// Push a `banned` event to every client matching the ban (user, fingerprints, IP), then close them.
pub fn broadcast_ban(data: &Value) {
    let mut user_ids: Vec<i64> = vec![];
    if let Some(u) = data.get("userId").filter(|v| crate::config::truthy(v)).and_then(crate::config::as_int) {
        user_ids.push(u);
    }
    if let Some(a) = data.get("userIds").and_then(Value::as_array) {
        user_ids.extend(a.iter().filter_map(crate::config::as_int));
    }
    let fps = str_set(data, "fingerprint", "fingerprints");
    let hws = str_set(data, "hwFingerprint", "hwFingerprints");
    let ips = str_set(data, "ip", "ips");
    let ip_hashes = str_set(data, "ipHash", "ipHashes");

    let reason = data.get("reason").filter(|v| crate::config::truthy(v)).cloned().unwrap_or(json!("Violation of community rules"));
    let ev = json!({ "type": "banned", "data": { "reason": reason, "expires": fmt_expires(data.get("expires")), "redirect": "/banned" } });
    for c in HUB.snapshot() {
        let hit = c.user_id.map(|u| user_ids.contains(&u)).unwrap_or(false)
            || c.fingerprint.as_ref().map(|f| fps.contains(f)).unwrap_or(false)
            || c.hw_fingerprint.as_ref().map(|h| hws.contains(h)).unwrap_or(false)
            || (!c.ip.is_empty() && ips.contains(&c.ip))
            || (!c.ip_hash.is_empty() && (ip_hashes.contains(&c.ip_hash) || ips.contains(&c.ip_hash)));
        if hit {
            tracing::info!("[WS] Delivering instant ban to client (userId: {:?}, ip: {}, tab: {})", c.user_id, c.ip, c.tab_id);
            c.send(&ev);
            c.close_later();
        }
    }
}

/// Removes the client from the hub when its socket task ends (disconnect or close).
struct ClientGuard(u64);

impl Drop for ClientGuard {
    fn drop(&mut self) {
        HUB.clients.write().remove(&self.0);
        HUB.broadcast_presence();
    }
}

/// Turn the request into axum's WebSocket extractor (the router hands handlers a `Req`, not parts).
async fn ws_upgrade(req: &mut Req) -> Result<WebSocketUpgrade, Response> {
    let (mut parts, ()) = axum::http::Request::new(()).into_parts();
    parts.method = req.method.clone();
    parts.version = req.version;
    parts.headers = req.headers.clone();
    parts.extensions = std::mem::take(&mut req.extensions);
    WebSocketUpgrade::from_request_parts(&mut parts, &()).await.map_err(IntoResponse::into_response)
}

async fn socket(st: AppState, mut req: Req) -> Res {
    if !crate::app::origin_allowed(&st, &req) {
        tracing::warn!("[WS] Rejected handshake from foreign origin {:?}", req.header("origin"));
        return Ok(crate::http::reply(403, "text/plain", "Forbidden"));
    }
    let ws = match ws_upgrade(&mut req).await {
        Ok(ws) => ws,
        Err(r) => return Ok(r),
    };
    let cfg = st.cfg();
    let tab_id = req.query("tabId").filter(|t| !t.is_empty()).unwrap_or("unknown").chars().take(100).collect::<String>();
    let cookie = req.cookie("session").filter(|c| !c.is_empty());
    let is_guest = cookie.is_none();
    let session_key = match cookie {
        Some(c) => crate::util::sha256(c),
        None => format!("guest-{tab_id}"),
    };

    // Soft cap per session: close the oldest connection to free a slot
    if !is_guest {
        let same: Vec<Arc<Client>> = HUB.snapshot().into_iter().filter(|c| c.session_key == session_key).collect();
        if same.len() >= MAX_TABS_PER_SESSION {
            same[0].close();
        }
    }

    let session = req.session().cloned();
    let ip = req.real_ip();
    let ip_hash = crate::security::hash_ip(&cfg, &ip);
    let fingerprint = session
        .as_ref()
        .and_then(|s| s.fingerprint.clone().filter(|f| !f.is_empty()).or_else(|| s.anon_fingerprint.clone().filter(|f| !f.is_empty())))
        .or_else(|| req.query("fp").filter(|f| !f.is_empty()).map(str::to_string));
    let hw_fingerprint = if hw_fingerprint_enabled(&cfg) { req.query("hw").filter(|h| !h.is_empty()).map(str::to_string) } else { None };
    let s = session.as_ref();
    let (tx, rx) = mpsc::channel::<String>(CLIENT_QUEUE);
    let client = Arc::new(Client {
        id: HUB.next_id.fetch_add(1, Ordering::Relaxed),
        user_id: s.map(|s| s.id),
        fingerprint,
        hw_fingerprint,
        username: s.map(|s| s.user.clone()).filter(|u| !u.is_empty()),
        display_name: s.and_then(|s| s.display_name.clone()).filter(|d| !d.is_empty()),
        avatar_file: s.and_then(|s| s.avatar_file.clone()).filter(|d| !d.is_empty()),
        avatar: s.and_then(|s| s.avatar).filter(|a| *a != 0),
        username_color: s.and_then(|s| s.username_color.clone()).filter(|d| !d.is_empty()),
        admin: s.map(|s| s.admin).unwrap_or(false),
        is_moderator: s.map(|s| s.is_moderator).unwrap_or(false),
        prefs: Mutex::new(Prefs {
            receive_system: s.and_then(|s| s.receive_system_notifications) != Some(false),
            receive_user: s.and_then(|s| s.receive_user_notifications) != Some(false),
            do_not_disturb: s.and_then(|s| s.do_not_disturb) == Some(true),
        }),
        session: session.clone(),
        session_key,
        is_guest,
        tab_id,
        ip,
        ip_hash,
        tx,
        close: Notify::new(),
    });

    // Clients only send pongs and close frames
    Ok(ws.max_message_size(4 * 1024).on_upgrade(move |socket| run_socket(st, socket, client, rx)))
}

/// Registers the client in the hub, sends it its queued events and keeps the connection alive.
async fn run_socket(st: AppState, socket: WebSocket, client: Arc<Client>, mut rx: mpsc::Receiver<String>) {
    // Already banned? Push the redirect right away.
    {
        let (st, c) = (st.clone(), client.clone());
        tokio::spawn(async move {
            let mut ban: Option<(Value, Value)> = None;
            if let Some(uid) = c.user_id {
                if let Ok(Some(u)) = sql!(r#"SELECT banned, ban_reason, ban_expires FROM "user" WHERE id = "#, uid as i32, " LIMIT 1").fetch_optional(&st.db).await {
                    if u.bool("banned") {
                        ban = Some((u.val("ban_reason").clone(), u.val("ban_expires").clone()));
                    }
                }
            }
            if ban.is_none() {
                if let Some(r) = crate::security::is_ip_banned(&st, &c.ip).await {
                    ban = Some((r.val("reason").clone(), r.val("expires").clone()));
                }
            }
            if ban.is_none() {
                if let Some(fp) = &c.fingerprint {
                    if let Some(r) = crate::security::is_fingerprint_banned(&st, fp).await {
                        ban = Some((r.val("reason").clone(), r.val("expires").clone()));
                    }
                }
            }
            if ban.is_none() {
                if let Some(hw) = &c.hw_fingerprint {
                    if let Some(r) = crate::security::is_hardware_banned(&st, hw).await {
                        ban = Some((r.val("reason").clone(), r.val("expires").clone()));
                    }
                }
            }
            if let Some((reason, expires)) = ban {
                tracing::info!("[WS] Connecting client is already banned, pushing instant redirect to /banned");
                let reason = if crate::config::truthy(&reason) { reason } else { json!("Violation of community rules") };
                c.send(&json!({ "type": "banned", "data": { "reason": reason, "expires": fmt_expires(Some(&expires)), "redirect": "/banned" } }));
                c.close_later();
            }
        });
    }

    // Unacknowledged warnings on connect
    if let Some(uid) = client.user_id.filter(|_| !client.is_guest) {
        let (st, c) = (st.clone(), client.clone());
        tokio::spawn(async move {
            match sql!("SELECT id, reason FROM user_warnings WHERE user_id = ", uid as i32, " AND acknowledged = FALSE ORDER BY created_at ASC").fetch_all(&st.db).await {
                Ok(rows) => {
                    for w in rows {
                        c.send(&json!({ "type": "warning", "data": { "warning_id": w.val("id"), "reason": w.val("reason") } }));
                    }
                }
                Err(e) => tracing::error!("[WS] Failed to fetch initial warnings: {e}"),
            }
        });
    }

    HUB.clients.write().insert(client.id, client.clone());
    let _guard = ClientGuard(client.id);
    HUB.broadcast_presence();

    let (mut sink, mut incoming) = socket.split();
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PING_EVERY, PING_EVERY);
    let mut last_seen = tokio::time::Instant::now();
    loop {
        let out = tokio::select! {
            _ = client.close.notified() => {
                let _ = tokio::time::timeout(SEND_TIMEOUT, sink.send(Message::Close(None))).await;
                break;
            }
            m = rx.recv() => match m {
                Some(text) => Message::Text(text.into()),
                None => break,
            },
            _ = ping.tick() => {
                if last_seen.elapsed() > IDLE_LIMIT {
                    break;
                }
                Message::Ping(Bytes::new())
            }
            m = incoming.next() => match m {
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {
                    last_seen = tokio::time::Instant::now();
                    continue;
                }
            },
        };
        if !matches!(tokio::time::timeout(SEND_TIMEOUT, sink.send(out)).await, Ok(Ok(()))) {
            break;
        }
    }
}

// ── Postgres LISTEN ─────────────────────────────────────────────────────────

const CHANNELS: [&str; 20] = [
    "notifications",
    "warnings",
    "bans",
    "profile_update",
    "activity",
    "tags",
    "comments",
    "favorites",
    "motd",
    "brand_image",
    "new_item",
    "delete_item",
    "emojis_updated",
    "private_message",
    "global_chat",
    "global_chat_clear",
    "global_chat_delete",
    "global_chat_background",
    "rethumb",
    "global_chat_topic",
];

/// Startup cleanup of stale unread notifications, then the LISTEN loop (reconnects on failure).
pub fn spawn_listener(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        if let Err(e) = sql!("UPDATE notifications SET is_read = true WHERE type = 'report' AND reference_id IN (SELECT id FROM reports WHERE status != 'pending') AND is_read = false")
            .execute(&st.db)
            .await
        {
            tracing::error!("[NOTIF CLEANUP] Failed to cleanup report notifications: {e}");
        }
        if let Err(e) = sql!("UPDATE notifications SET is_read = true WHERE type = 'admin_pending' AND item_id IN (SELECT id FROM items WHERE active = true OR is_deleted = true) AND is_read = false")
            .execute(&st.db)
            .await
        {
            tracing::error!("[NOTIF CLEANUP] Failed to cleanup admin_pending notifications: {e}");
        }

        let mut backoff = Duration::from_secs(1);
        loop {
            let mut listener = match sqlx::postgres::PgListener::connect_with(&st.db).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("[WS] DB listen connect failed: {e}");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            };
            if let Err(e) = listener.listen_all(CHANNELS).await {
                tracing::error!("[WS] DB LISTEN failed: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(30));
                continue;
            }
            backoff = Duration::from_secs(1);
            loop {
                match listener.recv().await {
                    Ok(n) => dispatch(&st, n.channel(), n.payload()).await,
                    Err(e) => {
                        tracing::error!("[WS] DB listen error: {e}");
                        break;
                    }
                }
            }
            tokio::time::sleep(backoff).await;
        }
    });
}

fn parse(payload: &str) -> Option<Value> {
    serde_json::from_str(payload).ok()
}

fn same_user(c: &Client, v: &Value) -> bool {
    matches!((c.user_id, v.as_i64()), (Some(a), Some(b)) if a == b)
}

fn lower_eq(a: Option<&str>, b: Option<&str>) -> bool {
    matches!((a, b), (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() && a.to_lowercase() == b.to_lowercase())
}

async fn dispatch(st: &AppState, channel: &str, payload: &str) {
    match channel {
        "motd" => {
            let motd = payload.to_string();
            st.settings.update(|s| s.motd = motd.clone());
            HUB.broadcast(&json!({ "type": "motd", "data": { "motd": motd } }));
        }
        "emojis_updated" => HUB.broadcast(&json!({ "type": "emojis_updated" })),
        "global_chat_clear" => HUB.broadcast(&json!({ "type": "global_chat_clear" })),
        _ => {
            let Some(data) = parse(payload) else {
                tracing::error!("[WS] {channel}: invalid payload");
                return;
            };
            if !data.is_object() {
                tracing::error!("[WS] {channel}: payload is not an object");
                return;
            }
            dispatch_json(st, channel, data).await;
        }
    }
}

async fn dispatch_json(st: &AppState, channel: &str, mut data: Value) {
    match channel {
        "notifications" => {
            let ty = data["type"].as_str().unwrap_or("").to_string();
            const SYS: [&str; 2] = ["upload_success", "upload_error"];
            const USR: [&str; 5] = ["comment", "comment_reply", "mention", "subscription", "upload_comment"];
            let ev = json!({ "type": "notify", "data": data });
            for c in HUB.snapshot() {
                if !same_user(&c, &ev["data"]["user_id"]) {
                    continue;
                }
                let p = *c.prefs.lock();
                if p.do_not_disturb {
                    continue;
                }
                if SYS.contains(&ty.as_str()) && !p.receive_system {
                    continue;
                }
                if ty != "warning" && USR.contains(&ty.as_str()) && !p.receive_user {
                    continue;
                }
                c.send(&ev);
            }
        }
        "warnings" => {
            let ev = json!({ "type": "warning", "data": data });
            for c in HUB.snapshot() {
                if same_user(&c, &ev["data"]["user_id"]) {
                    c.send(&ev);
                }
            }
        }
        "bans" => broadcast_ban(&data),
        "profile_update" => {
            const PUBLIC: [&str; 9] =
                ["user_id", "user", "display_name", "banner_file", "banner_position", "banner_size", "avatar_file", "username_color", "description"];
            let own = json!({ "type": "profile_update", "data": data });
            let mut public = Map::new();
            for k in PUBLIC {
                if let Some(v) = data.get(k) {
                    public.insert(k.into(), v.clone());
                }
            }
            let other = json!({ "type": "profile_update", "data": public });
            for c in HUB.snapshot() {
                if same_user(&c, &data["user_id"]) {
                    let mut p = c.prefs.lock();
                    if let Some(b) = data.get("receive_system_notifications").filter(|v| !v.is_null()) {
                        p.receive_system = crate::config::truthy(b);
                    }
                    if let Some(b) = data.get("receive_user_notifications").filter(|v| !v.is_null()) {
                        p.receive_user = crate::config::truthy(b);
                    }
                    if let Some(b) = data.get("do_not_disturb").filter(|v| !v.is_null()) {
                        p.do_not_disturb = b == &Value::Bool(true);
                    }
                    drop(p);
                    c.send(&own);
                } else {
                    c.send(&other);
                }
            }
        }
        "activity" => {
            enrich_activity(st, &mut data).await;
            let vis = data["visibility"].as_i64().unwrap_or(0);
            let ev = json!({ "type": "activity", "data": data });
            let d = &ev["data"];
            for c in HUB.snapshot() {
                let author = same_user(&c, &d["user_id"]);
                let owner = lower_eq(c.username.as_deref(), d["item_username"].as_str());
                if vis == 0 || author || owner || c.admin || c.is_moderator {
                    c.send(&ev);
                }
            }
        }
        "favorites" => {
            let cfg = st.cfg();
            let full = json!({ "type": "favorites", "data": data });
            for c in HUB.snapshot() {
                if !is_anonymize_session(&cfg, c.session.as_ref()) {
                    c.send(&full);
                    continue;
                }
                let favs: Vec<Value> = data["favs"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|f| {
                                let is_self = matches!((c.user_id, crate::config::as_int(&f["user_id"])), (Some(a), Some(b)) if a == b);
                                if is_self {
                                    return f.clone();
                                }
                                let mut m = Map::new();
                                m.insert("user_id".into(), Value::Null);
                                m.insert("user".into(), json!("anonymous"));
                                m.insert("login".into(), json!("anonymous"));
                                m.insert("display_name".into(), json!("Anonymous"));
                                m.insert("avatar".into(), Value::Null);
                                m.insert("avatar_file".into(), Value::Null);
                                m.insert("username_color".into(), Value::Null);
                                if let Some(h) = f.get("hide_fav_badge") {
                                    m.insert("hide_fav_badge".into(), h.clone());
                                }
                                m.insert("is_anon".into(), json!(true));
                                Value::Object(m)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let own = matches!((c.user_id, crate::config::as_int(&data["user_id"])), (Some(a), Some(b)) if a == b);
                let mut d = Map::new();
                d.insert("item_id".into(), data.get("item_id").cloned().unwrap_or(Value::Null));
                d.insert("user_id".into(), if own { data["user_id"].clone() } else { Value::Null });
                if let Some(f) = data.get("favorited") {
                    d.insert("favorited".into(), f.clone());
                }
                d.insert("favs".into(), Value::Array(favs));
                c.send(&json!({ "type": "favorites", "data": d }));
            }
        }
        "brand_image" => {
            let url = data.get("url").filter(|u| crate::config::truthy(u)).cloned().unwrap_or(Value::Null);
            HUB.broadcast(&json!({ "type": "brand_image", "data": { "url": url } }));
        }
        "new_item" => {
            let vis = data["visibility"].as_i64().unwrap_or(0);
            let ev = json!({ "type": "new_item", "data": data });
            for c in HUB.snapshot() {
                if vis == 0 || lower_eq(c.username.as_deref(), ev["data"]["username"].as_str()) {
                    c.send(&ev);
                }
            }
        }
        "private_message" => {
            let ev = json!({ "type": "private_message", "data": { "id": data["id"], "sender_id": data["sender_id"], "created_at": data["created_at"] } });
            for c in HUB.snapshot() {
                if same_user(&c, &data["recipient_id"]) && !c.prefs.lock().do_not_disturb {
                    c.send(&ev);
                }
            }
        }
        "global_chat" => {
            if let Some(uid) = crate::config::as_int(&data["user_id"]).and_then(|u| i32::try_from(u).ok()) {
                let q = sql!(
                    r#"SELECT u.user as username, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name
                       FROM "user" u LEFT JOIN user_options uo ON u.id = uo.user_id WHERE u.id = "#,
                    uid
                );
                match q.fetch_optional(&st.db).await {
                    Ok(Some(u)) => {
                        for k in ["username", "avatar", "avatar_file", "username_color"] {
                            data[k] = u.val(k).clone();
                        }
                        data["display_name"] = u.str("display_name").filter(|d| !d.is_empty()).map(Value::from).unwrap_or(Value::Null);
                    }
                    Ok(None) => {}
                    Err(e) => return tracing::error!("Global chat broadcast error: {e}"),
                }
            }
            HUB.broadcast(&json!({ "type": "global_chat", "data": data }));
        }
        // tags, comments, delete_item, global_chat_delete, global_chat_background, rethumb, global_chat_topic
        other => HUB.broadcast(&json!({ "type": other, "data": data })),
    }
}

/// The `activity` trigger only carries ids: add user, item and rating details.
async fn enrich_activity(st: &AppState, data: &mut Value) {
    let uid = crate::config::as_int(&data["user_id"]).and_then(|u| i32::try_from(u).ok());
    let iid = crate::config::as_int(&data["item_id"]).and_then(|u| i32::try_from(u).ok());
    let details = match uid {
        Some(uid) => sql!(
            r#"SELECT u.id as user_id, u.user as username, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name, uo.banner_file, uo.banner_position, uo.banner_size, uo.banner_repeat, i.mime, i.slug as item_slug, COALESCE(i.visibility, 0) as visibility, LOWER(i.username) as item_username,
            (SELECT tag_id FROM tags_assign WHERE item_id = i.id AND tag_id IN (1, 2) LIMIT 1) as tag_id
            FROM "user" u
            LEFT JOIN user_options uo ON u.id = uo.user_id
            LEFT JOIN items i ON i.id = "#,
            iid,
            " WHERE u.id = ",
            uid
        )
        .fetch_optional(&st.db)
        .await
        .unwrap_or_else(|e| {
            tracing::error!("Activity broadcast error: {e}");
            None
        }),
        None => None,
    };
    let Some(d) = details else {
        data["username"] = json!("System");
        data["visibility"] = json!(0);
        data["item_username"] = Value::Null;
        return;
    };
    let or_null = |v: &Value| if crate::config::truthy(v) { v.clone() } else { Value::Null };
    data["username"] = d.val("username").clone();
    data["avatar"] = d.val("avatar").clone();
    data["avatar_file"] = d.val("avatar_file").clone();
    for k in ["banner_file", "banner_position", "banner_size", "banner_repeat"] {
        let cur = or_null(&data[k]);
        data[k] = if cur.is_null() { or_null(d.val(k)) } else { cur };
    }
    data["mime"] = d.val("mime").clone();
    data["username_color"] = d.val("username_color").clone();
    data["display_name"] = or_null(d.val("display_name"));
    data["tag_id"] = d.val("tag_id").clone();
    data["item_slug"] = if enable_item_slugs(&st.cfg()) { d.val("item_slug").clone() } else { Value::Null };
    data["visibility"] = d.val("visibility").clone();
    data["item_username"] = d.val("item_username").clone();
}
