//! User management and admin impersonation (src/inc/routes/admin.mjs).

use super::{body_id, body_str, body_truthy, err_msg, require_id, totals};
use crate::db::{Row, RowExt, Sql};
use crate::http::{CookieExpiry, Guard, Req, Res, Router, cookie_options, html, json_raw, json_resp};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/users\/?$", Guard::Auth, users_page);
    r.post_g(r"^\/api\/v2\/admin\/users\/activate\/?$", Guard::Auth, activate);
    r.post_g(r"^\/api\/v2\/admin\/users\/set-password\/?$", Guard::Auth, set_password);
    r.post_g(r"^\/api\/v2\/admin\/users\/set-role\/?$", Guard::Auth, set_role);
    r.post_g(r"^\/api\/v2\/admin\/users\/set-groups\/?$", Guard::Auth, set_groups);
    r.post_g(r"^\/api\/v2\/admin\/users\/lock-layout\/?$", Guard::Auth, lock_layout);
    r.post_g(r"^\/api\/v2\/admin\/users\/delete\/?$", Guard::Auth, delete_user);
    r.post_g(r"^\/api\/v2\/admin\/users\/reassign-uploads\/?$", Guard::Auth, reassign_uploads);
    r.post_g(r"^\/api\/v2\/admin\/users\/bulk-delete-items\/?$", Guard::Auth, bulk_delete_items);
    r.post_g(r"^\/api\/v2\/admin\/users\/bulk-delete-comments\/?$", Guard::Auth, bulk_delete_comments);
    r.post_g(r"^\/api\/v2\/admin\/users\/reset-login-attempts\/?$", Guard::Auth, reset_login_attempts);
    r.post_g(r"^\/api\/v2\/admin\/users\/bulk-delete-halls\/?$", Guard::Auth, bulk_delete_halls);
    r.post_g(r"^\/api\/v2\/admin\/users\/rename\/?$", Guard::Auth, rename);
    r.post_g(r"^\/api\/v2\/admin\/users\/create\/?$", Guard::Auth, create);
    r.post_g(r"^\/api\/v2\/admin\/users\/set-display-name\/?$", Guard::Auth, set_display_name);
    r.post_g(r"^\/api\/v2\/admin\/subscribe-user-to-uploads\/?$", Guard::Auth, subscribe_to_uploads);
    r.get_g(r"^\/api\/v2\/admin\/users\/search\/?$", Guard::AdminAuth, search);
    r.post_g(r"^\/api\/v2\/admin\/impersonate\/?$", Guard::AdminAuth, impersonate);
    r.post(r"^\/api\/v2\/admin\/stop-impersonate\/?$", stop_impersonate);
}

fn sid(req: &Req) -> i64 {
    req.session().map(|s| s.id).unwrap_or(0)
}

/// `res.writeHead(code, json).end(...)`: the value, or `{ success: false, msg }` with `err_code`.
fn answer(r: anyhow::Result<Value>, err_code: u16) -> Res {
    Ok(match r {
        Ok(v) => json_raw(&v, 200),
        Err(e) => {
            tracing::debug!("[ADMIN] {e:#}");
            json_raw(&json!({ "success": false, "msg": err_msg(&e) }), err_code)
        }
    })
}

