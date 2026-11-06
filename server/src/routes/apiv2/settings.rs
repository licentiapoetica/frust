//! `/api/v2/settings/*` and `/api/v2/invite-request*` (port of src/inc/routes/apiv2/settings.mjs).
//! The passkey routes of that file live in `passkeys.rs`; avatar/banner uploads are the bypass
//! handlers in `routes/profile_media.rs`.
//!
//! The old handlers changed the cached session object in place after an update; here the cached
//! session row is dropped instead, so the next request reads the new value.

use crate::db::{Row, RowExt};
use crate::http::{Group, Guard, Req, Res, Router, html_code, json_resp, reply, with_header};
use crate::social::helpers::{js_parse_float, js_parse_int, js_str};
use crate::sql;
use crate::state::AppState;
use serde_json::{Map, Value, json};

/// Passkey-only accounts store this instead of a hash; it never verifies.
const PASSWORD_DISABLED: &str = "!";

pub fn register(r: &mut Router) {
    use Guard::{LoggedIn, RegisteredUser};
    let mut g = Group::new(r, r"^\/api\/v2\/settings");
    g.put_g(r"\/setAvatar", RegisteredUser, set_avatar);
    g.put_g(r"\/useCustomAvatar", RegisteredUser, use_custom_avatar);

    g.get_g(r"\/excluded_tags", LoggedIn, excluded_tags_get);
    g.post_g(r"\/excluded_tags", LoggedIn, excluded_tags_add);
    g.delete_g(r"\/excluded_tags\/(?<tag>.+)", LoggedIn, excluded_tags_remove);

    g.post_g(r"\/link\/token", RegisteredUser, link_token);
    g.get_g(r"\/link\/accounts", RegisteredUser, link_accounts);
    g.delete_g(r"\/link\/unlink\/(?<type>[a-z]+)\/(?<alias>.+)", RegisteredUser, link_unlink);
    g.post_g(r"\/discord\/token", RegisteredUser, discord_token);
    g.get_g(r"\/discord\/linked", RegisteredUser, discord_linked);
    g.delete_g(r"\/discord\/unlink\/(?<alias>.+)", RegisteredUser, discord_unlink);

    g.put_g(r"\/motd", LoggedIn, |st, req| bool_pref(st, req, "show_motd", "show", "show", "Update MOTD pref"));
    g.put_g(r"\/autoplay", LoggedIn, |st, req| bool_pref(st, req, "disable_autoplay", "disable_autoplay", "disable_autoplay", "Update Autoplay pref"));
    g.put_g(r"\/swiping", LoggedIn, |st, req| bool_pref(st, req, "disable_swiping", "disable_swiping", "disable_swiping", "Update Swiping pref"));
    g.put_g(r"\/layout", LoggedIn, |st, req| bool_pref(st, req, "use_new_layout", "use_new_layout", "use_new_layout", "Update Layout pref"));
    g.put_g(r"\/favorites_private", LoggedIn, |st, req| {
        bool_pref(st, req, "favorites_private", "favorites_private", "favorites_private", "Update Favorites Privacy pref")
    });
    g.put_g(r"\/hide_fav_badge", LoggedIn, |st, req| bool_pref(st, req, "hide_fav_badge", "hide_fav_badge", "hide_fav_badge", "Update Hide Fav Badge pref"));
    g.put_g(r"\/default_upload_visibility", RegisteredUser, default_upload_visibility);
    g.put_g(r"\/username_color", RegisteredUser, username_color);
    g.post_g(r"\/password\/disable$", RegisteredUser, password_disable);
    g.put_g(r"\/password", RegisteredUser, password_change);
    g.put_g(r"\/email", RegisteredUser, email);
    g.put_g(r"\/display_name", RegisteredUser, display_name);
    g.put_g(r"\/description", RegisteredUser, description);
    g.put_g(r"\/font$", LoggedIn, font);
    g.put_g(r"\/font_size$", LoggedIn, font_size);
    g.get_g(r"\/me$", LoggedIn, me);

    g.put_g(r"\/background", LoggedIn, |st, req| bool_pref(st, req, "show_background", "show_background", "show_background", "Update background"));
    g.put_g(r"\/ruffle", LoggedIn, ruffle);
    g.put_g(r"\/quote_emojis", LoggedIn, |st, req| bool_pref(st, req, "quote_emojis", "quote_emojis", "quote_emojis", "Update quote_emojis"));
    g.put_g(r"\/embed_youtube_in_comments", LoggedIn, |st, req| {
        bool_pref(st, req, "embed_youtube_in_comments", "embed_youtube_in_comments", "embed_youtube_in_comments", "Update embed_youtube_in_comments")
    });
    g.put_g(r"\/hide_koepfe", LoggedIn, |st, req| bool_pref(st, req, "hide_koepfe", "hide_koepfe", "hide_koepfe", "Update hide_koepfe"));
    g.put_g(r"\/alternative_infobox", LoggedIn, alternative_infobox);
    g.put_g(r"\/alternative_steuerung", LoggedIn, |st, req| {
        bool_pref(st, req, "use_alternative_steuerung", "use_alternative_steuerung", "use_alternative_steuerung", "Update alternative_steuerung")
    });
    g.put_g(r"\/language", LoggedIn, language);
    g.put_g(r"\/comment_display_mode", LoggedIn, comment_display_mode);
    // `group.post('/notifications', …)`: a string path, i.e. `new RegExp('/notifications')`
    g.post_g(r"\/notifications", LoggedIn, notifications);

    g.get_g(r"\/api-key$", RegisteredUser, api_key_get);
    g.post_g(r"\/api-key\/regenerate$", RegisteredUser, api_key_regenerate);
    g.delete_g(r"\/api-key$", RegisteredUser, api_key_delete);
    g.get_g(r"\/api-key\/sharex-config$", RegisteredUser, sharex_config);

    g.get_g(r"\/invites$", RegisteredUser, invites_get);
    g.post_g(r"\/invites\/create$", RegisteredUser, invites_create);
    g.post_g(r"\/invites\/delete$", RegisteredUser, invites_delete);

    r.post_g(r"^\/api\/v2\/invite-request\/?$", LoggedIn, invite_request);
    r.get_g(r"^\/api\/v2\/invite-request\/status\/?$", LoggedIn, invite_request_status);
}

// ── helpers ─────────────────────────────────────────────────────────────────

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn fail(msg: &str, code: u16) -> Res {
    j(json!({ "success": false, "msg": msg }), code)
}

/// `+req.session.id`
fn uid(req: &Req) -> i32 {
    req.session().map(|s| s.id as i32).unwrap_or(0)
}

