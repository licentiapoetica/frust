//! Admin dashboard, site settings, page texts, cleanup, sessions, invite tokens/requests, Köpfe,
//! the halls/chat pages and the config editor API (src/inc/routes/admin.mjs).

use super::{body_id, body_str, body_truthy, err_msg, fail_json, save_setting, totals};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_raw, json_resp, raw, redirect, reply};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin(\/)?$", Guard::Auth, dashboard);
    r.get_g(r"^\/admin\/sessions(\/)?$", Guard::Auth, sessions_page);
    r.post_g(r"^\/api\/v2\/admin\/sessions\/delete\/?$", Guard::Auth, session_delete);
    r.get_g(r"^\/admin\/approve\/?", Guard::ModAuth, approve_redirect);
    r.get_g(r"^\/admin\/deny\/?", Guard::ModAuth, deny_redirect);
    r.post_g(r"^\/admin\/deny-multi\/?", Guard::Auth, |_st, _req| async { Ok(location(307, "/mod/deny-multi")) });
    r.post_g(r"^\/admin\/purge-trash-all\/?", Guard::Auth, |_st, _req| async { Ok(location(307, "/mod/purge-trash-all")) });

    r.get_g(r"^\/admin\/tokens\/?$", Guard::Auth, tokens_page);
    r.get_g(r"^\/api\/v2\/admin\/tokens\/?$", Guard::Auth, tokens_list);
    r.post_g(r"^\/api\/v2\/admin\/tokens\/create\/?$", Guard::Auth, token_create);
    r.post_g(r"^\/api\/v2\/admin\/tokens\/delete\/?$", Guard::Auth, token_delete);
    r.get_g(r"^\/api\/v2\/admin\/invite-requests\/?$", Guard::Auth, invite_requests);
    r.post_g(r"^\/api\/v2\/admin\/invite-requests\/approve\/?$", Guard::Auth, invite_approve);
    r.post_g(r"^\/api\/v2\/admin\/invite-requests\/deny\/?$", Guard::Auth, invite_deny);

    r.get_g(r"^\/admin\/motd\/?$", Guard::Auth, motd_page);
    r.post_g(r"^\/admin\/motd\/?$", Guard::Auth, motd_save);
    r.post_g(r"^\/admin\/settings\/?$", Guard::Auth, settings_save);
    r.get(r"^\/api\/v2\/admin\/config\/?$", config_get);
    r.post(r"^\/api\/v2\/admin\/config\/?$", config_save);
    r.get_g(r"^\/admin\/cleanup\/?$", Guard::Auth, cleanup_page);
    r.post_g(r"^\/admin\/cleanup\/?$", Guard::Auth, cleanup_save);
    r.post_g(r"^\/admin\/cleanup\/run\/?$", Guard::Auth, cleanup_run);

    r.get_g(r"^\/admin\/about\/?$", Guard::Auth, |st, req| text_page(st, req, PageText::About));
    r.post_g(r"^\/admin\/about\/?$", Guard::Auth, |st, req| text_save(st, req, PageText::About));
    r.get_g(r"^\/admin\/rules\/?$", Guard::Auth, |st, req| text_page(st, req, PageText::Rules));
    r.post_g(r"^\/admin\/rules\/?$", Guard::Auth, |st, req| text_save(st, req, PageText::Rules));
    r.get_g(r"^\/admin\/terms\/?$", Guard::Auth, |st, req| text_page(st, req, PageText::Terms));
    r.post_g(r"^\/admin\/terms\/?$", Guard::Auth, |st, req| text_save(st, req, PageText::Terms));

    r.get_g(r"^\/admin\/halls\/?$", Guard::ModAuth, halls_page);
    r.get_g(r"^\/admin\/chat\/?$", Guard::Auth, chat_page);
    r.get_g(r"^\/admin\/koepfe\/?$", Guard::Auth, koepfe_page);
    r.get_g(r"^\/api\/v2\/admin\/koepfe\/?$", Guard::Auth, koepfe_list);
    r.post_g(r"^\/api\/v2\/admin\/koepfe\/delete\/?$", Guard::Auth, koepfe_delete);
}

