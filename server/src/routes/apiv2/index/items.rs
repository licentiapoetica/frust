//! Item mutations of apiv2/index.mjs: tag rename, title, delete (post / album entry), favourites,
//! OC flag, visibility, re-thumbnailing, expiry and rating.

use super::{is_staff, j};
use crate::db::{Row, RowExt};
use crate::f0cklib::recommend::spawn_update_user_affinity;
use crate::http::{Guard, Req, Res};
use crate::media::{delete as mdel, is_plain_name, queue};
use crate::private_items as pi;
use crate::session::Session;
use crate::settings as s;
use crate::social::helpers::{audit_log, js_parse_int, js_str, log_anon_activity, notify};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

/// `req.post?.x ?? fallback` for a body value that is present and not null.
fn present(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| !v.is_null())
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

async fn unlink(path: std::path::PathBuf) {
    let _ = tokio::fs::remove_file(path).await;
}

/// Move `<dir>/<name>` to `deleted/<sub>/<name>` (copy + unlink, like the old code).
async fn trash(from: std::path::PathBuf, to: std::path::PathBuf) {
    let _ = tokio::fs::copy(&from, &to).await;
    let _ = tokio::fs::remove_file(&from).await;
}

// ── PUT /tags/rename/:tagname ───────────────────────────────────────────────

pub(super) async fn tags_rename(st: AppState, mut req: Req) -> Res {
    let post = req.post_value().await;
    let param = req.param("tagname").unwrap_or("").to_string();
    let newtag = present(post.get("newtag")).filter(|v| crate::config::truthy(v)).cloned();
    let Some(newtag) = newtag.filter(|_| !param.is_empty()) else {
        return j(json!({ "success": false, "msg": "missing tagname or newtag", "debug": { "params": param, "post": post } }), 400);
    };
    let Some(tagname) = crate::util::decode_uri_component(&param) else {
        return Err(anyhow::anyhow!("URIError: malformed tag name").into());
    };
    let newtag = js_str(Some(&newtag)).unwrap_or_default();
    if ["sfw", "nsfw"].contains(&tagname.as_str()) || ["sfw", "nsfw"].contains(&newtag.as_str()) {
        return j(json!({ "msg": "f0ck you" }), 405);
    }
    if sql!(r#"select * from "tags" where tag = "#, tagname.as_str(), " limit 1").fetch_optional(&st.db).await?.is_none() {
        return j(json!({ "success": false, "msg": "no tag found" }), 404);
    }
    let q = sql!(r#"update "tags" set "tag" = "#, newtag.as_str(), " where tag = ", tagname.as_str(), " returning *").fetch_optional(&st.db).await?;
    j(q.map(Value::Object).unwrap_or(Value::Null), if tagname == newtag { 200 } else { 201 })
}

// ── PATCH /items/:id/title ──────────────────────────────────────────────────

pub(super) async fn set_title(st: AppState, mut req: Req) -> Res {
    let id: i32 = req.param("id").and_then(|i| i.parse().ok()).unwrap_or(0);
    if id == 0 {
        return j(json!({ "success": false, "msg": "Invalid item id" }), 400);
    }
    let post = req.post_value().await;
    let session = req.session().cloned().unwrap_or_default();
    let Some(item) = sql!("SELECT id, username FROM items WHERE id = ", id, " AND active = true LIMIT 1").fetch_optional(&st.db).await? else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    let owner = session.user == item.string("username");
    if !owner && !session.is_admin_or_mod() {
        return j(json!({ "success": false, "msg": "Forbidden" }), 403);
    }
    let title = js_str(post.get("title")).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).map(|t| crate::util::utf16_prefix(&t, 500));
    sql!("UPDATE items SET title = ", title.clone(), " WHERE id = ", id).execute(&st.db).await?;
    j(json!({ "success": true, "title": title }), 200)
}

// ── POST /admin/deletepost ──────────────────────────────────────────────────

/// Tell the uploader that a moderator removed their item (`item_deleted` notification + SSE).
async fn notify_uploader(st: &AppState, username: &str, by: i64, item_id: i32, reason: &str) -> anyhow::Result<()> {
    if username.is_empty() {
        return Ok(());
    }
    let Some(uid) = sql!(r#"SELECT id FROM "user" WHERE login = "#, username, r#" OR "user" = "#, username, " LIMIT 1").fetch_scalar::<i32, _>(&st.db).await? else {
        return Ok(());
    };
    if uid as i64 == by {
        return Ok(());
    }
    let nid = sql!(
        "INSERT INTO notifications (user_id, type, reference_id, item_id, data) VALUES (",
        uid,
        ", 'item_deleted', 0, ",
        item_id,
        ", ",
        json!({ "reason": reason }),
        ") RETURNING id"
    )
    .fetch_scalar::<i32, _>(&st.db)
    .await?;
    if let Some(nid) = nid {
        let payload = json!({ "user_id": uid, "type": "item_deleted", "item_id": item_id, "id": nid, "reason": reason });
        sql!("SELECT pg_notify('notifications', ", crate::util::json_string(&payload), ")").execute(&st.db).await?;
    }
    Ok(())
}