/// Drop the cached session row of this request so the next request sees the update.
fn refresh_session(st: &AppState, req: &Req) {
    if let Some(c) = req.cookie("session") {
        st.invalidate_session_cache(&crate::util::sha256(c));
    }
}

/// `x === true || x === 'true'`
fn post_bool(body: &Value, k: &str) -> bool {
    matches!(body.get(k), Some(Value::Bool(true))) || matches!(body.get(k), Some(Value::String(s)) if s == "true")
}

/// `parseInt(x, 10)` of a body value.
fn post_int(body: &Value, k: &str) -> Option<i64> {
    js_str(body.get(k)).and_then(|s| js_parse_int(&s))
}

/// A body value that is a string (`typeof x === 'string'`).
fn post_string(body: &Value, k: &str) -> Option<String> {
    body.get(k).and_then(Value::as_str).map(str::to_string)
}

/// `String.prototype.substring(0, n)` on UTF-16 units, without splitting a surrogate pair.
fn js_truncate(s: &str, n: usize) -> String {
    let mut units = 0;
    let mut out = String::new();
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            break;
        }
        out.push(c);
    }
    out
}

fn rows_value(rows: Vec<Row>) -> Value {
    Value::Array(rows.into_iter().map(Value::Object).collect())
}

/// Log the error like the old `console.error` and answer with the handler's generic message.
fn logged_500(ctx: &str, e: impl std::fmt::Display, msg: &str) -> Res {
    tracing::error!("{ctx} error: {e}");
    fail(msg, 500)
}

async fn notify_profile(st: &AppState, payload: Value) -> anyhow::Result<()> {
    sql!("select pg_notify('profile_update', ", payload.to_string(), ")").execute(&st.db).await?;
    Ok(())
}

/// Delete all sessions of the user except the current one, dropping their cached lookups.
async fn end_other_sessions(st: &AppState, user_id: i32, keep_sess_id: i64) -> anyhow::Result<()> {
    let rows = sql!(r#"delete from "user_sessions" where user_id = "#, user_id, " and id != ", keep_sess_id as i32, " returning session")
        .fetch_all(&st.db)
        .await?;
    for r in rows {
        if let Some(h) = r.str("session") {
            st.invalidate_session_cache(h);
        }
    }
    Ok(())
}

// ── avatar ──────────────────────────────────────────────────────────────────

async fn set_avatar(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    if !crate::config::truthy(body.get("avatar").unwrap_or(&Value::Null)) {
        return j(json!({ "msg": "no avatar provided", "debug": body }), 400);
    }
    // `+req.post.avatar`: anything that is not a plain integer can't name an item
    let avatar = js_str(body.get("avatar"))
        .and_then(|s| crate::social::helpers::js_number(&s))
        .filter(|f| f.fract() == 0.0 && *f > 0.0 && *f <= i32::MAX as f64)
        .map(|f| f as i32);
    let item = match avatar {
        Some(a) => sql!("select id from items where id = ", a, " and active = true").fetch_optional(&st.db).await?,
        None => None,
    };
    let Some(a) = avatar.filter(|_| item.is_some()) else {
        return j(json!({ "msg": "itemid not found" }), 404);
    };
    sql!("update user_options set avatar = ", a, " where user_id = ", uid(&req)).execute(&st.db).await?;
    refresh_session(&st, &req);
    // `res.json({ msg: q })` with the (empty) postgres.js result of the update
    j(json!({ "msg": [] }), 200)
}

async fn use_custom_avatar(st: AppState, req: Req) -> Res {
    let row = sql!("select avatar_file from user_options where user_id = ", uid(&req)).fetch_optional(&st.db).await?;
    let Some(file) = row.and_then(|r| r.str("avatar_file").filter(|f| !f.is_empty()).map(str::to_string)) else {
        return j(json!({ "success": false, "msg": "No custom avatar uploaded" }), 400);
    };
    // The old code set `avatar = 0`, which violates the foreign key to items (there is no item 0)
    // and failed with a 500. NULL is what the avatar upload sets so the custom file wins.
    sql!("update user_options set avatar = NULL where user_id = ", uid(&req)).execute(&st.db).await?;
    refresh_session(&st, &req);
    j(json!({ "success": true, "avatar_file": file, "msg": "Switched to custom avatar" }), 200)
}

// ── excluded tags ───────────────────────────────────────────────────────────

fn exclude_forbidden(st: &AppState, req: &Req) -> bool {
    crate::settings::is_anon_session(req.session()) && !crate::settings::can_anon_do(&st.cfg(), "exclude_tags")
}

async fn excluded_tag_rows(st: &AppState, user_id: i32) -> anyhow::Result<Vec<Row>> {
    sql!(
        "select t.id, t.tag, t.normalized from unnest((select excluded_tags from user_options where user_id = ", user_id,
        ")) as et(id) join tags t on t.id = et.id"
    )
    .fetch_all(&st.db)
    .await
}

async fn find_tag(st: &AppState, id: Option<i64>, name: &str) -> anyhow::Result<Option<Row>> {
    match id {
        Some(id) => match i32::try_from(id) {
            Ok(id) => sql!("select id, tag, normalized from tags where id = ", id).fetch_optional(&st.db).await,
            Err(_) => Ok(None),
        },
        None => sql!("select id, tag, normalized from tags where normalized = slugify(", name, ") or tag = ", name).fetch_optional(&st.db).await,
    }
}

async fn excluded_tags_get(st: AppState, req: Req) -> Res {
    if exclude_forbidden(&st, &req) {
        return fail("Tag exclusion is disabled for anonymous users", 403);
    }
    let tags = excluded_tag_rows(&st, uid(&req)).await?;
    j(json!({ "success": true, "tags": rows_value(tags) }), 200)
}

async fn excluded_tags_add(st: AppState, mut req: Req) -> Res {
    if exclude_forbidden(&st, &req) {
        return fail("Tag exclusion is disabled for anonymous users", 403);
    }
    let body = req.post_value().await;
    let truthy = |k: &str| body.get(k).filter(|v| crate::config::truthy(v));
    let tagname = truthy("tagname").and_then(|v| js_str(Some(v)));
    let tag_id = truthy("tag_id");
    if tagname.is_none() && tag_id.is_none() {
        return fail("No tag provided", 400);
    }
    let tag = match tag_id {
        // `+tagId`: NaN never matches
        Some(v) => match js_str(Some(v)).and_then(|s| crate::social::helpers::js_number(&s)).filter(|f| f.fract() == 0.0) {
            Some(f) => find_tag(&st, Some(f as i64), "").await?,
            None => None,
        },
        None => find_tag(&st, None, tagname.as_deref().unwrap_or("")).await?,
    };
    let Some(tag) = tag else { return fail("Tag not found", 404) };
    let tag_id = tag.i64("id").unwrap_or(0) as i32;
    sql!(
        "update user_options set excluded_tags = array_append(coalesce(excluded_tags, '{}'), ", tag_id,
        ") where user_id = ", uid(&req), " and not (", tag_id, " = any(coalesce(excluded_tags, '{}')))"
    )
    .execute(&st.db)
    .await?;
    refresh_session(&st, &req);
    let tags = excluded_tag_rows(&st, uid(&req)).await?;
    j(json!({ "success": true, "tags": rows_value(tags), "tag": tag }), 200)
}

async fn excluded_tags_remove(st: AppState, req: Req) -> Res {
    if exclude_forbidden(&st, &req) {
        return fail("Tag exclusion is disabled for anonymous users", 403);
    }
    let raw = req.param("tag").unwrap_or("");
    let Some(param) = crate::util::decode_uri_component(raw) else { return fail("Tag not found", 404) };
    let tag = if !param.is_empty() && param.bytes().all(|b| b.is_ascii_digit()) {
        match param.parse::<i64>() {
            Ok(n) => find_tag(&st, Some(n), "").await?,
            Err(_) => None,
        }
    } else {
        find_tag(&st, None, &param).await?
    };
    let Some(tag) = tag else { return fail("Tag not found", 404) };
    let tag_id = tag.i64("id").unwrap_or(0) as i32;
    sql!("update user_options set excluded_tags = array_remove(coalesce(excluded_tags, '{}'), ", tag_id, ") where user_id = ", uid(&req))
        .execute(&st.db)
        .await?;
    refresh_session(&st, &req);
    let tags = excluded_tag_rows(&st, uid(&req)).await?;
    j(json!({ "success": true, "tags": rows_value(tags), "tag": tag }), 200)
}

// ── chat account linking ────────────────────────────────────────────────────

/// A fresh link token for the current user. The old code used 6 characters of `Math.random()` and
/// an upsert `ON CONFLICT (token) DO UPDATE` that kept the *previous owner's* user id on a
/// collision (so the new user's token linked chat accounts to someone else). Now: 256 random bits
/// from the OS RNG (upper-case hex, which the bots expect) and a plain insert.
async fn new_link_token(st: &AppState, user_id: i32) -> anyhow::Result<String> {
    let token = crate::util::generate_token().to_uppercase();
    sql!("INSERT INTO link_token (user_id, token) VALUES (", user_id, ", ", token.clone(), ")").execute(&st.db).await?;
    Ok(token)
}

async fn link_token(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let kind = body.get("type").filter(|v| crate::config::truthy(v)).cloned().unwrap_or_else(|| json!("discord"));
    match new_link_token(&st, uid(&req)).await {
        Ok(token) => j(json!({ "success": true, "token": token, "type": kind }), 200),
        Err(e) => Err(e.into()),
    }
}

async fn link_accounts(st: AppState, req: Req) -> Res {
    let rows = match sql!("SELECT alias, type FROM user_alias WHERE userid = ", uid(&req), " ORDER BY type DESC, alias ASC").fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => return logged_500("Get linked", e, "Error fetching linked accounts"),
    };
    let aliases: Vec<Value> = rows
        .iter()
        .map(|a| {
            let alias = a.string("alias").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
            let kind = a.str("type").filter(|t| !t.is_empty()).unwrap_or("discord");
            json!({ "alias": alias, "type": kind })
        })
        .collect();
    j(json!({ "success": true, "aliases": aliases }), 200)
}