/// `res.writeHead(code, { Location })` (no other headers).
pub(crate) fn location(code: u16, url: &str) -> Response {
    let mut r = raw(code, "");
    if let Ok(v) = HeaderValue::from_str(url) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

fn rows(v: Vec<Row>) -> Value {
    Value::Array(v.into_iter().map(Value::Object).collect())
}

fn render(st: &AppState, req: &Req, view: &str, data: Value) -> Res {
    Ok(html(crate::tpl::render(st, Some(req), view, data)))
}

fn sid(req: &Req) -> i64 {
    req.session().map(|s| s.id).unwrap_or(0)
}

// ── Dashboard ───────────────────────────────────────────────────────────────

async fn dashboard(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    // Dashboard counters (cheap aggregate queries; failures just show 0)
    let q = sql!(
        r#"SELECT
            (SELECT count(*)::int FROM reports WHERE status = 'pending') AS open_reports,
            (SELECT count(*)::int FROM "user" WHERE NOT EXISTS (SELECT 1 FROM anon_identities ai WHERE ai.user_id = "user".id)) AS users,
            (SELECT count(*)::int FROM "user" WHERE EXISTS (SELECT 1 FROM anon_identities ai WHERE ai.user_id = "user".id)) AS anon_users,
            (SELECT count(*)::int FROM items WHERE active = false AND is_deleted = true AND is_purged = false) AS trash,
            (SELECT count(*)::int FROM invite_requests WHERE status = 'pending') AS invite_requests"#
    );
    let dash = match q.fetch_optional(&st.db).await {
        Ok(Some(r)) => {
            let n = |k: &str| r.i64(k).unwrap_or(0);
            json!({ "open_reports": n("open_reports"), "users": n("users"), "anon_users": n("anon_users"), "trash": n("trash"), "invite_requests": n("invite_requests") })
        }
        other => {
            if let Err(e) = other {
                tracing::error!("[ADMIN] dashboard counters failed: {}", err_msg(&e));
            }
            json!({ "open_reports": 0, "users": 0, "anon_users": 0, "trash": 0, "invite_requests": 0 })
        }
    };
    let s = st.settings.get();
    let data = json!({
        "dash": dash,
        "totals": totals(&st).await,
        "session": req.session_value(),
        "manual_approval": s.manual_approval,
        "min_tags": s.min_tags,
        "trusted_uploads": s.trusted_uploads,
        "registration_open": st.settings.registration_open(&cfg),
        "registration_web_toggle_enabled": cfg.enabled("websrv.open_registration_web_toggle"),
        "log_user_ips": crate::settings::log_user_ips(&cfg),
        "hash_user_ips": crate::settings::hash_user_ips(&cfg),
        "enable_cleanup": st.settings.enable_cleanup(&cfg),
        "shitpost_mode": crate::settings::shitpost_mode(&cfg),
        "enable_cleanup_config": cfg.enabled("websrv.enable_cleanup"),
        "current_brand_image": s.brand_image_url,
        "tmp": null,
    });
    render(&st, &req, "admin", data)
}

async fn sessions_page(st: AppState, req: Req) -> Res {
    let list = sql!(
        r#"select "user_sessions".*, "user".user
           from "user_sessions"
           left join "user" on "user".id = "user_sessions".user_id
           order by "user_sessions".last_used desc"#
    )
    .fetch_all(&st.db)
    .await?;
    let now = crate::util::now_secs();
    let mut active: Vec<Value> = Vec::new();
    for r in &list {
        if now - r.i64("last_used").unwrap_or(0) < 15 * 60 {
            let u = r.val("user").clone();
            if !active.contains(&u) {
                active.push(u);
            }
        }
    }
    let data = json!({
        "session": req.session_value(),
        "sessions": rows(list),
        "activeUsers": active.len(),
        "activeUserList": active,
        "totals": totals(&st).await,
        "log_user_ips": crate::settings::log_user_ips(&st.cfg()),
        "tmp": null,
    });
    render(&st, &req, "admin/sessions", data)
}

async fn session_delete(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r: anyhow::Result<()> = async {
        let id = super::require_id(&body, "id").map_err(|_| anyhow::anyhow!("Missing session ID"))?;
        // Prevent self-deletion of current session
        if req.session().map(|s| s.sess_id) == Some(id as i64) {
            anyhow::bail!("Cannot delete your current session");
        }
        sql!(r#"delete from "user_sessions" where id = "#, id).execute(&st.db).await?;
        Ok(())
    }
    .await;
    Ok(json_resp(&match r {
        Ok(()) => json!({ "success": true }),
        Err(e) => fail_json(&e),
    }, 200))
}

async fn approve_redirect(_st: AppState, req: Req) -> Res {
    Ok(match req.query("id").filter(|i| !i.is_empty()) {
        Some(id) => redirect(&format!("/mod/approve?id={}", crate::util::encode_uri_component(id))),
        None => redirect("/mod/approve"),
    })
}

async fn deny_redirect(_st: AppState, req: Req) -> Res {
    Ok(match req.query("id").filter(|i| !i.is_empty()) {
        Some(id) => redirect(&format!("/mod/deny?id={}", crate::util::encode_uri_component(id))),
        None => redirect("/mod/approve"),
    })
}

// ── Invite tokens / requests ────────────────────────────────────────────────

async fn tokens_page(st: AppState, req: Req) -> Res {
    render(&st, &req, "admin/tokens", json!({ "session": req.session_value(), "tmp": null }))
}

async fn tokens_list(st: AppState, _req: Req) -> Res {
    let tokens = sql!(
        r#"select invite_tokens.*,
                  u_used.user as used_by_name,
                  u_created.user as created_by_name
           from invite_tokens
           left join "user" as u_used on u_used.id = invite_tokens.used_by
           left join "user" as u_created on u_created.id = invite_tokens.created_by
           order by created_at desc"#
    )
    .fetch_all(&st.db)
    .await?;
    Ok(json_resp(&json!({ "success": true, "tokens": rows(tokens) }), 200))
}

/// 128-bit random invite token (upper-case hex).
fn new_token() -> String {
    crate::util::random_hex(16).to_uppercase()
}

async fn insert_token(st: &AppState, created_by: i64) -> anyhow::Result<(String, i64)> {
    let token = new_token();
    let row = sql!(
        "insert into invite_tokens (token, created_at, created_by) values (", token.clone(), ", ", crate::util::now_secs(), ", ", created_by as i32, ") returning id"
    )
    .fetch_optional(&st.db)
    .await?;
    Ok((token, row.and_then(|r| r.i64("id")).unwrap_or(0)))
}

async fn token_create(st: AppState, req: Req) -> Res {
    match insert_token(&st, sid(&req)).await {
        Ok((token, _)) => Ok(json_resp(&json!({ "success": true, "token": token }), 200)),
        Err(e) => {
            let msg = crate::util::log_error(&e, "Token creation failed");
            Ok(json_resp(&json!({ "success": false, "msg": msg }), 200))
        }
    }
}

async fn token_delete(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    if !body_truthy(&body, "id") {
        return Ok(json_resp(&json!({ "success": false }), 200));
    }
    let Some(id) = body_id(&body, "id").map(|i| i as i32) else {
        return Ok(json_resp(&json!({ "success": false }), 200));
    };
    // Revoke any linked invite request and notify the user
    let linked = sql!("SELECT ir.id, ir.user_id FROM invite_requests ir WHERE ir.token_id = ", id, " AND ir.status = 'approved'")
        .fetch_optional(&st.db)
        .await?;
    if let Some(l) = linked {
        sql!(
            "UPDATE invite_requests SET status = 'revoked', reviewed_at = NOW(), reviewed_by = ", sid(&req) as i32,
            " WHERE id = ", l.i64("id").unwrap_or(0) as i32
        )
        .execute(&st.db)
        .await?;
        if let Some(uid) = l.i64("user_id") {
            sql!(
                "INSERT INTO notifications (user_id, type, reference_id, data) VALUES (", uid as i32, ", 'invite_denied', 0, ",
                json!({ "revoked": true }), ")"
            )
            .execute(&st.db)
            .await?;
        }
    }
    sql!("delete from invite_tokens where id = ", id).execute(&st.db).await?;
    Ok(json_resp(&json!({ "success": true }), 200))
}

async fn invite_requests(st: AppState, _req: Req) -> Res {
    let r = sql!(
        r#"SELECT ir.*,
                  u_reviewer.user as reviewed_by_name,
                  u_requester.login as requester_login,
                  u_requester.user as requester_name
           FROM invite_requests ir
           LEFT JOIN "user" u_reviewer ON u_reviewer.id = ir.reviewed_by
           LEFT JOIN "user" u_requester ON u_requester.id = ir.user_id
           ORDER BY CASE WHEN ir.status = 'pending' THEN 0 ELSE 1 END, ir.created_at DESC
           LIMIT 200"#
    )
    .fetch_all(&st.db)
    .await;
    Ok(match r {
        Ok(list) => json_resp(&json!({ "success": true, "requests": rows(list) }), 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": crate::util::log_error(&e, "Failed to fetch invite requests") }), 200),
    })
}

/// Pending invite request by the posted `id` (or the response to send).
async fn pending_request(st: &AppState, body: &Value) -> Result<(i32, Row), Response> {
    let Some(id) = body_truthy(body, "id").then(|| body_id(body, "id")).flatten() else {
        return Err(json_resp(&json!({ "success": false, "msg": "Missing request id" }), 200));
    };
    let id = id as i32;
    match sql!("SELECT * FROM invite_requests WHERE id = ", id, " AND status = 'pending'").fetch_optional(&st.db).await {
        Ok(Some(r)) => Ok((id, r)),
        Ok(None) => Err(json_resp(&json!({ "success": false, "msg": "Request not found or already processed" }), 200)),
        Err(e) => Err(json_resp(&json!({ "success": false, "msg": crate::util::log_error(&e, "Failed to load invite request") }), 200)),
    }
}

async fn notify_requester(st: &AppState, fingerprint: &str, kind: &str, data: Value) -> anyhow::Result<()> {
    let anon = sql!("SELECT user_id FROM anon_identities WHERE fingerprint = ", fingerprint, " LIMIT 1").fetch_optional(&st.db).await?;
    if let Some(uid) = anon.and_then(|r| r.i64("user_id")) {
        sql!("INSERT INTO notifications (user_id, type, reference_id, data) VALUES (", uid as i32, ", ", kind, ", 0, ", data, ")")
            .execute(&st.db)
            .await?;
    }
    Ok(())
}

fn fp_prefix(fp: &str) -> String {
    fp.chars().take(12).collect()
}

async fn invite_approve(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let (id, request) = match pending_request(&st, &body).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let r: anyhow::Result<String> = async {
        let (token, token_id) = insert_token(&st, sid(&req)).await?;
        sql!(
            "UPDATE invite_requests SET status = 'approved', token_id = ", token_id as i32, ", reviewed_by = ", sid(&req) as i32,
            ", reviewed_at = NOW() WHERE id = ", id
        )
        .execute(&st.db)
        .await?;
        let fp = request.string("fingerprint");
        notify_requester(&st, &fp, "invite_approved", json!({ "token": token, "request_id": id })).await?;
        tracing::info!("[INVITE-REQ] Approved request #{id} (fp: {}…) → token {}…", fp_prefix(&fp), &token[..8]);
        Ok(token)
    }
    .await;
    Ok(match r {
        Ok(token) => json_resp(&json!({ "success": true, "token": token }), 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": crate::util::log_error(&e, "Failed to approve invite request") }), 200),
    })
}

async fn invite_deny(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let (id, request) = match pending_request(&st, &body).await {
        Ok(v) => v,
        Err(r) => return Ok(r),
    };
    let r: anyhow::Result<()> = async {
        sql!("UPDATE invite_requests SET status = 'denied', reviewed_by = ", sid(&req) as i32, ", reviewed_at = NOW() WHERE id = ", id)
            .execute(&st.db)
            .await?;
        let fp = request.string("fingerprint");
        notify_requester(&st, &fp, "invite_denied", json!({ "request_id": id })).await?;
        tracing::info!("[INVITE-REQ] Denied request #{id} (fp: {}…)", fp_prefix(&fp));
        Ok(())
    }
    .await;
    Ok(match r {
        Ok(()) => json_resp(&json!({ "success": true }), 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": crate::util::log_error(&e, "Failed to deny invite request") }), 200),
    })
}

// ── MOTD, settings, page texts ──────────────────────────────────────────────

async fn setting_value(st: &AppState, key: &str) -> anyhow::Result<String> {
    let r = sql!("SELECT value FROM site_settings WHERE key = ", key, " LIMIT 1").fetch_optional(&st.db).await?;
    Ok(r.map(|r| r.string("value")).unwrap_or_default())
}

async fn motd_page(st: AppState, req: Req) -> Res {
    let motd = setting_value(&st, "motd").await?;
    let data = json!({ "session": req.session_value(), "motd": motd, "totals": totals(&st).await, "tmp": null });
    render(&st, &req, "admin/motd", data)
}

/// Longest prefix of `s` with at most `max` bytes, cut at a char boundary.
fn byte_prefix(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The XHR/form answer shared by the save handlers.
fn saved(req: &Req, ok: Value, back: &str) -> Response {
    if req.is_xhr() { json_raw(&ok, 200) } else { location(302, back) }
}

fn save_failed(req: &Req, msg: String) -> Response {
    if req.is_xhr() { json_raw(&json!({ "success": false, "msg": msg }), 500) } else { reply(500, "text/html", msg) }
}

async fn motd_save(st: AppState, mut req: Req) -> Res {
    let motd = body_str(req.post().await?, "motd").unwrap_or_default();
    let r: anyhow::Result<()> = async {
        save_setting(&st, "motd", &motd).await?;
        st.settings.update(|s| s.motd = motd.clone());
        crate::audit::log(&st, sid(&req), "update_motd", "system", "0".to_string(), json!({ "motd": motd })).await;
        // Broadcast to live clients (pg_notify payloads are limited to 8000 bytes)
        if let Err(e) = sql!("SELECT pg_notify('motd', ", byte_prefix(&motd, 7000), ")").execute(&st.db).await {
            tracing::error!("[ADMIN] MOTD Notify failed: {}", err_msg(&e));
        }
        Ok(())
    }
    .await;
    Ok(match r {
        Ok(()) => saved(&req, json!({ "success": true, "motd": motd }), "/admin/motd"),
        Err(e) => {
            tracing::error!("[ADMIN] MOTD Save failed: {e:#}");
            save_failed(&req, format!("Failed to save MOTD: {}", err_msg(&e)))
        }
    })
}

/// `parseInt(v)` of a form value.
fn parse_int(v: Option<String>) -> Option<i64> {
    v.as_deref().and_then(crate::social::helpers::js_parse_int)
}

async fn settings_save(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let body = req.post().await?.clone();
    let manual_approval = body_str(&body, "manual_approval").as_deref() == Some("on");
    let registration_open = body_str(&body, "registration_open").as_deref() == Some("on");
    let min_tags = parse_int(body_str(&body, "min_tags")).unwrap_or(0).max(0);
    // The old `Math.max(0, parseInt(x) ?? 3)` stored "NaN" for a non-numeric value; default to 3 instead.
    let trusted_uploads = parse_int(body_str(&body, "trusted_uploads")).unwrap_or(0).max(0);

    save_setting(&st, "manual_approval", if manual_approval { "true" } else { "false" }).await?;
    let web_toggle = cfg.enabled("websrv.open_registration_web_toggle");
    if web_toggle {
        save_setting(&st, "registration_open", if registration_open { "true" } else { "false" }).await?;
    }
    save_setting(&st, "min_tags", &min_tags.to_string()).await?;
    save_setting(&st, "trusted_uploads", &trusted_uploads.to_string()).await?;
    st.settings.update(|s| {
        s.manual_approval = manual_approval;
        if web_toggle {
            s.registration_open = registration_open;
        }
        s.min_tags = min_tags;
        s.trusted_uploads = trusted_uploads;
    });

    if req.is_xhr() {
        let s = st.settings.get();
        return Ok(json_resp(
            &json!({
                "success": true,
                "manual_approval": s.manual_approval,
                "registration_open": st.settings.registration_open(&cfg),
                "min_tags": s.min_tags,
                "trusted_uploads": s.trusted_uploads,
            }),
            200,
        ));
    }
    Ok(location(302, "/admin"))
}

#[derive(Clone, Copy)]
enum PageText {
    About,
    Rules,
    Terms,
}

impl PageText {
    fn key(self) -> &'static str {
        match self {
            PageText::About => "about_text",
            PageText::Rules => "rules_text",
            PageText::Terms => "terms_text",
        }
    }
    fn name(self) -> &'static str {
        match self {
            PageText::About => "about",
            PageText::Rules => "rules",
            PageText::Terms => "terms",
        }
    }
}

async fn text_page(st: AppState, req: Req, page: PageText) -> Res {
    let text = setting_value(&st, page.key()).await?;
    let mut data = json!({ "session": req.session_value(), "totals": totals(&st).await, "tmp": null });
    data[page.key()] = json!(text);
    render(&st, &req, &format!("admin/{}", page.name()), data)
}

async fn text_save(st: AppState, mut req: Req, page: PageText) -> Res {
    let text = body_str(req.post().await?, page.key()).unwrap_or_default();
    let r = save_setting(&st, page.key(), &text).await;
    Ok(match r {
        Ok(()) => {
            st.settings.update(|s| match page {
                PageText::About => s.about_text = text.clone(),
                PageText::Rules => s.rules_text = text.clone(),
                PageText::Terms => s.terms_text = text.clone(),
            });
            crate::audit::log(&st, sid(&req), &format!("update_{}", page.key()), "system", None, json!({})).await;
            saved(&req, json!({ "success": true }), &format!("/admin/{}", page.name()))
        }
        Err(e) => {
            tracing::error!("[ADMIN] {} Save failed: {e:#}", page.name());
            save_failed(&req, format!("Failed to save {} text: {}", page.name(), err_msg(&e)))
        }
    })
}

// ── Config editor API ───────────────────────────────────────────────────────
//
// The old handlers also set `Access-Control-Allow-Origin: <request Origin>` with
// `Access-Control-Allow-Credentials: true` themselves; that per-route reflection is dropped here.
// NOTE: the global `/api/` CORS middleware (`app::cors`, from index.mjs) still reflects any origin.

fn config_unauthorized(req: &Req) -> Option<Response> {
    (!req.session().map(|s| s.admin).unwrap_or(false)).then(|| json_raw(&json!({ "success": false, "msg": "Unauthorized" }), 401))
}

async fn config_get(st: AppState, req: Req) -> Res {
    if let Some(r) = config_unauthorized(&req) {
        return Ok(r);
    }
    let path = st.cfg().base.join("config.yaml");
    let r = async {
        let text = tokio::fs::read_to_string(&path).await?;
        crate::config_write::parse_yaml(&text)
    }
    .await;
    Ok(match r {
        Ok(config) => json_resp(&json!({ "success": true, "config": config }), 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": err_msg(&e) }), 500),
    })
}

/// The posted config: `{ config: {...} }`, `{ config: "<json>" }` or the object itself.
fn config_payload(body: &Value) -> anyhow::Result<Value> {
    let mut cfg = match body.get("config") {
        Some(c) if crate::config::truthy(c) => c.clone(),
        _ => body.clone(),
    };
    if let Value::String(s) = &cfg {
        cfg = serde_json::from_str(s).map_err(|e| anyhow::anyhow!("Invalid JSON: {e}"))?;
    }
    let Value::Object(map) = &mut cfg else { anyhow::bail!("Invalid configuration payload") };
    // A form/JSON body may carry the CSRF token next to the config keys.
    map.remove("csrf_token");
    // Replacing the whole file with a partial object would delete every missing section.
    for section in ["main", "websrv", "sql"] {
        if !map.get(section).map(Value::is_object).unwrap_or(false) {
            anyhow::bail!("Invalid configuration payload: missing \"{section}\" section");
        }
    }
    Ok(cfg)
}

async fn config_save(st: AppState, mut req: Req) -> Res {
    if let Some(r) = config_unauthorized(&req) {
        return Ok(r);
    }
    let body = req.post().await?.clone();
    let r: anyhow::Result<Value> = async {
        let new_config = config_payload(&body)?;
        let base = st.cfg().base.clone();
        let nc = new_config.clone();
        // Saved to config.yaml with its comments kept, then applied to the running config
        tokio::task::spawn_blocking(move || crate::config_write::write_config_file(&base, &nc)).await??;
        let reloaded = st.cfg().reload()?;
        st.cfg.store(Arc::new(reloaded));
        if crate::settings::enable_item_slugs(&st.cfg()) {
            let s = st.clone();
            tokio::spawn(async move { crate::backfill_slugs(&s).await });
        }
        Ok(new_config)
    }
    .await;
    match r {
        Ok(cfg) => {
            let keys: Vec<&String> = cfg.as_object().map(|m| m.keys().collect()).unwrap_or_default();
            crate::audit::log(&st, sid(&req), "update_config_file", "system", "0".to_string(), json!({ "updatedKeys": keys })).await;
            Ok(json_resp(&json!({ "success": true, "message": "Configuration saved to config.yaml" }), 200))
        }
        Err(e) => {
            tracing::error!("[ADMIN] Config Save Error: {e:#}");
            Ok(json_resp(&fail_json(&e), 400))
        }
    }
}

// ── Cleanup ─────────────────────────────────────────────────────────────────

fn cleanup_data(st: &AppState, req: &Req) -> Value {
    let s = st.settings.get();
    json!({
        "session": req.session_value(),
        "enable_cleanup": st.settings.enable_cleanup(&st.cfg()),
        "cleanup_start_date": s.cleanup_start_date,
        "cleanup_end_date": s.cleanup_end_date,
        "cleanup_include_engaged": s.cleanup_include_engaged,
        "tmp": null,
    })
}

async fn cleanup_page(st: AppState, req: Req) -> Res {
    if !st.settings.enable_cleanup(&st.cfg()) {
        return Ok(redirect("/admin"));
    }
    let mut data = cleanup_data(&st, &req);
    data["totals"] = totals(&st).await;
    render(&st, &req, "admin/cleanup", data)
}

fn flag_value(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => matches!(s.as_str(), "true" | "on" | "1"),
        _ => false,
    }
}

async fn cleanup_save(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let start = body_str(&body, "cleanup_start_date").unwrap_or_default();
    let end = body_str(&body, "cleanup_end_date").unwrap_or_default();
    let engaged = flag_value(body.get("cleanup_include_engaged"));
    let r: anyhow::Result<()> = async {
        save_setting(&st, "cleanup_start_date", &start).await?;
        save_setting(&st, "cleanup_end_date", &end).await?;
        save_setting(&st, "cleanup_include_engaged", if engaged { "true" } else { "false" }).await?;
        Ok(())
    }
    .await;
    Ok(match r {
        Ok(()) => {
            st.settings.update(|s| {
                s.cleanup_start_date = start.clone();
                s.cleanup_end_date = end.clone();
                s.cleanup_include_engaged = engaged;
            });
            let s = st.settings.get();
            saved(
                &req,
                json!({
                    "success": true,
                    "enable_cleanup": st.settings.enable_cleanup(&st.cfg()),
                    "cleanup_start_date": s.cleanup_start_date,
                    "cleanup_end_date": s.cleanup_end_date,
                    "cleanup_include_engaged": s.cleanup_include_engaged,
                }),
                "/admin/cleanup",
            )
        }
        Err(e) => {
            tracing::error!("[ADMIN] Cleanup Settings Save failed: {e:#}");
            save_failed(&req, format!("Failed to save Cleanup settings: {}", err_msg(&e)))
        }
    })
}

/// `~~(new Date("YYYY-MM-DD").getTime() / 1000)` (date-only strings are UTC midnight).
fn date_stamp(d: &str) -> Option<i64> {
    let d = d.trim();
    if let Ok(nd) = chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d") {
        return nd.and_hms_opt(0, 0, 0).map(|t| t.and_utc().timestamp());
    }
    chrono::DateTime::parse_from_rfc3339(d).ok().map(|t| t.timestamp())
}

async fn cleanup_run(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r: anyhow::Result<Value> = async {
        // Settings are re-read from the DB before running
        let list = sql!("SELECT key, value FROM site_settings WHERE key IN ('enable_cleanup', 'cleanup_start_date', 'cleanup_end_date', 'cleanup_include_engaged')")
            .fetch_all(&st.db)
            .await?;
        let get = |k: &str| list.iter().find(|r| r.str("key") == Some(k)).map(|r| r.string("value"));
        let cfg = st.cfg();
        let enabled = match get("enable_cleanup") {
            Some(v) => v == "true",
            None => st.settings.enable_cleanup(&cfg),
        };
        let start = get("cleanup_start_date").unwrap_or_default();
        let end = get("cleanup_end_date").unwrap_or_default();
        let engaged = match body.get("include_engaged") {
            Some(v) if !v.is_null() => flag_value(Some(v)),
            _ => get("cleanup_include_engaged").as_deref() == Some("true") || st.settings.get().cleanup_include_engaged,
        };
        st.settings.update(|s| {
            s.enable_cleanup = enabled;
            s.cleanup_start_date = start.clone();
            s.cleanup_end_date = end.clone();
            s.cleanup_include_engaged = engaged;
        });
        if !enabled || cfg.is_false("websrv.enable_cleanup") {
            anyhow::bail!("Cleanup is disabled in settings.");
        }
        if start.is_empty() || end.is_empty() {
            anyhow::bail!("Please select both a Start Date and an End Date.");
        }
        let (Some(start_stamp), Some(end_day)) = (date_stamp(&start), date_stamp(&end)) else {
            anyhow::bail!("Invalid date.");
        };
        let end_stamp = end_day + 86399; // Include full end day
        tracing::info!("[ADMIN] Starting manual cleanup for period {start} to {end} (includeEngaged: {engaged})...");

        let total: i64 = sql!("SELECT count(*) FROM items WHERE is_purged = false AND is_pinned = false").fetch_scalar(&st.db).await?.unwrap_or(0);
        let in_range: i64 = sql!("SELECT count(*) FROM items WHERE is_purged = false AND is_pinned = false AND stamp >= ", start_stamp, " AND stamp <= ", end_stamp)
            .fetch_scalar(&st.db)
            .await?
            .unwrap_or(0);
        let mut q = sql!(
            "SELECT i.id, i.dest, i.mime FROM items i WHERE i.is_purged = false AND i.is_pinned = false AND i.stamp >= ", start_stamp,
            " AND i.stamp <= ", end_stamp
        );
        if !engaged {
            q.push(
                " AND (
                    (i.active = true AND i.is_deleted = false AND NOT EXISTS (SELECT 1 FROM comments WHERE item_id = i.id) AND NOT EXISTS (SELECT 1 FROM favorites WHERE item_id = i.id))
                    OR (i.is_deleted = true)
                    OR (i.active = false AND i.is_deleted = false)
                  )",
            );
        }
        let old = q.fetch_all(&st.db).await?;
        let stats = format!("(Total items: {total}, In range: {in_range})");
        tracing::info!("[ADMIN] Cleanup diagnostic: found {} targets. {stats}", old.len());

        let mut count = 0i64;
        if !old.is_empty() {
            for item in &old {
                let id = item.i64("id").unwrap_or(0);
                let res: anyhow::Result<()> = async {
                    crate::media::delete::safe_delete_media_file(&st, &item.string("dest"), id).await;
                    let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("{id}.webp"))).await;
                    let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("{id}_blur.webp"))).await;
                    if item.string("mime").starts_with("audio") {
                        let _ = tokio::fs::remove_file(cfg.paths.ca.join(format!("{id}.webp"))).await;
                    }
                    sql!("DELETE FROM tags_assign WHERE item_id = ", id as i32).execute(&st.db).await?;
                    sql!("UPDATE items SET is_deleted = true, is_purged = true, active = false WHERE id = ", id as i32).execute(&st.db).await?;
                    Ok(())
                }
                .await;
                match res {
                    Ok(()) => count += 1,
                    Err(e) => tracing::error!("[CLEANUP] Failed to delete item {id}: {}", err_msg(&e)),
                }
            }
            // Remaining orphan tag assignments of deleted or inactive items
            sql!("DELETE FROM tags_assign WHERE item_id IN (SELECT id FROM items WHERE is_deleted = true OR active = false)").execute(&st.db).await?;
        }
        crate::audit::log(&st, sid(&req), "run_cleanup_manual", "system", "0".to_string(), json!({ "count": count, "startDate": start, "endDate": end })).await;
        Ok(json!({ "success": true, "count": count, "msg": format!("Successfully cleaned up {count} posts. {stats}") }))
    }
    .await;
    match r {
        Ok(response) => {
            if req.is_xhr() {
                return Ok(json_raw(&response, 200));
            }
            let mut data = cleanup_data(&st, &req);
            for (k, v) in response.as_object().into_iter().flatten() {
                data[k] = v.clone();
            }
            data["totals"] = totals(&st).await;
            render(&st, &req, "admin/cleanup", data)
        }
        Err(e) => {
            tracing::error!("[ADMIN] Cleanup execution failed: {e:#}");
            Ok(save_failed(&req, format!("Cleanup failed: {}", err_msg(&e))))
        }
    }
}

