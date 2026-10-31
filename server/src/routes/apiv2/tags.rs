//! Tag assignment, removal and rating (port of src/inc/routes/apiv2/tags.mjs).

use crate::db::RowExt;
use crate::http::{Guard, Group, Req, Res, Router, json_resp};
use crate::session::Session;
use crate::settings as s;
use crate::social::helpers::{audit_log, js_str, log_anon_activity, notify};
use crate::sql;
use crate::state::AppState;
use crate::util::get_tags;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.post_g(r"^\/api\/v2\/tags\/bulk\/?$", Guard::LoggedIn, bulk);
    let mut g = Group::new(r, r"^\/api\/v2\/tags\/(?<postid>\d+)");
    g.get_g(r"$", Guard::LoggedIn, get_item_tags);
    g.post_g(r"$", Guard::LoggedIn, add_tag);
    g.put_g(r"\/cycle-rating$", Guard::LoggedIn, cycle_rating);
    g.put_g(r"\/toggle$", Guard::ModAuth, toggle);
    g.delete_g(r"\/(?<tagname>.*)", Guard::ModAuth, delete_tag);
}

const PROTECTED: [&str; 3] = ["sfw", "nsfw", "nsfl"];

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

fn is_mod_or_admin(s: Option<&Session>) -> bool {
    s.map(|s| s.admin || s.is_moderator).unwrap_or(false)
}

fn session_id(req: &Req) -> i32 {
    req.session().map(|s| s.id as i32).unwrap_or(0)
}

/// `/api/v2/tags/:postid` → the item id (`+req.params.postid`); `None` if it is not a valid int4.
fn postid(req: &Req) -> Option<i32> {
    req.param("postid").filter(|p| !p.is_empty()).and_then(|p| p.parse::<i32>().ok())
}

/// The rating tag id for NSFL (the `nsfl` tag row, else `cfg.nsfl_tag_id || 11517`).
pub async fn nsfl_id(st: &AppState) -> i32 {
    match sql!("SELECT id FROM tags WHERE normalized = 'nsfl' LIMIT 1").fetch_scalar::<i32, _>(&st.db).await {
        Ok(Some(id)) => id,
        _ => match st.cfg().int("nsfl_tag_id") {
            Some(n) if n != 0 => n as i32,
            _ => 11517,
        },
    }
}

/// `album_items.id` for a subf0ck reference (id, order index or slug) of item `postid`.
async fn resolve_album_item(st: &AppState, postid: i32, subf0ck: Option<&str>) -> anyhow::Result<Option<i32>> {
    let Some(sub) = subf0ck.filter(|s| !s.is_empty()) else { return Ok(None) };
    let mut id: Option<i32> = None;
    if let Some(n) = js_int(sub).filter(|&n| n > 0).and_then(|n| i32::try_from(n).ok()) {
        id = sql!(
            "SELECT id FROM album_items WHERE (id = ", n, " OR (item_id = ", postid, " AND order_index = ", n, ")) AND item_id = ", postid, " LIMIT 1"
        )
        .fetch_scalar(&st.db)
        .await?;
    }
    if id.is_none() {
        id = sql!("SELECT id FROM album_items WHERE slug = ", sub, " AND item_id = ", postid, " LIMIT 1").fetch_scalar(&st.db).await?;
    }
    Ok(id)
}

/// `Number.isInteger(+s)` → the integer.
fn js_int(s: &str) -> Option<i64> {
    let f = crate::social::helpers::js_number(s)?;
    (f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15).then_some(f as i64)
}

/// First present value among body keys (`req.post?.a || req.body?.a || ...`), JS-truthy only.
fn first_truthy(body: &Value, keys: &[&str]) -> Option<Value> {
    keys.iter().filter_map(|k| body.get(*k)).find(|v| crate::config::truthy(v)).cloned()
}

/// `lib.getTags(...)`, errors logged and mapped to an empty list.
async fn tags_of(st: &AppState, postid: i32, session: Option<&Session>, subf0ck: Option<&str>) -> Vec<Value> {
    match get_tags(st, postid as i64, session, subf0ck).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[TAGS] getTags failed for {postid}: {e}");
            Vec::new()
        }
    }
}