async fn link_unlink(st: AppState, req: Req) -> Res {
    let Some(alias) = crate::util::decode_uri_component(req.param("alias").unwrap_or("")) else {
        return fail("Error unlinking account", 500);
    };
    let kind = req.param("type").unwrap_or("").to_string();
    let r = sql!(
        "DELETE FROM user_alias WHERE lower(alias) = lower(", alias, ") AND userid = ", uid(&req), " AND type = ", kind, " RETURNING alias"
    )
    .fetch_all(&st.db)
    .await;
    match r {
        Ok(rows) if !rows.is_empty() => j(json!({ "success": true, "msg": "Account unlinked" }), 200),
        Ok(_) => fail("Account not found", 404),
        Err(e) => logged_500("Unlink", e, "Error unlinking account"),
    }
}

async fn discord_token(st: AppState, req: Req) -> Res {
    match new_link_token(&st, uid(&req)).await {
        Ok(token) => j(json!({ "success": true, "token": token }), 200),
        Err(e) => {
            tracing::error!("[SETTINGS] discord token error: {e}");
            j(json!({ "success": false }), 500)
        }
    }
}

async fn discord_linked(st: AppState, req: Req) -> Res {
    let rows = sql!("SELECT alias FROM user_alias WHERE userid = ", uid(&req), " AND type = 'discord'").fetch_all(&st.db).await?;
    let aliases: Vec<Value> = rows.iter().map(|a| json!({ "alias": a.val("alias") })).collect();
    j(json!({ "success": true, "aliases": aliases }), 200)
}

async fn discord_unlink(st: AppState, req: Req) -> Res {
    let Some(alias) = crate::util::decode_uri_component(req.param("alias").unwrap_or("")) else {
        return Err(anyhow::anyhow!("URIError: malformed alias").into());
    };
    sql!("DELETE FROM user_alias WHERE lower(alias) = lower(", alias, ") AND userid = ", uid(&req), " AND type = 'discord'").execute(&st.db).await?;
    j(json!({ "success": true, "msg": "Account unlinked" }), 200)
}

// ── simple preferences ──────────────────────────────────────────────────────

/// `update user_options set <column> = (post[key] === true || 'true')`, answering `{ success, <out>: v }`.
/// `column` is always a literal from `register`.
async fn bool_pref(st: AppState, mut req: Req, column: &'static str, key: &'static str, out: &'static str, ctx: &'static str) -> Res {
    let body = req.post_value().await;
    let v = post_bool(&body, key);
    let mut q = sql!("update user_options set ");
    q.push(column).push(" = ").bind(v).push(" where user_id = ").bind(uid(&req));
    if let Err(e) = q.execute(&st.db).await {
        return logged_500(ctx, e, "Error updating preference");
    }
    refresh_session(&st, &req);
    let mut m = Map::new();
    m.insert("success".into(), json!(true));
    m.insert(out.into(), json!(v));
    j(Value::Object(m), 200)
}