// ── Halls / chat / Köpfe ────────────────────────────────────────────────────

async fn halls_page(st: AppState, req: Req) -> Res {
    let list = crate::social::halls::get_halls(&st).await;
    render(&st, &req, "admin/halls", json!({ "session": req.session_value(), "hallsList": rows(list), "tmp": null }))
}

async fn chat_page(st: AppState, req: Req) -> Res {
    if !st.cfg().truthy("websrv.enable_global_chat") {
        return Ok(redirect("/admin"));
    }
    let data = json!({ "session": req.session_value(), "totals": totals(&st).await, "tmp": null });
    render(&st, &req, "admin/chat", data)
}

async fn koepfe_page(st: AppState, req: Req) -> Res {
    render(&st, &req, "admin/koepfe", json!({ "session": req.session_value(), "tmp": null }))
}

fn is_koepfe_image(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif"].iter().any(|e| lower.ends_with(e))
}

async fn koepfe_list(st: AppState, _req: Req) -> Res {
    let dir = st.cfg().paths.koepfe.clone();
    let mut files: Vec<String> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if let Some(n) = e.file_name().to_str() {
                if is_koepfe_image(n) {
                    files.push(n.to_string());
                }
            }
        }
    }
    files.sort();
    let list: Vec<Value> = files.into_iter().map(|f| json!({ "filename": f, "url": format!("/s/koepfe/{f}") })).collect();
    Ok(json_resp(&json!({ "koepfe": list }), 200))
}