/// Fresh tags for each item → `pg_notify('tags', ...)`, in the background.
fn notify_items(st: &AppState, ids: Vec<i32>, blur: bool) {
    let st = st.clone();
    tokio::spawn(async move {
        let cfg = st.cfg();
        for id in ids {
            if blur {
                ensure_blur(&st, &cfg, id).await;
            }
            let tags = tags_of(&st, id, None, None).await;
            notify(&st, "tags", &json!({ "item_id": id, "fresh": true, "tags": tags })).await;
        }
    });
}

/// Generate `t/<id>_blur.webp` if it does not exist yet.
async fn ensure_blur(_st: &AppState, cfg: &crate::config::Config, id: i32) {
    let p = cfg.paths.t.join(format!("{id}_blur.webp"));
    if tokio::fs::metadata(&p).await.is_err() {
        crate::media::queue::gen_blurred_thumbnail(cfg, id as i64, false).await;
    }
}

/// Tags from the body: an array, or a newline/comma separated string.
fn raw_tags(body: &Value) -> Option<Vec<Value>> {
    match first_truthy(body, &["tags"])? {
        Value::String(s) => Some(s.split(['\n', ',']).map(str::trim).filter(|t| !t.is_empty()).map(|t| json!(t)).collect()),
        Value::Array(a) => Some(a),
        _ => None,
    }
}

/// Whether this session may read/tag item `postid`. Missing items pass (the old behaviour: GET
/// answers an empty list, POST fails on the foreign key). Private (2) and admin-only (3) uploads,
/// pending and deleted items are limited to their owner and moderators — the old routes exposed
/// and changed their tags for every logged-in user. Unlisted (1) items stay taggable, their item
/// page is reachable by link.
async fn may_access_item(st: &AppState, session: Option<&Session>, postid: i32) -> anyhow::Result<bool> {
    let Some(item) = sql!("SELECT username, active, is_deleted, COALESCE(visibility, 0) AS visibility FROM items WHERE id = ", postid, " LIMIT 1")
        .fetch_optional(&st.db)
        .await?
    else {
        return Ok(true);
    };
    let visibility = item.i64("visibility").unwrap_or(0);
    let public = item.bool("active") && !item.bool("is_deleted") && visibility <= 1;
    if public {
        return Ok(true);
    }
    let Some(s) = session else { return Ok(false) };
    if visibility == 3 {
        return Ok(s.admin);
    }
    if s.admin || s.is_moderator {
        return Ok(true);
    }
    let owner = s::session_owner_name(session).map(|o| o.to_lowercase());
    let item_user = item.str("username").unwrap_or("").to_lowercase();
    Ok(!item_user.is_empty() && owner.as_deref() == Some(item_user.as_str()) && !item.bool("is_deleted"))
}

fn item_not_found() -> Res {
    j(json!({ "success": false, "msg": "Item not found" }), 404)
}

// ── POST /api/v2/tags/bulk ──────────────────────────────────────────────────