async fn default_upload_visibility(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if cfg.get("allow_user_upload_visibility") == &Value::Bool(false) || cfg.get("websrv.allow_user_upload_visibility") == &Value::Bool(false) {
        return fail("Custom upload visibility is disabled by the administrator", 403);
    }
    let body = req.post_value().await;
    let Some(vis) = post_int(&body, "default_upload_visibility").filter(|v| [0, 1, 2].contains(v)) else {
        return fail("Invalid visibility option", 400);
    };
    if let Err(e) = sql!("update user_options set default_upload_visibility = ", vis as i16, " where user_id = ", uid(&req)).execute(&st.db).await {
        return logged_500("Update Default Upload Visibility pref", e, "Error updating preference");
    }
    refresh_session(&st, &req);
    j(json!({ "success": true, "default_upload_visibility": vis }), 200)
}

async fn username_color(st: AppState, mut req: Req) -> Res {
    static COLOR: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)^#([0-9A-F]{3}){1,2}$").unwrap());
    let body = req.post_value().await;
    let color = js_str(body.get("color")).filter(|c| crate::config::truthy(&body["color"]) && COLOR.is_match(c));
    let Some(color) = color else { return fail("Invalid color format", 400) };
    let s = req.session().cloned().unwrap_or_default();
    let r = async {
        sql!("update user_options set username_color = ", color.clone(), " where user_id = ", s.id as i32).execute(&st.db).await?;
        refresh_session(&st, &req);
        notify_profile(&st, json!({ "user_id": s.id, "user": s.user, "username_color": color })).await
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true, "color": color }), 200),
        Err(e) => logged_500("Update Username Color", e, "Error updating color"),
    }
}

// ── password / email ────────────────────────────────────────────────────────

