//! Ban status page (port of src/inc/routes/banned.mjs).

use crate::db::RowExt;
use crate::http::{Req, Res, Router, html};
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/banned\/?$", banned);
}

async fn banned(st: AppState, req: Req) -> Res {
    let mut is_banned = false;
    let mut reason = "Violation of community rules".to_string();
    let mut expires: Option<String> = None;

    if let Some(s) = req.session().filter(|s| s.banned) {
        is_banned = true;
        reason = s.ban_reason.clone().filter(|r| !r.is_empty()).unwrap_or(reason);
        expires = s.ban_expires.clone();
    }

    let client_ip = req.real_ip();
    if let Some(b) = crate::security::is_ip_banned(&st, &client_ip).await {
        is_banned = true;
        reason = b.str("reason").filter(|r| !r.is_empty()).map(str::to_string).unwrap_or(reason);
        expires = b.str("expires").map(str::to_string);
    }

    if let Some(fp) = req.session().and_then(|s| s.fingerprint.clone().or_else(|| s.anon_fingerprint.clone())) {
        if let Some(b) = crate::security::is_fingerprint_banned(&st, &fp).await {
            is_banned = true;
            reason = b.str("reason").filter(|r| !r.is_empty()).map(str::to_string).unwrap_or(reason);
            expires = b.str("expires").map(str::to_string);
        }
    }

    if let Some(c) = req.cookie("f0ck_banned") {
        if let Ok(Value::Object(b)) = serde_json::from_str::<Value>(&crate::util::decode_lossy(c)) {
            if crate::config::truthy(b.get("reason").unwrap_or(&Value::Null)) || crate::config::truthy(b.get("banned").unwrap_or(&Value::Null)) {
                is_banned = true;
                if let Some(r) = b.get("reason").and_then(Value::as_str).filter(|r| !r.is_empty()) {
                    reason = r.to_string();
                }
                if let Some(e) = b.get("expires").and_then(Value::as_str).filter(|e| !e.is_empty()) {
                    expires = Some(e.to_string());
                }
            }
        }
    }

    let expires_text = expires
        .as_deref()
        .and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok())
        .map(|d| crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None))
        .unwrap_or_else(|| if expires.is_some() { "Invalid Date".into() } else { "Permanent".into() });

    let body = crate::tpl::render(
        &st,
        Some(&req),
        "banned",
        json!({
            "session": req.session_value(),
            "reason": reason,
            "expires": expires_text,
            "isBanned": is_banned,
            "clientIp": client_ip,
            "page_meta": { "title": if is_banned { "Banned" } else { "Ban Status" } }
        }),
    );
    Ok(html(body))
}
