//! Admin panel (port of src/inc/routes/admin.mjs without the login/logout/password-reset routes,
//! which live in `routes/register.rs`), plus the upload page (src/inc/routes/upload.mjs).
//!
//! * `dashboard` — /admin, site settings, MOTD and page texts, cleanup, sessions, tokens and
//!   invite requests, Köpfe list, halls/chat pages, the config editor API
//! * `bans` — ban lists and (un)ban APIs, per-user IP history
//! * `users` — user management and admin impersonation
//! * `upload_page` — GET /upload
//!
//! Errors meant for the admin are returned as `{ success: false, msg }` like the old
//! `throw new Error(...)` handlers; database errors only report their first line (the old code
//! sent `err.message`, our query errors also carry the SQL text).

mod bans;
mod dashboard;
mod upload_page;
mod users;

use crate::config::Config;
use crate::db::RowExt;
use crate::http::Router;
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    dashboard::register(r);
    bans::register(r);
    users::register(r);
    upload_page::register(r);
}

/// Message for the client from an error (first line only — no SQL text).
pub(crate) fn err_msg(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or("").to_string()
}

/// `{ success: false, msg }` with the error's message, logging internal errors.
pub(crate) fn fail_json(e: &anyhow::Error) -> Value {
    json!({ "success": false, "msg": err_msg(e) })
}

/// `String(req.post[key])` for present, non-null values (numbers as JS prints them).
pub(crate) fn body_str(body: &Value, key: &str) -> Option<String> {
    match body.get(key)? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(crate::util::js_num(n.as_f64().unwrap_or(0.0))),
        _ => None,
    }
}

/// JS truthiness of `req.post[key]`.
pub(crate) fn body_truthy(body: &Value, key: &str) -> bool {
    body.get(key).map(crate::config::truthy).unwrap_or(false)
}

/// `+req.post[key]` as an integer id; `None` for missing, empty or non-integral values
/// (the old code passed `NaN` on to the database and reported its error).
pub(crate) fn body_id(body: &Value, key: &str) -> Option<i64> {
    let f = match body.get(key)? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (f.fract() == 0.0 && f.abs() < i32::MAX as f64).then_some(f as i64)
}

/// Required integer id from the body, as an error message otherwise.
pub(crate) fn require_id(body: &Value, key: &str) -> anyhow::Result<i32> {
    if !body_truthy(body, key) {
        anyhow::bail!("Missing {key}");
    }
    body_id(body, key).map(|n| n as i32).ok_or_else(|| anyhow::anyhow!("Invalid {key}"))
}

/// `INSERT INTO site_settings ... ON CONFLICT DO UPDATE`.
pub(crate) async fn save_setting(st: &AppState, key: &str, value: &str) -> anyhow::Result<()> {
    crate::settings::save(st, key, value).await
}

/// `lib.countf0cks()` — item counters shown on the admin pages.
pub(crate) async fn totals(st: &AppState) -> Value {
    let cfg: std::sync::Arc<Config> = st.cfg();
    let nsfl_id = cfg.nsfl_tag_id() as i32;
    let q = sql!(
        "SELECT
           count(*) FILTER (WHERE i.active AND NOT i.is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id)) AS tagged,
           count(*) FILTER (WHERE i.active AND NOT i.is_deleted AND NOT EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id IN (1, 2, ", nsfl_id, "))) AS untagged,
           count(*) FILTER (WHERE i.active AND NOT i.is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 1)) AS sfw,
           count(*) FILTER (WHERE i.active AND NOT i.is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 2)) AS nsfw,
           count(*) FILTER (WHERE i.active AND NOT i.is_deleted AND EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = ", nsfl_id, ")) AS nsfl,
           count(*) FILTER (WHERE NOT i.active AND i.is_deleted) AS deleted,
           count(*) FILTER (WHERE NOT i.active AND NOT i.is_deleted) AS pending,
           max(i.id) AS last
         FROM items i"
    );
    let row = match q.fetch_optional(&st.db).await {
        Ok(r) => r.unwrap_or_default(),
        Err(e) => {
            tracing::error!("[ADMIN] countf0cks failed: {e}");
            Default::default()
        }
    };
    let n = |k: &str| row.i64(k).unwrap_or(0);
    let (tagged, untagged, deleted, pending) = (n("tagged"), n("untagged"), n("deleted"), n("pending"));
    json!({
        "tagged": tagged,
        "untagged": untagged,
        "total": tagged + untagged,
        "deleted": deleted,
        "pending": pending,
        "untracked": n("last") - (tagged + untagged + deleted + pending),
        "sfw": n("sfw"),
        "nsfw": n("nsfw"),
        "nsfl": if cfg.enable_nsfl() { n("nsfl") } else { 0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_ids() {
        let b = json!({ "a": "12", "b": 7, "c": "x", "d": "", "e": 1.5, "f": " 3 " });
        assert_eq!(body_id(&b, "a"), Some(12));
        assert_eq!(body_id(&b, "b"), Some(7));
        assert_eq!(body_id(&b, "c"), None);
        assert_eq!(body_id(&b, "d"), None);
        assert_eq!(body_id(&b, "e"), None);
        assert_eq!(body_id(&b, "f"), Some(3));
        assert_eq!(body_id(&b, "zz"), None);
        assert_eq!(require_id(&b, "zz").unwrap_err().to_string(), "Missing zz");
        assert_eq!(require_id(&b, "c").unwrap_err().to_string(), "Invalid c");
    }
}