async fn koepfe_delete(st: AppState, mut req: Req) -> Res {
    let filename = body_str(req.post().await?, "filename").unwrap_or_default();
    let r: anyhow::Result<()> = async {
        if filename.is_empty() || filename.contains("..") || filename.contains('/') || !crate::media::is_plain_name(&filename) {
            anyhow::bail!("Invalid filename");
        }
        tokio::fs::remove_file(st.cfg().paths.koepfe.join(&filename)).await?;
        Ok(())
    }
    .await;
    Ok(match r {
        Ok(()) => json_resp(&json!({ "success": true }), 200),
        Err(e) => json_resp(&fail_json(&e), 500),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_payload_shapes() {
        let full = json!({ "main": {}, "websrv": {}, "sql": {} });
        assert_eq!(config_payload(&json!({ "config": full.clone() })).unwrap(), full);
        assert_eq!(config_payload(&json!({ "config": full.to_string() })).unwrap(), full);
        let mut with_token = full.clone();
        with_token["csrf_token"] = json!("x");
        assert_eq!(config_payload(&with_token).unwrap(), full);
        assert!(config_payload(&json!({ "main": {} })).is_err());
        assert!(config_payload(&json!({ "config": "[1]" })).is_err());
        assert!(config_payload(&json!({ "config": "{bad" })).is_err());
    }

    #[test]
    fn dates_and_prefixes() {
        assert_eq!(date_stamp("2024-01-02"), Some(1704153600));
        assert_eq!(date_stamp("nope"), None);
        assert_eq!(byte_prefix("äöü", 3), "ä");
        assert!(is_koepfe_image("A.JPG") && !is_koepfe_image("x.svg"));
    }
}