async fn password_disable(st: AppState, mut req: Req) -> Res {
    let r: anyhow::Result<Res> = async {
        let body = req.post_value().await;
        let s = req.session().cloned().unwrap_or_default();
        let Some(user) = sql!(r#"select password from "user" where id = "#, s.id as i32).fetch_optional(&st.db).await? else {
            return Ok(fail("User not found", 404));
        };
        let hash = user.string("password");
        if hash == PASSWORD_DISABLED {
            return Ok(fail("Password login is already disabled", 400));
        }
        let current = post_string(&body, "current_password").filter(|p| !p.is_empty());
        let ok = match current {
            Some(p) => crate::util::verify_password(p, hash).await,
            None => false,
        };
        if !ok {
            return Ok(fail("Incorrect current password", 401));
        }
        if crate::webauthn::count(&st, s.id).await? < 1 {
            return Ok(fail("Add a passkey first, otherwise you could not log in anymore", 400));
        }
        sql!(r#"update "user" set password = "#, PASSWORD_DISABLED, ", force_password_change = false where id = ", s.id as i32).execute(&st.db).await?;
        // Sessions elsewhere may have been opened with the password: end them, keep this one
        end_other_sessions(&st, s.id as i32, s.sess_id).await?;
        refresh_session(&st, &req);
        let _ = sql!("delete from login_attempts where username = ", s.login.clone()).execute(&st.db).await;
        Ok(j(json!({ "success": true, "msg": "Password login disabled. Use your passkey to sign in." }), 200))
    }
    .await;
    match r {
        Ok(res) => res,
        Err(e) => logged_500("[SETTINGS] Disable password", e, "Failed to disable password"),
    }
}

async fn password_change(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let new_password = post_string(&body, "new_password").filter(|p| !p.is_empty());
    let confirm = post_string(&body, "new_password_confirm").filter(|p| !p.is_empty());
    let (Some(new_password), Some(confirm)) = (new_password, confirm) else {
        return fail("New password and confirmation are required", 400);
    };
    let s = req.session().cloned().unwrap_or_default();
    let Some(user) = sql!(r#"select password, force_password_change from "user" where id = "#, s.id as i32).fetch_optional(&st.db).await? else {
        return fail("User not found", 404);
    };
    let hash = user.string("password");
    // Passkey-only accounts have no current password: setting one turns password login back on
    let password_disabled = hash == PASSWORD_DISABLED;
    if !user.bool("force_password_change") && !password_disabled {
        let Some(current) = post_string(&body, "current_password").filter(|p| !p.is_empty()) else {
            return fail("Current password is required", 400);
        };
        if !crate::util::verify_password(current, hash).await {
            return fail("Incorrect current password", 401);
        }
    }
    if new_password != confirm {
        return fail("New passwords do not match", 400);
    }
    if new_password.encode_utf16().count() < 20 {
        return fail("New password must be at least 20 characters long", 400);
    }
    let hash = crate::util::hash_password(new_password).await;
    if hash.is_empty() {
        return Err(anyhow::anyhow!("password hashing failed").into());
    }
    sql!(r#"update "user" set password = "#, hash, ", force_password_change = false where id = ", s.id as i32).execute(&st.db).await?;
    // Invalidate all other sessions (Issue 21 fix); this one keeps working with the new flag
    end_other_sessions(&st, s.id as i32, s.sess_id).await?;
    refresh_session(&st, &req);
    j(json!({ "success": true, "msg": "Password updated successfully" }), 200)
}

async fn email(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let raw = post_string(&body, "email").unwrap_or_default();
    let clean = raw.trim();
    if clean.is_empty() {
        return fail("Email is required", 400);
    }
    // Length cap added (RFC 5321 path limit); the old code only looked for an `@`.
    if !clean.contains('@') || clean.len() > 254 {
        return fail("Invalid email address", 400);
    }
    let existing = sql!(r#"select id from "user" where lower(email) = lower("#, clean, ") and id != ", uid(&req), " limit 1").fetch_optional(&st.db).await?;
    if existing.is_some() {
        return fail("Email already in use", 400);
    }
    sql!(r#"update "user" set email = "#, clean, " where id = ", uid(&req)).execute(&st.db).await?;
    j(json!({ "success": true, "msg": "Email updated successfully" }), 200)
}

// ── profile ─────────────────────────────────────────────────────────────────

/// `display_name !== undefined && typeof display_name !== 'string'` → invalid (null included).
fn optional_text(body: &Value, k: &str, max: usize) -> Result<Option<String>, ()> {
    match body.get(k) {
        None => Ok(None),
        Some(Value::String(s)) if s.is_empty() => Ok(None),
        Some(Value::String(s)) => Ok(Some(js_truncate(s.trim(), max))),
        Some(_) => Err(()),
    }
}

async fn display_name(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let Ok(clean) = optional_text(&body, "display_name", 32) else { return fail("Invalid display name format", 400) };
    let s = req.session().cloned().unwrap_or_default();
    let r = async {
        sql!("update user_options set display_name = ", clean.clone(), " where user_id = ", s.id as i32).execute(&st.db).await?;
        refresh_session(&st, &req);
        notify_profile(&st, json!({ "user_id": s.id, "user": s.user, "display_name": clean })).await
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true, "display_name": clean, "msg": "Display name updated successfully" }), 200),
        Err(e) => logged_500("Update Display Name", e, "Error updating display name"),
    }
}

async fn description(st: AppState, mut req: Req) -> Res {
    if !st.cfg().truthy("websrv.enable_profile_description") {
        return fail("Profile descriptions are disabled", 403);
    }
    let body = req.post_value().await;
    let Ok(clean) = optional_text(&body, "description", 2000) else { return fail("Invalid description format", 400) };
    let s = req.session().cloned().unwrap_or_default();
    let r = async {
        sql!("update user_options set description = ", clean.clone(), " where user_id = ", s.id as i32).execute(&st.db).await?;
        refresh_session(&st, &req);
        notify_profile(&st, json!({ "user_id": s.id, "user": s.user, "description": clean })).await
    }
    .await;
    match r {
        Ok(()) => j(json!({ "success": true, "description": clean }), 200),
        Err(e) => logged_500("Update Description", e, "Error updating description"),
    }
}

async fn font(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    // F-023: the font is rendered into a CSS url() in header.html, so it must be a real file name
    let font = body.get("font").filter(|v| crate::config::truthy(v)).and_then(|v| js_str(Some(v)));
    if let Some(f) = &font {
        // Built-in or uploaded font file (crate::fonts)
        if !crate::fonts::exists(&st.cfg(), f) {
            return fail("Invalid font selection", 400);
        }
    }
    // Optional weight (families with several weights), kept only when the family has it
    let weight = font.as_deref().and_then(|f| crate::fonts::weight_for(&st.cfg(), f, body.get("weight").and_then(crate::config::as_int)));
    if let Err(e) = sql!("update user_options set font = ", font.clone(), ", font_weight = ", weight.map(i32::from), " where user_id = ", uid(&req)).execute(&st.db).await {
        return logged_500("Update Font", e, "Error updating font");
    }
    refresh_session(&st, &req);
    let css = font.as_deref().map(|f| crate::fonts::user_css_url(f, weight)).unwrap_or_default();
    j(json!({ "success": true, "font": font, "weight": weight, "css": css }), 200)
}

/// `PUT /api/v2/settings/font_size` (`size`: a percent of crate::fonts::SIZES, anything else = 100)
async fn font_size(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let size = crate::fonts::size_for(body.get("size").and_then(crate::config::as_int));
    if let Err(e) = sql!("update user_options set font_size = ", size.map(i32::from), " where user_id = ", uid(&req)).execute(&st.db).await {
        return logged_500("Update Font Size", e, "Error updating text size");
    }
    refresh_session(&st, &req);
    j(json!({ "success": true, "size": size }), 200)
}

async fn me(st: AppState, req: Req) -> Res {
    let row = sql!(
        r#"SELECT u.login, u.user, uo.display_name FROM "user" u LEFT JOIN user_options uo ON uo.user_id = u.id WHERE u.id = "#,
        uid(&req),
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(row) = row else { return j(json!({ "success": false }), 404) };
    let display = row.str("display_name").filter(|d| !d.is_empty());
    j(json!({ "success": true, "login": row.val("login"), "user": row.val("user"), "display_name": display }), 200)
}


async fn ruffle(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let background = post_bool(&body, "ruffle_background");
    // `req.post.ruffle_volume !== undefined ? parseFloat(...) : undefined`
    let volume = match body.get("ruffle_volume") {
        None => None,
        Some(v) => match js_str(Some(v)).and_then(|s| js_parse_float(&s)) {
            Some(f) if (0.0..=1.0).contains(&f) => Some(f),
            _ => return fail("Invalid volume: must be 0-1", 400),
        },
    };
    let mut q = sql!("update user_options set ruffle_background = ", background);
    if let Some(v) = volume {
        q.push(", ruffle_volume = ").bind(v);
    }
    q.push(" where user_id = ").bind(uid(&req));
    if let Err(e) = q.execute(&st.db).await {
        return logged_500("Update Ruffle pref", e, "Error updating preference");
    }
    refresh_session(&st, &req);
    let mut m = Map::new();
    m.insert("success".into(), json!(true));
    if let Some(v) = volume {
        m.insert("ruffle_volume".into(), json!(v));
    }
    m.insert("ruffle_background".into(), json!(background));
    j(Value::Object(m), 200)
}

async fn alternative_infobox(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let v = post_bool(&body, "use_alternative_infobox");
    let s = req.session().cloned().unwrap_or_default();
    let cfg = st.cfg();
    let mode = s.mode.filter(|m| *m != 0).unwrap_or(0) as i32;
    let theme = s
        .theme
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| cfg.str("websrv.theme").filter(|t| !t.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "amoled".into());
    let r = sql!(
        "insert into user_options (user_id, use_alternative_infobox, mode, theme) values (", s.id as i32, ", ", v, ", ", mode, ", ", theme,
        ") on conflict (user_id) do update set use_alternative_infobox = excluded.use_alternative_infobox"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        return logged_500("Update alternative_infobox", e, "Error updating preference");
    }
    refresh_session(&st, &req);
    j(json!({ "success": true, "use_alternative_infobox": v }), 200)
}

async fn language(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if cfg.is_false("websrv.allow_language_change") {
        return fail("Language change is disabled by the site administrator", 403);
    }
    let body = req.post_value().await;
    // NULL means "use site default"; only known locale codes
    let lang = match body.get("language") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.is_empty() => None,
        Some(Value::String(s)) if crate::i18n::is_selectable(&cfg, s) => Some(s.clone()),
        Some(_) => return fail("Unsupported language", 400),
    };
    if let Err(e) = sql!("update user_options set language = ", lang.clone(), " where user_id = ", uid(&req)).execute(&st.db).await {
        return logged_500("Update language", e, "Error updating preference");
    }
    refresh_session(&st, &req);
    j(json!({ "success": true, "language": lang }), 200)
}

async fn comment_display_mode(st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let Some(mode) = post_int(&body, "mode").filter(|m| *m == 0 || *m == 1) else { return fail("Invalid mode", 400) };
    let forced = sql!("select force_comment_display_mode from user_options where user_id = ", uid(&req)).fetch_optional(&st.db).await?;
    if forced.is_some_and(|r| r.bool("force_comment_display_mode")) {
        return fail("Comment layout is locked for your account.", 403);
    }
    if let Err(e) = sql!("update user_options set comment_display_mode = ", mode as i32, " where user_id = ", uid(&req)).execute(&st.db).await {
        return logged_500("Update comment_display_mode", e, "Error updating preference");
    }
    refresh_session(&st, &req);
    j(json!({ "success": true, "mode": mode }), 200)
}

async fn notifications(st: AppState, mut req: Req) -> Res {
    const KEYS: &[&str] = &["receive_system_notifications", "receive_user_notifications", "do_not_disturb"];
    let body = req.post_value().await;
    let Some(key) = body.get("key").and_then(Value::as_str).and_then(|k| KEYS.iter().find(|x| **x == k)).copied() else {
        return fail("Invalid preference key", 400);
    };
    let v = post_bool(&body, "value");
    let s = req.session().cloned().unwrap_or_default();
    let r = async {
        // `key` is one of the literals above
        let mut q = sql!("update user_options set ");
        q.push(key).push(" = ").bind(v).push(" where user_id = ").bind(s.id as i32);
        q.execute(&st.db).await?;
        refresh_session(&st, &req);
        let mut payload = Map::new();
        payload.insert("user_id".into(), json!(s.id));
        payload.insert(key.into(), json!(v));
        notify_profile(&st, Value::Object(payload)).await
    }
    .await;
    if let Err(e) = r {
        return logged_500(&format!("Update notification preference ({key})"), e, "Error updating preference");
    }
    let mut m = Map::new();
    m.insert("success".into(), json!(true));
    m.insert(key.into(), json!(v));
    j(Value::Object(m), 200)
}

// ── upload API keys ─────────────────────────────────────────────────────────

fn api_keys_disabled(st: &AppState) -> bool {
    st.cfg().is_false("websrv.enable_user_api_keys")
}

async fn api_key_get(st: AppState, req: Req) -> Res {
    if api_keys_disabled(&st) {
        return fail("API keys are disabled", 403);
    }
    let row = match sql!("SELECT api_key, created_at FROM user_api_keys WHERE user_id = ", uid(&req), " LIMIT 1").fetch_optional(&st.db).await {
        Ok(r) => r,
        Err(e) => return logged_500("[API KEY] GET", e, "Error fetching API key"),
    };
    let Some(row) = row else { return j(json!({ "success": true, "has_key": false }), 200) };
    let key = row.string("api_key");
    let tail: String = key.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
    j(json!({ "success": true, "has_key": true, "preview": format!("****{tail}"), "created_at": row.val("created_at") }), 200)
}

async fn api_key_regenerate(st: AppState, req: Req) -> Res {
    if api_keys_disabled(&st) {
        return fail("API keys are disabled", 403);
    }
    let key = crate::util::random_hex(32);
    let r = sql!(
        "INSERT INTO user_api_keys (user_id, api_key, created_at) VALUES (", uid(&req), ", ", key.clone(),
        ", now()) ON CONFLICT (user_id) DO UPDATE SET api_key = EXCLUDED.api_key, created_at = now()"
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        return logged_500("[API KEY] Regenerate", e, "Error generating API key");
    }
    j(json!({ "success": true, "api_key": key, "msg": "API key generated. Copy it now — it will not be shown again in full." }), 200)
}

async fn api_key_delete(st: AppState, req: Req) -> Res {
    if api_keys_disabled(&st) {
        return fail("API keys are disabled", 403);
    }
    match sql!("DELETE FROM user_api_keys WHERE user_id = ", uid(&req), " RETURNING user_id").fetch_all(&st.db).await {
        Ok(rows) if rows.is_empty() => fail("No API key to revoke", 404),
        Ok(_) => j(json!({ "success": true, "msg": "API key revoked" }), 200),
        Err(e) => logged_500("[API KEY] Delete", e, "Error revoking API key"),
    }
}

async fn sharex_config(st: AppState, req: Req) -> Res {
    if api_keys_disabled(&st) {
        return Ok(html_code("API keys are disabled", 403));
    }
    let row = match sql!("SELECT api_key FROM user_api_keys WHERE user_id = ", uid(&req), " LIMIT 1").fetch_optional(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[API KEY] ShareX config error: {e}");
            return Ok(html_code("Error generating config", 500));
        }
    };
    let Some(row) = row else { return Ok(html_code("No API key — generate one first in Settings.", 404)) };
    let cfg = st.cfg();
    let domain = cfg.str_or("main.url.domain", "").to_string();
    let full = cfg.str_or("main.url.full", "").to_string();
    let sxcu = json!({
        "Version": "15.0.0",
        "Name": domain,
        "DestinationType": "ImageUploader, FileUploader",
        "RequestMethod": "POST",
        "RequestURL": format!("{full}/api/v2/upload"),
        "Headers": { "X-Api-Key": row.val("api_key") },
        "Body": "MultipartFormData",
        "FileFormName": "file",
        // The server requires a rating field; users can change it in ShareX.
        "Parameters": { "rating": "sfw" },
        "URL": "{json:file_url}",
        "ThumbnailURL": "{json:file_url}",
        "ErrorMessage": "{response}"
    });
    let body = serde_json::to_string_pretty(&sxcu)?;
    // The domain comes from the config; keep the header value to a safe file name anyway.
    let fname: String = domain.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':')).collect();
    let r = reply(200, "application/json", body);
    Ok(with_header(r, "content-disposition", &format!("attachment; filename=\"{fname}.sxcu\"")))
}

