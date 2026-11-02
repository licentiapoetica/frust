//! Moderator panel: dashboard, approval queue, soft-deleted items, audit log, MOTD, pins and halls
//! (port of src/inc/routes/mod.mjs).
//!
//! Differences to the old code:
//! * Handlers that ended with `res.reply({ success, ... })` sent an empty `text/html` 200 (flummpress
//!   ignores unknown keys); they answer the JSON object now.
//! * Denying a pending upload left its file in `pending/b` (the old code called `moveToDeleted`,
//!   which only looks at the public folder), so the soft-deleted preview was broken. The file is
//!   moved to `deleted/b` now; restore/purge already look there. YouTube items (no file) now get
//!   their thumbnails moved as well.
//! * The `new_item` live event of an approval always had `is_oc: false` (the column wasn't selected).
//! * Pending files are served by basename only, after percent-decoding (like the deleted route).

use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, files, html, json_raw, raw, reply};
use crate::media::{self, delete as mdelete, is_plain_name, queue};
use crate::social::helpers::{audit_log, js_number, js_str, notify};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

pub fn register(r: &mut Router) {
    use crate::http::Guard::{Auth, ModAuth};
    r.get_g(r"^\/mod(\/)?$", ModAuth, dashboard);
    r.get_g(r"^\/mod\/reports\/?$", ModAuth, reports_page);
    r.get_g(r"^\/mod\/approve\/?", ModAuth, approve_queue);
    r.get_g(r"^\/mod\/trash\/?$", ModAuth, trash_page);
    r.post_g(r"^\/mod\/approve\/?", ModAuth, approve);
    r.post_g(r"^\/mod\/deny\/?", ModAuth, deny);
    r.get_g(r"^\/mod\/audit\/?", ModAuth, audit);
    r.post_g(r"^\/mod\/deny-multi\/?", ModAuth, deny_multi);
    r.get_g(r"^\/mod\/pending\/(?<type>[btca])\/(?<file>.+)", ModAuth, pending_file);
    r.get_g(r"^\/mod\/deleted\/(?<type>[bt])\/(?<file>.+)", ModAuth, deleted_file);
    r.post_g(r"^\/mod\/album-item\/deny\/?$", ModAuth, album_item_deny);
    r.post_g(r"^\/mod\/trash\/restore\/?$", ModAuth, trash_restore);
    r.post_g(r"^\/mod\/trash\/purge\/?$", Auth, trash_purge);
    r.post_g(r"^\/mod\/(?:purge-trash-all|trash\/purge-all)\/?$", Auth, purge_all);
    r.get_g(r"^\/mod\/motd\/?$", ModAuth, motd_page);
    r.post_g(r"^\/mod\/motd\/?$", ModAuth, motd_save);
    r.post_g(r"^\/mod\/pin\/?", ModAuth, pin);
    r.post_g(r"^\/mod\/unpin\/?", ModAuth, unpin);
    r.post_g(r"^\/mod\/halls\/add\/?$", ModAuth, halls_add);
    r.post_g(r"^\/mod\/halls\/remove\/?$", ModAuth, halls_remove);
    r.post_g(r"^\/mod\/halls\/update\/?$", ModAuth, halls_update);
    r.get_g(r"^\/mod\/halls\/?$", ModAuth, halls_page);
}

// ── helpers ─────────────────────────────────────────────────────────────────

const QUEUE_PAGE_SIZE: i64 = 20;

fn sid(req: &Req) -> i64 {
    req.session().map(|s| s.id).unwrap_or(0)
}

fn is_admin(req: &Req) -> bool {
    req.session().map(|s| s.admin).unwrap_or(false)
}

/// `X-Requested-With: XMLHttpRequest` or an `Accept` containing `application/json`.
fn wants_json(req: &Req) -> bool {
    req.is_xhr() || req.accepts_json()
}

fn jr(code: u16, v: Value) -> Res {
    Ok(json_raw(&v, code))
}

/// `+(x || 0)` for an id from a body/query value, as an `integer` bind (`None` for 0/NaN/out of range).
fn to_id(v: Option<&Value>) -> Option<i32> {
    let f = match v? {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) => js_number(s)?,
        Value::Bool(true) => 1.0,
        _ => return None,
    };
    (f.fract() == 0.0 && f > 0.0 && f <= i32::MAX as f64).then_some(f as i32)
}

/// `Math.max(1, +req.url.qs.page || 1)` (integral, bounded so the offset can't overflow).
fn queue_page(req: &Req) -> i64 {
    let p = req.query("page").and_then(js_number).filter(|f| f.is_finite() && *f != 0.0).unwrap_or(1.0);
    (p.floor() as i64).clamp(1, 1_000_000)
}

pub fn tag_badge(tag: &str, normalized: &str) -> &'static str {
    static CYR: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new("[а-яё]").unwrap());
    if tag.starts_with('>') {
        "badge-greentext badge-light"
    } else if normalized == "ukraine" {
        "badge-ukraine badge-light"
    } else if CYR.is_match(normalized) || normalized == "russia" {
        "badge-russia badge-light"
    } else if normalized == "german" {
        "badge-german badge-light"
    } else if normalized == "dutch" {
        "badge-dutch badge-light"
    } else if normalized == "sfw" {
        "badge-success"
    } else if normalized == "nsfw" {
        "badge-danger"
    } else {
        "badge-light"
    }
}

/// `processQueueItems`: drop null tags (items without tags), add badge classes.
fn process_queue_item(mut p: Row) -> Row {
    let tags: Vec<Value> = p
        .get("tags")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| t.get("tag").map(|v| !v.is_null()).unwrap_or(false))
        .map(|mut t| {
            let badge = tag_badge(t["tag"].as_str().unwrap_or(""), t["normalized"].as_str().unwrap_or(""));
            t["badge"] = json!(badge);
            t
        })
        .collect();
    p.insert("tags".into(), Value::Array(tags));
    p
}

