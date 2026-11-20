//! Audit log (port of `src/inc/audit.mjs`): every moderation and admin action, one row in
//! `audit_log` (who, what, on which target, details as JSON).
//!
//! * [`log`] writes an entry (and tells open audit log pages about it, live event `audit_entry`);
//! * [`ACTIONS`] / [`CATEGORIES`]: the known actions, their labels and groups for the page;
//! * [`Filter`]: the audit log page's filters (`/mod/audit?...`, also used by the export);
//! * entries older than the admin's retention setting (`audit_retention_days`, 0 = forever) are
//!   deleted by the hourly retention job ([`prune_older_than`], see `retention.rs`).

use crate::db::Sql;
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

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
        user_id as i32, ", ", action, ", ", target_type, ", ", target_id.clone(), ", ", details, ") RETURNING id"
    );
    match q.fetch_scalar::<i32, _>(&st.db).await {
        Ok(id) => {
            tracing::info!("[AUDIT] User {user_id} performed {action} on {target_type}:{}", target_id.as_deref().unwrap_or("null"));
            let ev = json!({ "type": "audit_entry", "data": { "id": id, "action": action } });
            crate::routes::notifications::send_with_perm(st, crate::perms::P::ModAudit, &ev);
        }
        Err(e) => tracing::error!("[AUDIT] Failed to write log: {e}"),
    }
}

/// Delete entries older than `days` days; the number deleted.
pub async fn prune_older_than(st: &AppState, days: i64) -> anyhow::Result<u64> {
    if days <= 0 {
        return Ok(0);
    }
    sql!("DELETE FROM audit_log WHERE created_at < now() - make_interval(days => ", days.min(36_500) as i32, ")").execute(&st.db).await
}

// ── Catalog ──────────────────────────────────────────────────────────────────

/// Action groups: key, label (in page order). Actions not in [`ACTIONS`] are "other".
pub const CATEGORIES: &[(&str, &str)] = &[
    ("items", "Items"),
    ("tags", "Tags"),
    ("comments", "Comments"),
    ("lists", "Lists"),
    ("users", "Users"),
    ("bans", "Bans & warnings"),
    ("reports", "Reports"),
    ("permissions", "Permissions"),
    ("site", "Site & settings"),
    ("audit", "Audit log"),
    ("other", "Other"),
];