async fn bulk(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !is_mod_or_admin(req.session()) {
        return j(json!({ "success": false, "msg": "Bulk selection is only available to administrators and moderators" }), 403);
    }
    let body = req.post().await?.clone();
    let action = first_truthy(&body, &["action"]).and_then(|v| js_str(Some(&v))).unwrap_or_else(|| "add".into());
    let raw_ids = match first_truthy(&body, &["item_ids"]) {
        Some(Value::Array(a)) if !a.is_empty() => a,
        _ => return j(json!({ "success": false, "msg": "No items selected" }), 400),
    };
    let mut item_ids: Vec<i32> = Vec::new();
    for v in &raw_ids {
        let n = match v {
            Value::Number(n) => n.as_f64(),
            Value::String(s) => crate::social::helpers::js_number(s),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        };
        if let Some(id) = n.filter(|f| f.fract() == 0.0 && *f > 0.0 && *f <= i32::MAX as f64).map(|f| f as i32) {
            if !item_ids.contains(&id) {
                item_ids.push(id);
            }
        }
    }
    item_ids.truncate(500);
    if item_ids.is_empty() {
        return j(json!({ "success": false, "msg": "Invalid item IDs" }), 400);
    }
    let uid = session_id(&req);

    match action.as_str() {
        "add" => {
            if !s::check_anon_permission(&cfg, req.session(), "tag") {
                return j(json!({ "success": false, "msg": "Anonymous tagging is disabled" }), 403);
            }
            let Some(raw) = raw_tags(&body).filter(|t| !t.is_empty()) else {
                return j(json!({ "success": false, "msg": "No tags provided" }), 400);
            };
            let mut valid: Vec<String> = Vec::new();
            for t in &raw {
                let t = t.as_str().map(str::trim).unwrap_or("");
                if !t.is_empty() && t.encode_utf16().count() <= 85 && !PROTECTED.contains(&t.to_lowercase().as_str()) && !valid.iter().any(|v| v == t) {
                    valid.push(t.to_string());
                }
            }
            if valid.is_empty() {
                return j(json!({ "success": false, "msg": "Tags invalid or contain only reserved names" }), 400);
            }
            let res: anyhow::Result<()> = async {
                let mut tag_ids: Vec<i32> = Vec::new();
                for name in &valid {
                    if let Some(id) = find_or_create_tag(&st, name).await? {
                        tag_ids.push(id);
                    }
                }
                for &item in &item_ids {
                    for &tag in &tag_ids {
                        let r = sql!(r#"INSERT INTO "tags_assign" (tag_id, item_id, user_id) VALUES ("#, tag, ", ", item, ", ", uid, ") ON CONFLICT DO NOTHING")
                            .execute(&st.db)
                            .await;
                        if let Err(e) = r {
                            tracing::warn!("[BULK_TAG_ASSIGN_WARN] Item {item}, tag {tag}: {e}");
                        }
                    }
                }
                Ok(())
            }
            .await;
            if let Err(e) = res {
                tracing::error!("[BULK_ADD_TAGS_ERROR] {e}");
                return j(json!({ "success": false, "msg": "Failed to add tags" }), 500);
            }
            audit_null_target(&st, uid, "bulk_add_tags", Some(json!({ "item_ids": item_ids, "tags": valid }))).await;
            if req.session().map(|s| s.is_anon).unwrap_or(false) {
                log_anon_activity(&st, &req, "bulk_tag", Some(item_ids[0] as i64), Some(json!({ "item_ids": item_ids, "tags": valid }))).await;
            }
            notify_items(&st, item_ids.clone(), false);
            j(
                json!({
                    "success": true,
                    "count": item_ids.len(),
                    "tags": valid,
                    "msg": format!("Successfully added {} tag(s) to {} item(s)", valid.len(), item_ids.len()),
                }),
                200,
            )
        }
        "remove" => {
            let Some(raw) = raw_tags(&body).filter(|t| !t.is_empty()) else {
                return j(json!({ "success": false, "msg": "No tags provided to remove" }), 400);
            };
            let names: Vec<String> = raw.iter().filter_map(Value::as_str).map(|t| t.trim().to_lowercase()).collect();
            let res: anyhow::Result<Option<Vec<String>>> = async {
                let rows = sql!(r#"SELECT id, tag, normalized FROM "tags" WHERE normalized = ANY("#, names, ")").fetch_all(&st.db).await?;
                if rows.is_empty() {
                    return Ok(None);
                }
                let tag_ids: Vec<i32> = rows.iter().filter_map(|r| r.i64("id")).map(|i| i as i32).collect();
                // Only moderators/admins get here (checked above), so no ownership filter is needed.
                sql!(r#"DELETE FROM "tags_assign" WHERE item_id = ANY("#, item_ids.clone(), ") AND tag_id = ANY(", tag_ids, ")").execute(&st.db).await?;
                Ok(Some(rows.iter().map(|r| r.string("tag")).collect()))
            }
            .await;
            let removed = match res {
                Ok(Some(t)) => t,
                Ok(None) => return j(json!({ "success": false, "msg": "Tags not found" }), 404),
                Err(e) => {
                    tracing::error!("[BULK_REMOVE_TAGS_ERROR] {e}");
                    return j(json!({ "success": false, "msg": "Failed to remove tags" }), 500);
                }
            };
            audit_null_target(&st, uid, "bulk_remove_tags", Some(json!({ "item_ids": item_ids, "tags": removed }))).await;
            notify_items(&st, item_ids.clone(), false);
            j(
                json!({
                    "success": true,
                    "count": item_ids.len(),
                    "msg": format!("Successfully removed tag(s) from {} item(s)", item_ids.len()),
                }),
                200,
            )
        }
        "set_rating" => {
            if !s::check_anon_permission(&cfg, req.session(), "rate_item") {
                return j(json!({ "success": false, "msg": "Anonymous rating is disabled" }), 403);
            }
            let rating = first_truthy(&body, &["rating"]).and_then(|v| js_str(Some(&v))).unwrap_or_default().to_lowercase();
            if !PROTECTED.contains(&rating.as_str()) {
                return j(json!({ "success": false, "msg": "Invalid rating. Choose sfw, nsfw, or nsfl" }), 400);
            }
            let nsfl = nsfl_id(&st).await;
            let target = match rating.as_str() {
                "sfw" => 1,
                "nsfw" => 2,
                _ => nsfl,
            };
            let res: anyhow::Result<Vec<i32>> = async {
                let eligible = item_ids.clone();
                let mut tx = st.db.begin().await?;
                sql!(
                    "DELETE FROM tags_assign WHERE item_id = ANY(", eligible.clone(), ") AND (tag_id IN (1, 2, ", nsfl,
                    ") OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl')))"
                )
                .execute(&mut *tx)
                .await?;
                for &id in &eligible {
                    sql!("INSERT INTO tags_assign (item_id, tag_id, user_id) VALUES (", id, ", ", target, ", ", uid, ")").execute(&mut *tx).await?;
                }
                tx.commit().await?;
                Ok(eligible)
            }
            .await;
            let eligible = match res {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("[BULK_SET_RATING_ERROR] {e}");
                    return j(json!({ "success": false, "msg": "Failed to update rating" }), 500);
                }
            };
            audit_null_target(&st, uid, "bulk_set_rating", Some(json!({ "item_ids": eligible, "rating": rating }))).await;
            notify_items(&st, eligible.clone(), true);
            j(
                json!({
                    "success": true,
                    "count": eligible.len(),
                    "rating": rating,
                    "msg": format!("Successfully set rating to {} for {} item(s)", rating.to_uppercase(), eligible.len()),
                }),
                200,
            )
        }
        _ => j(json!({ "success": false, "msg": "Unknown bulk action" }), 400),
    }
}

/// `audit.log(id, action, 'items', null, details)` (the shared helper takes a non-null target).
async fn audit_null_target(st: &AppState, uid: i32, action: &str, details: Option<Value>) {
    let details = details.unwrap_or_else(|| json!({}));
    let r = sql!("INSERT INTO audit_log (user_id, action, target_type, target_id, details) VALUES (", uid, ", ", action, ", 'items', NULL, ", details, ")")
        .execute(&st.db)
        .await;
    match r {
        Ok(_) => tracing::info!("[AUDIT] User {uid} performed {action} on items:null"),
        Err(e) => tracing::error!("[AUDIT] Failed to write log: {e}"),
    }
}

/// Existing tag id by normalized name, or a newly created tag.
async fn find_or_create_tag(st: &AppState, name: &str) -> anyhow::Result<Option<i32>> {
    if let Some(id) = sql!(r#"SELECT id FROM "tags" WHERE normalized = slugify("#, name, ") LIMIT 1").fetch_scalar::<i32, _>(&st.db).await? {
        return Ok(Some(id));
    }
    match sql!(r#"INSERT INTO "tags" (tag) VALUES ("#, name, ") RETURNING id").fetch_scalar::<i32, _>(&st.db).await {
        Ok(id) => Ok(id),
        Err(_) => Ok(sql!(r#"SELECT id FROM "tags" WHERE normalized = slugify("#, name, ") LIMIT 1").fetch_scalar::<i32, _>(&st.db).await?),
    }
}

// ── GET /api/v2/tags/:postid ────────────────────────────────────────────────

async fn get_item_tags(st: AppState, req: Req) -> Res {
    let Some(id) = postid(&req) else {
        return j(json!({ "success": false, "msg": "missing postid" }), 200);
    };
    if !may_access_item(&st, req.session(), id).await? {
        return item_not_found();
    }
    let sub = req.query("subf0ck_id").filter(|v| !v.is_empty()).or_else(|| req.query("subf0ck").filter(|v| !v.is_empty()));
    let tags = tags_of(&st, id, req.session(), sub).await;
    j(json!({ "success": true, "tags": tags }), 200)
}

// ── POST /api/v2/tags/:postid ───────────────────────────────────────────────

async fn add_tag(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !s::check_anon_permission(&cfg, req.session(), "tag") {
        return j(json!({ "success": false, "msg": "Anonymous tagging is disabled" }), 403);
    }
    let body = req.post().await?.clone();
    let raw_tagname = first_truthy(&body, &["tagname"]).and_then(|v| js_str(Some(&v)));
    let (Some(postid), Some(raw_tagname)) = (postid(&req), raw_tagname) else {
        return j(json!({ "success": false, "msg": "missing postid or tag" }), 200);
    };
    let tagname = raw_tagname.trim().to_string();
    if PROTECTED.contains(&tagname.to_lowercase().as_str()) {
        return j(json!({ "success": false, "msg": format!("The tag \"{tagname}\" is reserved for system rating modes.") }), 200);
    }
    if tagname.encode_utf16().count() > 85 {
        return j(json!({ "success": false, "msg": "tag is too long!" }), 200);
    }
    // An empty name (only whitespace) would create an unusable tag.
    if tagname.is_empty() {
        return j(json!({ "success": false, "msg": "missing postid or tag" }), 200);
    }
    if !may_access_item(&st, req.session(), postid).await? {
        return item_not_found();
    }

    let sub_raw = first_truthy(&body, &["subf0ck_id", "subf0ck"]);
    let sub = sub_raw.as_ref().and_then(|v| js_str(Some(v)));
    let sub_json = sub_raw.clone().unwrap_or(Value::Null);
    let session = req.session().cloned();
    let sref = session.as_ref();
    let uid = session_id(&req);

    let res: anyhow::Result<bool> = async {
        let album_item = resolve_album_item(&st, postid, sub.as_deref()).await?;
        let nsfl = nsfl_id(&st).await;
        let Some(tagid) = find_or_create_tag(&st, &tagname).await? else { anyhow::bail!("tag could not be created") };
        if let Some(aid) = album_item {
            sql!(r#"INSERT INTO "album_items_tags_assign" (album_item_id, tag_id, user_id) VALUES ("#, aid, ", ", tagid, ", ", uid, ") ON CONFLICT DO NOTHING")
                .execute(&st.db)
                .await?;
            sql!(r#"INSERT INTO "tags_assign" (item_id, tag_id, user_id) VALUES ("#, postid, ", ", tagid, ", ", uid, ") ON CONFLICT DO NOTHING")
                .execute(&st.db)
                .await?;
            // The album item inherits the parent's rating when it has none.
            let sub_rating = sql!("SELECT tag_id FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id IN (1, 2, ", nsfl, ") LIMIT 1")
                .fetch_optional(&st.db)
                .await?;
            if sub_rating.is_none() {
                let parent = sql!("SELECT tag_id FROM tags_assign WHERE item_id = ", postid, " AND tag_id IN (1, 2, ", nsfl, ") LIMIT 1")
                    .fetch_scalar::<i32, _>(&st.db)
                    .await?;
                if let Some(p) = parent {
                    let _ = sql!(
                        "INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", aid, ", ", p, ", ", uid, ") ON CONFLICT DO NOTHING"
                    )
                    .execute(&st.db)
                    .await;
                }
            }
            Ok(true)
        } else {
            let n = sql!(r#"INSERT INTO "tags_assign" (tag_id, item_id, user_id) VALUES ("#, tagid, ", ", postid, ", ", uid, ") ON CONFLICT DO NOTHING")
                .execute(&st.db)
                .await?;
            Ok(n > 0)
        }
    }
    .await;

    let failure = match res {
        Ok(true) => None,
        Ok(false) => Some("Tag already exists"),
        Err(e) => {
            tracing::error!("[ADD_TAG_ERROR] {e}");
            Some("Failed to add tag")
        }
    };
    if let Some(msg) = failure {
        let tags = tags_of(&st, postid, sref, sub.as_deref()).await;
        return j(json!({ "success": false, "msg": msg, "tags": tags }), 200);
    }

    let fresh = tags_of(&st, postid, sref, sub.as_deref()).await;
    if sref.map(|s| s.is_anon).unwrap_or(false) {
        log_anon_activity(&st, &req, "tag", Some(postid as i64), Some(json!({ "tag": tagname, "subf0ck_id": sub_json }))).await;
    }
    tracing::info!("[API] Notifying 'tags' for item {postid} (subf0ck: {}) with {} tags", sub.as_deref().unwrap_or("none"), fresh.len());
    notify(&st, "tags", &json!({ "item_id": postid, "subf0ck_id": sub_json, "fresh": true, "tags": fresh })).await;
    j(json!({ "success": true, "postid": postid, "subf0ck_id": sub_json, "tag": tagname, "tags": fresh }), 200)
}

// ── PUT /api/v2/tags/:postid/cycle-rating ───────────────────────────────────

async fn cycle_rating(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !s::check_anon_permission(&cfg, req.session(), "rate_item") {
        return j(json!({ "success": false, "msg": "Anonymous rating is disabled" }), 403);
    }
    let Some(postid) = postid(&req) else {
        return j(json!({ "success": false, "msg": "missing postid" }), 200);
    };
    let item = sql!("SELECT id, username, active, is_deleted FROM items WHERE id = ", postid, " AND active = true AND is_deleted = false LIMIT 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(item) = item else {
        return j(json!({ "success": false, "msg": "Item not found" }), 404);
    };
    let owner = s::session_owner_name(req.session());
    let item_user = item.str("username").unwrap_or("");
    let is_owner = !item_user.is_empty() && owner.as_deref().map(|o| !o.is_empty() && o.to_lowercase() == item_user.to_lowercase()).unwrap_or(false);
    if !is_owner && !is_mod_or_admin(req.session()) {
        return j(json!({ "success": false, "msg": "Unauthorized" }), 403);
    }

    let nsfl = nsfl_id(&st).await;
    let cycle = [1, 2, nsfl];
    let body = req.post().await?.clone();
    let pick = |k: &str| first_truthy(&body, &[k]).and_then(|v| js_str(Some(&v))).or_else(|| req.query(k).filter(|v| !v.is_empty()).map(str::to_string));
    let sub = pick("subf0ck_id");
    let req_rating = pick("rating");
    let uid = session_id(&req);

    let next_for = |current: i32| -> i32 {
        match req_rating.as_deref() {
            Some("sfw") => 1,
            Some("nsfw") => 2,
            Some("nsfl") => nsfl,
            _ => {
                let idx = cycle.iter().position(|&c| c == current).map(|i| i as i64).unwrap_or(-1);
                cycle[((idx + 1) % 3) as usize]
            }
        }
    };

    let res: anyhow::Result<(i32, i32)> = async {
        let album_item = resolve_album_item(&st, postid, sub.as_deref()).await?;
        let mut tx = st.db.begin().await?;
        sql!("SELECT id FROM items WHERE id = ", postid, " FOR UPDATE").execute(&mut *tx).await?;
        let (from, next);
        if let Some(aid) = album_item {
            from = sql!("SELECT tag_id FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id IN (1, 2, ", nsfl, ") LIMIT 1")
                .fetch_scalar::<i32, _>(&mut *tx)
                .await?
                .unwrap_or(0);
            next = next_for(from);
            sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id IN (1, 2, ", nsfl, ")").execute(&mut *tx).await?;
            if next > 0 {
                sql!("INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", aid, ", ", next, ", ", uid, ")").execute(&mut *tx).await?;
            }
        } else {
            from = sql!(
                "SELECT tag_id FROM tags_assign WHERE item_id = ", postid, " AND (tag_id IN (1, 2, ", nsfl,
                ") OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl'))) ORDER BY tag_id DESC LIMIT 1"
            )
            .fetch_scalar::<i32, _>(&mut *tx)
            .await?
            .unwrap_or(0);
            next = next_for(from);
            sql!(
                "DELETE FROM tags_assign WHERE item_id = ", postid, " AND (tag_id IN (1, 2, ", nsfl,
                ") OR tag_id IN (SELECT id FROM tags WHERE normalized IN ('sfw', 'nsfw', 'nsfl')))"
            )
            .execute(&mut *tx)
            .await?;
            if next > 0 {
                sql!("INSERT INTO tags_assign (item_id, tag_id, user_id) VALUES (", postid, ", ", next, ", ", uid, ")").execute(&mut *tx).await?;
            }
            // Albums: the rating applies to every sub-item
            let is_album = sql!("SELECT id FROM items WHERE id = ", postid, " AND is_album = true LIMIT 1").fetch_optional(&mut *tx).await?.is_some();
            if is_album {
                let subs: Vec<i32> = sql!("SELECT id FROM album_items WHERE item_id = ", postid).fetch_scalar_list(&mut *tx).await?;
                for sid in subs {
                    sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", sid, " AND tag_id IN (1, 2, ", nsfl, ")").execute(&mut *tx).await?;
                    if next > 0 {
                        sql!(
                            "INSERT INTO album_items_tags_assign (album_item_id, tag_id, user_id) VALUES (", sid, ", ", next, ", ", uid, ") ON CONFLICT DO NOTHING"
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
        }
        tx.commit().await?;
        Ok((from, next))
    }
    .await;

    let (from, next) = match res {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[CYCLE_RATING_ERROR] {e}");
            return j(json!({ "success": false, "msg": "Failed to update rating" }), 200);
        }
    };
    // The old code generated the blurred thumbnail inside the transaction; doing it after the
    // commit keeps the row lock short.
    ensure_blur(&st, &cfg, postid).await;

    let (label, class) = match next {
        1 => ("SFW", "sfw"),
        2 => ("NSFW", "nsfw"),
        n if n == nsfl => ("NSFL", "nsfl"),
        _ => ("SFW", "sfw"),
    };
    audit_log(&st, uid as i64, "cycle_rating", "item", &postid.to_string(), Some(json!({ "from": from, "to": next }))).await;
    if req.session().map(|s| s.is_anon).unwrap_or(false) {
        log_anon_activity(&st, &req, "cycle_rating", Some(postid as i64), Some(json!({ "from": from, "to": next, "rating_label": label }))).await;
    }
    let fresh = tags_of(&st, postid, None, None).await;
    notify(&st, "tags", &json!({ "item_id": postid, "fresh": true, "tags": fresh })).await;
    j(json!({ "success": true, "rating_tag_id": next, "rating_label": label, "rating_class": class }), 200)
}

// ── PUT /api/v2/tags/:postid/toggle ─────────────────────────────────────────

async fn toggle(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let Some(postid) = postid(&req) else {
        return j(json!({ "success": false, "msg": "missing postid" }), 200);
    };
    let uid = session_id(&req);
    let current = tags_of(&st, postid, None, None).await;
    let has = |id: i64| current.iter().any(|t| t["id"].as_i64() == Some(id));
    let (has_sfw, has_nsfw) = (has(1), has(2));
    let details = if !has_sfw && !has_nsfw {
        sql!(r#"insert into "tags_assign" (item_id, tag_id, user_id) values ("#, postid, ", 1, ", uid, ")").execute(&st.db).await?;
        json!({ "tag": "sfw", "action": "added" })
    } else {
        sql!(r#"update "tags_assign" set tag_id = (array[2,1])[tag_id] where tag_id = any(array[1,2]) and item_id = "#, postid).execute(&st.db).await?;
        if has_nsfw {
            json!({ "tag": "sfw", "from": "nsfw" })
        } else {
            json!({ "tag": "nsfw", "from": "sfw" })
        }
    };
    audit_log(&st, uid as i64, "toggle_tag", "item", &postid.to_string(), Some(details)).await;
    ensure_blur(&st, &cfg, postid).await;
    let fresh = tags_of(&st, postid, None, None).await;
    tracing::info!("[API] Notifying 'tags' (toggle) for item {postid}");
    notify(&st, "tags", &json!({ "item_id": postid, "fresh": true, "tags": fresh })).await;
    j(json!({ "success": true, "tags": fresh }), 200)
}

// ── DELETE /api/v2/tags/:postid/:tagname ────────────────────────────────────

async fn delete_tag(st: AppState, mut req: Req) -> Res {
    let tagname_raw = req.param("tagname").unwrap_or("").to_string();
    let (Some(postid), false) = (postid(&req), tagname_raw.is_empty()) else {
        return j(json!({ "success": false, "msg": "missing postid or tagname" }), 200);
    };
    let tagname = crate::util::decode_lossy(&tagname_raw);
    let body = req.post().await?.clone();
    let sub_raw = first_truthy(&body, &["subf0ck_id"])
        .or_else(|| req.query("subf0ck_id").filter(|v| !v.is_empty()).map(|v| json!(v)))
        .or_else(|| req.query("subf0ck").filter(|v| !v.is_empty()).map(|v| json!(v)));
    let sub = sub_raw.as_ref().and_then(|v| js_str(Some(v)));
    let sub_json = sub_raw.clone().unwrap_or(Value::Null);
    let session = req.session().cloned();
    let sref = session.as_ref();

    let album_item = resolve_album_item(&st, postid, sub.as_deref()).await?;
    let tags = tags_of(&st, postid, sref, sub.as_deref()).await;
    let lower = tagname.to_lowercase();
    let tagid = tags
        .iter()
        .find(|t| t["tag"].as_str() == Some(tagname.as_str()) || t["normalized"].as_str() == Some(lower.as_str()))
        .and_then(|t| t["id"].as_i64())
        .map(|i| i as i32);
    let Some(tagid) = tagid else {
        let tags = tags_of(&st, postid, sref, sub.as_deref()).await;
        return j(json!({ "success": false, "msg": "tag is not assigned", "tags": tags }), 200);
    };

    // postgres.js resolves a DELETE to a (truthy) result array, so the old `reply = !!q` is always true.
    let reply = true;
    if let Some(aid) = album_item {
        sql!("DELETE FROM album_items_tags_assign WHERE album_item_id = ", aid, " AND tag_id = ", tagid).execute(&st.db).await?;
        let other = sql!(
            "SELECT 1 FROM album_items_tags_assign aita JOIN album_items ai ON ai.id = aita.album_item_id WHERE ai.item_id = ", postid,
            " AND aita.tag_id = ", tagid, " LIMIT 1"
        )
        .fetch_optional(&st.db)
        .await?;
        if other.is_none() {
            let _ = sql!("DELETE FROM tags_assign WHERE item_id = ", postid, " AND tag_id = ", tagid).execute(&st.db).await;
        }
    } else {
        sql!(r#"delete from "tags_assign" where tag_id = "#, tagid, " and item_id = ", postid).execute(&st.db).await?;
    }

    let reason = first_truthy(&body, &["reason"])
        .and_then(|v| js_str(Some(&v)))
        .or_else(|| req.query("reason").filter(|v| !v.is_empty()).map(str::to_string))
        .unwrap_or_else(|| "No reason provided".into());
    let uid = session_id(&req);
    audit_log(&st, uid as i64, "delete_tag", "item", &postid.to_string(), Some(json!({ "tag": tagname, "subf0ck_id": sub_json, "reason": reason }))).await;
    if sref.map(|s| s.is_anon).unwrap_or(false) {
        log_anon_activity(&st, &req, "delete_tag", Some(postid as i64), Some(json!({ "tag": tagname, "subf0ck_id": sub_json, "reason": reason }))).await;
    }
    let fresh = tags_of(&st, postid, sref, sub.as_deref()).await;
    tracing::info!("[API] Notifying 'tags' (delete) for item {postid} (subf0ck: {})", sub.as_deref().unwrap_or("none"));
    notify(&st, "tags", &json!({ "item_id": postid, "subf0ck_id": sub_json, "fresh": true, "tags": fresh })).await;
    j(json!({ "success": reply, "tagid": tagid, "subf0ck_id": sub_json, "tags": fresh }), 200)
}