fn media_kind(mime: &str) -> &'static str {
    if mime == "video/youtube" {
        "youtube"
    } else if mime.starts_with("video") {
        "video"
    } else if mime.starts_with("image") {
        "image"
    } else if mime.starts_with("audio") {
        "audio"
    } else if mime == "application/pdf" {
        "pdf"
    } else {
        "file"
    }
}

/// `dest.replace(/\.[^.]+$/, '')`
fn strip_ext(dest: &str) -> &str {
    match dest.rfind('.') {
        Some(i) if i + 1 < dest.len() => &dest[..i],
        _ => dest,
    }
}

fn iso_from_secs(stamp: i64) -> String {
    chrono::DateTime::from_timestamp(stamp, 0).map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap_or_default()
}

/// Uploader `{ uploader_id, uploader_name }` for the audit log (empty when unknown).
async fn uploader_info(st: &AppState, username: &str) -> Map<String, Value> {
    let mut m = Map::new();
    if let Ok(Some(u)) = sql!(r#"select id, "user" as username from "user" where login = "#, username, r#" or "user" = "#, username, " limit 1")
        .fetch_optional(&st.db)
        .await
    {
        m.insert("uploader_id".into(), u.val("id").clone());
        m.insert("uploader_name".into(), u.val("username").clone());
    }
    m
}

fn details(base: Value, extra: Map<String, Value>) -> Value {
    let mut m = match base {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    m.extend(extra);
    Value::Object(m)
}

async fn exists(p: &Path) -> bool {
    tokio::fs::metadata(p).await.is_ok()
}

/// `moveSafe` of the old code: missing sources are skipped silently.
async fn move_quiet(src: &Path, dst: &Path) {
    if tokio::fs::symlink_metadata(src).await.is_ok() {
        media::move_safe(src, dst).await;
    }
}

async fn unlink(p: PathBuf) {
    let _ = tokio::fs::remove_file(p).await;
}

/// Move an item's media (and album sub-items) from the pending or deleted folder back to the public folders.
async fn move_item_files_to_public(st: &AppState, item: &Row, id: i64) {
    let cfg = st.cfg();
    let dest = item.string("dest");
    let mime = item.string("mime");
    if !is_plain_name(&dest) {
        return;
    }
    let is_youtube = mime == "video/youtube";
    for (label, base) in [("pending", &cfg.paths.pending), ("deleted", &cfg.paths.deleted)] {
        let b = base.join("b").join(&dest);
        let t = base.join("t").join(format!("{id}.webp"));
        let ca = base.join("ca").join(format!("{id}.webp"));
        if !exists(if is_youtube { &t } else { &b }).await {
            continue;
        }
        tracing::info!("[MOD MOVE] Moving files for item {id} from {label}");
        if !is_youtube {
            move_quiet(&b, &cfg.paths.b.join(&dest)).await;
        }
        move_quiet(&t, &cfg.paths.t.join(format!("{id}.webp"))).await;
        move_quiet(&base.join("t").join(format!("{id}_blur.webp")), &cfg.paths.t.join(format!("{id}_blur.webp"))).await;
        if mime.starts_with("audio") {
            move_quiet(&ca, &cfg.paths.ca.join(format!("{id}.webp"))).await;
        }
        if item.bool("is_album") {
            if let Ok(subs) = sql!("SELECT dest FROM album_items WHERE item_id = ", id as i32).fetch_all(&st.db).await {
                for sub in subs {
                    let sd = sub.string("dest");
                    if !is_plain_name(&sd) {
                        continue;
                    }
                    let sb = strip_ext(&sd);
                    move_quiet(&cfg.paths.pending.join("b").join(&sd), &cfg.paths.b.join(&sd)).await;
                    move_quiet(&cfg.paths.pending.join("t").join(format!("{sb}.webp")), &cfg.paths.t.join(format!("{sb}.webp"))).await;
                }
            }
        }
        break;
    }
}

/// Move a live or pending item's files to the deleted folder (deny / soft delete).
async fn move_item_files_to_deleted(st: &AppState, item: &Row, id: i64) {
    let cfg = st.cfg();
    let dest = item.string("dest");
    let mime = item.string("mime");
    if !is_plain_name(&dest) {
        return;
    }
    let is_youtube = mime == "video/youtube";
    let del = &cfg.paths.deleted;
    for (pending, base) in [(true, cfg.paths.pending.clone()), (false, PathBuf::new())] {
        let (b, t_dir, ca_dir) = if pending {
            (base.join("b").join(&dest), base.join("t"), base.join("ca"))
        } else {
            (cfg.paths.b.join(&dest), cfg.paths.t.clone(), cfg.paths.ca.clone())
        };
        let t = t_dir.join(format!("{id}.webp"));
        if !exists(if is_youtube { &t } else { &b }).await {
            continue;
        }
        if !is_youtube {
            if pending {
                move_quiet(&b, &del.join("b").join(&dest)).await;
            } else {
                // Public file: keep reposts that symlink to it working
                mdelete::move_to_deleted(st, &dest, id).await;
            }
        }
        move_quiet(&t, &del.join("t").join(format!("{id}.webp"))).await;
        move_quiet(&t_dir.join(format!("{id}_blur.webp")), &del.join("t").join(format!("{id}_blur.webp"))).await;
        if mime.starts_with("audio") {
            move_quiet(&ca_dir.join(format!("{id}.webp")), &del.join("ca").join(format!("{id}.webp"))).await;
        }
        break;
    }
}

/// Permanently remove an item's files and comments and flag it purged (admin action).
async fn purge_item(st: &AppState, item: &Row, id: i64) -> anyhow::Result<()> {
    let cfg = st.cfg();
    let dest = item.string("dest");
    if is_plain_name(&dest) {
        mdelete::safe_delete_media_file(st, &dest, id).await;
        unlink(cfg.paths.pending.join("b").join(&dest)).await;
        unlink(cfg.paths.deleted.join("b").join(&dest)).await;
    }
    unlink(cfg.paths.t.join(format!("{id}.webp"))).await;
    unlink(cfg.paths.t.join(format!("{id}_blur.webp"))).await;
    unlink(cfg.paths.pending.join("t").join(format!("{id}.webp"))).await;
    unlink(cfg.paths.deleted.join("t").join(format!("{id}.webp"))).await;
    unlink(cfg.paths.deleted.join("t").join(format!("{id}_blur.webp"))).await;
    if item.string("mime").starts_with("audio") {
        unlink(cfg.paths.ca.join(format!("{id}.webp"))).await;
        unlink(cfg.paths.pending.join("ca").join(format!("{id}.webp"))).await;
        unlink(cfg.paths.deleted.join("ca").join(format!("{id}.webp"))).await;
    }
    sql!(r#"update "items" set is_purged = true where id = "#, id as i32).execute(&st.db).await?;
    sql!("delete from comments where item_id = ", id as i32).execute(&st.db).await?;
    Ok(())
}

fn update_private_index(st: &AppState, item: &Row, id: i64, live: bool) {
    let dest = item.string("dest");
    let owner = item.string("username");
    match item.i64("visibility") {
        Some(2) if live => crate::private_items::add_private(st, id, &dest, &owner),
        Some(3) if live => crate::private_items::add_unavailable(st, id, &dest, &owner),
        Some(2) => crate::private_items::remove_private(st, id, &dest),
        Some(3) => crate::private_items::remove_unavailable(st, id, &dest),
        _ => {}
    }
}

fn new_item_payload(item: &Row, id: i64) -> Value {
    json!({
        "id": id,
        "dest": item.val("dest"),
        "mime": item.val("mime"),
        "username": item.val("username"),
        "tag_id": item.val("tag_id"),
        "is_oc": item.bool("is_oc"),
        "is_album": item.bool("is_album"),
        "album_count": item.i64("album_count").unwrap_or(0),
    })
}

// ── Pages ───────────────────────────────────────────────────────────────────

async fn dashboard(st: AppState, req: Req) -> Res {
    let pending: i64 = sql!(r#"select count(*) as c from "items" where active = false and is_deleted = false"#).fetch_scalar(&st.db).await?.unwrap_or(0);
    let trash: i64 = sql!(r#"select count(*) as c from "items" where active = false and is_deleted = true and is_purged = false"#).fetch_scalar(&st.db).await?.unwrap_or(0);
    let reports: i64 = sql!("select count(*)::int8 as c from reports where status = 'pending'").fetch_scalar(&st.db).await.ok().flatten().unwrap_or(0);
    let data = json!({
        "pendingCount": pending,
        "trashCount": trash,
        "reportsCount": reports,
        "manualApproval": st.settings.manual_approval(),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "mod", data)))
}

async fn reports_page(st: AppState, req: Req) -> Res {
    Ok(html(crate::tpl::render(&st, Some(&req), "mod_reports", json!({ "tmp": null }))))
}

async fn approve_queue(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let page = queue_page(&req);
    let total: i64 = sql!(r#"select count(*)::int8 as n from "items" where active = false and is_deleted = false"#).fetch_scalar(&st.db).await?.unwrap_or(0);
    let pending = sql!(
        r#"select i.id, i.mime, i.username, i.dest, i.title, i.stamp, i.size, i.width, i.height,
               i.original_filename, i.is_album, i.album_count,
               json_agg(json_build_object('tag', t.tag, 'normalized', t.normalized)) as tags
        from "items" i
        left join "tags_assign" ta on ta.item_id = i.id
        left join "tags" t on t.id = ta.tag_id
        where i.active = false and i.is_deleted = false
        group by i.id
        order by i.id desc
        limit "#,
        QUEUE_PAGE_SIZE,
        " offset ",
        (page - 1) * QUEUE_PAGE_SIZE
    )
    .fetch_all(&st.db)
    .await?;

    // Album pictures for the albums on this page (cover = order_index 0 uses the item's own files)
    let album_ids: Vec<i32> = pending.iter().filter(|p| p.bool("is_album")).filter_map(|p| p.i64("id")).map(|i| i as i32).collect();
    let album_rows = if album_ids.is_empty() {
        vec![]
    } else {
        sql!(
            "select item_id, id, dest, mime, width, height, order_index from album_items where item_id = any(",
            album_ids,
            "::int[]) order by item_id, order_index asc"
        )
        .fetch_all(&st.db)
        .await?
    };

    let nsfl_id = cfg.nsfl_tag_id();
    let tr = st.i18n.translator("en");
    let now = crate::util::now_secs();
    let cards: Vec<Value> = pending
        .into_iter()
        .map(process_queue_item)
        .map(|mut p| {
            let id = p.i64("id").unwrap_or(0);
            let names: Vec<&str> = p["tags"].as_array().map(|a| a.iter().filter_map(|t| t["normalized"].as_str()).collect()).unwrap_or_default();
            let rating = if names.contains(&"nsfl") {
                "nsfl"
            } else if names.contains(&"nsfw") {
                "nsfw"
            } else if names.contains(&"sfw") {
                "sfw"
            } else {
                "untagged"
            };
            let mime = p.string("mime");
            let dest = p.string("dest");
            let rows: Vec<&Row> = if p.bool("is_album") { album_rows.iter().filter(|r| r.i64("item_id") == Some(id)).collect() } else { vec![] };
            let cover = rows.iter().position(|r| r.str("dest") == Some(dest.as_str())).or_else(|| rows.iter().position(|r| r.i64("order_index") == Some(0)));
            let mut media = vec![json!({
                "index": 1,
                "aid": cover.map(|c| rows[c].val("id").clone()).unwrap_or(Value::Null),
                "kind": media_kind(&mime),
                "mime": mime,
                "src": format!("/mod/pending/b/{dest}"),
                "thumb": format!("/mod/pending/t/{id}.webp"),
                "width": p.val("width"),
                "height": p.val("height"),
            })];
            for (i, r) in rows.iter().enumerate() {
                if Some(i) == cover {
                    continue;
                }
                let rd = r.string("dest");
                let rm = r.string("mime");
                media.push(json!({
                    "index": media.len() + 1,
                    "aid": r.val("id"),
                    "kind": media_kind(&rm),
                    "mime": rm,
                    "src": format!("/mod/pending/b/{rd}"),
                    "thumb": format!("/mod/pending/t/{}.webp", strip_ext(&rd)),
                    "width": r.val("width"),
                    "height": r.val("height"),
                }));
            }
            let stamp = p.i64("stamp").filter(|s| *s != 0);
            let size_fmt = if crate::config::truthy(p.val("size")) { crate::util::format_size(p.val("size")) } else { String::new() };
            let dims = match (p.i64("width").filter(|w| *w != 0), p.i64("height").filter(|h| *h != 0)) {
                (Some(w), Some(h)) => format!("{w}×{h}"),
                _ => String::new(),
            };
            p.insert("rating".into(), json!(rating));
            p.insert("kind".into(), json!(media_kind(&mime)));
            p.insert("media_json".into(), json!(crate::util::json_string(&Value::Array(media.clone()))));
            p.insert("is_album_view".into(), json!(media.len() > 1));
            p.insert("media".into(), Value::Array(media));
            p.insert("time_ago".into(), json!(stamp.map(|s| crate::util::time_ago(&tr, now - s)).unwrap_or_default()));
            p.insert("time_full".into(), json!(stamp.map(iso_from_secs).unwrap_or_default()));
            p.insert("size_fmt".into(), json!(size_fmt));
            p.insert("dims".into(), json!(dims));
            Value::Object(p)
        })
        .collect();

    let data = json!({
        "pending": cards,
        "total": total,
        "page": page,
        "pages": ((total + QUEUE_PAGE_SIZE - 1) / QUEUE_PAGE_SIZE).max(1),
        "enable_nsfl": cfg.enable_nsfl(),
        "nsfl_tag_id": nsfl_id,
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "mod/approve", data)))
}

async fn trash_page(st: AppState, req: Req) -> Res {
    let page = queue_page(&req);
    let total: i64 = sql!(r#"select count(*)::int8 as n from "items" where active = false and is_deleted = true and is_purged = false"#)
        .fetch_scalar(&st.db)
        .await?
        .unwrap_or(0);
    let trash = sql!(
        r#"select i.id, i.mime, i.username, i.dest,
            json_agg(json_build_object('tag', t.tag, 'normalized', t.normalized)) as tags,
            (select details->>'reason' from audit_log where target_id = i.id::text and action = 'delete_item' order by created_at desc limit 1) as delete_reason
        from "items" i
        left join "tags_assign" ta on ta.item_id = i.id
        left join "tags" t on t.id = ta.tag_id
        where i.active = false and i.is_deleted = true and i.is_purged = false
        group by i.id
        order by i.id desc
        limit "#,
        QUEUE_PAGE_SIZE,
        " offset ",
        (page - 1) * QUEUE_PAGE_SIZE
    )
    .fetch_all(&st.db)
    .await?;
    let data = json!({
        "trash": trash.into_iter().map(process_queue_item).map(Value::Object).collect::<Vec<_>>(),
        "total": total,
        "page": page,
        "pages": ((total + QUEUE_PAGE_SIZE - 1) / QUEUE_PAGE_SIZE).max(1),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "mod/trash", data)))
}

// ── Approve / deny ──────────────────────────────────────────────────────────

async fn approve(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = to_id(body.get("id")) else { return jr(400, json!({ "success": false, "msg": "No ID provided" })) };
    let id64 = id as i64;

    let item = sql!(
        r#"select i.dest, i.mime, i.username, i.id, i.visibility, i.is_album, i.album_count, i.is_oc,
               (select ta2.tag_id from tags_assign ta2
                join tags t2 on t2.id = ta2.tag_id
                where ta2.item_id = i.id and t2.tag in ('sfw','nsfw','nsfl')
                limit 1) as tag_id
        from "items" i
        where i.id = "#,
        id,
        " and i.active = false and i.is_deleted = false limit 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(item) = item else {
        return jr(404, json!({ "success": false, "msg": format!("f0ck {id}: not in the approval queue") }));
    };
    let username = item.string("username");
    let info = uploader_info(&st, &username).await;

    // Side effects only when this request flipped the item live (double clicks / races)
    let changed = sql!(r#"update "items" set active = true, is_deleted = false where id = "#, id, " and active = false").execute(&st.db).await?;
    crate::hidden_items::invalidate();

    if changed == 1 {
        let _ = sql!("UPDATE notifications SET is_read = true WHERE type = 'admin_pending' AND item_id = ", id).execute(&st.db).await;
        audit_log(&st, sid(&req), "approve_item", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest") }), info.clone()))).await;

        if let Some(uid) = info.get("uploader_id").and_then(crate::config::as_int) {
            let r = sql!("INSERT INTO notifications (user_id, type, reference_id, item_id) VALUES (", uid as i32, ", 'approve', 0, ", id, ")").execute(&st.db).await;
            if let Err(e) = r {
                tracing::error!("[MOD APPROVE] Failed to notify user: {e}");
            }
        }

        if item.i64("visibility").unwrap_or(0) == 0 {
            let cfg = st.cfg();
            let message = format!("{username} uploaded a new video {}", crate::bot::item_url(&st, id64).await);
            if let Some(hook) = cfg.client("discord").and_then(|c| c["webhook_url"].as_str()).filter(|h| !h.is_empty()).map(str::to_string) {
                let (http, msg) = (st.http.clone(), message.clone());
                tokio::spawn(async move {
                    match http.post(&hook).json(&json!({ "content": msg })).send().await {
                        Ok(r) if r.status().as_u16() >= 400 => tracing::error!("[MOD APPROVE] Webhook returned status {}", r.status().as_u16()),
                        Ok(_) => {}
                        Err(e) => tracing::error!("[MOD APPROVE] Webhook failed: {e}"),
                    }
                });
            }
            st.bot.notify_matrix(&cfg, &message).await;
            notify(&st, "new_item", &new_item_payload(&item, id64)).await;
        }
    }

    move_item_files_to_public(&st, &item, id64).await;

    if wants_json(&req) {
        return jr(200, json!({ "success": true, "item_id": id, "msg": "Item approved" }));
    }
    let mut r = raw(302, "");
    if let Ok(v) = HeaderValue::from_str(&format!("/{id}")) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    Ok(r)
}

async fn deny(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = to_id(body.get("id")) else { return jr(400, json!({ "success": false, "msg": "No ID provided" })) };
    let id64 = id as i64;
    let item = sql!(r#"select id, dest, mime, is_deleted, active, username, visibility from "items" where id = "#, id, " limit 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(item) = item else { return jr(200, json!({ "success": false, "msg": "Item not found" })) };

    if item.bool("is_deleted") {
        // Purge (admins only)
        if !is_admin(&req) {
            return jr(403, json!({ "success": false, "msg": "Only admins can purge items permanently." }));
        }
        purge_item(&st, &item, id64).await?;
        let info = uploader_info(&st, &item.string("username")).await;
        audit_log(&st, sid(&req), "purge_item", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest") }), info))).await;
        if wants_json(&req) {
            return jr(200, json!({ "success": true, "action": "purge", "msg": "Item purged" }));
        }
        return Ok(crate::http::json_resp(&json!({ "success": true, "action": "purge" }), 200));
    }

    // Deny: move to the soft-deleted folder
    move_item_files_to_deleted(&st, &item, id64).await;
    let reason = js_str(body.get("reason")).filter(|r| !r.is_empty()).unwrap_or_else(|| "Denied by moderator".into());
    sql!(r#"update "items" set is_deleted = true, active = false where id = "#, id).execute(&st.db).await?;
    crate::hidden_items::invalidate();
    update_private_index(&st, &item, id64, false);
    let _ = sql!("UPDATE notifications SET is_read = true WHERE type = 'admin_pending' AND item_id = ", id).execute(&st.db).await;

    let info = uploader_info(&st, &item.string("username")).await;
    if let Some(uid) = info.get("uploader_id").and_then(crate::config::as_int) {
        let r = sql!(
            "INSERT INTO notifications (user_id, type, reference_id, item_id, data) VALUES (",
            uid as i32,
            ", 'deny', 0, ",
            id,
            ", ",
            json!({ "reason": reason }),
            ")"
        )
        .execute(&st.db)
        .await;
        if let Err(e) = r {
            tracing::error!("[MOD DENY] Failed to notify uploader: {e}");
        }
    }
    audit_log(&st, sid(&req), "deny_item", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest"), "reason": reason }), info))).await;

    if wants_json(&req) {
        return jr(200, json!({ "success": true, "action": "deny", "msg": "Item denied" }));
    }
    Ok(crate::http::json_resp(&json!({ "success": true, "action": "deny" }), 200))
}

async fn deny_multi(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(ids) = body.get("ids").and_then(Value::as_array).cloned() else {
        let msg = crate::util::log_error(&"ids must be an array", "deny-multi");
        return Ok(crate::http::json_resp(&json!({ "success": false, "msg": msg }), 500));
    };
    let admin = is_admin(&req);
    let mut count = 0;
    for raw_id in &ids {
        let Some(id) = to_id(Some(raw_id)) else { continue };
        let id64 = id as i64;
        let Some(item) = sql!(r#"select id, dest, mime, is_deleted, active, username, visibility from "items" where id = "#, id, " limit 1")
            .fetch_optional(&st.db)
            .await?
        else {
            continue;
        };
        if item.bool("is_deleted") {
            if !admin {
                continue;
            }
            purge_item(&st, &item, id64).await?;
            let info = uploader_info(&st, &item.string("username")).await;
            audit_log(&st, sid(&req), "purge_item_multi", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest") }), info))).await;
        } else {
            move_item_files_to_deleted(&st, &item, id64).await;
            sql!(r#"update "items" set is_deleted = true, active = false where id = "#, id).execute(&st.db).await?;
            crate::hidden_items::invalidate();
            update_private_index(&st, &item, id64, false);
            let _ = sql!("UPDATE notifications SET is_read = true WHERE type = 'admin_pending' AND item_id = ", id).execute(&st.db).await;
            let info = uploader_info(&st, &item.string("username")).await;
            audit_log(&st, sid(&req), "deny_item_multi", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest") }), info))).await;
        }
        count += 1;
    }
    Ok(crate::http::json_resp(&json!({ "success": true, "count": count }), 200))
}

// ── Audit log ───────────────────────────────────────────────────────────────

fn process_log_entry(mut l: Row) -> Value {
    let det = match l.remove("details").unwrap_or(Value::Null) {
        Value::String(s) => serde_json::from_str::<Value>(&s).unwrap_or_else(|_| json!({})),
        v => v,
    };
    let has_details = crate::config::truthy(&det);
    let action = l.string("action");

    let fmt = l
        .str("created_at")
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
        .map(|d| crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None))
        .unwrap_or_else(|| "Invalid Date".into());
    l.insert("created_at_fmt".into(), json!(fmt));
    l.insert("created_at".into(), json!(fmt));

    // `entry.x = details ? details.x : null` — undefined keys are dropped from JSON
    let pick = |l: &mut Row, k: &str| {
        if !has_details {
            l.insert(k.into(), Value::Null);
        } else if let Some(v) = det.get(k) {
            l.insert(k.into(), v.clone());
        } else {
            l.remove(k);
        }
    };
    for k in ["reason", "old_content", "new_content", "item_id"] {
        pick(&mut l, k);
    }

    if action == "toggle_tag" && has_details {
        if det.get("action").and_then(Value::as_str) == Some("added") {
            set_or_remove(&mut l, "new_content", det.get("tag"));
        } else if det.get("from").map(crate::config::truthy).unwrap_or(false) {
            set_or_remove(&mut l, "old_content", det.get("from"));
            set_or_remove(&mut l, "new_content", det.get("tag"));
        }
    }

    if has_details {
        if let Some(name) = det.get("uploader_name").filter(|v| crate::config::truthy(v)) {
            let uid = det.get("uploader_id").filter(|v| crate::config::truthy(v)).map(|v| js_str(Some(v)).unwrap_or_default()).unwrap_or_else(|| "?".into());
            l.insert("uploader_info".into(), json!(format!("Uploader: {} (ID: {uid})", js_str(Some(name)).unwrap_or_default())));
        }
    }

    let mut other = Map::new();
    if let Value::Object(m) = &det {
        for (k, v) in m {
            let standard = ["reason", "old_content", "new_content", "item_id", "uploader_name", "uploader_id"].contains(&k.as_str());
            let toggle = action == "toggle_tag" && ["tag", "from", "action"].contains(&k.as_str());
            if !standard && !toggle {
                other.insert(k.clone(), v.clone());
            }
        }
    }
    l.insert("details_json".into(), if other.is_empty() { Value::Null } else { json!(crate::util::json_string(&Value::Object(other))) });
    Value::Object(l)
}

fn set_or_remove(l: &mut Row, k: &str, v: Option<&Value>) {
    match v {
        Some(v) => {
            l.insert(k.into(), v.clone());
        }
        None => {
            l.remove(k);
        }
    }
}

async fn audit(st: AppState, req: Req) -> Res {
    let page = req.query("page").map(|p| js_number(p).unwrap_or(f64::NAN)).unwrap_or(1.0);
    let page = if page.is_finite() && page >= 1.0 { (page.floor() as i64).min(1_000_000) } else { 1 };
    let limit = 50i64;
    let offset = (page - 1) * limit;
    let filter_action = req.query("action").map(str::trim).unwrap_or("").to_string();
    let filter_user = req.query("user").map(str::trim).unwrap_or("").to_string();

    let mut filter = crate::db::Sql::empty();
    if !filter_action.is_empty() {
        filter.push(" AND al.action = ").bind(filter_action.clone());
    }
    if !filter_user.is_empty() {
        filter.push(" AND u.user ILIKE ").bind(format!("%{filter_user}%"));
    }
    let logs = sql!(
        r#"SELECT al.*, u.user as username FROM audit_log al LEFT JOIN "user" u ON al.user_id = u.id WHERE true"#,
        @filter,
        " ORDER BY al.created_at DESC LIMIT ",
        limit,
        " OFFSET ",
        offset
    )
    .fetch_all(&st.db)
    .await?;
    let total: i64 = sql!(r#"SELECT count(*) as c FROM audit_log al LEFT JOIN "user" u ON al.user_id = u.id WHERE true"#, @filter)
        .fetch_scalar(&st.db)
        .await?
        .unwrap_or(0);
    let pages = (total + limit - 1) / limit;
    let processed: Vec<Value> = logs.into_iter().map(process_log_entry).collect();

    if req.is_xhr() {
        return jr(
            200,
            json!({
                "success": true,
                "logs": processed,
                "page": page,
                "pages": pages,
                "hasMore": page < pages,
                "filterAction": filter_action,
                "filterUser": filter_user,
            }),
        );
    }
    let data = json!({
        "logs": processed,
        "page": page,
        "pages": pages,
        "filterAction": filter_action,
        "filterUser": filter_user,
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "mod/audit", data)))
}

// ── Pending / deleted media ─────────────────────────────────────────────────

fn mime_by_ext(file: &str) -> &'static str {
    match file.rsplit('.').next().unwrap_or("") {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// `path.basename(decodeURIComponent(file))` inside `<root>/<type>`.
async fn serve_mod_file(req: &Req, root: &Path) -> Response {
    let ty = req.param("type").unwrap_or("");
    let raw_file = req.param("file").unwrap_or("");
    let decoded = crate::util::decode_uri_component(raw_file).unwrap_or_else(|| raw_file.to_string());
    let file = decoded.rsplit('/').next().unwrap_or("");
    if !is_plain_name(file) {
        return raw(403, "Forbidden");
    }
    let path = root.join(ty).join(file);
    match tokio::fs::metadata(&path).await {
        Ok(m) if m.is_file() => files::serve_file(req, &path, Some(mime_by_ext(file))).await,
        _ => raw(404, "File not found"),
    }
}

async fn pending_file(st: AppState, req: Req) -> Res {
    let root = st.cfg().paths.pending.clone();
    Ok(serve_mod_file(&req, &root).await)
}

async fn deleted_file(st: AppState, req: Req) -> Res {
    let root = st.cfg().paths.deleted.clone();
    Ok(serve_mod_file(&req, &root).await)
}

// ── Album item deny ─────────────────────────────────────────────────────────

async fn album_item_deny(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let item_id = to_id(body.get("item_id"));
    let sub_id = to_id(body.get("album_item_id"));
    let reason = js_str(body.get("reason")).unwrap_or_default().trim().to_string();
    let (Some(item_id), Some(sub_id)) = (item_id, sub_id) else {
        return jr(400, json!({ "success": false, "msg": "Missing item_id or album_item_id" }));
    };
    if reason.is_empty() {
        return jr(400, json!({ "success": false, "msg": "A reason is required" }));
    }
    let item = sql!(r#"select id, dest, mime, username, is_album from "items" where id = "#, item_id, " and active = false and is_deleted = false limit 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(item) = item.filter(|i| i.bool("is_album")) else { return jr(404, json!({ "success": false, "msg": "Not a pending album" })) };

    let subs = sql!("select id, dest, mime, order_index from album_items where item_id = ", item_id, " order by order_index asc, id asc")
        .fetch_all(&st.db)
        .await?;
    let Some(target) = subs.iter().find(|r| r.i64("id") == Some(sub_id as i64)).cloned() else {
        return jr(404, json!({ "success": false, "msg": "Album item not found" }));
    };
    if subs.len() <= 1 {
        return jr(400, json!({ "success": false, "msg": "Only one item left, deny the whole post instead" }));
    }
    let remaining: Vec<&Row> = subs.iter().filter(|r| r.i64("id") != Some(sub_id as i64)).collect();
    let target_dest = target.string("dest");
    let is_cover = target_dest == item.string("dest");
    let degraded = remaining.len() == 1;
    let new_cover = remaining[0].clone();

    let mut tx = st.db.begin().await?;
    sql!("delete from album_items where id = ", sub_id).execute(&mut *tx).await?;
    for (i, r) in remaining.iter().enumerate() {
        sql!("update album_items set order_index = ", i as i32, " where id = ", r.i64("id").unwrap_or(0) as i32).execute(&mut *tx).await?;
    }
    if is_cover {
        sql!(
            r#"update "items" i set dest = a.dest, mime = a.mime, size = a.size, checksum = a.checksum, phash = a.phash, width = a.width, height = a.height
               from album_items a where a.id = "#,
            new_cover.i64("id").unwrap_or(0) as i32,
            " and i.id = ",
            item_id
        )
        .execute(&mut *tx)
        .await?;
    }
    if degraded {
        sql!("delete from album_items where item_id = ", item_id).execute(&mut *tx).await?;
        sql!(r#"update "items" set is_album = false, album_count = 0 where id = "#, item_id).execute(&mut *tx).await?;
    } else {
        sql!(r#"update "items" set album_count = "#, remaining.len() as i32, " where id = ", item_id).execute(&mut *tx).await?;
    }
    tx.commit().await?;

    // Remove the denied picture's files from the pending folders
    let cfg = st.cfg();
    if is_plain_name(&target_dest) {
        let base = strip_ext(&target_dest).to_string();
        unlink(cfg.paths.pending.join("b").join(&target_dest)).await;
        unlink(cfg.paths.pending.join("t").join(format!("{base}.webp"))).await;
        unlink(cfg.paths.pending.join("ca").join(format!("{base}.webp"))).await;
    }

    // New cover: regenerate the item's main + blurred thumbnail from it
    if is_cover {
        let size = queue::thumb_size(0);
        queue::gen_thumbnail(&cfg, &new_cover.string("dest"), &new_cover.string("mime"), item_id as i64, "", true, size).await;
        queue::gen_blurred_thumbnail(&cfg, item_id as i64, true).await;
    }

    let info = uploader_info(&st, &item.string("username")).await;
    audit_log(
        &st,
        sid(&req),
        "deny_album_item",
        "item",
        &item_id.to_string(),
        Some(details(
            json!({
                "filename": target_dest, "album_item_id": sub_id, "reason": reason,
                "remaining": remaining.len(), "cover_changed": is_cover, "degraded": degraded,
            }),
            info,
        )),
    )
    .await;
    jr(200, json!({ "success": true, "remaining": remaining.len(), "cover_changed": is_cover, "degraded": degraded }))
}

// ── Soft deleted: restore / purge ───────────────────────────────────────────

async fn trash_restore(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = to_id(body.get("id")) else { return jr(400, json!({ "success": false, "msg": "No ID provided" })) };
    let item = sql!(
        r#"select i.id, i.dest, i.mime, i.username, i.visibility, i.is_album, i.album_count, i.is_oc,
               (select ta2.tag_id from tags_assign ta2 join tags t2 on t2.id = ta2.tag_id
                where ta2.item_id = i.id and t2.tag in ('sfw','nsfw','nsfl') limit 1) as tag_id
        from "items" i
        where i.id = "#,
        id,
        " and i.is_deleted = true and i.is_purged = false limit 1"
    )
    .fetch_optional(&st.db)
    .await?;
    let Some(item) = item else { return jr(404, json!({ "success": false, "msg": format!("Item {id} is not soft deleted") })) };

    let changed = sql!(r#"update "items" set active = true, is_deleted = false where id = "#, id, " and is_deleted = true").execute(&st.db).await?;
    crate::hidden_items::invalidate();
    if changed != 1 {
        return jr(409, json!({ "success": false, "msg": "Item was already restored" }));
    }
    update_private_index(&st, &item, id as i64, true);
    move_item_files_to_public(&st, &item, id as i64).await;
    let info = uploader_info(&st, &item.string("username")).await;
    audit_log(&st, sid(&req), "restore_item", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest") }), info))).await;

    if item.i64("visibility").unwrap_or(0) == 0 {
        notify(&st, "new_item", &new_item_payload(&item, id as i64)).await;
    }
    jr(200, json!({ "success": true, "item_id": id, "msg": "Item restored" }))
}

async fn trash_purge(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = to_id(body.get("id")) else { return jr(400, json!({ "success": false, "msg": "No ID provided" })) };
    let reason = js_str(body.get("reason")).unwrap_or_default().trim().to_string();
    if reason.is_empty() {
        return jr(400, json!({ "success": false, "msg": "A reason is required" }));
    }
    let item = sql!(r#"select id, dest, mime, username from "items" where id = "#, id, " and is_deleted = true and is_purged = false limit 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(item) = item else { return jr(404, json!({ "success": false, "msg": format!("Item {id} is not soft deleted") })) };
    purge_item(&st, &item, id as i64).await?;
    let info = uploader_info(&st, &item.string("username")).await;
    audit_log(&st, sid(&req), "purge_item", "item", &id.to_string(), Some(details(json!({ "filename": item.val("dest"), "reason": reason }), info))).await;
    jr(200, json!({ "success": true, "item_id": id, "msg": "Item purged" }))
}

async fn purge_all(st: AppState, req: Req) -> Res {
    let trash = match sql!(r#"select id, dest, mime from "items" where active = false and is_deleted = true and is_purged = false"#).fetch_all(&st.db).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[MOD PURGE ALL] {e}");
            return jr(500, json!({ "success": false, "msg": "Purge failed" }));
        }
    };
    let mut count = 0;
    for item in &trash {
        let id = item.i64("id").unwrap_or(0);
        match purge_item(&st, item, id).await {
            Ok(()) => count += 1,
            Err(e) => tracing::error!("[MOD PURGE ALL] item {id}: {e}"),
        }
    }
    audit_log(&st, sid(&req), "purge_trash", "system", "0", Some(json!({ "count": count }))).await;
    jr(200, json!({ "success": true, "count": count }))
}

// ── MOTD ────────────────────────────────────────────────────────────────────

async fn motd_page(st: AppState, req: Req) -> Res {
    let motd: Option<String> = sql!("SELECT value FROM site_settings WHERE key = 'motd' LIMIT 1").fetch_scalar(&st.db).await?;
    Ok(html(crate::tpl::render(&st, Some(&req), "mod/motd", json!({ "motd": motd.unwrap_or_default(), "tmp": null }))))
}

/// At most `max` bytes, cut on a char boundary (pg_notify payloads are limited to 8000 bytes).
fn truncate_bytes(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

async fn motd_save(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let mut motd = js_str(body.get("motd")).unwrap_or_default();
    if !motd.trim().is_empty() {
        // Append the " t. name" signature (display name if set, otherwise username)
        let s = req.session();
        let name = s.and_then(|s| s.display_name.clone()).filter(|d| !d.is_empty()).or_else(|| s.map(|s| s.user.clone())).unwrap_or_default();
        let suffix = format!(" t. {name}");
        if !motd.ends_with(&suffix) {
            motd = format!("{}{suffix}", motd.trim());
        }
    }

    if let Err(e) = crate::settings::save(&st, "motd", &motd).await {
        tracing::error!("[MOD MOTD] Save failed: {e:#}");
        let msg = "Failed to save MOTD";
        if req.is_xhr() {
            return jr(500, json!({ "success": false, "msg": msg }));
        }
        return Ok(reply(500, "text/html", msg));
    }
    let m = motd.clone();
    st.settings.update(|s| s.motd = m);
    audit_log(&st, sid(&req), "update_motd", "system", "0", Some(json!({ "motd": motd }))).await;
    if let Err(e) = sql!("SELECT pg_notify('motd', ", truncate_bytes(&motd, 7000), ")").execute(&st.db).await {
        tracing::error!("[MOD MOTD] Notify failed: {e}");
    }

    if req.is_xhr() {
        return jr(200, json!({ "success": true, "motd": motd }));
    }
    let mut r = raw(302, "");
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/mod/motd"));
    Ok(r)
}

// ── Pin / unpin ─────────────────────────────────────────────────────────────

async fn pin(st: AppState, req: Req) -> Res {
    set_pinned(st, req, true).await
}

async fn unpin(st: AppState, req: Req) -> Res {
    set_pinned(st, req, false).await
}

async fn set_pinned(st: AppState, req: Req, pinned: bool) -> Res {
    let Some(raw_id) = req.query("id").filter(|s| !s.is_empty()) else {
        return jr(200, json!({ "success": false, "msg": "No ID provided" }));
    };
    let missing = if pinned { "Item not found or not active" } else { "Item not found" };
    let Some(id) = to_id(Some(&json!(raw_id))) else { return jr(200, json!({ "success": false, "msg": missing })) };
    let n = if pinned {
        sql!(r#"update "items" set is_pinned = true where id = "#, id, " and active = true").execute(&st.db).await?
    } else {
        sql!(r#"update "items" set is_pinned = false where id = "#, id).execute(&st.db).await?
    };
    if n == 1 {
        audit_log(&st, sid(&req), if pinned { "pin_item" } else { "unpin_item" }, "item", &id.to_string(), None).await;
        return jr(200, json!({ "success": true, "pinned": pinned }));
    }
    jr(200, json!({ "success": false, "msg": missing }))
}

// ── Halls ───────────────────────────────────────────────────────────────────

/// `{ success: result.success, msg: result.message }` (an undefined message is dropped).
fn hall_result(r: &Value) -> Value {
    let mut m = Map::new();
    m.insert("success".into(), r.get("success").cloned().unwrap_or(Value::Null));
    if let Some(msg) = r.get("message") {
        m.insert("msg".into(), msg.clone());
    }
    Value::Object(m)
}

/// `{ hall, description }` (an undefined description is dropped, like JSON.stringify).
fn hall_details(hall: &str, description: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("hall".into(), json!(hall));
    if let Some(d) = description {
        m.insert("description".into(), json!(d));
    }
    Value::Object(m)
}

async fn halls_add(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let hall = js_str(body.get("hall")).unwrap_or_default();
    let description = js_str(body.get("description"));
    if !crate::config::truthy(body.get("id").unwrap_or(&Value::Null)) || hall.is_empty() {
        return jr(200, json!({ "success": false, "msg": "Missing id or hall" }));
    }
    let Some(id) = to_id(body.get("id")) else { return jr(200, json!({ "success": false, "msg": "Invalid item id" })) };
    let r = crate::social::halls::add_item_to_hall(&st, id as i64, &hall, Some(sid(&req)), description.as_deref()).await;
    if crate::config::truthy(&r["success"]) {
        audit_log(&st, sid(&req), "add_to_hall", "item", &id.to_string(), Some(hall_details(&hall, description.as_deref()))).await;
    }
    jr(200, hall_result(&r))
}

async fn halls_remove(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let hall = js_str(body.get("hall")).unwrap_or_default();
    if !crate::config::truthy(body.get("id").unwrap_or(&Value::Null)) || hall.is_empty() {
        return jr(200, json!({ "success": false, "msg": "Missing id or hall" }));
    }
    let Some(id) = to_id(body.get("id")) else { return jr(200, json!({ "success": false, "msg": "Invalid item id" })) };
    let r = crate::social::halls::remove_item_from_hall(&st, id as i64, &hall).await;
    if crate::config::truthy(&r["success"]) {
        audit_log(&st, sid(&req), "remove_from_hall", "item", &id.to_string(), Some(json!({ "hall": hall }))).await;
    }
    jr(200, hall_result(&r))
}

async fn halls_update(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let hall = js_str(body.get("hall")).unwrap_or_default();
    let description = js_str(body.get("description"));
    if hall.is_empty() {
        return jr(200, json!({ "success": false, "msg": "Missing hall slug" }));
    }
    let r = crate::social::halls::update_hall_metadata(&st, &hall, description.as_deref()).await;
    if crate::config::truthy(&r["success"]) {
        audit_log(&st, sid(&req), "update_hall_metadata", "hall", "0", Some(hall_details(&hall, description.as_deref()))).await;
    }
    jr(200, hall_result(&r))
}

async fn halls_page(st: AppState, req: Req) -> Res {
    let halls: Vec<Value> = crate::social::halls::get_halls(&st).await.into_iter().map(Value::Object).collect();
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/halls", json!({ "hallsList": halls, "tmp": null }))))
}
