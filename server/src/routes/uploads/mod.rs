//! Upload bypass handlers from index.mjs: `/api/v2/upload` (upload_handler.mjs),
//! `/api/v2/meta/extract-file` + `/api/v2/meta/strip-gps` (meta_*_handler.mjs) and
//! `/api/v2/items/:id/thumbnail` (rethumb_handler.mjs). They read the raw request body
//! themselves, so they are registered as `pre` routes like the old bypass middleware.
//!
//! The old handlers looked the session up again themselves (the middleware ran in parallel);
//! here the session middleware has already run, so `req.session` is used, which also applies
//! bans, anonymous-access rules and forced password changes.

mod meta;
mod rethumb;
mod upload;

use crate::db::RowExt;
use crate::http::{Req, Res, Router, json_raw};
use crate::session::Session;
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/upload$", None, upload::handle_upload);
    r.pre(Method::POST, r"^\/api\/v2\/meta\/extract-file$", None, meta::handle_meta_extract);
    r.pre(Method::POST, r"^\/api\/v2\/meta\/strip-gps$", None, meta::handle_meta_strip);
    r.pre(Method::POST, r"^\/api\/v2\/items\/(?<id>[^/]+)\/thumbnail$", None, rethumb::handle_rethumb_upload);
}

/// `sendJson(res, data, code)`: `application/json` without charset.
pub(crate) fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

pub(crate) fn fail(code: u16, msg: &str) -> Res {
    send(json!({ "success": false, "msg": msg }), code)
}

/// The handlers accept the CSRF token only from the `X-CSRF-Token` header (checked before the body
/// is read, so a forged upload is rejected without streaming it to disk).
pub(crate) fn csrf_header_ok(req: &Req, session: &Session) -> bool {
    match (session.csrf_token.as_deref().filter(|t| !t.is_empty()), req.header("x-csrf-token")) {
        (Some(expected), Some(got)) => crate::util::ct_eq(got, expected),
        _ => false,
    }
}

/// `Content-Length` when present and numeric.
pub(crate) fn declared_len(req: &Req) -> Option<u64> {
    req.header("content-length").and_then(|v| v.trim().parse().ok())
}

/// Session for an `X-Api-Key` upload (`user_api_keys`); `None` when the key is unknown.
pub(crate) async fn api_key_session(st: &AppState, key: &str) -> anyhow::Result<Option<Session>> {
    let mut q = sql!("SELECT ");
    q.push(crate::session::USER_COLUMNS)
        .push(r#" FROM user_api_keys k JOIN "user" ON "user".id = k.user_id LEFT JOIN "user_options" ON "user_options".user_id = "user".id WHERE k.api_key = "#)
        .bind(key)
        .push(" LIMIT 1");
    let Some(row) = q.fetch_optional(&st.db).await? else { return Ok(None) };
    let mut s = Session::from_row(row)?;
    s.api_key_auth = true;
    Ok(Some(s))
}

/// Tag id by normalized name, creating the tag when missing.
pub(crate) async fn ensure_tag(st: &AppState, name: &str) -> anyhow::Result<Option<i64>> {
    let find = || sql!("select id from tags where normalized = slugify(", name, ") limit 1");
    if let Some(r) = find().fetch_optional(&st.db).await? {
        return Ok(r.i64("id"));
    }
    sql!("insert into tags (tag) values (", name, ")").execute(&st.db).await?;
    Ok(find().fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

/// `INSERT INTO tags_assign ... ON CONFLICT DO NOTHING`
pub(crate) async fn assign_tag(st: &AppState, item_id: i64, tag_id: i64, user_id: i64) -> anyhow::Result<()> {
    sql!("insert into tags_assign (item_id, tag_id, user_id) values (", item_id as i32, ", ", tag_id as i32, ", ", user_id as i32, ") on conflict do nothing")
        .execute(&st.db)
        .await?;
    Ok(())
}

/// `logAnonActivity` (anon_auth.mjs) for anonymous sessions.
pub(crate) async fn log_anon_activity(st: &AppState, req: &Req, action: &str, target_id: Option<i64>, details: Value) {
    let Some(s) = req.session() else { return };
    if s.id == 0 {
        return;
    }
    let cfg = st.cfg();
    let raw_ip = req.real_ip();
    let ip = crate::security::storable_ip(&cfg, &raw_ip);
    let fp = s.fingerprint.clone().or_else(|| s.anon_fingerprint.clone());
    let r = sql!(
        "INSERT INTO anon_activity_log (user_id, fingerprint, hw_fingerprint, ip, action, target_id, details) VALUES (",
        s.id as i32, ", ", fp, ", ", Option::<String>::None, ", ", ip, ", ", action, ", ", target_id.map(|t| t as i32), ", ", details, ")"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        tracing::error!("[ANON_ACTIVITY_LOG] Failed to log activity: {e}");
        return;
    }
    crate::security::log_user_ip(st, s.id, &raw_ip).await;
}