/// Usernames: A-Z, 0-9, `_`, `-`, `.`
fn valid_username_chars(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

/// A name as an exact (case-insensitive) `ILIKE` pattern. The old code passed names straight to
/// `ILIKE`, so `_` and `%` (allowed in usernames) matched other users' uploads too.
fn like(name: &str) -> String {
    crate::util::escape_like(name)
}

fn hall_custom_dir(st: &AppState) -> std::path::PathBuf {
    let s = st.cfg().paths.s.clone();
    s.parent().map(|p| p.join("hall_custom")).unwrap_or_else(|| s.join("../hall_custom"))
}

// ── Users list ──────────────────────────────────────────────────────────────

async fn users_page(st: AppState, req: Req) -> Res {
    let raw_q = req.query("q").unwrap_or("").to_string();
    // Exact match mode: "quoted" search
    let exact = raw_q.starts_with('"') && raw_q.ends_with('"') && raw_q.chars().count() > 2;
    let q = if exact { raw_q[1..raw_q.len() - 1].to_string() } else { raw_q.clone() };
    let page = req.query("page").and_then(crate::social::helpers::js_parse_int).filter(|p| *p >= 1).unwrap_or(1).min(1_000_000);
    let limit: i64 = 50;
    let offset = (page - 1) * limit;
    let raw_status = req.query("status").filter(|s| !s.is_empty()).or_else(|| req.query("filter")).unwrap_or("").trim().to_lowercase();
    let raw_role = req.query("role").unwrap_or("").trim().to_lowercase();
    let flag = |k: &str| matches!(req.query(k), Some("1") | Some("true"));
    let only_legacy = flag("legacy") || flag("legacy_only") || flag("only_legacy") || raw_status == "legacy";
    let status = if only_legacy { String::new() } else { raw_status };
    let role = if only_legacy { String::new() } else { raw_role };
    let pattern = format!("%{}%", crate::util::escape_like(&q));

    let (users, total): (Vec<Row>, i64) = if only_legacy {
        let mut cond = Sql::empty();
        if !q.is_empty() {
            if exact {
                cond.push(" AND lower(i.username) = lower(").bind(q.clone()).push(")");
            } else {
                cond.push(" AND (i.username ILIKE ").bind(pattern.clone()).push(")");
            }
        }
        let users = sql!(
            r#"WITH ghost_users AS (
                SELECT NULL::int as id, i.username as login, i.username as "user", 'Legacy Account' as email,
                       to_timestamp(MIN(i.stamp)) as created_at, false as banned, false as is_moderator, false as admin, ARRAY[]::text[] as groups, true as activated,
                       NULL::text as avatar_file, NULL::varchar as display_name, 0 as force_comment_display_mode, 0 as comment_display_mode, 'Legacy' as reg_method
                FROM items i
                WHERE i.username IS NOT NULL AND i.username != ''
                  AND NOT EXISTS (SELECT 1 FROM "user" u WHERE u.login = i.username OR u.user = i.username)"#,
            @cond,
            r#" GROUP BY i.username
            ),
            paginated_users AS (SELECT * FROM ghost_users ORDER BY created_at DESC LIMIT "#, limit, " OFFSET ", offset,
            r#")
            SELECT pu.*,
                   EXTRACT(DAY FROM (now() - pu.created_at)) as age_days,
                   COALESCE(ic.upload_count, 0) as upload_count,
                   0::bigint as comment_count,
                   0::bigint as failed_attempts
            FROM paginated_users pu
            LEFT JOIN LATERAL (
                SELECT COUNT(*) as upload_count FROM items WHERE (username = pu.login OR username = pu.user) AND is_deleted = false
            ) ic ON true"#
        )
        .fetch_all(&st.db)
        .await?;
        let total: i64 = sql!(
            r#"SELECT COUNT(DISTINCT i.username) FROM items i
               WHERE i.username IS NOT NULL AND i.username != ''
                 AND NOT EXISTS (SELECT 1 FROM "user" u WHERE u.login = i.username OR u.user = i.username)"#,
            @cond
        )
        .fetch_scalar(&st.db)
        .await?
        .unwrap_or(0);
        (users, total)
    } else {
        let mut cond = Sql::empty();
        if !q.is_empty() {
            if exact {
                cond.push(" AND (lower(u.login) = lower(").bind(q.clone()).push(") OR lower(u.user) = lower(").bind(q.clone()).push(") OR lower(u.email) = lower(").bind(q.clone()).push("))");
            } else {
                cond.push(" AND (u.login ILIKE ").bind(pattern.clone()).push(" OR u.user ILIKE ").bind(pattern.clone()).push(" OR u.email ILIKE ").bind(pattern.clone()).push(")");
            }
        }
        match status.as_str() {
            "banned" => {
                cond.push(" AND u.banned = true");
            }
            "active" => {
                cond.push(" AND (u.activated = true AND u.banned = false)");
            }
            "pending" => {
                cond.push(" AND (u.activated = false AND u.banned = false)");
            }
            _ => {}
        }
        if role == "staff" || status == "staff" {
            cond.push(" AND (u.admin = true OR u.is_moderator = true)");
        } else if role == "admin" {
            cond.push(" AND u.admin = true");
        } else if role == "mod" {
            cond.push(" AND (u.is_moderator = true AND u.admin = false)");
        } else if role == "user" {
            cond.push(" AND (u.admin = false AND u.is_moderator = false)");
        }
        let users = sql!(
            r#"WITH filtered_users AS (
                SELECT u.id, u.login, u.user, u.email, u.created_at, u.banned, u.is_moderator, u.admin, u.groups, u.activated,
                       uo.avatar_file, uo.display_name, uo.force_comment_display_mode, uo.comment_display_mode,
                       (SELECT token FROM invite_tokens WHERE used_by = u.id ORDER BY created_at DESC LIMIT 1) as reg_method
                FROM "user" u
                LEFT JOIN user_options uo ON uo.user_id = u.id
                WHERE true"#,
            @cond,
            r#"),
            paginated_users AS (SELECT * FROM filtered_users ORDER BY created_at DESC LIMIT "#, limit, " OFFSET ", offset,
            r#")
            SELECT pu.*,
                   EXTRACT(DAY FROM (now() - pu.created_at)) as age_days,
                   COALESCE(ic.upload_count, 0) as upload_count,
                   COALESCE(cc.comment_count, 0) as comment_count,
                   COALESCE(la.failed_attempts, 0) as failed_attempts
            FROM paginated_users pu
            LEFT JOIN LATERAL (
                SELECT COUNT(*) as upload_count FROM items WHERE (username = pu.login OR username = pu.user) AND is_deleted = false
            ) ic ON true
            LEFT JOIN LATERAL (
                SELECT COUNT(*) as comment_count FROM comments WHERE user_id = pu.id AND is_deleted = false
            ) cc ON pu.id IS NOT NULL
            LEFT JOIN LATERAL (
                SELECT COUNT(*) as failed_attempts FROM login_attempts
                WHERE username = pu.login AND success = false AND type = 'login' AND attempted_at > now() - interval '10 hours'
            ) la ON true"#
        )
        .fetch_all(&st.db)
        .await?;
        let total: i64 = sql!(r#"SELECT COUNT(*) FROM "user" u WHERE true"#, @cond).fetch_scalar(&st.db).await?.unwrap_or(0);
        (users, total)
    };

    let (total_label, empty_msg) = if only_legacy {
        ("legacy accounts", "No legacy users matched your search.")
    } else if status == "banned" {
        ("banned members", "No banned users found.")
    } else if status == "pending" {
        ("pending members", "No pending users found.")
    } else if status == "active" {
        ("active members", "No active users found.")
    } else if role == "staff" || status == "staff" {
        ("staff members", "No staff members found.")
    } else if role == "admin" {
        ("admin members", "No admin users found.")
    } else if role == "mod" {
        ("moderator members", "No moderator users found.")
    } else if role == "user" {
        ("regular users", "No regular users found.")
    } else {
        ("registered members", "No users matched your search.")
    };

    let has_more = users.len() as i64 == limit;
    // Counters as numbers so the template can compare them (postgres.js returned bigint strings,
    // which JavaScript coerced in `u.failed_attempts > 0`).
    let users: Vec<Value> = users
        .into_iter()
        .map(|mut u| {
            for k in ["upload_count", "comment_count", "failed_attempts", "age_days"] {
                if let Some(n) = u.f64(k) {
                    u.insert(k.into(), json!(n));
                }
            }
            Value::Object(u)
        })
        .collect();
    let data = json!({
        "session": req.session_value(),
        "users": users,
        "q": q,
        "page": page,
        "total": total,
        "hasMore": has_more,
        "onlyLegacy": only_legacy,
        "status": status,
        "role": role,
        "totalLabel": total_label,
        "emptyMsg": empty_msg,
        "totals": totals(&st).await,
        "log_user_ips": crate::settings::log_user_ips(&st.cfg()),
        "tmp": null,
    });

    if req.is_xhr() {
        req.set_header("x-total-count", &total.to_string());
        req.set_header("x-total-label", total_label);
        req.set_header("x-empty-msg", empty_msg);
        req.set_header("x-has-more", if has_more { "true" } else { "false" });
        return Ok(html(crate::tpl::render(&st, Some(&req), "admin/users_list", data)));
    }
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/users", data)))
}

// ── Account actions ─────────────────────────────────────────────────────────

async fn activate(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let row = sql!(r#"UPDATE "user" SET activated = true, activation_token = NULL WHERE id = "#, uid, " AND login != 'deleted_user' RETURNING login")
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found"))?;
        let login = row.string("login");
        crate::audit::log(&st, sid(&req), "manual_verify_user", "user", uid.to_string(), json!({ "target_login": login })).await;
        Ok(json!({ "success": true, "msg": format!("User {login} manually verified.") }))
    }
    .await;
    answer(r, 200)
}

async fn set_password(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let password = body_str(&body, "password").unwrap_or_default();
        if !body_truthy(&body, "user_id") || password.is_empty() {
            anyhow::bail!("Missing user_id or password");
        }
        let uid = require_id(&body, "user_id")?;
        if password.encode_utf16().count() < 20 {
            anyhow::bail!("Password must be at least 20 characters");
        }
        let hash = crate::util::hash_password(password).await;
        if hash.is_empty() {
            anyhow::bail!("Password hashing failed");
        }
        sql!(r#"UPDATE "user" SET password = "#, hash, ", force_password_change = true WHERE id = ", uid, " AND login != 'deleted_user' RETURNING id")
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found or account is protected."))?;
        // Invalidate existing sessions
        sql!(r#"DELETE FROM "user_sessions" WHERE user_id = "#, uid).execute(&st.db).await?;
        crate::audit::log(&st, sid(&req), "admin_reset_password", "user", uid.to_string(), json!({})).await;
        Ok(json!({ "success": true, "msg": "Password updated and user forced to change on next login." }))
    }
    .await;
    answer(r, 500)
}

async fn target_user(st: &AppState, uid: i32) -> anyhow::Result<Row> {
    sql!(r#"SELECT id, login, "user" FROM "user" WHERE id = "#, uid, " LIMIT 1").fetch_optional(&st.db).await?.ok_or_else(|| anyhow::anyhow!("User not found."))
}

async fn set_role(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let role = body_str(&body, "role").unwrap_or_default();
        if !["user", "mod", "admin"].contains(&role.as_str()) {
            anyhow::bail!("Invalid role. Must be user, mod, or admin.");
        }
        let target = target_user(&st, uid).await?;
        if target.str("login") == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is protected.");
        }
        // Prevent self-demotion
        if uid as i64 == sid(&req) && role != "admin" {
            anyhow::bail!("You cannot change your own role away from admin.");
        }
        sql!(r#"UPDATE "user" SET admin = "#, role == "admin", ", is_moderator = ", role == "mod", " WHERE id = ", uid).execute(&st.db).await?;
        // Sessions are dropped so the new permissions apply on next login
        sql!("DELETE FROM user_sessions WHERE user_id = ", uid).execute(&st.db).await?;
        let login = target.string("login");
        crate::audit::log(&st, sid(&req), "admin_set_role", "user", uid.to_string(), json!({ "target_login": login, "role": role })).await;
        Ok(json!({ "success": true, "role": role, "msg": format!("Role for \"{login}\" set to {role}.") }))
    }
    .await;
    answer(r, 200)
}

/// `groups` as an array or a comma-separated string → trimmed, lower-cased, non-empty names.
fn parse_groups(v: Option<&Value>) -> Vec<String> {
    let norm = |s: &str| s.trim().to_lowercase();
    match v {
        Some(Value::Array(a)) => a.iter().map(|x| norm(&crate::social::helpers::js_str(Some(x)).unwrap_or_else(|| "null".into()))).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(',').map(norm).filter(|s| !s.is_empty()).collect(),
        _ => vec![],
    }
}

async fn set_groups(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let target = target_user(&st, uid).await?;
        let groups = parse_groups(body.get("groups"));
        sql!(r#"UPDATE "user" SET groups = "#, groups.clone(), " WHERE id = ", uid).execute(&st.db).await?;
        // Sessions are dropped so the new groups load immediately
        sql!("DELETE FROM user_sessions WHERE user_id = ", uid).execute(&st.db).await?;
        let login = target.string("login");
        crate::audit::log(&st, sid(&req), "admin_set_groups", "user", uid.to_string(), json!({ "target_login": login, "groups": groups })).await;
        let shown = if groups.is_empty() { "none".to_string() } else { groups.join(", ") };
        Ok(json!({ "success": true, "groups": groups, "msg": format!("Groups for \"{login}\" updated to: {shown}") }))
    }
    .await;
    answer(r, 200)
}

async fn lock_layout(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let locked = matches!(body.get("lock"), Some(Value::Bool(true))) || body.get("lock") == Some(&json!("true")) || body.get("lock") == Some(&json!(1));
        let mode = body_str(&body, "mode").as_deref().and_then(crate::social::helpers::js_parse_int).map(|m| m as i32);
        let force = if locked { 1 } else { 0 };
        let mut q = sql!("UPDATE user_options SET force_comment_display_mode = ", force as i32);
        if let Some(m) = mode {
            q.push(", comment_display_mode = ").bind(m);
        }
        q.push(" WHERE user_id = ").bind(uid).push(" RETURNING user_id");
        q.fetch_optional(&st.db).await?.ok_or_else(|| anyhow::anyhow!("User options not found"))?;
        crate::audit::log(&st, sid(&req), if locked { "lock_user_layout" } else { "unlock_user_layout" }, "user", uid.to_string(), json!({ "mode": mode })).await;
        Ok(json!({
            "success": true,
            "msg": format!("User layout {}.", if locked { "locked" } else { "unlocked" }),
            "force_comment_display_mode": force,
            "comment_display_mode": mode,
        }))
    }
    .await;
    answer(r, 200)
}

/// The system ghost that deleted users' content is reassigned to. It can never log in: password
/// `!` matches no hash, it is not activated and it is banned (the unban route refuses to touch it).
async fn ensure_ghost_user(st: &AppState) -> anyhow::Result<i32> {
    let find = || sql!(r#"SELECT id FROM "user" WHERE login = 'deleted_user' LIMIT 1"#);
    if let Some(id) = find().fetch_optional(&st.db).await?.and_then(|r| r.i64("id")) {
        return Ok(id as i32);
    }
    sql!(
        r#"INSERT INTO "user" (login, "user", password, admin, is_moderator, activated, banned, ban_reason, created_at)
           VALUES ('deleted_user', 'deleted_user', '!', false, false, false, true, 'System account', now())
           ON CONFLICT (login) DO NOTHING"#
    )
    .execute(&st.db)
    .await?;
    let id = find()
        .fetch_optional(&st.db)
        .await?
        .and_then(|r| r.i64("id"))
        .ok_or_else(|| anyhow::anyhow!("Could not create the \"deleted_user\" ghost account."))? as i32;
    sql!(
        "INSERT INTO user_options (user_id, mode, theme, fullscreen, avatar, avatar_file, display_name) VALUES (", id,
        ", 0, 'amoled', 0, null, 'default.png', 'deleted user') ON CONFLICT (user_id) DO NOTHING"
    )
    .execute(&st.db)
    .await?;
    Ok(id)
}

async fn delete_user(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let target = target_user(&st, uid).await.map_err(|_| anyhow::anyhow!("User not found"))?;
        if target.str("login") == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is protected and cannot be deleted.");
        }
        // Created on first use, so it never takes an id before the admin
        let ghost = ensure_ghost_user(&st).await?;
        let (login, user) = (target.string("login"), target.string("user"));
        let custom = hall_custom_dir(&st);

        let mut tx = st.db.begin().await?;
        // 1. Content (items, comments, tags)
        sql!("UPDATE items SET username = 'deleted_user' WHERE username ILIKE ", like(&login), " OR username ILIKE ", like(&user)).execute(&mut *tx).await?;
        for (table, col) in [
            ("comments", "user_id"),
            ("tags_assign", "user_id"),
            // 2. Social/admin rows (tokens, reports, messages, warnings)
            ("invite_tokens", "used_by"),
            ("invite_tokens", "created_by"),
            ("reports", "user_id"),
            ("reports", "reporter_id"),
            ("reports", "resolved_by"),
            ("private_messages", "sender_id"),
            ("private_messages", "recipient_id"),
            ("user_warnings", "user_id"),
            ("user_warnings", "admin_id"),
        ] {
            let mut q = Sql::new("UPDATE ");
            q.push(table).push(" SET ").push(col).push(" = ").bind(ghost).push(" WHERE ").push(col).push(" = ").bind(uid);
            q.execute(&mut *tx).await?;
        }

        // Halls move to the ghost, renamed on slug collisions
        let halls = sql!("SELECT id, slug, custom_image FROM user_halls WHERE user_id = ", uid).fetch_all(&mut *tx).await?;
        let mut taken: std::collections::HashSet<String> =
            sql!("SELECT slug FROM user_halls WHERE user_id = ", ghost).fetch_all(&mut *tx).await?.iter().map(|r| r.string("slug")).collect();
        for h in &halls {
            let slug = h.string("slug");
            let mut final_slug = slug.clone();
            if taken.contains(&slug) {
                final_slug = format!("{slug}-from-{uid}");
                while taken.contains(&final_slug) {
                    final_slug = format!("{slug}-from-{uid}-{}", crate::util::random_hex(2));
                }
            }
            taken.insert(final_slug.clone());
            if h.bool("custom_image") {
                // Slugs are only used in file names when they are plain names
                if crate::media::is_plain_name(&slug) && crate::media::is_plain_name(&final_slug) {
                    let old = custom.join(format!("u_{uid}_{slug}.webp"));
                    let new = custom.join(format!("u_{ghost}_{final_slug}.webp"));
                    if let Err(e) = tokio::fs::rename(&old, &new).await {
                        tracing::warn!("[ADMIN_DELETE] Failed to move hall image: {} -> {}: {e}", old.display(), new.display());
                    }
                }
            }
            sql!("UPDATE user_halls SET user_id = ", ghost, ", slug = ", final_slug, " WHERE id = ", h.i64("id").unwrap_or(0) as i32).execute(&mut *tx).await?;
        }
        sql!("UPDATE halls_assign SET user_id = ", ghost, " WHERE user_id = ", uid).execute(&mut *tx).await?;
        sql!("UPDATE user_halls_assign SET user_id = ", ghost, " WHERE user_id = ", uid).execute(&mut *tx).await?;
        sql!("UPDATE audit_log SET user_id = ", ghost, " WHERE user_id = ", uid).execute(&mut *tx).await?;

        // 3. Personal and ephemeral data
        for table in [
            "user_sessions",
            "user_options",
            "user_pubkeys",
            "user_video_views",
            "comment_subscriptions",
            "notifications",
            "favorites",
            "link_token",
            "discord_queue",
        ] {
            let mut q = Sql::new("DELETE FROM ");
            q.push(table).push(" WHERE user_id = ").bind(uid);
            q.execute(&mut *tx).await?;
        }
        sql!("DELETE FROM user_conversation_states WHERE user_id = ", uid, " OR other_id = ", uid).execute(&mut *tx).await?;
        // 4. The account itself
        sql!(r#"DELETE FROM "user" WHERE id = "#, uid).execute(&mut *tx).await?;
        tx.commit().await?;

        crate::audit::log(&st, sid(&req), "admin_delete_user", "user", uid.to_string(), json!({ "target_login": login })).await;
        Ok(json!({ "success": true, "msg": format!("User {login} deleted and content reassigned to deleted_user.") }))
    }
    .await;
    if let Err(e) = &r {
        tracing::error!("[ADMIN] Deletion failed: {e:#}");
    }
    answer(r, 500)
}

async fn reassign_uploads(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let source_name = body_str(&body, "source_username").unwrap_or_default();
        let has_source_id = body_truthy(&body, "source_user_id");
        if !has_source_id && source_name.is_empty() {
            anyhow::bail!("Missing source_user_id or source_username");
        }
        let target_name = body_str(&body, "target_username").unwrap_or_default();
        if target_name.trim().is_empty() {
            anyhow::bail!("Missing target_username");
        }
        // Source: a registered user or a legacy username
        let source_id = if has_source_id { Some(require_id(&body, "source_user_id")?) } else { None };
        let (source_login, source_user) = match source_id {
            Some(id) => {
                let s = sql!(r#"SELECT id, login, "user" FROM "user" WHERE id = "#, id, " LIMIT 1")
                    .fetch_optional(&st.db)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Source user not found"))?;
                if s.str("login") == Some("deleted_user") {
                    anyhow::bail!("Cannot reassign uploads from the protected deleted_user account.");
                }
                (s.string("login"), s.string("user"))
            }
            None => (source_name.trim().to_string(), source_name.trim().to_string()),
        };
        let tname = target_name.trim().to_string();
        let target = sql!(r#"SELECT id, login, "user" FROM "user" WHERE login ILIKE "#, like(&tname), " LIMIT 1")
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Target user \"{tname}\" not found"))?;
        let (target_login, target_id) = (target.string("login"), target.i64("id").unwrap_or(0));
        if source_id.map(|s| s as i64) == Some(target_id) {
            anyhow::bail!("Source and target user are the same.");
        }
        let count = sql!("UPDATE items SET username = ", target_login.clone(), " WHERE username ILIKE ", like(&source_login), " OR username ILIKE ", like(&source_user))
            .execute(&st.db)
            .await?;
        crate::audit::log(
            &st,
            sid(&req),
            "admin_reassign_uploads",
            "user",
            source_id.map(|s| s.to_string()),
            json!({ "source_login": source_login, "target_login": target_login, "target_id": target_id, "count": count }),
        )
        .await;
        Ok(json!({ "success": true, "count": count, "msg": format!("Successfully reassigned {count} uploads from {source_login} to {target_login}.") }))
    }
    .await;
    answer(r, 200)
}

async fn bulk_delete_items(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let username = body_str(&body, "username").unwrap_or_default();
        let has_id = body_truthy(&body, "user_id");
        if !has_id && username.is_empty() {
            anyhow::bail!("Missing user_id or username");
        }
        let uid = if has_id { body_id(&body, "user_id").map(|i| i as i32) } else { None };
        let mut names: Option<(String, String)> = None;
        if let Some(id) = uid {
            if let Some(t) = sql!(r#"SELECT login, "user" FROM "user" WHERE id = "#, id, " LIMIT 1").fetch_optional(&st.db).await? {
                names = Some((t.string("login"), t.string("user")));
            }
        }
        if names.is_none() && !username.is_empty() {
            names = Some((username.clone(), username.clone()));
        }
        let (login, user) = names.ok_or_else(|| anyhow::anyhow!("User not found"))?;
        let ids: Vec<i32> = sql!("UPDATE items SET is_deleted = true, active = false WHERE username ILIKE ", like(&login), " OR username ILIKE ", like(&user), " RETURNING id")
            .fetch_all(&st.db)
            .await?
            .iter()
            .filter_map(|r| r.i64("id").map(|i| i as i32))
            .collect();
        let count = ids.len() as u64;
        crate::routes::reports::resolve_for_deleted_items(&st, ids, Some(sid(&req) as i32)).await;
        crate::audit::log(&st, sid(&req), "admin_bulk_delete_items", "user", uid.map(|i| i.to_string()), json!({ "target_login": login, "count": count })).await;
        Ok(json!({ "success": true, "count": count, "msg": format!("Successfully marked {count} items as deleted.") }))
    }
    .await;
    answer(r, 200)
}

async fn bulk_delete_comments(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let count = sql!("DELETE FROM comments WHERE user_id = ", uid).execute(&st.db).await?;
        crate::audit::log(&st, sid(&req), "admin_bulk_delete_comments", "user", uid.to_string(), json!({ "count": count })).await;
        Ok(json!({ "success": true, "count": count, "msg": format!("Successfully deleted {count} comments.") }))
    }
    .await;
    answer(r, 200)
}