// ── user invites ────────────────────────────────────────────────────────────

/// `Number.isFinite(+x) ? +x : dflt` for one invite criterion.
fn criterion(ic: &Value, k: &str, dflt: f64) -> f64 {
    let n = match ic.get(k) {
        None => f64::NAN,
        Some(Value::Null) => 0.0,
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(s)) => crate::social::helpers::js_number(s).unwrap_or(f64::NAN),
        Some(Value::Array(a)) if a.is_empty() => 0.0,
        Some(Value::Array(a)) if a.len() == 1 => js_str(a.first()).and_then(|s| crate::social::helpers::js_number(&s)).unwrap_or(f64::NAN),
        Some(_) => f64::NAN,
    };
    if n.is_finite() { n } else { dflt }
}

/// A number as JSON the way `JSON.stringify` writes it (integers without `.0`).
fn js_json_num(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() < 9e15 { json!(f as i64) } else { json!(f) }
}

struct Criteria {
    uploads: f64,
    age_days: f64,
    comments: f64,
    tags: f64,
}

fn invite_criteria(st: &AppState) -> Criteria {
    let cfg = st.cfg();
    let ic = cfg.get("websrv.invite_criteria");
    let ic = if crate::config::truthy(ic) { ic.clone() } else { json!({}) };
    Criteria {
        uploads: criterion(&ic, "uploads", 150.0),
        age_days: criterion(&ic, "age_days", 30.0),
        comments: criterion(&ic, "comments", 66.0),
        tags: criterion(&ic, "tags", 200.0),
    }
}

