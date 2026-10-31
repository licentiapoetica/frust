//! Audit log (port of `src/inc/audit.mjs`).

use crate::sql;
use crate::state::AppState;
use serde_json::Value;

/// `audit.log(userId, action, targetType, targetId, details)`. Failures are only logged, like the
/// old helper: an audit write must never break the action it records.
///
/// `target_id` is stored as text (`audit_log.target_id` is varchar): pass `id.to_string()`, a slug,
/// or `None`. `details` defaults to `{}` in the old code; pass `json!({})` for that.
pub async fn log(st: &AppState, user_id: i64, action: &str, target_type: &str, target_id: impl Into<Option<String>>, details: Value) {
    let target_id: Option<String> = target_id.into();
    let details = if details.is_null() { Value::Object(Default::default()) } else { details };
    let q = sql!(
        "INSERT INTO audit_log (user_id, action, target_type, target_id, details) VALUES (",
        user_id as i32, ", ", action, ", ", target_type, ", ", target_id.clone(), ", ", details, ")"
    );
    match q.execute(&st.db).await {
        Ok(_) => tracing::info!("[AUDIT] User {user_id} performed {action} on {target_type}:{}", target_id.as_deref().unwrap_or("null")),
        Err(e) => tracing::error!("[AUDIT] Failed to write log: {e}"),
    }
}