async fn reset_login_attempts(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let username = body_str(&body, "username").filter(|u| !u.is_empty()).ok_or_else(|| anyhow::anyhow!("Missing username"))?;
        crate::security::clear_attempts(&st, "", Some(&username)).await;
        let target = sql!(r#"SELECT id FROM "user" WHERE login = "#, username.clone(), " LIMIT 1").fetch_optional(&st.db).await?;
        crate::audit::log(&st, sid(&req), "admin_reset_login_attempts", "user", target.and_then(|t| t.i64("id")).map(|i| i.to_string()), json!({ "target_login": username }))
            .await;
        Ok(json!({ "success": true, "msg": format!("Successfully reset login attempts for {username}.") }))
    }
    .await;
    answer(r, 200)
}

async fn bulk_delete_halls(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let slugs: Vec<String> = sql!("SELECT slug FROM user_halls WHERE user_id = ", uid).fetch_scalar_list(&st.db).await?;
        let count = sql!("DELETE FROM user_halls WHERE user_id = ", uid).execute(&st.db).await?;
        sql!("DELETE FROM user_halls_assign WHERE user_id = ", uid).execute(&st.db).await?;
        // Custom hall images on disk
        let custom = hall_custom_dir(&st);
        for slug in slugs.iter().filter(|s| crate::media::is_plain_name(s)) {
            let _ = tokio::fs::remove_file(custom.join(format!("u_{uid}_{slug}.webp"))).await;
        }
        crate::audit::log(&st, sid(&req), "admin_bulk_delete_halls", "user", uid.to_string(), json!({ "count": count })).await;
        Ok(json!({ "success": true, "count": count, "msg": format!("Successfully deleted {count} halls and all assignments.") }))
    }
    .await;
    answer(r, 200)
}