fn invite_slots(st: &AppState) -> i64 {
    let cfg = st.cfg();
    js_str(Some(cfg.get("websrv.user_invite_slots"))).and_then(|s| js_parse_int(&s)).filter(|n| *n > 0).unwrap_or(2)
}

fn invites_disabled(st: &AppState) -> bool {
    st.cfg().is_false("websrv.enable_user_invites")
}

struct Stats {
    uploads: f64,
    age_days: f64,
    comments: f64,
    tags: f64,
}

async fn invite_stats(st: &AppState, user_id: i32, username: &str) -> anyhow::Result<Stats> {
    let row = sql!(
        "SELECT (SELECT COUNT(*) FROM items WHERE username = ", username, " AND active = true AND is_deleted = false)::int AS upload_count,
          EXTRACT(EPOCH FROM (NOW() - u.created_at)) / 86400 AS age_days,
          (SELECT COUNT(*) FROM comments WHERE user_id = ", user_id, " AND is_deleted = false)::int AS comment_count,
          (SELECT COUNT(*) FROM tags_assign WHERE user_id = ", user_id, r#")::int AS tag_count
        FROM "user" u WHERE u.id = "#, user_id
    )
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("user {user_id} not found"))?;
    let f = |k: &str| match row.val(k) {
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        Value::String(s) => s.parse::<f64>().unwrap_or(0.0),
        _ => 0.0,
    };
    Ok(Stats { uploads: f("upload_count"), age_days: f("age_days"), comments: f("comment_count"), tags: f("tag_count") })
}

