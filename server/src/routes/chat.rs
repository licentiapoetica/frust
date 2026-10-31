//! Global chat (port of src/inc/routes/chat.mjs).
//!
//! New messages reach clients through the `global_chat` insert trigger → notifications hub;
//! background/topic/delete/clear events are sent with `pg_notify` here.
//!
//! Changes from the old code:
//! * JSON answers are sent as `application/json` (the old `res.reply({ headers })` ignored the
//!   headers, so chat messages went out as `text/html`).
//! * Background and topic are read from `global_chat_settings` on each request instead of a
//!   process-wide cache loaded once (stays correct with several server processes).
//! * The background option string additionally loses `\`, quotes and `url(`/`image(`-style
//!   functions, so it cannot pull in third-party images (only the validated URL is loaded).

use crate::http::{Req, Res, Router, json_resp};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;

const MAX_MSG_LEN: usize = 500;
const RATE_LIMIT_MS: i64 = 1000;

static USER_LAST_MSG: Lazy<Mutex<HashMap<i64, i64>>> = Lazy::new(|| Mutex::new(HashMap::new()));

pub fn register(r: &mut Router) {
    r.exact(Method::GET, "/api/chat", None, get_messages);
    r.exact(Method::POST, "/api/chat", None, post_message);
    r.exact(Method::GET, "/api/chat/background", None, get_background);
    r.exact(Method::POST, "/api/chat/background", None, set_background);
    r.exact(Method::GET, "/api/chat/topic", None, get_topic);
    r.exact(Method::POST, "/api/chat/topic", None, set_topic);
    r.delete(r"^\/api\/chat\/(?<id>\d+)", delete_message);
    r.exact(Method::DELETE, "/api/chat", None, clear);
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn enabled(st: &AppState) -> bool {
    st.cfg().truthy("websrv.enable_global_chat")
}

fn disabled() -> Res {
    j(json!({ "success": false }), 404)
}

fn is_staff(req: &Req) -> bool {
    req.session().is_some_and(|s| s.admin || s.is_moderator)
}

fn forbidden() -> Res {
    j(json!({ "success": false, "msg": "Forbidden" }), 403)
}

async fn notify(st: &AppState, channel: &str, payload: String) -> anyhow::Result<()> {
    sql!("SELECT pg_notify(", channel, ", ", payload, ")").execute(&st.db).await?;
    Ok(())
}

async fn setting(st: &AppState, key: &str) -> Option<String> {
    match sql!("SELECT value FROM global_chat_settings WHERE key = ", key).fetch_scalar::<String, _>(&st.db).await {
        Ok(v) => v.filter(|s| !s.is_empty()),
        Err(e) => {
            tracing::warn!("[Chat] Could not load settings: {e}");
            None
        }
    }
}

async fn save_setting(st: &AppState, key: &str, value: Option<&str>) {
    let r = sql!(
        "INSERT INTO global_chat_settings (key, value) VALUES (", key, ", ", value, ") ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        tracing::warn!("[Chat] Could not persist setting: {key} {e}");
    }
}

// ── Messages ────────────────────────────────────────────────────────────────

async fn get_messages(st: AppState, req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    if req.session.is_none() {
        return j(json!({ "success": false, "msg": "Login required" }), 401);
    }
    let r = sql!(
        r#"SELECT gc.id, gc.user_id, gc.message, gc.created_at, u.user as username, uo.avatar, uo.avatar_file, uo.username_color, uo.display_name
           FROM global_chat gc JOIN "user" u ON u.id = gc.user_id LEFT JOIN user_options uo ON uo.user_id = gc.user_id
           ORDER BY gc.created_at DESC LIMIT 50"#
    )
    .fetch_all(&st.db)
    .await;
    match r {
        Ok(mut rows) => {
            rows.reverse();
            j(json!({ "success": true, "messages": rows }), 200)
        }
        Err(e) => {
            tracing::error!("[Chat] GET error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn post_message(st: AppState, mut req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    let Some(s) = req.session.clone() else { return j(json!({ "success": false, "msg": "Login required" }), 401) };
    if s.banned {
        return j(json!({ "success": false, "msg": "You are banned" }), 403);
    }
    let message = req.post_str("message").await.unwrap_or_default().trim().to_string();
    if message.is_empty() || message.encode_utf16().count() > MAX_MSG_LEN {
        return j(json!({ "success": false, "msg": "Invalid message" }), 400);
    }
    let now = crate::util::now_ms();
    {
        let mut last = USER_LAST_MSG.lock();
        if now - last.get(&s.id).copied().unwrap_or(0) < RATE_LIMIT_MS {
            return j(json!({ "success": false, "msg": "Slow down!" }), 429);
        }
        last.insert(s.id, now);
    }
    match sql!("INSERT INTO global_chat (user_id, message) VALUES (", s.id as i32, ", ", message, ")").execute(&st.db).await {
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[Chat] POST error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── Background ──────────────────────────────────────────────────────────────

async fn get_background(st: AppState, _req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    let bg = setting(&st, "background").await;
    j(json!({ "success": true, "background": bg }), 200)
}

/// Hosts a chat background may come from: the site itself plus `allowed_comment_images`.
fn bg_url_allowed(st: &AppState, url: &str) -> bool {
    let cfg = st.cfg();
    let base = cfg.str("main.url.base").filter(|b| !b.is_empty()).map(str::to_string).unwrap_or_else(|| format!("http://{}", cfg.str_or("main.url.domain", "")));
    let site_host = url::Url::parse(&base)
        .ok()
        .and_then(|u| u.host_str().map(|h| match u.port() {
            Some(p) => format!("{h}:{p}"),
            None => h.to_string(),
        }))
        .unwrap_or_default();
    let mut hosts = vec![site_host];
    hosts.extend(cfg.str_list("websrv.allowed_comment_images").unwrap_or_default());
    let alts: Vec<String> = hosts.iter().map(|h| regex::escape(h)).collect();
    match regex::Regex::new(&format!("(?i)^https?://(?:{})/", alts.join("|"))) {
        Ok(re) => re.is_match(url),
        Err(_) => false,
    }
}

/// `opts.replace(/[;<>{}]/g, '')`, plus no escapes, quotes or CSS functions that load resources.
fn safe_bg_opts(opts: &str) -> String {
    static FUNCS: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)(url|image|image-set|-webkit-image-set|cross-fade|element|expression|src)\s*\(").unwrap());
    let stripped: String = opts.chars().filter(|c| !matches!(c, ';' | '<' | '>' | '{' | '}' | '\\' | '"' | '\'')).collect();
    let mut out = stripped;
    while FUNCS.is_match(&out) {
        out = FUNCS.replace_all(&out, "").into_owned();
    }
    out
}

async fn set_background(st: AppState, mut req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    if !is_staff(&req) {
        return forbidden();
    }
    let body = req.post_value().await;
    let url = match body.get("url") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None | Some(Value::Bool(false)) => String::new(),
        Some(v) => v.to_string(),
    };
    if url.is_empty() {
        save_setting(&st, "background", None).await;
        if let Err(e) = notify(&st, "global_chat_background", json!({ "background": null }).to_string()).await {
            tracing::error!("[Chat] BACKGROUND error: {e}");
            return j(json!({ "success": false }), 500);
        }
        return j(json!({ "success": true }), 200);
    }
    if !bg_url_allowed(&st, &url) {
        return j(json!({ "success": false, "msg": "URL not from an allowed host" }), 400);
    }
    let opts = body.get("opts").and_then(Value::as_str).filter(|o| !o.is_empty()).unwrap_or("center / cover no-repeat");
    let css = format!("url({}) {}", Value::String(url).to_string(), safe_bg_opts(opts));
    save_setting(&st, "background", Some(&css)).await;
    match notify(&st, "global_chat_background", json!({ "background": css }).to_string()).await {
        Ok(()) => j(json!({ "success": true, "background": css }), 200),
        Err(e) => {
            tracing::error!("[Chat] BACKGROUND error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── Topic ───────────────────────────────────────────────────────────────────

async fn get_topic(st: AppState, _req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    let topic = setting(&st, "topic").await;
    j(json!({ "success": true, "topic": topic }), 200)
}

async fn set_topic(st: AppState, mut req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    if !is_staff(&req) {
        return forbidden();
    }
    let raw = req.post_str("topic").await.unwrap_or_default().trim().to_string();
    let topic = (!raw.is_empty()).then_some(raw);
    save_setting(&st, "topic", topic.as_deref()).await;
    match notify(&st, "global_chat_topic", json!({ "topic": topic }).to_string()).await {
        Ok(()) => j(json!({ "success": true, "topic": topic }), 200),
        Err(e) => {
            tracing::error!("[Chat] TOPIC error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

// ── Moderation ──────────────────────────────────────────────────────────────

async fn delete_message(st: AppState, req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    if !is_staff(&req) {
        return forbidden();
    }
    let id = req.param("id").and_then(|i| i.parse::<i64>().ok()).unwrap_or(0);
    if id == 0 {
        return j(json!({ "success": false }), 400);
    }
    let r: anyhow::Result<()> = async {
        sql!("DELETE FROM global_chat WHERE id = ", id).execute(&st.db).await?;
        notify(&st, "global_chat_delete", json!({ "id": id }).to_string()).await
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[Chat] DELETE message error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn clear(st: AppState, req: Req) -> Res {
    if !enabled(&st) {
        return disabled();
    }
    if !is_staff(&req) {
        return forbidden();
    }
    let r: anyhow::Result<()> = async {
        sql!("TRUNCATE global_chat RESTART IDENTITY").execute(&st.db).await?;
        notify(&st, "global_chat_clear", String::new()).await
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[Chat] CLEAR error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bg_opts() {
        assert_eq!(safe_bg_opts("center / cover no-repeat"), "center / cover no-repeat");
        assert_eq!(safe_bg_opts("center; color: red"), "center color: red");
        assert_eq!(safe_bg_opts("center, url(https://x/y.png)"), "center, https://x/y.png)");
        assert_eq!(safe_bg_opts("uurl(rl(x)"), "x)");
    }
}