async fn rename(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let new_name = body_str(&body, "new_username").unwrap_or_default().trim().to_string();
        if new_name.is_empty() {
            anyhow::bail!("Missing new_username");
        }
        if !valid_username_chars(&new_name) {
            anyhow::bail!("Invalid username. Only A-Z, 0-9, _, -, and . are allowed.");
        }
        if new_name.len() < 2 || new_name.len() > 32 {
            anyhow::bail!("Username must be between 2 and 32 characters.");
        }
        let target = target_user(&st, uid).await.map_err(|_| anyhow::anyhow!("User not found"))?;
        if target.str("login") == Some("deleted_user") {
            anyhow::bail!("The deleted_user account is protected and cannot be renamed.");
        }
        let (old_login, old_user) = (target.string("login"), target.string("user"));
        let new_login = new_name.to_lowercase();
        if new_login == old_login && new_name == old_user {
            anyhow::bail!("New username is the same as the current one.");
        }
        let conflict = sql!(
            r#"SELECT id FROM "user" WHERE (lower(login) = "#, new_login.clone(), r#" OR lower("user") = lower("#, new_name.clone(), ")) AND id != ", uid, " LIMIT 1"
        )
        .fetch_optional(&st.db)
        .await?;
        if conflict.is_some() {
            anyhow::bail!("Username \"{new_name}\" is already taken.");
        }
        let mut tx = st.db.begin().await?;
        sql!(r#"UPDATE "user" SET login = "#, new_login.clone(), r#", "user" = "#, new_name.clone(), " WHERE id = ", uid).execute(&mut *tx).await?;
        sql!("UPDATE items SET username = ", new_login.clone(), " WHERE username ILIKE ", like(&old_login), " OR username ILIKE ", like(&old_user))
            .execute(&mut *tx)
            .await?;
        // The new name starts with a clean login-attempt record
        sql!("DELETE FROM login_attempts WHERE username = ", old_login.clone()).execute(&mut *tx).await?;
        tx.commit().await?;
        // The user has to log in again with the new name
        sql!("DELETE FROM user_sessions WHERE user_id = ", uid).execute(&st.db).await?;
        crate::audit::log(
            &st,
            sid(&req),
            "admin_rename_user",
            "user",
            uid.to_string(),
            json!({ "old_login": old_login, "new_login": new_login, "old_user": old_user, "new_user": new_name }),
        )
        .await;
        Ok(json!({
            "success": true,
            "new_login": new_login,
            "new_user": new_name,
            "msg": format!("User renamed from \"{old_login}\" to \"{new_login}\". All uploads updated. Sessions invalidated."),
        }))
    }
    .await;
    if let Err(e) = &r {
        tracing::error!("[ADMIN] Rename failed: {e:#}");
    }
    answer(r, 200)
}