async fn invites_get(st: AppState, req: Req) -> Res {
    if invites_disabled(&st) {
        return fail("Invite system is disabled", 403);
    }
    let r: anyhow::Result<Value> = async {
        let s = req.session().cloned().unwrap_or_default();
        let user_id = s.id as i32;
        let total_slots = invite_slots(&st);
        let refresh_days = 30;
        let is_admin = s.admin;
        let tokens = sql!(
            r#"SELECT it.id, it.token, it.is_used, it.used_at, it.created_at, u.user AS used_by_name
            FROM invite_tokens it LEFT JOIN "user" u ON u.id = it.used_by
            WHERE it.created_by = "#,
            user_id,
            " ORDER BY it.created_at DESC"
        )
        .fetch_all(&st.db)
        .await?;

        let (eligible, criteria, consumed, available) = if is_admin {
            // Admins bypass all criteria and slot limits
            (true, Value::Null, 0, 0)
        } else {
            let stats = invite_stats(&st, user_id, &s.user).await?;
            let c = invite_criteria(&st);
            let entry = |cur: Value, cur_n: f64, req: f64| json!({ "current": cur, "required": js_json_num(req), "met": cur_n >= req });
            let criteria = json!({
                "uploads": entry(js_json_num(stats.uploads), stats.uploads, c.uploads),
                "age_days": entry(js_json_num(stats.age_days.floor()), stats.age_days, c.age_days),
                "comments": entry(js_json_num(stats.comments), stats.comments, c.comments),
                "tags": entry(js_json_num(stats.tags), stats.tags, c.tags),
            });
            let eligible = criteria.as_object().map(|o| o.values().all(|v| v["met"] == json!(true))).unwrap_or(false);
            // Slots consumed = tokens used within the last 30 days
            let cutoff = chrono::Utc::now() - chrono::Duration::days(refresh_days);
            let consumed = tokens
                .iter()
                .filter(|t| t.bool("is_used"))
                .filter_map(|t| t.str("used_at").and_then(|u| chrono::DateTime::parse_from_rfc3339(u).ok()))
                .filter(|u| *u > cutoff)
                .count() as i64;
            (eligible, criteria, consumed, (total_slots - consumed).max(0))
        };
        let nul = |v: i64| if is_admin { Value::Null } else { json!(v) };
        Ok(json!({
            "success": true,
            "is_admin": is_admin,
            "eligible": eligible,
            "criteria": criteria,
            "tokens": rows_value(tokens),
            "slots_total": nul(total_slots),
            "slots_consumed": nul(consumed),
            "slots_available": nul(available),
            "refresh_days": refresh_days,
            "expire_days": crate::routes::register::INVITE_TOKEN_TTL_SECS / 86400,
        }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => logged_500("[INVITES] GET", e, "Error fetching invite data"),
    }
}

async fn invites_create(st: AppState, req: Req) -> Res {
    if invites_disabled(&st) {
        return fail("Invite system is disabled", 403);
    }
    let r: anyhow::Result<Res> = async {
        let s = req.session().cloned().unwrap_or_default();
        let user_id = s.id as i32;
        if !s.admin {
            let stats = invite_stats(&st, user_id, &s.user).await?;
            let c = invite_criteria(&st);
            let eligible = stats.uploads >= c.uploads && stats.age_days >= c.age_days && stats.comments >= c.comments && stats.tags >= c.tags;
            if !eligible {
                return Ok(fail("You do not meet the eligibility criteria", 403));
            }
            let consumed: i64 = sql!(
                "SELECT COUNT(*)::int AS n FROM invite_tokens WHERE created_by = ", user_id,
                " AND is_used = true AND used_at > now() - interval '30 days'"
            )
            .fetch_optional(&st.db)
            .await?
            .and_then(|r| r.i64("n"))
            .unwrap_or(0);
            if consumed >= invite_slots(&st) {
                return Ok(fail("No invite slots available. Slots refresh 30 days after use.", 403));
            }
        }
        let token = crate::util::random_hex(16).to_uppercase();
        sql!("INSERT INTO invite_tokens (token, created_at, created_by) VALUES (", token.clone(), ", ", crate::util::now_secs(), ", ", user_id, ")")
            .execute(&st.db)
            .await?;
        tracing::info!("[INVITES] User {} ({}) generated an invite token", s.user, user_id);
        Ok(j(json!({ "success": true, "token": token }), 200))
    }
    .await;
    match r {
        Ok(res) => res,
        Err(e) => logged_500("[INVITES] Create", e, "Error creating invite token"),
    }
}

async fn invites_delete(st: AppState, mut req: Req) -> Res {
    if invites_disabled(&st) {
        return fail("Invite system is disabled", 403);
    }
    let body = req.post_value().await;
    if !crate::config::truthy(body.get("id").unwrap_or(&Value::Null)) {
        return fail("Missing token ID", 400);
    }
    // `+id`: NaN made the old query fail (500); it can't match a token either way
    let id = js_str(body.get("id")).and_then(|s| crate::social::helpers::js_number(&s)).filter(|f| f.fract() == 0.0 && f.abs() <= i32::MAX as f64);
    let Some(id) = id else { return fail("Token not found or already used", 404) };
    let r = sql!(
        "DELETE FROM invite_tokens WHERE id = ", id as i32, " AND created_by = ", uid(&req), " AND is_used = false RETURNING id"
    )
    .fetch_all(&st.db)
    .await;
    match r {
        Ok(rows) if rows.is_empty() => fail("Token not found or already used", 404),
        Ok(_) => j(json!({ "success": true }), 200),
        Err(e) => logged_500("[INVITES] Delete", e, "Error deleting invite token"),
    }
}

// ── invite requests (anonymous users) ───────────────────────────────────────

/// Fingerprint of the anonymous identity behind this session.
fn session_fingerprint(req: &Req) -> Option<String> {
    req.session().and_then(|s| s.fingerprint.clone().or_else(|| s.anon_fingerprint.clone())).filter(|f| !f.is_empty())
}

async fn invite_request(st: AppState, mut req: Req) -> Res {
    let r: anyhow::Result<Res> = async {
        let s = req.session().cloned().unwrap_or_default();
        if !s.is_anon {
            return Ok(fail("You already have a registered account", 400));
        }
        let Some(fp) = session_fingerprint(&req) else { return Ok(fail("No passkey identity found", 400)) };
        let existing = sql!("SELECT id, status FROM invite_requests WHERE fingerprint = ", fp.clone(), " AND status = 'pending' LIMIT 1")
            .fetch_optional(&st.db)
            .await?;
        if existing.is_some() {
            return Ok(fail("You already have a pending invite request", 409));
        }
        let body = req.post_value().await;
        let reason = js_truncate(js_str(body.get("reason")).filter(|r| crate::config::truthy(&json!(r))).unwrap_or_default().trim(), 500);
        // The old code stored an unsalted sha256 of the IP (trivial to reverse for IPv4). Use the
        // keyed IP hash instead, or nothing when no IP hash secret is configured.
        let ip = req.real_ip();
        let cfg = st.cfg();
        let secret = crate::security::ip_hash_secrets(&cfg).current;
        let ip_hash = (!ip.is_empty() && !secret.is_empty()).then(|| crate::security::hash_ip_with(&ip, &secret)[..16].to_string());
        sql!(
            "INSERT INTO invite_requests (user_id, fingerprint, ip_hash, reason) VALUES (", s.id as i32, ", ", fp.clone(), ", ", ip_hash, ", ", reason.clone(), ")"
        )
        .execute(&st.db)
        .await?;

        // Notify all admin users
        let anon_login = [s.login.as_str(), s.user.as_str()].into_iter().find(|x| !x.is_empty()).unwrap_or("anonymous").to_string();
        let admins: Vec<i32> = sql!(r#"SELECT id FROM "user" WHERE admin = true"#).fetch_scalar_list(&st.db).await?;
        if !admins.is_empty() {
            let data = json!({ "username": anon_login, "reason": js_truncate(&reason, 100) });
            sql!(
                "INSERT INTO notifications (user_id, type, reference_id, data) SELECT a, 'invite_request', 0, ", data,
                "::jsonb FROM unnest(", admins, "::int[]) AS a"
            )
            .execute(&st.db)
            .await?;
        }
        tracing::info!("[INVITE-REQ] New request from fp: {}…", fp.chars().take(12).collect::<String>());
        Ok(j(json!({ "success": true, "msg": "Invite request submitted! An admin will review it shortly." }), 200))
    }
    .await;
    match r {
        Ok(res) => res,
        Err(e) => logged_500("[INVITE-REQ] Submit", e, "Failed to submit request"),
    }
}

async fn invite_request_status(st: AppState, req: Req) -> Res {
    let r: anyhow::Result<Value> = async {
        let Some(fp) = session_fingerprint(&req) else { return Ok(json!({ "success": true, "status": null })) };
        let row = sql!(
            "SELECT ir.id, ir.status, ir.created_at, ir.reviewed_at, it.token, it.is_used, it.created_at AS token_created_at FROM invite_requests ir
             LEFT JOIN invite_tokens it ON it.id = ir.token_id WHERE ir.fingerprint = ", fp,
            " ORDER BY ir.created_at DESC LIMIT 1"
        )
        .fetch_optional(&st.db)
        .await?;
        let Some(row) = row else { return Ok(json!({ "success": true, "status": null })) };
        // Auto-revoke if the token was deleted, used or expired but the status still says approved
        let mut status = row.val("status").clone();
        let expired = row
            .i64("token_created_at")
            .is_some_and(|c| c <= crate::util::now_secs() - crate::routes::register::INVITE_TOKEN_TTL_SECS);
        if status == json!("approved") && (!row.bool("token") || row.bool("is_used") || expired) {
            sql!("UPDATE invite_requests SET status = 'revoked' WHERE id = ", row.i64("id").unwrap_or(0) as i32).execute(&st.db).await?;
            status = json!("revoked");
        }
        let mut m = Map::new();
        m.insert("success".into(), json!(true));
        m.insert("status".into(), status.clone());
        m.insert("created_at".into(), row.val("created_at").clone());
        m.insert("reviewed_at".into(), row.val("reviewed_at").clone());
        if status == json!("approved") {
            m.insert("token".into(), row.val("token").clone());
        }
        Ok(Value::Object(m))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => logged_500("[INVITE-REQ] Status check", e, "Failed to check status"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_counts_utf16_units() {
        assert_eq!(js_truncate("abcdef", 3), "abc");
        assert_eq!(js_truncate("a😀b", 2), "a", "a surrogate pair is not split");
        assert_eq!(js_truncate("a😀b", 3), "a😀");
    }

    #[test]
    fn criteria_like_js() {
        let ic = json!({ "uploads": "10", "age_days": null, "comments": "x", "tags": 5.5 });
        assert_eq!(criterion(&ic, "uploads", 150.0), 10.0);
        assert_eq!(criterion(&ic, "age_days", 30.0), 0.0);
        assert_eq!(criterion(&ic, "comments", 66.0), 66.0);
        assert_eq!(criterion(&ic, "tags", 200.0), 5.5);
        assert_eq!(criterion(&ic, "missing", 7.0), 7.0);
        assert_eq!(js_json_num(150.0), json!(150));
        assert_eq!(js_json_num(5.5), json!(5.5));
    }

    #[test]
    fn bools() {
        assert!(post_bool(&json!({ "a": true }), "a"));
        assert!(post_bool(&json!({ "a": "true" }), "a"));
        assert!(!post_bool(&json!({ "a": 1 }), "a"));
        assert!(!post_bool(&json!({}), "a"));
    }
}