/// Every action the code logs: key, category, label.
pub const ACTIONS: &[(&str, &str, &str)] = &[
    ("approve_item", "items", "Approved upload"),
    ("deny_item", "items", "Denied upload"),
    ("deny_item_multi", "items", "Denied upload (bulk)"),
    ("deny_album_item", "items", "Denied album file"),
    ("delete_album_item", "items", "Deleted album file"),
    ("delete_item", "items", "Deleted item"),
    ("restore_item", "items", "Restored item"),
    ("purge_item", "items", "Purged item"),
    ("purge_item_multi", "items", "Purged item (bulk)"),
    ("withdraw_item", "items", "Withdrew upload"),
    ("pin_item", "items", "Pinned item"),
    ("unpin_item", "items", "Unpinned item"),
    ("update_rating", "items", "Changed rating"),
    ("cycle_rating", "items", "Cycled rating"),
    ("toggle_oc", "items", "Changed OC mark"),
    ("set_item_expiry", "items", "Set expiry"),
    ("rethumb_item", "items", "New thumbnail"),
    ("lock_thread", "items", "Locked comments"),
    ("unlock_thread", "items", "Unlocked comments"),
    ("toggle_tag", "tags", "Changed tag"),
    ("add_tag", "tags", "Added tag"),
    ("delete_tag", "tags", "Removed tag"),
    ("nsfp_add", "tags", "Added NSFP tag"),
    ("nsfp_remove", "tags", "Removed NSFP tag"),
    ("delete_comment", "comments", "Deleted comment"),
    ("edit_comment", "comments", "Edited comment"),
    ("pin_comment", "comments", "Pinned comment"),
    ("delete_attachment", "comments", "Deleted attachment"),
    ("delete_poll", "comments", "Deleted poll"),
    ("add_to_hall", "lists", "Added to list"),
    ("remove_from_hall", "lists", "Removed from list"),
    ("create_hall", "lists", "Created list"),
    ("rename_hall", "lists", "Renamed list"),
    ("update_hall", "lists", "Changed list"),
    ("update_hall_metadata", "lists", "Changed list details"),
    ("delete_hall", "lists", "Deleted list"),
    ("admin_create_user", "users", "Created user"),
    ("admin_delete_user", "users", "Deleted user"),
    ("self_delete_user", "users", "User deleted own account"),
    ("admin_rename_user", "users", "Renamed user"),
    ("admin_set_display_name", "users", "Set display name"),
    ("admin_set_role", "users", "Changed role"),
    ("admin_reset_password", "users", "Reset password"),
    ("admin_reset_login_attempts", "users", "Reset login attempts"),
    ("admin_reassign_uploads", "users", "Moved uploads"),
    ("admin_bulk_delete_items", "users", "Deleted all items of a user"),
    ("admin_bulk_delete_comments", "users", "Deleted all comments of a user"),
    ("admin_bulk_delete_halls", "users", "Deleted all lists of a user"),
    ("manual_verify_user", "users", "Verified user"),
    ("lock_user_layout", "users", "Locked layout"),
    ("unlock_user_layout", "users", "Unlocked layout"),
    ("ban_user", "bans", "Banned user"),
    ("unban_user", "bans", "Unbanned user"),
    ("ban_ip", "bans", "Banned IP"),
    ("unban_ip", "bans", "Unbanned IP"),
    ("ban_fingerprint", "bans", "Banned fingerprint"),
    ("unban_fingerprint", "bans", "Unbanned fingerprint"),
    ("ban_hardware", "bans", "Banned device"),
    ("unban_hardware", "bans", "Unbanned device"),
    ("issue_warning", "bans", "Warned user"),
    ("resolve_report", "reports", "Closed report"),
    ("admin_set_permissions", "permissions", "Set user permissions"),
    ("admin_set_groups", "permissions", "Set user groups"),
    ("admin_set_role_permissions", "permissions", "Changed role defaults"),
    ("admin_set_group_permissions", "permissions", "Changed group"),
    ("admin_delete_group_permissions", "permissions", "Removed group"),
    ("purge_trash", "site", "Emptied trash"),
    ("run_cleanup_manual", "site", "Ran cleanup"),
    ("update_motd", "site", "Changed MOTD"),
    ("update_about_text", "site", "Changed about page"),
    ("update_rules_text", "site", "Changed rules"),
    ("update_terms_text", "site", "Changed terms"),
    ("update_config_file", "site", "Changed config file"),
    ("maintenance_on", "site", "Maintenance on"),
    ("maintenance_off", "site", "Maintenance off"),
    ("update_maintenance", "site", "Changed maintenance page"),
    ("add_wordfilter", "site", "Added word filter"),
    ("delete_wordfilter", "site", "Removed word filter"),
    ("upload_font", "site", "Uploaded font"),
    ("delete_font", "site", "Deleted font"),
    ("update_favicon", "site", "Changed favicon"),
    ("delete_favicon", "site", "Removed favicon"),
    ("update_brand_text", "site", "Changed brand text"),
    ("update_brand_image", "site", "Changed brand image"),
    ("delete_brand_image", "site", "Removed brand image"),
    ("update_brand_glitch", "site", "Changed brand glitch"),
    ("update_fy_brand", "site", "Changed f0ckYou brand"),
    ("update_fy_brand_image", "site", "Changed f0ckYou brand image"),
    ("delete_fy_brand_image", "site", "Removed f0ckYou brand image"),
    ("audit_set_retention", "audit", "Changed audit retention"),
    ("audit_prune", "audit", "Deleted old audit entries"),
];

/// The category key of an action ("other" when unknown).
pub fn category_of(action: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.0 == action).map(|a| a.1).unwrap_or("other")
}

pub fn category_label(key: &str) -> &'static str {
    CATEGORIES.iter().find(|c| c.0 == key).map(|c| c.1).unwrap_or("Other")
}

/// A readable label: the catalog's, else the key with spaces ("foo_bar" → "Foo bar").
pub fn action_label(action: &str) -> String {
    if let Some(a) = ACTIONS.iter().find(|a| a.0 == action) {
        return a.2.to_string();
    }
    let s = action.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ── Filters ──────────────────────────────────────────────────────────────────

/// The audit log page's filters (query string), all optional.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filter {
    /// Category key ([`CATEGORIES`])
    pub category: String,
    pub action: String,
    /// Who did it (user id)
    pub actor: Option<i64>,
    pub target_type: String,
    pub target_id: String,
    /// Text in the target id or the details
    pub q: String,
    /// `YYYY-MM-DD`, inclusive
    pub from: String,
    pub to: String,
}

fn date(s: Option<&str>) -> String {
    let s = s.map(str::trim).unwrap_or("");
    if chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok() { s.to_string() } else { String::new() }
}

fn short(s: Option<&str>, max: usize) -> String {
    s.map(str::trim).unwrap_or("").chars().take(max).collect()
}