async fn create(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let cfg = st.cfg();
    let r = async {
        let username = body_str(&body, "username").unwrap_or_default().trim().to_string();
        if username.is_empty() {
            anyhow::bail!("Username is required.");
        }
        if !valid_username_chars(&username) {
            anyhow::bail!("Username contains invalid characters. Only A-Z, 0-9, _, -, and . are allowed.");
        }
        if username.len() < 2 || username.len() > 32 {
            anyhow::bail!("Username must be between 2 and 32 characters.");
        }
        let password = body_str(&body, "password").unwrap_or_default();
        if password.encode_utf16().count() < 20 {
            anyhow::bail!("Password must be at least 20 characters long.");
        }
        let email = body_str(&body, "email").filter(|e| !e.is_empty());
        let role = body_str(&body, "role").unwrap_or_default();
        let mut q = sql!(r#"SELECT id FROM "user" WHERE lower(login) = lower("#, username.clone(), r#") OR lower("user") = lower("#, username.clone(), ")");
        if let Some(e) = &email {
            q.push(" OR (email IS NOT NULL AND lower(email) = lower(").bind(e.clone()).push("))");
        }
        q.push(" LIMIT 1");
        if q.fetch_optional(&st.db).await?.is_some() {
            anyhow::bail!("Username or email is already taken.");
        }
        let hash = crate::util::hash_password(password).await;
        if hash.is_empty() {
            anyhow::bail!("Password hashing failed");
        }
        let login = username.to_lowercase();
        let row = sql!(
            r#"INSERT INTO "user" (login, password, "user", created_at, admin, is_moderator, email, activated, activation_token)
               VALUES ("#, login.clone(), ", ", hash, ", ", username.clone(), ", to_timestamp(", crate::util::now_secs() as f64, "), ",
            role == "admin", ", ", role == "mod", ", ", email, ", true, NULL) RETURNING id, login"
        )
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| anyhow::anyhow!("User insert failed"))?;
        let uid = row.i64("id").unwrap_or(0);
        sql!(
            "INSERT INTO user_options (user_id, mode, theme, fullscreen, avatar, avatar_file, use_new_layout, disable_autoplay, disable_swiping, use_alternative_infobox) VALUES (",
            uid as i32, ", 3, 'amoled', 0, NULL, 'default.png', ", st.settings.default_layout() == "modern", ", ",
            cfg.is_false("websrv.enable_autoplay"), ", ", cfg.is_false("websrv.enable_swiping"), ", ", cfg.enabled("websrv.user_alternative_infobox"), ")"
        )
        .execute(&st.db)
        .await?;
        let shown_role = if role.is_empty() { "user".to_string() } else { role.clone() };
        crate::audit::log(&st, sid(&req), "admin_create_user", "user", uid.to_string(), json!({ "new_login": login, "role": shown_role })).await;
        Ok(json!({ "success": true, "user_id": uid, "username": login, "msg": format!("User \"{login}\" created successfully.") }))
    }
    .await;
    if let Err(e) = &r {
        tracing::error!("[ADMIN] Create user failed: {e:#}");
    }
    answer(r, 200)
}

async fn set_display_name(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let target = sql!(r#"SELECT login, "user" FROM "user" WHERE id = "#, uid, " AND login != 'deleted_user' LIMIT 1")
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found or account is protected."))?;
        let trimmed = body_str(&body, "display_name").unwrap_or_default().trim().to_string();
        let name: Option<String> = (!trimmed.is_empty()).then(|| trimmed.clone());
        sql!(
            "INSERT INTO user_options (user_id, display_name, mode, theme, excluded_tags) VALUES (", uid, ", ", name.clone(),
            ", 0, '4d', '{}'::integer[]) ON CONFLICT (user_id) DO UPDATE SET display_name = ", name.clone()
        )
        .execute(&st.db)
        .await?;
        crate::audit::log(&st, sid(&req), "admin_set_display_name", "user", uid.to_string(), json!({ "target_login": target.val("login"), "display_name": name })).await;
        // The target's live session updates its navbar
        let payload = json!({ "user_id": uid, "user": target.val("user"), "display_name": name });
        let _ = sql!("SELECT pg_notify('profile_update', ", payload.to_string(), ")").execute(&st.db).await;
        let msg = if trimmed.is_empty() { "Display name cleared.".to_string() } else { format!("Display name set to \"{trimmed}\"") };
        Ok(json!({ "success": true, "display_name": name, "msg": msg }))
    }
    .await;
    answer(r, 500)
}

async fn subscribe_to_uploads(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let uid = require_id(&body, "user_id")?;
        let t = target_user(&st, uid).await.map_err(|_| anyhow::anyhow!("User not found"))?;
        // Items by login or display name (case-insensitive) — covers renamed and legacy accounts
        sql!(
            "INSERT INTO comment_subscriptions (user_id, item_id) SELECT ", uid, ", i.id FROM items i WHERE i.username ILIKE ",
            like(&t.string("login")), " OR i.username ILIKE ", like(&t.string("user")), " ON CONFLICT DO NOTHING"
        )
        .execute(&st.db)
        .await?;
        Ok(json!({ "success": true, "message": "User subscribed to all their uploads" }))
    }
    .await;
    Ok(match r {
        Ok(v) => json_resp(&v, 200),
        Err(e) => {
            tracing::error!("[ADMIN] Failed to subscribe user to uploads: {e:#}");
            json_resp(&json!({ "success": false, "msg": err_msg(&e) }), 200)
        }
    })
}