pub(super) async fn delete_post(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session = req.session().cloned().unwrap_or_default();
    let Some(raw) = present(post.get("postid")) else {
        return j(json!({ "success": false, "msg": "no postid" }), 200);
    };
    let raw = js_str(Some(raw)).unwrap_or_default();
    let n = crate::social::helpers::js_number(&raw).unwrap_or(f64::NAN);
    if n < 0.0 {
        return j(json!({ "success": false }), 200);
    }
    let id: Option<i32> = (n.is_finite() && n.fract() == 0.0 && n <= i32::MAX as f64).then_some(n as i32);
    let f0ck = match id {
        Some(id) => sql!(r#"select dest, mime, username from "items" where id = "#, id, " and active = true limit 1").fetch_optional(&st.db).await?,
        None => None,
    };
    let (Some(id), Some(f0ck)) = (id, f0ck) else {
        return j(json!({ "success": false, "msg": format!("f0ck {}: f0ck not found", crate::util::js_num(n)) }), 200);
    };
    sql!(r#"update "items" set active = 'false', is_deleted = true where id = "#, id).execute(&st.db).await?;
    crate::routes::reports::resolve_for_deleted_items(&st, vec![id], Some(session.id as i32)).await;
    let dest = f0ck.string("dest");
    // keeps reposts that symlink to the file working (the old code just copied and unlinked)
    mdel::move_to_deleted(&st, &dest, id as i64).await;
    let thumb = format!("{id}.webp");
    trash(cfg.paths.t.join(&thumb), cfg.paths.deleted.join("t").join(&thumb)).await;

    let reason = js_str(post.get("reason")).filter(|r| !r.is_empty()).unwrap_or_else(|| "No reason provided".into());
    audit_log(&st, session.id, "delete_item", "item", &id.to_string(), Some(json!({ "filename": dest, "reason": reason }))).await;
    notify(&st, "delete_item", &json!({ "id": id })).await;
    if let Err(e) = notify_uploader(&st, &f0ck.string("username"), session.id, id, &reason).await {
        tracing::error!("[DELETEPOST] Failed to notify uploader: {e}");
    }
    if f0ck.string("mime").starts_with("audio") {
        trash(cfg.paths.ca.join(&thumb), cfg.paths.deleted.join("ca").join(&thumb)).await;
    }
    j(json!({ "success": true }), 200)
}

// ── POST /admin/delete-album-item ───────────────────────────────────────────

/// `dest` without its extension (`/\.[^.]+$/`).
fn strip_ext(dest: &str) -> &str {
    match dest.rfind('.') {
        Some(i) if i + 1 < dest.len() => &dest[..i],
        _ => dest,
    }
}

pub(super) async fn delete_album_item(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session = req.session().cloned().unwrap_or_default();
    let postid = js_str(post.get("postid")).and_then(|p| crate::social::helpers::js_number(&p)).unwrap_or(f64::NAN);
    if !(postid.is_finite() && postid > 0.0 && postid.fract() == 0.0 && postid <= i32::MAX as f64) {
        return j(json!({ "success": false, "msg": "Invalid postid" }), 400);
    }
    let postid = postid as i32;
    let Some(item) = sql!(
        "SELECT id, dest, mime, username, is_album, album_count, is_deleted, active FROM items WHERE id = ",
        postid,
        " AND active = true AND is_deleted = false LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?
    else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    if !item.bool("is_album") {
        return j(json!({ "success": false, "msg": "Item is not an album" }), 400);
    }
    let owner = !session.user.is_empty() && !item.string("username").is_empty() && session.user.to_lowercase() == item.string("username").to_lowercase();
    if !session.is_admin_or_mod() && !owner {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 403);
    }
    let sub_id = present(post.get("sub_id")).or_else(|| present(post.get("subf0ck_id"))).and_then(|v| js_str(Some(v))).filter(|v| !v.is_empty());
    let sub_slug = present(post.get("sub_slug")).filter(|v| crate::config::truthy(v)).and_then(|v| js_str(Some(v)));
    let sub_index = present(post.get("order_index")).and_then(|v| js_str(Some(v))).filter(|v| !v.is_empty());

    let all: Vec<Row> = sql!(
        "SELECT id, item_id, dest, mime, size, checksum, order_index, slug FROM album_items WHERE item_id = ",
        postid,
        " ORDER BY order_index ASC"
    )
    .fetch_all(&st.db)
    .await?;
    if all.is_empty() {
        return j(json!({ "success": false, "msg": "No album items found" }), 404);
    }
    let num = |v: &str| crate::social::helpers::js_number(v).filter(|n| n.fract() == 0.0).map(|n| n as i64);
    let mut target: Option<&Row> = None;
    if let Some(sid) = &sub_id {
        target = all.iter().find(|s| (num(sid).is_some() && s.i64("id") == num(sid)) || s.str("slug") == Some(sid.as_str()));
    }
    if target.is_none() {
        if let Some(slug) = &sub_slug {
            target = all.iter().find(|s| s.str("slug") == Some(slug.as_str()));
        }
    }
    if target.is_none() {
        if let Some(idx) = &sub_index {
            target = all.iter().find(|s| num(idx).is_some() && s.i64("order_index") == num(idx));
        }
    }
    let Some(target) = target.cloned() else {
        return j(json!({ "success": false, "msg": "Album sub-item not found" }), 404);
    };
    let tid = target.i64("id").unwrap_or(0) as i32;
    let reason = js_str(post.get("reason")).filter(|r| !r.is_empty()).unwrap_or_else(|| "No reason provided".into());

    if all.len() <= 1 {
        mdel::safe_delete_media_file(&st, &item.string("dest"), postid as i64).await;
        unlink(cfg.paths.t.join(format!("{postid}.webp"))).await;
        unlink(cfg.paths.t.join(format!("{postid}_blur.webp"))).await;
        if item.string("mime").starts_with("audio") {
            unlink(cfg.paths.ca.join(format!("{postid}.webp"))).await;
        }
        let _ = sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", tid).execute(&st.db).await;
        let _ = sql!("DELETE FROM album_items WHERE id = ", tid).execute(&st.db).await;
        sql!("UPDATE items SET active = false, is_deleted = true WHERE id = ", postid).execute(&st.db).await?;
        crate::routes::reports::resolve_for_deleted_items(&st, vec![postid], Some(session.id as i32)).await;
        audit_log(&st, session.id, "delete_item", "item", &postid.to_string(), Some(json!({ "filename": item.string("dest"), "reason": reason }))).await;
        notify(&st, "delete_item", &json!({ "id": postid })).await;
        return j(json!({ "success": true, "post_deleted": true, "msg": "Last album item removed, post deleted" }), 200);
    }

    let sub_dest = target.string("dest");
    if !sub_dest.is_empty() && !sub_dest.starts_with("yt:") {
        mdel::safe_delete_media_file(&st, &sub_dest, postid as i64).await;
        let base = strip_ext(&sub_dest);
        if is_plain_name(base) {
            unlink(cfg.paths.t.join(format!("{base}.webp"))).await;
            if target.string("mime").starts_with("audio") {
                unlink(cfg.paths.ca.join(format!("{base}.webp"))).await;
            }
        }
    } else if sub_dest.starts_with("yt:") {
        unlink(cfg.paths.t.join(format!("{tid}.webp"))).await;
    }
    let _ = sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", tid).execute(&st.db).await;
    sql!("DELETE FROM album_items WHERE id = ", tid).execute(&st.db).await?;

    let remaining: Vec<&Row> = all.iter().filter(|s| s.i64("id") != Some(tid as i64)).collect();
    let order_index = target.i64("order_index").unwrap_or(0);
    let cover_deleted = order_index == 0;
    sql!("UPDATE album_items SET order_index = order_index - 1 WHERE item_id = ", postid, " AND order_index > ", order_index as i32)
        .execute(&st.db)
        .await?;
    let count = remaining.len() as i32;
    if cover_deleted {
        let c = remaining[0];
        sql!(
            "UPDATE items SET dest = ",
            c.string("dest"),
            ", mime = ",
            c.string("mime"),
            ", size = ",
            c.i64("size").unwrap_or(0) as i32,
            ", checksum = ",
            c.string("checksum"),
            ", album_count = ",
            count,
            " WHERE id = ",
            postid
        )
        .execute(&st.db)
        .await?;
        queue::gen_thumbnail(&cfg, &c.string("dest"), &c.string("mime"), postid as i64, "", false, 512).await;
        queue::gen_blurred_thumbnail(&cfg, postid as i64, false).await;
    } else {
        sql!("UPDATE items SET album_count = ", count, " WHERE id = ", postid).execute(&st.db).await?;
    }
    audit_log(
        &st,
        session.id,
        "delete_album_item",
        "album_item",
        &tid.to_string(),
        Some(json!({ "postid": postid, "filename": sub_dest, "order_index": order_index, "reason": reason })),
    )
    .await;
    j(
        json!({
            "success": true, "post_deleted": false, "deleted_sub_id": tid, "deleted_order_index": order_index,
            "remaining_count": count, "promoted_cover": cover_deleted,
        }),
        200,
    )
}

// ── POST /togglefav ─────────────────────────────────────────────────────────

fn anon_fav() -> Value {
    json!({
        "user_id": null, "user": "anonymous", "login": "anonymous", "display_name": "Anonymous", "avatar": null,
        "avatar_file": null, "username_color": null, "hide_fav_badge": true, "is_anon": true,
    })
}

pub(super) async fn toggle_fav(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    if s::is_anon_session(Some(&session)) && !s::can_anon_do(&cfg, "favorite") {
        return j(json!({ "success": false, "msg": "Anonymous favorites are disabled" }), 403);
    }
    let raw = present(post.get("postid")).and_then(|v| js_str(Some(v))).or_else(|| req.query("postid").map(str::to_string));
    let Some(raw) = raw else {
        return j(json!({ "success": false, "msg": "Missing postid" }), 400);
    };
    let numeric = is_numeric(&raw);
    let mut found: Option<i32> = if numeric {
        match raw.parse::<i32>() {
            Ok(n) => sql!("SELECT id FROM items WHERE id = ", n, " AND active = true AND is_deleted = false LIMIT 1").fetch_scalar(&st.db).await?,
            Err(_) => None,
        }
    } else {
        sql!("SELECT id FROM items WHERE slug = ", raw.as_str(), " AND active = true AND is_deleted = false LIMIT 1").fetch_scalar(&st.db).await?
    };
    if found.is_none() {
        let chan_url = present(post.get("chan_url")).or_else(|| present(post.get("url"))).filter(|v| crate::config::truthy(v)).and_then(|v| js_str(Some(v)));
        if let Some(u) = chan_url {
            found = sql!("SELECT id FROM items WHERE src = ", u, " AND active = true AND is_deleted = false LIMIT 1").fetch_scalar(&st.db).await?;
        }
        if found.is_none() && numeric {
            found = sql!(
                "SELECT id FROM items WHERE (src LIKE ",
                format!("%/{raw}.%"),
                " OR src LIKE ",
                format!("%/{raw}s.%"),
                ") AND active = true AND is_deleted = false LIMIT 1"
            )
            .fetch_scalar(&st.db)
            .await?;
        }
    }
    let Some(postid) = found else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    let uid = session.id as i32;
    let existing = sql!(r#"select 1 from "favorites" where item_id = "#, postid, " and user_id = ", uid, " limit 1").fetch_optional(&st.db).await?.is_some();

    let action = present(post.get("action")).and_then(|v| js_str(Some(v)));
    let fav_req = present(post.get("favorited"));
    let should_fav = if matches!(action.as_deref(), Some("add" | "favorite")) || matches!(fav_req, Some(Value::Bool(true))) || fav_req.and_then(Value::as_str) == Some("true") {
        true
    } else if matches!(action.as_deref(), Some("delete" | "remove" | "unfavorite")) || matches!(fav_req, Some(Value::Bool(false))) || fav_req.and_then(Value::as_str) == Some("false") {
        false
    } else {
        !existing
    };
    if !should_fav {
        if existing {
            sql!(r#"delete from "favorites" where user_id = "#, uid, " and item_id = ", postid).execute(&st.db).await?;
            spawn_update_user_affinity(&st, session.id, postid as i64, -5.0);
            if session.is_anon {
                log_anon_activity(&st, &req, "unfavorite", Some(postid as i64), None).await;
            }
        }
    } else if !existing {
        sql!(r#"insert into "favorites" (item_id, user_id) values ("#, postid, ", ", uid, ") on conflict do nothing").execute(&st.db).await?;
        spawn_update_user_affinity(&st, session.id, postid as i64, 5.0);
        if session.is_anon {
            log_anon_activity(&st, &req, "favorite", Some(postid as i64), None).await;
        }
    }
    let favorited = should_fav;

    let favs = sql!(
        r#"select "favorites".user_id, "user".user, "user".login, "user_options".avatar, "user_options".avatar_file, "user_options".display_name, "user_options".username_color, "user_options".hide_fav_badge, "anon_identities".fingerprint as anon_fingerprint
        from "favorites"
        left join "user" on "user".id = "favorites".user_id
        left join "user_options" on "user_options".user_id = "favorites".user_id
        left join "anon_identities" on "anon_identities".user_id = "favorites".user_id
        where "favorites".item_id = "#,
        postid
    )
    .fetch_all(&st.db)
    .await?;
    let sanitized: Vec<Value> = favs
        .iter()
        .map(|f| {
            let user = f.str("user").unwrap_or("");
            if crate::config::truthy(f.val("anon_fingerprint")) || user == "anonymous" || user.starts_with("anon_") {
                anon_fav()
            } else {
                Value::Object(f.clone())
            }
        })
        .collect();
    notify(&st, "favorites", &json!({ "item_id": postid, "user_id": uid, "favorited": favorited, "favs": sanitized })).await;

    let client_favs: Vec<Value> = if s::is_anonymize_session(&cfg, Some(&session)) {
        favs.iter()
            .map(|f| {
                let fid = f.i64("user_id").unwrap_or(0);
                if session.id != 0 && fid != 0 && fid == session.id {
                    return Value::Object(f.clone());
                }
                let mut a = anon_fav();
                a["hide_fav_badge"] = f.val("hide_fav_badge").clone();
                a
            })
            .collect()
    } else {
        sanitized
    };
    j(json!({ "success": true, "itemid": postid, "favorited": favorited, "favs": client_favs }), 200)
}

// ── POST /favorites/import ──────────────────────────────────────────────────

pub(super) async fn favorites_import(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    if s::is_anon_session(Some(&session)) && !s::can_anon_do(&cfg, "favorite") {
        return j(json!({ "success": false, "msg": "Anonymous favorites are disabled" }), 403);
    }
    let num = |s: &str| crate::social::helpers::js_number(s);
    let raw: Vec<f64> = match post.get("ids") {
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| match v {
                Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
                Value::String(s) => num(s).unwrap_or(f64::NAN),
                Value::Null => 0.0,
                Value::Bool(b) => *b as i64 as f64,
                _ => f64::NAN,
            })
            .collect(),
        Some(Value::String(s)) => s.split(',').map(|x| num(x).unwrap_or(f64::NAN)).collect(),
        _ => vec![],
    };
    let ids: Vec<i32> = raw.into_iter().filter(|n| n.fract() == 0.0 && *n > 0.0 && *n <= i32::MAX as f64).map(|n| n as i32).take(500).collect();
    if ids.is_empty() {
        return j(json!({ "success": true, "imported": 0 }), 200);
    }
    let r: anyhow::Result<i64> = async {
        let valid: Vec<i32> = sql!("SELECT id FROM items WHERE id = ANY(", ids, "::int[]) AND active = true AND is_deleted = false").fetch_scalar_list(&st.db).await?;
        let mut count = 0;
        for item_id in valid {
            let inserted = sql!("INSERT INTO favorites (user_id, item_id) VALUES (", session.id as i32, ", ", item_id, ") ON CONFLICT DO NOTHING RETURNING item_id")
                .fetch_optional(&st.db)
                .await?;
            if inserted.is_some() {
                count += 1;
                spawn_update_user_affinity(&st, session.id, item_id as i64, 5.0);
            }
        }
        Ok(count)
    }
    .await;
    match r {
        Ok(count) => {
            if session.is_anon {
                log_anon_activity(&st, &req, "favorites_import", None, Some(json!({ "count": count }))).await;
            }
            j(json!({ "success": true, "imported": count }), 200)
        }
        Err(e) => {
            // the old code echoed the internal error message
            let msg = crate::util::log_error(&e, "[FAVORITES_IMPORT_ERROR]");
            j(json!({ "success": false, "message": msg }), 500)
        }
    }
}

// ── POST /toggle-oc ─────────────────────────────────────────────────────────

pub(super) async fn toggle_oc(st: AppState, mut req: Req) -> Res {
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    let postid = js_str(post.get("postid")).and_then(|p| crate::social::helpers::js_number(&p)).unwrap_or(f64::NAN);
    if !(postid.is_finite() && postid != 0.0) {
        return j(json!({ "success": false, "msg": "No postid provided" }), 400);
    }
    let postid: Option<i32> = (postid.fract() == 0.0 && postid.abs() <= i32::MAX as f64).then_some(postid as i32);
    let item = match postid {
        Some(p) => sql!("SELECT id, username, is_oc FROM items WHERE id = ", p, " AND active = true AND is_deleted = false LIMIT 1").fetch_optional(&st.db).await?,
        None => None,
    };
    let (Some(postid), Some(item)) = (postid, item) else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    if item.string("username") != session.user && !session.is_admin_or_mod() {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 403);
    }
    let old = item.bool("is_oc");
    let new_status = !old;
    let names = ["oc", "original content"];
    let mut tx = st.db.begin().await?;
    sql!("UPDATE items SET is_oc = ", new_status, " WHERE id = ", postid).execute(&mut *tx).await?;
    if new_status {
        for name in names {
            let norm = crate::util::slugify(name);
            let mut tag_id: Option<i32> = sql!("SELECT id FROM tags WHERE normalized = ", norm.as_str()).fetch_scalar(&mut *tx).await?;
            if tag_id.is_none() {
                tag_id = sql!("INSERT INTO tags (tag, normalized) VALUES (", name, ", ", norm.as_str(), ") RETURNING id").fetch_scalar(&mut *tx).await?;
            }
            let tag_id = tag_id.ok_or_else(|| anyhow::anyhow!("tag insert returned nothing"))?;
            sql!("INSERT INTO tags_assign (item_id, tag_id, user_id) VALUES (", postid, ", ", tag_id, ", ", session.id as i32, ") ON CONFLICT DO NOTHING")
                .execute(&mut *tx)
                .await?;
        }
    } else {
        let ids: Vec<i32> = sql!("SELECT id FROM tags WHERE normalized IN (", crate::util::slugify(names[0]), ", ", crate::util::slugify(names[1]), ")")
            .fetch_scalar_list(&mut *tx)
            .await?;
        if !ids.is_empty() {
            sql!("DELETE FROM tags_assign WHERE item_id = ", postid, " AND tag_id = ANY(", ids, ")").execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;
    for name in names {
        if new_status {
            audit_log(&st, session.id, "add_tag", "item", &postid.to_string(), Some(json!({ "tag": name, "auto": true }))).await;
        } else {
            audit_log(&st, session.id, "delete_tag", "item", &postid.to_string(), Some(json!({ "tag": name, "auto": true, "reason": "OC status removed" }))).await;
        }
    }
    audit_log(&st, session.id, "toggle_oc", "item", &postid.to_string(), Some(json!({ "old": old, "new": new_status }))).await;
    let tags = crate::util::get_tags(&st, postid as i64, None, None).await?;
    notify(&st, "tags", &json!({ "item_id": postid, "fresh": true, "tags": tags })).await;
    j(json!({ "success": true, "is_oc": new_status, "tags": tags }), 200)
}

// ── POST /item/visibility, /items/bulk-visibility ───────────────────────────

fn apply_visibility_cache(st: &AppState, id: i64, dest: &str, username: &str, visibility: i64) {
    match visibility {
        2 => {
            pi::add_private(st, id, dest, username);
            pi::remove_unavailable(st, id, dest);
        }
        3 => {
            pi::add_unavailable(st, id, dest, username);
            pi::remove_private(st, id, dest);
        }
        _ => {
            pi::remove_private(st, id, dest);
            pi::remove_unavailable(st, id, dest);
        }
    }
}

fn visibility_param(post: &Value) -> Option<i64> {
    js_str(post.get("visibility")).and_then(|v| js_parse_int(&v)).filter(|v| (0..=3).contains(v))
}

pub(super) async fn set_visibility(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    if cfg.is_false("enable_private_uploads") && !session.admin {
        return j(json!({ "success": false, "msg": "Private uploads feature disabled" }), 403);
    }
    let postid = post.get("postid").filter(|v| crate::config::truthy(v)).or_else(|| post.get("id").filter(|v| crate::config::truthy(v))).and_then(|v| js_str(Some(v)));
    let visibility = visibility_param(&post);
    let (Some(postid), Some(visibility)) = (postid, visibility) else {
        return j(json!({ "success": false, "msg": "Invalid parameters" }), 400);
    };
    let item = if is_numeric(&postid) {
        match postid.parse::<i32>() {
            Ok(n) => sql!("SELECT id, slug, username, visibility, dest FROM items WHERE id = ", n, " AND active = true AND is_deleted = false LIMIT 1").fetch_optional(&st.db).await?,
            Err(_) => None,
        }
    } else {
        sql!("SELECT id, slug, username, visibility, dest FROM items WHERE slug = ", postid.as_str(), " AND active = true AND is_deleted = false LIMIT 1")
            .fetch_optional(&st.db)
            .await?
    };
    let Some(item) = item else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    let staff = session.is_admin_or_mod();
    if item.string("username") != session.user && !staff {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 403);
    }
    if visibility == 3 && !staff {
        return j(json!({ "success": false, "msg": "Only moderators or administrators can make an item unavailable" }), 403);
    }
    let id = item.i64("id").unwrap_or(0);
    sql!("UPDATE items SET visibility = ", visibility as i16, " WHERE id = ", id as i32).execute(&st.db).await?;
    apply_visibility_cache(&st, id, &item.string("dest"), &item.string("username"), visibility);
    crate::f0cklib::clear_count_cache(&st);
    j(json!({ "success": true, "itemid": id, "slug": item.val("slug"), "visibility": visibility }), 200)
}

pub(super) async fn bulk_visibility(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    if !is_staff(Some(&session)) {
        return j(json!({ "success": false, "msg": "Bulk selection is only available to administrators and moderators" }), 403);
    }
    if cfg.is_false("enable_private_uploads") && !session.admin {
        return j(json!({ "success": false, "msg": "Private uploads feature disabled" }), 403);
    }
    let Some(visibility) = visibility_param(&post) else {
        return j(json!({ "success": false, "msg": "Invalid visibility" }), 400);
    };
    let Some(raw) = post.get("item_ids").and_then(Value::as_array).filter(|a| !a.is_empty()) else {
        return j(json!({ "success": false, "msg": "No items selected" }), 400);
    };
    let mut ids: Vec<i32> = vec![];
    for v in raw {
        let n = match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => crate::social::helpers::js_number(s),
            _ => None,
        };
        if let Some(n) = n.filter(|n| n.fract() == 0.0 && *n > 0.0 && *n <= i32::MAX as f64) {
            if !ids.contains(&(n as i32)) {
                ids.push(n as i32);
            }
        }
    }
    ids.truncate(500);
    if ids.is_empty() {
        return j(json!({ "success": false, "msg": "Invalid item IDs" }), 400);
    }
    let r: anyhow::Result<Value> = async {
        let items = sql!("SELECT id, dest, username, COALESCE(visibility, 0) AS visibility FROM items WHERE id = ANY(", ids, "::int[]) AND active = true AND is_deleted = false")
            .fetch_all(&st.db)
            .await?;
        let changed: Vec<&Row> = items.iter().filter(|it| it.i64("visibility") != Some(visibility)).collect();
        if changed.is_empty() {
            return Ok(json!({ "success": true, "updated": 0, "visibility": visibility, "msg": "Nothing to change" }));
        }
        let cids: Vec<i32> = changed.iter().filter_map(|it| it.i64("id")).map(|n| n as i32).collect();
        sql!("UPDATE items SET visibility = ", visibility as i16, " WHERE id = ANY(", cids.clone(), "::int[])").execute(&st.db).await?;
        for it in &changed {
            apply_visibility_cache(&st, it.i64("id").unwrap_or(0), &it.string("dest"), &it.string("username"), visibility);
        }
        crate::f0cklib::clear_count_cache(&st);
        // audit.log(..., 'items', null, ...): target_id NULL
        if let Err(e) = sql!(
            "INSERT INTO audit_log (user_id, action, target_type, target_id, details) VALUES (",
            session.id as i32,
            ", 'bulk_set_visibility', 'items', NULL, ",
            json!({ "item_ids": cids, "visibility": visibility }),
            ")"
        )
        .execute(&st.db)
        .await
        {
            tracing::error!("[AUDIT] Failed to write log: {e}");
        }
        Ok(json!({ "success": true, "updated": cids.len(), "item_ids": cids, "visibility": visibility }))
    }
    .await;
    match r {
        Ok(v) => j(v, 200),
        Err(e) => {
            tracing::error!("[BULK VISIBILITY] {e}");
            j(json!({ "success": false, "msg": "Failed to update visibility" }), 500)
        }
    }
}

// ── POST /items/:id/rethumb ─────────────────────────────────────────────────

async fn owned_item(st: &AppState, req: &Req, cols: &str) -> anyhow::Result<Result<Row, Res>> {
    let id: i32 = req.param("id").and_then(|i| i.parse().ok()).unwrap_or(0);
    if id == 0 {
        return Ok(Err(j(json!({ "success": false, "msg": "No itemid provided" }), 400)));
    }
    let mut q = crate::db::Sql::new("SELECT ");
    q.push(cols).push(" FROM items WHERE id = ").bind(id).push(" AND active = true AND is_deleted = false LIMIT 1");
    let Some(item) = q.fetch_optional(&st.db).await? else {
        return Ok(Err(j(json!({ "success": false, "msg": "Item not found" }), 404)));
    };
    let session = req.session();
    let owner = session.map(|s| s.user == item.string("username")).unwrap_or(false);
    if !owner && !is_staff(session) {
        return Ok(Err(j(json!({ "success": false, "msg": "Unauthorized" }), 403)));
    }
    Ok(Ok(item))
}

pub(super) async fn rethumb(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let item = match owned_item(&st, &req, "id, dest, mime, username, src").await? {
        Ok(i) => i,
        Err(res) => return res,
    };
    let id = item.i64("id").unwrap_or(0);
    let mime = item.string("mime");
    let r = queue::gen_thumbnail(&cfg, &item.string("dest"), &mime, id, &item.string("src"), false, 512).await;
    queue::gen_blurred_thumbnail(&cfg, id, false).await;
    let mut has_coverart = false;
    if mime.starts_with("audio/") {
        has_coverart = r.cover_extracted;
        if !has_coverart {
            has_coverart = tokio::fs::metadata(cfg.paths.ca.join(format!("{id}.webp"))).await.map(|m| m.len() > 0).unwrap_or(false);
        }
        sql!("UPDATE items SET has_coverart = ", has_coverart, " WHERE id = ", id as i32).execute(&st.db).await?;
    }
    if !r.ok {
        return j(json!({ "success": false, "msg": "Thumbnail regeneration failed" }), 500);
    }
    notify(&st, "rethumb", &json!({ "item_id": id, "has_coverart": has_coverart })).await;
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    audit_log(&st, uid, "rethumb_item", "item", &id.to_string(), Some(json!({}))).await;
    j(json!({ "success": true, "itemid": id, "has_coverart": has_coverart }), 200)
}

// ── POST /items/:id/expiry ──────────────────────────────────────────────────

pub(super) async fn set_expiry(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !s::enable_expiring_uploads(&cfg) {
        return j(json!({ "success": false, "msg": "Expiring uploads feature is disabled" }), 403);
    }
    let post = req.post_value().await;
    let item = match owned_item(&st, &req, "id, username, expires_at").await? {
        Ok(i) => i,
        Err(res) => return res,
    };
    let id = item.i64("id").unwrap_or(0);
    let raw = present(post.get("expiry")).or_else(|| present(post.get("expires_at"))).and_then(|v| js_str(Some(v)));
    let now = crate::util::now_secs();
    let target = crate::media::calculate_expires_at(&cfg, raw.as_deref(), now);
    sql!("UPDATE items SET expires_at = ", target, " WHERE id = ", id as i32).execute(&st.db).await?;
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    audit_log(&st, uid, "set_item_expiry", "item", &id.to_string(), Some(json!({ "expires_at": target }))).await;
    let purged = target.is_some_and(|t| t <= now);
    if purged {
        mdel::purge_expired_uploads(&st).await;
    }
    let expires_in = target.and_then(|t| crate::util::expires_in(&json!(t)));
    j(json!({ "success": true, "itemid": id, "expires_at": target, "expires_in": expires_in, "purged": purged }), 200)
}

// ── POST /item/:id/rating ───────────────────────────────────────────────────

pub(super) async fn rating(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    // ratingAuth
    let Some(sess) = req.session() else {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 401);
    };
    if s::is_anon_session(Some(sess)) {
        if !s::can_anon_do(&cfg, "rate_item") {
            return j(json!({ "success": false, "msg": "Anonymous rating is disabled" }), 403);
        }
    } else if let Some(res) = crate::http::check_guard(&st, &mut req, Guard::RegisteredUser).await {
        return Ok(res);
    }
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    let itemid: i32 = req.param("id").and_then(|i| i.parse().ok()).unwrap_or(0);
    if itemid == 0 {
        return j(json!({ "success": false, "msg": "No itemid provided" }), 400);
    }
    let Some(item) = sql!("SELECT id, username, active, is_deleted FROM items WHERE id = ", itemid, " AND active = true AND is_deleted = false LIMIT 1")
        .fetch_optional(&st.db)
        .await?
    else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    let owner_name = s::session_owner_name(Some(&session)).unwrap_or_default();
    let item_user = item.string("username");
    let is_owner = !item_user.is_empty() && !owner_name.is_empty() && item_user.to_lowercase() == owner_name.to_lowercase();
    if !is_owner && !session.is_admin_or_mod() {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 403);
    }
    let nsfl_id: i32 = match sql!("SELECT id FROM tags WHERE normalized = 'nsfl' LIMIT 1").fetch_scalar::<i32, _>(&st.db).await? {
        Some(id) => id,
        None => cfg.int("nsfl_tag_id").filter(|&n| n != 0).unwrap_or(11517) as i32,
    };

    let subf0ck: Value = [post.get("subf0ck_id").cloned(), req.query("subf0ck_id").map(|q| json!(q))]
        .into_iter()
        .flatten()
        .find(crate::config::truthy)
        .unwrap_or(Value::Null);
    let sub_str = js_str(Some(&subf0ck));
    let mut album_item_id: Option<i32> = None;
    if let Some(sub) = &sub_str {
        if let Some(n) = crate::social::helpers::js_number(sub).filter(|n| n.fract() == 0.0 && *n > 0.0 && *n <= i32::MAX as f64) {
            let n = n as i32;
            album_item_id = sql!(
                "SELECT id FROM album_items WHERE (id = ", n, " OR (item_id = ", itemid, " AND order_index = ", n, ")) AND item_id = ", itemid, " LIMIT 1"
            )
            .fetch_scalar(&st.db)
            .await?;
        }
        if album_item_id.is_none() {
            album_item_id = sql!("SELECT id FROM album_items WHERE slug = ", sub.as_str(), " AND item_id = ", itemid, " LIMIT 1").fetch_scalar(&st.db).await?;
        }
    }
    let req_rating = [post.get("rating").cloned(), req.query("rating").map(|q| json!(q))]
        .into_iter()
        .flatten()
        .find(crate::config::truthy)
        .and_then(|v| v.as_str().map(str::to_string));
    let uid = session.id as i32;
    let rating_ids = vec![1, 2, nsfl_id];

    let mut tx = st.db.begin().await?;
    sql!("SELECT id FROM items WHERE id = ", itemid, " FOR UPDATE").execute(&mut *tx).await?;
    let current: Option<i32> = match album_item_id {
        Some(aid) => sql!("SELECT tag_id FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id = ANY(", rating_ids.clone(), "::int[]) LIMIT 1")
            .fetch_scalar(&mut *tx)
            .await?,
        None => sql!(
            "SELECT tag_id FROM tags_assign WHERE item_id = ",
            itemid,
            " AND (tag_id = ANY(",
            rating_ids.clone(),
            "::int[]) OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl'))) ORDER BY tag_id DESC LIMIT 1"
        )
        .fetch_scalar(&mut *tx)
        .await?,
    };
    let new_id: i32 = match req_rating.as_deref() {
        Some("sfw") => 1,
        Some("nsfw") => 2,
        Some("nsfl") => nsfl_id,
        _ => match current {
            Some(1) => 2,
            Some(2) => {
                if cfg.enable_nsfl() {
                    nsfl_id
                } else {
                    1
                }
            }
            _ => 1,
        },
    };
    match album_item_id {
        Some(aid) => {
            sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id = ANY(", rating_ids.clone(), "::int[])").execute(&mut *tx).await?;
            sql!("INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", aid, ", ", new_id, ", ", uid, ")").execute(&mut *tx).await?;
        }
        None => {
            sql!(
                "DELETE FROM tags_assign WHERE item_id = ",
                itemid,
                " AND (tag_id = ANY(",
                rating_ids.clone(),
                "::int[]) OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl')))"
            )
            .execute(&mut *tx)
            .await?;
            sql!("INSERT INTO tags_assign (item_id, tag_id, user_id) VALUES (", itemid, ", ", new_id, ", ", uid, ")").execute(&mut *tx).await?;
            let is_album = sql!("SELECT id FROM items WHERE id = ", itemid, " AND is_album = true LIMIT 1").fetch_optional(&mut *tx).await?.is_some();
            if is_album {
                sql!(
                    "DELETE FROM album_items_tags_assign WHERE album_item_id IN (SELECT id FROM album_items WHERE item_id = ",
                    itemid,
                    ") AND tag_id = ANY(",
                    rating_ids.clone(),
                    "::int[])"
                )
                .execute(&mut *tx)
                .await?;
                sql!(
                    "INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) SELECT id, ",
                    new_id,
                    ", ",
                    uid,
                    " FROM album_items WHERE item_id = ",
                    itemid,
                    " ON CONFLICT DO NOTHING"
                )
                .execute(&mut *tx)
                .await?;
            }
        }
    }
    tx.commit().await?;
    queue::gen_blurred_thumbnail(&cfg, itemid as i64, false).await;

    let label = |id: i32| -> Option<&'static str> {
        match id {
            1 => Some("sfw"),
            2 => Some("nsfw"),
            n if n == nsfl_id => Some("nsfl"),
            _ => None,
        }
    };
    let new_rating = match new_id {
        1 => "sfw",
        2 => "nsfw",
        _ => "nsfl",
    };
    let mut details = serde_json::Map::new();
    match current {
        None => {
            details.insert("old".into(), json!("none"));
        }
        Some(c) => {
            if let Some(l) = label(c) {
                details.insert("old".into(), json!(l));
            }
        }
    }
    details.insert("new".into(), json!(new_rating));
    details.insert("subf0ck_id".into(), subf0ck.clone());
    audit_log(&st, session.id, "update_rating", "item", &itemid.to_string(), Some(Value::Object(details))).await;
    let tags = crate::util::get_tags(&st, itemid as i64, Some(&session), sub_str.as_deref()).await?;
    notify(&st, "tags", &json!({ "item_id": itemid, "subf0ck_id": subf0ck, "fresh": true, "tags": tags })).await;
    j(json!({ "success": true, "itemid": itemid, "subf0ck_id": subf0ck, "rating": new_rating, "tags": tags }), 200)
}