impl Filter {
    /// From the query: `category`, `action`, `actor`, `target_type`, `target_id`, `q`, `from`, `to`.
    /// Unknown categories and malformed dates are dropped.
    pub fn from_query(get: impl Fn(&str) -> Option<String>) -> Filter {
        let action = short(get("action").as_deref(), 50);
        let category = short(get("category").as_deref(), 32);
        // An action from another category would match nothing: the action wins
        let known = CATEGORIES.iter().any(|c| c.0 == category) && (action.is_empty() || category_of(&action) == category);
        Filter {
            category: if known { category } else { String::new() },
            action,
            actor: get("actor").and_then(|a| a.trim().parse::<i64>().ok()).filter(|a| *a > 0 && *a <= i32::MAX as i64),
            target_type: short(get("target_type").as_deref(), 50),
            target_id: short(get("target_id").as_deref(), 50),
            q: short(get("q").as_deref(), 200),
            from: date(get("from").as_deref()),
            to: date(get("to").as_deref()),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Filter::default()
    }

    /// ` AND ...` conditions on `audit_log al`.
    pub fn sql(&self) -> Sql {
        let mut f = Sql::empty();
        if !self.category.is_empty() {
            if self.category == "other" {
                let known: Vec<String> = ACTIONS.iter().map(|a| a.0.to_string()).collect();
                f.push(" AND NOT (al.action = ANY(").bind(known).push("))");
            } else {
                let of: Vec<String> = ACTIONS.iter().filter(|a| a.1 == self.category).map(|a| a.0.to_string()).collect();
                f.push(" AND al.action = ANY(").bind(of).push(")");
            }
        }
        if !self.action.is_empty() {
            f.push(" AND al.action = ").bind(self.action.clone());
        }
        if let Some(a) = self.actor {
            f.push(" AND al.user_id = ").bind(a as i32);
        }
        if !self.target_type.is_empty() {
            f.push(" AND al.target_type = ").bind(self.target_type.clone());
        }
        if !self.target_id.is_empty() {
            f.push(" AND al.target_id = ").bind(self.target_id.clone());
        }
        if !self.q.is_empty() {
            let like = format!("%{}%", self.q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            f.push(" AND (al.target_id ILIKE ").bind(like.clone()).push(" OR al.details::text ILIKE ").bind(like).push(")");
        }
        if !self.from.is_empty() {
            f.push(" AND al.created_at >= ").bind(self.from.clone()).push("::date");
        }
        if !self.to.is_empty() {
            f.push(" AND al.created_at < ").bind(self.to.clone()).push("::date + 1");
        }
        f
    }

    /// The set filters as `(name, value)` pairs, in query order.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut v = vec![];
        let mut add = |k: &'static str, s: &str| {
            if !s.is_empty() {
                v.push((k, s.to_string()));
            }
        };
        add("category", &self.category);
        add("action", &self.action);
        add("actor", &self.actor.map(|a| a.to_string()).unwrap_or_default());
        add("target_type", &self.target_type);
        add("target_id", &self.target_id);
        add("q", &self.q);
        add("from", &self.from);
        add("to", &self.to);
        v
    }

    /// Query string of the set filters (no leading `?`), with `extra` pairs added.
    pub fn query_string(&self, extra: &[(&str, String)]) -> String {
        let mut s = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in self.pairs() {
            s.append_pair(k, &v);
        }
        for (k, v) in extra {
            s.append_pair(k, v);
        }
        s.finish()
    }

    /// `/mod/audit` with these filters plus `extra`.
    pub fn href(&self, extra: &[(&str, String)]) -> String {
        let q = self.query_string(extra);
        if q.is_empty() { "/mod/audit".to_string() } else { format!("/mod/audit?{q}") }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_is_consistent() {
        for (key, cat, label) in ACTIONS {
            assert!(CATEGORIES.iter().any(|c| c.0 == *cat), "{key}: unknown category {cat}");
            assert!(!label.is_empty() && key.len() <= 50, "{key}");
            assert_eq!(ACTIONS.iter().filter(|a| a.0 == *key).count(), 1, "{key} listed twice");
        }
        assert_eq!(category_of("approve_item"), "items");
        assert_eq!(category_of("something_new"), "other");
        assert_eq!(action_label("something_new"), "Something new");
    }

    #[test]
    fn filters() {
        let q = |pairs: &'static [(&'static str, &'static str)]| {
            Filter::from_query(move |k| pairs.iter().find(|p| p.0 == k).map(|p| p.1.to_string()))
        };
        let f = q(&[("category", "nope"), ("actor", "-3"), ("from", "2026-13-01"), ("to", "2026-02-03"), ("q", "  x  ")]);
        assert_eq!(f, Filter { to: "2026-02-03".into(), q: "x".into(), ..Default::default() });
        assert_eq!(f.href(&[("page", "2".into())]), "/mod/audit?q=x&to=2026-02-03&page=2");
        assert!(q(&[]).is_empty());
        assert_eq!(q(&[("category", "users"), ("action", "approve_item")]).category, "");
        assert_eq!(q(&[("category", "items"), ("action", "approve_item")]).category, "items");
        assert_eq!(q(&[]).href(&[]), "/mod/audit");
        let f = q(&[("category", "other"), ("actor", "7"), ("target_type", "item"), ("target_id", "12")]);
        let s = f.sql().sql();
        assert!(s.contains("NOT (al.action = ANY(") && s.contains("al.user_id = ") && s.contains("al.target_id = "), "{s}");
    }
}