// ── Admin bar: impersonation ────────────────────────────────────────────────

/// GET /api/v2/admin/users/search?q= — autocomplete for the admin bar "View as" input.
async fn search(st: AppState, req: Req) -> Res {
    let q = req.query("q").unwrap_or("").trim().to_string();
    if q.is_empty() {
        return Ok(json_resp(&json!([]), 200));
    }
    let r = sql!(
        r#"SELECT id, login as user FROM "user" WHERE login ILIKE "#, format!("%{}%", crate::util::escape_like(&q)),
        " AND activated = true AND banned = false ORDER BY login ASC LIMIT 10"
    )
    .fetch_all(&st.db)
    .await;
    Ok(match r {
        Ok(list) => json_resp(&Value::Array(list.into_iter().map(|u| json!({ "id": u.val("id"), "user": u.val("user") })).collect()), 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": err_msg(&e) }), 200),
    })
}

/// The signed `impersonate` cookie value: `base64url(JSON) "." hex(HMAC-SHA256)`, bound to the
/// admin's session cookie (verified by `app::impersonation_target`).
pub(crate) fn impersonation_cookie(secret: &str, uid: i64, session_cookie: &str, ts: i64) -> String {
    use base64::Engine;
    let payload = json!({ "uid": uid, "orig": crate::util::sha256(session_cookie), "ts": ts });
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string());
    let sig = crate::util::hmac_sha256_hex(secret.as_bytes(), b64.as_bytes());
    format!("{b64}.{sig}")
}

async fn impersonate(st: AppState, mut req: Req) -> Res {
    let username = body_str(req.post().await?, "username").unwrap_or_default();
    let r = async {
        if username.is_empty() {
            anyhow::bail!("Username required");
        }
        let target = sql!(r#"SELECT id, login as user FROM "user" WHERE login = "#, username.trim().to_lowercase(), " AND activated = true LIMIT 1")
            .fetch_optional(&st.db)
            .await?
            .ok_or_else(|| anyhow::anyhow!("User not found"))?;
        let tid = target.i64("id").unwrap_or(0);
        if tid == sid(&req) {
            anyhow::bail!("Cannot impersonate yourself");
        }
        let session_cookie = req.cookie("session").ok_or_else(|| anyhow::anyhow!("Session cookie required"))?.to_string();
        let value = impersonation_cookie(&st.impersonation_secret(), tid, &session_cookie, crate::util::now_ms());
        req.set_cookie(format!("impersonate={value}; {}", cookie_options(&st.cfg(), CookieExpiry::Expires("Fri, 31 Dec 9999 23:59:59 GMT"), true)));
        Ok(json!({ "success": true, "username": target.val("user") }))
    }
    .await;
    Ok(match r {
        Ok(v) => json_raw(&v, 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": err_msg(&e) }), 200),
    })
}

/// POST /api/v2/admin/stop-impersonate — no guard needed, it only clears the cookie.
async fn stop_impersonate(st: AppState, req: Req) -> Res {
    req.set_cookie(format!("impersonate=; {}", cookie_options(&st.cfg(), CookieExpiry::Expires(crate::http::EXPIRED), true)));
    Ok(json_raw(&json!({ "success": true }), 200))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_parsing() {
        assert_eq!(parse_groups(Some(&json!(" 4chan, VIP ,,"))), vec!["4chan", "vip"]);
        assert_eq!(parse_groups(Some(&json!(["A", " b ", ""]))), vec!["a", "b"]);
        assert!(parse_groups(None).is_empty());
    }

    #[test]
    fn usernames() {
        assert!(valid_username_chars("a.b-c_1"));
        assert!(!valid_username_chars("a b"));
        assert!(!valid_username_chars("ä"));
        assert!(!valid_username_chars(""));
    }

    #[test]
    fn impersonation_cookie_verifies() {
        use base64::Engine;
        let c = impersonation_cookie("secret", 42, "sess", 1);
        let (payload, sig) = c.split_once('.').unwrap();
        assert_eq!(sig, crate::util::hmac_sha256_hex(b"secret", payload.as_bytes()));
        let data: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(data["uid"], 42);
        assert_eq!(data["orig"], crate::util::sha256("sess"));
    }
}
