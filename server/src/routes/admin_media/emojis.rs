//! Custom emojis and sticker packs: src/inc/routes/emojis.mjs, src/emoji_upload_handler.mjs and
//! src/sticker_pack_handler.mjs (Telegram import).

use super::{admin_session, convert_to_webp, csrf_header_ok, file_size, local_emoji_file, magick, pg_code, read_form, send, trimmed};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Map, Value, json};
use std::path::Path;
use std::time::Duration;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/emojis\/?$", Guard::Auth, admin_page);
    r.exact(Method::GET, "/api/v2/emojis", None, list_emojis);
    r.exact(Method::GET, "/api/v2/admin/sticker-packs", None, list_packs);
    r.delete(r"\/api\/v2\/admin\/sticker-packs\/(?<id>\d+)", delete_pack);
    r.post(r"\/api\/v2\/admin\/sticker-packs\/(?<id>\d+)\/rename", rename_pack);
    r.post(r"\/api\/v2\/admin\/sticker-packs\/(?<id>\d+)\/thumb", set_pack_thumb);
    r.exact(Method::POST, "/api/v2/admin/emojis/reconvert", None, reconvert);
    r.post(r"\/api\/v2\/admin\/emojis\/(?<id>\d+)\/delete", delete_emoji);
    r.exact(Method::GET, "/api/v2/user/emoji-favorites", Some(Guard::LoggedIn), favorites);
    r.exact(Method::POST, "/api/v2/user/emoji-favorites/toggle", Some(Guard::LoggedIn), toggle_favorite);
    // index.mjs bypass middleware
    r.pre(Method::POST, r"^\/api\/v2\/admin\/emojis$", None, upload_emoji);
    r.pre(Method::POST, r"^\/api\/v2\/admin\/emojis\/(?<id>\d+)\/edit$", None, edit_emoji);
    r.pre(Method::POST, r"^\/api\/v2\/admin\/sticker-packs\/import$", None, import_telegram_pack);
}

fn msg(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "message": m }), code)
}

/// The inline `!req.session.admin` → 403 and `X-CSRF-Token` checks of the emoji routes.
fn admin_csrf(req: &Req) -> Result<crate::session::Session, Res> {
    let Some(s) = admin_session(req) else { return Err(msg(403, "Forbidden")) };
    if !csrf_header_ok(req, &s) {
        return Err(msg(403, "Invalid CSRF token"));
    }
    Ok(s)
}

async fn notify_emojis(st: &AppState) {
    if let Err(e) = sql!("NOTIFY emojis_updated, '{}'").execute(&st.db).await {
        tracing::error!("[EMOJIS] notify failed: {e}");
    }
}

fn id_param(req: &Req) -> Option<i32> {
    req.param("id").and_then(|i| i.parse().ok())
}

async fn admin_page(st: AppState, req: Req) -> Res {
    let body = crate::tpl::render(&st, Some(&req), "admin/emojis", json!({ "session": req.session_value(), "tmp": null }));
    Ok(html(body))
}

// ── Listing ──────────────────────────────────────────────────────────────────

/// Flat list plus the emojis grouped by pack (standalone emojis first, as "Custom Emojis").
pub(crate) fn group_emojis(rows: &[Row]) -> (Vec<Value>, Vec<Value>) {
    let flat: Vec<Value> = rows.iter().map(|e| json!({ "id": e.val("id"), "name": e.val("name"), "url": e.val("url"), "pack_id": e.val("pack_id") })).collect();
    let mut order: Vec<Option<i64>> = vec![None];
    let mut packs: std::collections::HashMap<Option<i64>, Map<String, Value>> = std::collections::HashMap::new();
    let mut none = Map::new();
    none.insert("id".into(), Value::Null);
    none.insert("name".into(), json!("Custom Emojis"));
    none.insert("tg_name".into(), Value::Null);
    none.insert("emojis".into(), json!([]));
    packs.insert(None, none);
    for e in rows {
        let pid = e.i64("pack_id").filter(|p| *p != 0);
        if let Some(p) = pid {
            packs.entry(Some(p)).or_insert_with(|| {
                order.push(Some(p));
                let name = [e.str("pack_name"), e.str("tg_title")].into_iter().flatten().find(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("Pack #{p}"));
                let thumb = e.str("pack_thumb_url").filter(|s| !s.is_empty()).map(|s| json!(s)).unwrap_or(Value::Null);
                let mut m = Map::new();
                m.insert("id".into(), e.val("pack_id").clone());
                m.insert("name".into(), json!(name));
                m.insert("tg_name".into(), e.val("tg_name").clone());
                m.insert("thumb_url".into(), thumb);
                m.insert("emojis".into(), json!([]));
                m
            });
        }
        if let Some(Value::Array(list)) = packs.get_mut(&pid).and_then(|m| m.get_mut("emojis")) {
            list.push(json!({ "id": e.val("id"), "name": e.val("name"), "url": e.val("url") }));
        }
    }
    let grouped = order
        .into_iter()
        .filter_map(|k| packs.remove(&k))
        .filter(|m| m.get("emojis").and_then(Value::as_array).map(|a| !a.is_empty()).unwrap_or(false))
        .map(Value::Object)
        .collect();
    (flat, grouped)
}

async fn list_emojis(st: AppState, _req: Req) -> Res {
    let rows = sql!(
        "SELECT e.id, e.name, e.url, e.pack_id, p.name as pack_name, p.tg_name, p.tg_title, p.thumb_url as pack_thumb_url
         FROM custom_emojis e LEFT JOIN sticker_packs p ON p.id = e.pack_id
         ORDER BY e.pack_id NULLS FIRST, e.id ASC"
    )
    .fetch_all(&st.db)
    .await;
    match rows {
        Ok(rows) => {
            let (flat, packs) = group_emojis(&rows);
            send(json!({ "success": true, "emojis": flat, "packs": packs }), 200)
        }
        Err(e) => {
            tracing::error!("[EMOJIS] list failed: {e}");
            msg(500, "Database error")
        }
    }
}

async fn list_packs(st: AppState, req: Req) -> Res {
    if admin_session(&req).is_none() {
        return msg(403, "Forbidden");
    }
    let rows = sql!(
        "SELECT sp.id, sp.name, sp.tg_name, sp.tg_title, sp.thumb_url, sp.sticker_count, sp.created_at,
                count(e.id)::int as actual_count
         FROM sticker_packs sp LEFT JOIN custom_emojis e ON e.pack_id = sp.id
         GROUP BY sp.id ORDER BY sp.created_at DESC"
    )
    .fetch_all(&st.db)
    .await;
    match rows {
        Ok(rows) => send(json!({ "success": true, "packs": rows }), 200),
        Err(e) => {
            tracing::error!("[STICKER PACKS] list failed: {e}");
            msg(500, "Database error")
        }
    }
}

// ── Sticker pack management ──────────────────────────────────────────────────

async fn delete_pack(st: AppState, req: Req) -> Res {
    if let Err(r) = admin_csrf(&req) {
        return r;
    }
    let Some(id) = id_param(&req) else { return msg(500, "Database error") };
    let r: anyhow::Result<()> = async {
        let files: Vec<String> = sql!("SELECT url FROM custom_emojis WHERE pack_id = ", id).fetch_scalar_list(&st.db).await?;
        sql!("DELETE FROM custom_emojis WHERE pack_id = ", id).execute(&st.db).await?;
        sql!("DELETE FROM sticker_packs WHERE id = ", id).execute(&st.db).await?;
        let dir = st.cfg().paths.emojis.clone();
        for url in &files {
            if let Some(name) = local_emoji_file(url) {
                let _ = tokio::fs::remove_file(dir.join(name)).await;
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[STICKER PACKS] delete failed: {e:#}");
        return msg(500, "Database error");
    }
    notify_emojis(&st).await;
    send(json!({ "success": true }), 200)
}

/// Slug of a pack's display name for emoji names: `"My Cool Pack"` → `"my_cool_pack"`. The import
/// trims every leading/trailing `_` (`/^_|_$/g`); the rename backfill only the first match
/// (`/^_|_$/` without `g`) — both kept as they were so renamed packs keep producing the same names.
pub(crate) fn pack_slug(name: &str, global_trim: bool) -> String {
    let mut s = String::with_capacity(name.len());
    for c in name.chars() {
        let c = if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' };
        if c == '_' && s.ends_with('_') {
            continue;
        }
        s.push(c);
    }
    if global_trim {
        s.trim_matches('_').to_string()
    } else if let Some(rest) = s.strip_prefix('_') {
        rest.to_string()
    } else if let Some(rest) = s.strip_suffix('_') {
        rest.to_string()
    } else {
        s
    }
}

/// New name for an emoji of a renamed pack: `<slug>_<NNN>` from the old name's numeric suffix.
pub(crate) fn renamed_emoji(old: &str, slug: &str) -> Option<String> {
    let (_, suffix) = old.rsplit_once('_')?;
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("{slug}_{suffix:0>3}"))
}

async fn rename_pack(st: AppState, mut req: Req) -> Res {
    if let Err(r) = admin_csrf(&req) {
        return r;
    }
    let Some(id) = id_param(&req) else { return msg(500, "Database error") };
    let body = req.post().await.cloned().unwrap_or(Value::Null);
    let new_name = trimmed(body.get("name").and_then(Value::as_str));
    let backfill = body.get("backfill") == Some(&Value::Bool(true));
    if new_name.is_empty() {
        return msg(400, "name is required");
    }
    let r: anyhow::Result<Option<(i64, u64)>> = async {
        if !backfill {
            sql!("UPDATE sticker_packs SET name = ", &new_name, " WHERE id = ", id).execute(&st.db).await?;
            return Ok(None);
        }
        let slug = pack_slug(&new_name, false);
        let emojis = sql!("SELECT id, name FROM custom_emojis WHERE pack_id = ", id, " ORDER BY id").fetch_all(&st.db).await?;
        let renames: Vec<(i32, String, String)> = emojis
            .iter()
            .filter_map(|e| {
                let old = e.string("name");
                let new = renamed_emoji(&old, &slug)?;
                (new != old).then(|| (e.i64("id").unwrap_or(0) as i32, old, new))
            })
            .collect();
        let (mut renamed, mut comments) = (0i64, 0u64);
        let mut tx = st.db.begin().await?;
        sql!("UPDATE sticker_packs SET name = ", &new_name, " WHERE id = ", id).execute(&mut *tx).await?;
        for (eid, old, new) in &renames {
            let conflict = sql!("SELECT id FROM custom_emojis WHERE name = ", new, " AND id != ", *eid, " LIMIT 1").fetch_optional(&mut *tx).await?;
            if conflict.is_some() {
                tracing::warn!("[RENAME] Skipping {old} → {new}: name conflict");
                continue;
            }
            sql!("UPDATE custom_emojis SET name = ", new, " WHERE id = ", *eid).execute(&mut *tx).await?;
            renamed += 1;
            let like = format!("%:{}:%", crate::util::escape_like(old));
            comments += sql!(
                "UPDATE comments SET content = REPLACE(content, ", format!(":{old}:"), ", ", format!(":{new}:"), ") WHERE content LIKE ", like
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(Some((renamed, comments)))
    }
    .await;
    match r {
        Ok(res) => {
            notify_emojis(&st).await;
            match res {
                None => send(json!({ "success": true }), 200),
                Some((renamed, comments)) => send(json!({ "success": true, "emojiRenamed": renamed, "commentRowsUpdated": comments }), 200),
            }
        }
        Err(e) => {
            tracing::error!("[RENAME PACK] {e:#}");
            msg(500, &format!("Database error: {e}"))
        }
    }
}

/// First frame of a `.webm` sticker as a static WebP in the emojis dir; `None` when that fails.
async fn webm_first_frame(st: &AppState, webm: &Path) -> Option<String> {
    let cfg = st.cfg();
    let work = crate::media::WorkDir::new(&cfg, "packthumb").await.ok()?;
    let png = work.join("frame.png");
    let rand = crate::util::random_hex(24);
    let webp = cfg.paths.emojis.join(format!("{rand}_thumb.webp"));
    let (w, p, o) = (webm.display().to_string(), png.display().to_string(), webp.display().to_string());
    let r: anyhow::Result<()> = async {
        crate::proc::spawn("ffmpeg", &["-y", "-i", &w, "-vframes", "1", &p]).await?;
        magick(&cfg, &[&p, "-quality", "85", &o]).await?;
        Ok(())
    }
    .await;
    if let Err(e) = r {
        tracing::warn!("[SET THUMB] Could not generate WebP from .webm: {e}");
        let _ = tokio::fs::remove_file(&webp).await;
        return None;
    }
    if file_size(&webp).await == 0 {
        return None;
    }
    crate::media::make_public(&webp).await;
    let url = format!("/s/emojis/{rand}_thumb.webp");
    tracing::info!("[SET THUMB] Generated WebP thumbnail from .webm: {url}");
    Some(url)
}

async fn set_pack_thumb(st: AppState, mut req: Req) -> Res {
    if let Err(r) = admin_csrf(&req) {
        return r;
    }
    let Some(id) = id_param(&req) else { return msg(500, "Database error") };
    let body = req.post().await.cloned().unwrap_or(Value::Null);
    let thumb_url = trimmed(body.get("thumb_url").and_then(Value::as_str));
    // Only local emoji paths (no SSRF, no paths outside the emojis dir)
    let Some(file) = local_emoji_file(&thumb_url).map(str::to_string) else {
        return msg(400, "thumb_url must be a local /s/emojis/ path");
    };
    let mut final_url = thumb_url.clone();
    if thumb_url.ends_with(".webm") {
        let path = st.cfg().paths.emojis.join(&file);
        if let Some(u) = webm_first_frame(&st, &path).await {
            final_url = u;
        }
    }
    if let Err(e) = sql!("UPDATE sticker_packs SET thumb_url = ", &final_url, " WHERE id = ", id).execute(&st.db).await {
        tracing::error!("[SET THUMB] {e}");
        return msg(500, "Database error");
    }
    notify_emojis(&st).await;
    send(json!({ "success": true, "thumb_url": final_url }), 200)
}

// ── Emojis ───────────────────────────────────────────────────────────────────

async fn reconvert(st: AppState, req: Req) -> Res {
    if let Err(r) = admin_csrf(&req) {
        return r;
    }
    let cfg = st.cfg();
    let emojis = match sql!("SELECT id, name, url FROM custom_emojis ORDER BY id").fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[EMOJI RECONVERT] Fatal error: {e}");
            return msg(500, &e.to_string());
        }
    };
    let (mut converted, mut skipped, mut errors) = (0, 0, 0);
    let mut error_list: Vec<String> = vec![];
    for e in &emojis {
        let (id, name, url) = (e.i64("id").unwrap_or(0) as i32, e.string("name"), e.string("url"));
        let Some(filename) = local_emoji_file(&url) else {
            skipped += 1;
            continue;
        };
        let ext = Path::new(filename).extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext == "webp" {
            skipped += 1;
            continue;
        }
        let original = cfg.paths.emojis.join(filename);
        if tokio::fs::metadata(&original).await.is_err() {
            tracing::warn!("[EMOJI RECONVERT] Missing file for emoji \"{name}\": {}", original.display());
            skipped += 1;
            continue;
        }
        let stem = Path::new(filename).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| filename.to_string());
        let webp_name = format!("{stem}.webp");
        let webp_path = cfg.paths.emojis.join(&webp_name);
        let r: anyhow::Result<()> = async {
            convert_to_webp(&cfg, &original, &webp_path).await?;
            sql!("UPDATE custom_emojis SET url = ", format!("/s/emojis/{webp_name}"), " WHERE id = ", id).execute(&st.db).await?;
            tokio::fs::remove_file(&original).await?;
            Ok(())
        }
        .await;
        match r {
            Ok(()) => {
                tracing::info!("[EMOJI RECONVERT] Converted \"{name}\": {filename} → {webp_name}");
                converted += 1;
            }
            Err(err) => {
                tracing::error!("[EMOJI RECONVERT] Failed \"{name}\": {err}");
                let _ = tokio::fs::remove_file(&webp_path).await;
                error_list.push(format!("{name}: {err}"));
                errors += 1;
            }
        }
    }
    if converted > 0 {
        notify_emojis(&st).await;
    }
    send(json!({ "success": true, "converted": converted, "skipped": skipped, "errors": errors, "errorList": error_list }), 200)
}

async fn delete_emoji(st: AppState, req: Req) -> Res {
    if let Err(r) = admin_csrf(&req) {
        return r;
    }
    let Some(id) = id_param(&req) else { return msg(500, "Database error") };
    if let Err(e) = sql!("DELETE FROM custom_emojis WHERE id = ", id).execute(&st.db).await {
        tracing::error!("[EMOJIS] delete failed: {e}");
        return msg(500, "Database error");
    }
    notify_emojis(&st).await;
    send(json!({ "success": true }), 200)
}

// ── Favorites ────────────────────────────────────────────────────────────────

async fn favorites(st: AppState, req: Req) -> Res {
    let uid = req.session().map(|s| s.id).unwrap_or(0) as i32;
    match sql!("SELECT emoji_name AS name, emoji_url AS url FROM user_emoji_favorites WHERE user_id = ", uid, " ORDER BY created_at ASC").fetch_all(&st.db).await {
        Ok(rows) => send(json!({ "success": true, "favorites": rows }), 200),
        Err(e) => {
            tracing::error!("[EMOJI FAVS GET] {e}");
            send(json!({ "success": false }), 500)
        }
    }
}

async fn toggle_favorite(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session().cloned() else { return msg(403, "Invalid CSRF token") };
    if !csrf_header_ok(&req, &session) {
        return msg(403, "Invalid CSRF token");
    }
    let body = req.post().await.cloned().unwrap_or(Value::Null);
    let name = crate::social::helpers::js_str(body.get("name")).filter(|s| !s.is_empty());
    let url = crate::social::helpers::js_str(body.get("url")).filter(|s| !s.is_empty());
    let (Some(name), Some(url)) = (name, url) else { return msg(400, "name and url required") };
    let uid = session.id as i32;
    let r = if body.get("action").and_then(Value::as_str) == Some("remove") {
        sql!("DELETE FROM user_emoji_favorites WHERE user_id = ", uid, " AND emoji_name = ", &name).execute(&st.db).await
    } else {
        sql!(
            "INSERT INTO user_emoji_favorites (user_id, emoji_name, emoji_url) VALUES (", uid, ", ", &name, ", ", &url,
            ") ON CONFLICT (user_id, emoji_name) DO NOTHING"
        )
        .execute(&st.db)
        .await
    };
    match r {
        Ok(_) => send(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[EMOJI FAVS TOGGLE] {e}");
            send(json!({ "success": false }), 500)
        }
    }
}

// ── Upload / edit (emoji_upload_handler.mjs) ─────────────────────────────────

const MAX_EMOJI_UPLOAD: u64 = 20 * 1024 * 1024 + 64 * 1024;

fn valid_emoji_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Store an uploaded image as `<random>.webp` in the emojis dir; returns its URL.
async fn store_emoji_file(st: &AppState, src: &Path) -> anyhow::Result<String> {
    let cfg = st.cfg();
    tokio::fs::create_dir_all(&cfg.paths.emojis).await?;
    let name = format!("{}.webp", crate::util::random_hex(24));
    convert_to_webp(&cfg, src, &cfg.paths.emojis.join(&name)).await?;
    Ok(format!("/s/emojis/{name}"))
}

async fn upload_emoji(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        tracing::warn!("[CSRF] Blocked emoji upload for user {}. Invalid token.", session.user);
        return msg(403, "Invalid CSRF token");
    }
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_EMOJI_UPLOAD, "message").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let name = trimmed(form.text("name")).to_lowercase();
    let mut url = trimmed(form.text("url"));
    if name.is_empty() {
        return msg(400, "Emoji name is required");
    }
    if !valid_emoji_name(&name) {
        return msg(400, "Invalid name. Use lowercase a-z, 0-9, _, - only.");
    }
    let r: anyhow::Result<Res> = async {
        if let Some(file) = form.file("file").filter(|f| f.size > 0) {
            url = store_emoji_file(&st, &file.path).await?;
        }
        if url.is_empty() {
            return Ok(msg(400, "Either image URL or File is required"));
        }
        let row = sql!("INSERT INTO custom_emojis (name, url) VALUES (", &name, ", ", &url, ") RETURNING id, name, url").fetch_optional(&st.db).await?;
        tracing::info!("[EMOJI HANDLER] Success: {name}");
        notify_emojis(&st).await;
        Ok(send(json!({ "success": true, "emoji": row }), 200))
    }
    .await;
    match r {
        Ok(r) => r,
        Err(e) if pg_code(&e).as_deref() == Some("23505") => msg(400, "Emoji name already exists"),
        Err(e) => {
            tracing::error!("[EMOJI HANDLER ERROR] {e:#}");
            msg(500, &e.to_string())
        }
    }
}

async fn edit_emoji(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        tracing::warn!("[CSRF] Blocked emoji edit for user {}. Invalid token.", session.user);
        return msg(403, "Invalid CSRF token");
    }
    let Some(id) = id_param(&req) else { return msg(404, "Emoji not found") };
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_EMOJI_UPLOAD, "message").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let name = trimmed(form.text("name")).to_lowercase();
    let mut url = trimmed(form.text("url"));
    if name.is_empty() {
        return msg(400, "Emoji name is required");
    }
    if !valid_emoji_name(&name) {
        return msg(400, "Invalid name. Use lowercase a-z, 0-9, _, - only.");
    }
    let r: anyhow::Result<Res> = async {
        let Some(current) = sql!("SELECT id, name, url FROM custom_emojis WHERE id = ", id, " LIMIT 1").fetch_optional(&st.db).await? else {
            return Ok(msg(404, "Emoji not found"));
        };
        if name != current.string("name") {
            let conflict = sql!("SELECT id FROM custom_emojis WHERE name = ", &name, " AND id != ", id, " LIMIT 1").fetch_optional(&st.db).await?;
            if conflict.is_some() {
                return Ok(msg(400, "Emoji name already exists"));
            }
        }
        if let Some(file) = form.file("file").filter(|f| f.size > 0) {
            let new_url = store_emoji_file(&st, &file.path).await?;
            if let Some(old) = local_emoji_file(current.str("url").unwrap_or("")) {
                let _ = tokio::fs::remove_file(cfg.paths.emojis.join(old)).await;
            }
            url = new_url;
        }
        if url.is_empty() {
            url = current.string("url");
        }
        let row = sql!("UPDATE custom_emojis SET name = ", &name, ", url = ", &url, " WHERE id = ", id, " RETURNING id, name, url").fetch_optional(&st.db).await?;
        notify_emojis(&st).await;
        Ok(send(json!({ "success": true, "emoji": row }), 200))
    }
    .await;
    match r {
        Ok(r) => r,
        Err(e) if pg_code(&e).as_deref() == Some("23505") => msg(400, "Emoji name already exists"),
        Err(e) => {
            tracing::error!("[EMOJI EDIT ERROR] {e:#}");
            msg(500, &e.to_string())
        }
    }
}

// ── Telegram sticker pack import (sticker_pack_handler.mjs) ──────────────────

/// Largest sticker file we download (Telegram stickers are ≤ 512 KB static / 256 KB video).
const MAX_STICKER_BYTES: usize = 10 * 1024 * 1024;
const TG_TIMEOUT: Duration = Duration::from_secs(30);

/// `t.me/addstickers/<name>` URL or bare name → the pack's short name.
pub(crate) fn tg_pack_name(input: &str) -> String {
    let t = input.trim();
    let lower = t.to_ascii_lowercase();
    for p in ["https://t.me/addstickers/", "http://t.me/addstickers/"] {
        if lower.starts_with(p) {
            return t[p.len()..].to_string();
        }
    }
    t.to_string()
}

async fn tg_json(st: &AppState, url: &str) -> anyhow::Result<Value> {
    let resp = st.http.get(url).timeout(TG_TIMEOUT).send().await?;
    let bytes = read_limited(resp, 2 * 1024 * 1024).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

async fn read_limited(mut resp: reqwest::Response, max: usize) -> anyhow::Result<Vec<u8>> {
    if resp.content_length().map(|l| l as usize > max).unwrap_or(false) {
        anyhow::bail!("response too large");
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        out.extend_from_slice(&chunk);
        if out.len() > max {
            anyhow::bail!("response too large");
        }
    }
    Ok(out)
}

/// Download a Telegram file (`file_path` from getFile) into `dir`.
async fn tg_download(st: &AppState, token: &str, file_path: &str, dest: &Path) -> anyhow::Result<()> {
    let url = format!("https://api.telegram.org/file/bot{token}/{file_path}");
    let resp = st.http.get(&url).timeout(TG_TIMEOUT).send().await?;
    if resp.status().as_u16() >= 400 {
        // Never echo the URL: it contains the bot token
        anyhow::bail!("HTTP {} fetching sticker file", resp.status().as_u16());
    }
    let data = read_limited(resp, MAX_STICKER_BYTES).await?;
    tokio::fs::write(dest, data).await?;
    Ok(())
}

/// `getFile` → the file's path on Telegram's file server.
async fn tg_file_path(st: &AppState, token: &str, file_id: &str) -> anyhow::Result<String> {
    let data = tg_json(st, &format!("https://api.telegram.org/bot{token}/getFile?file_id={}", crate::util::encode_uri_component(file_id))).await?;
    match data["result"]["file_path"].as_str() {
        Some(p) if data["ok"] == Value::Bool(true) && !p.is_empty() => Ok(p.to_string()),
        _ => anyhow::bail!("getFile failed: {}", data["description"].as_str().unwrap_or("no file_path")),
    }
}

fn file_ext(path: &str) -> String {
    match path.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()) => e.to_ascii_lowercase(),
        _ => "webp".into(),
    }
}

/// Download one sticker and store it (video stickers as `.webm`, the rest converted to WebP).
async fn save_sticker(st: &AppState, work: &Path, token: &str, file_id: &str, is_video: bool) -> anyhow::Result<String> {
    let cfg = st.cfg();
    let file_path = tg_file_path(st, token, file_id).await?;
    let tmp = work.join(format!("dl_{}", crate::util::random_hex(8)));
    let _guard = crate::media::TempPath::new(tmp.clone());
    tg_download(st, token, &file_path, &tmp).await?;
    tokio::fs::create_dir_all(&cfg.paths.emojis).await?;
    if is_video {
        let mime = crate::media::mime::detect(&tmp).await?;
        if file_ext(&file_path) != "webm" || mime != "video/webm" {
            anyhow::bail!("unexpected video sticker type: {mime}");
        }
        let name = format!("{}.webm", crate::util::random_hex(24));
        let dest = cfg.paths.emojis.join(&name);
        crate::media::move_file(&tmp, &dest).await?;
        if file_size(&dest).await == 0 {
            anyhow::bail!("Saved file is empty");
        }
        crate::media::make_public(&dest).await;
        return Ok(format!("/s/emojis/{name}"));
    }
    let name = format!("{}.webp", crate::util::random_hex(24));
    convert_to_webp(&cfg, &tmp, &cfg.paths.emojis.join(&name)).await?;
    Ok(format!("/s/emojis/{name}"))
}

async fn import_telegram_pack(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        return msg(403, "Invalid CSRF token");
    }
    let body: Value = match req.body_bytes(64 * 1024).await.ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v) => v,
        None => return msg(400, "Invalid JSON body"),
    };
    let Some(pack_name) = body.get("pack_name").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return msg(400, "pack_name is required");
    };
    let cfg = st.cfg();
    let token = cfg
        .get("clients")
        .as_array()
        .and_then(|a| a.iter().find(|c| c["type"] == "tg"))
        .and_then(|c| c["token"].as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let Some(token) = token else { return msg(500, "Telegram bot not configured") };
    let tg_name = tg_pack_name(pack_name);
    if tg_name.is_empty() {
        return msg(400, "Could not extract pack name from URL");
    }
    tracing::info!("[STICKER PACK] Importing Telegram pack: {tg_name}");

    match sql!("SELECT id FROM sticker_packs WHERE tg_name = ", &tg_name, " LIMIT 1").fetch_optional(&st.db).await {
        Ok(Some(r)) => return msg(409, &format!("Pack \"{tg_name}\" was already imported (ID {})", r.i64("id").unwrap_or(0))),
        Ok(None) => {}
        Err(e) => {
            tracing::error!("[STICKER PACK] {e}");
            return msg(500, "Database error");
        }
    }

    let set = match tg_json(&st, &format!("https://api.telegram.org/bot{token}/getStickerSet?name={}", crate::util::encode_uri_component(&tg_name))).await {
        Ok(d) if d["ok"] == Value::Bool(true) => d["result"].clone(),
        Ok(d) => return msg(400, &format!("Telegram API error: {}", d["description"].as_str().unwrap_or("unknown"))),
        Err(e) => {
            tracing::error!("[STICKER PACK] Telegram API fetch failed: {e}");
            // reqwest errors can contain the URL (and with it the bot token)
            return msg(500, "Failed to contact Telegram API");
        }
    };
    let set_title = set["title"].as_str().unwrap_or("").to_string();
    let display = body.get("display_name").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let pack_title = display.map(str::to_string).or_else(|| Some(set_title.clone()).filter(|s| !s.is_empty())).unwrap_or_else(|| tg_name.clone());
    let stickers = set["stickers"].as_array().cloned().unwrap_or_default();
    tracing::info!("[STICKER PACK] Found {} stickers in \"{set_title}\"", stickers.len());

    let new_pack = match sql!(
        "INSERT INTO sticker_packs (name, tg_name, tg_title, sticker_count) VALUES (", &pack_title, ", ", &tg_name, ", ",
        Some(set_title.clone()).filter(|_| set["title"].is_string()), ", ", stickers.len() as i32, ") RETURNING id, name, tg_name, tg_title"
    )
    .fetch_optional(&st.db)
    .await
    {
        Ok(Some(r)) => r,
        Ok(None) => return msg(500, "Database error"),
        Err(e) => {
            tracing::error!("[STICKER PACK] {e}");
            return msg(500, "Database error");
        }
    };
    let pack_id = new_pack.i64("id").unwrap_or(0) as i32;
    let work = match crate::media::WorkDir::new(&cfg, "stickers").await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("[STICKER PACK] {e}");
            return msg(500, "Import failed");
        }
    };

    // Static pack thumbnail from the first sticker's thumbnail
    if let Some(fid) = stickers.first().and_then(|s| s["thumbnail"]["file_id"].as_str()) {
        match save_sticker(&st, &work.path, &token, fid, false).await {
            Ok(url) => {
                let _ = sql!("UPDATE sticker_packs SET thumb_url = ", &url, " WHERE id = ", pack_id).execute(&st.db).await;
                tracing::info!("[STICKER PACK] Pack thumbnail saved: {url}");
            }
            Err(e) => tracing::warn!("[STICKER PACK] Could not save pack thumbnail: {e}"),
        }
    }

    let mut safe_name = pack_slug(&pack_title, true);
    if safe_name.is_empty() {
        safe_name = tg_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '_' }).collect();
    }
    let (mut imported, mut failed) = (0i64, 0i64);
    let mut errors: Vec<String> = vec![];
    for (i, sticker) in stickers.iter().enumerate() {
        if sticker["is_animated"] == Value::Bool(true) {
            tracing::info!("[STICKER PACK] Skipping animated (TGS) sticker {}", i + 1);
            continue;
        }
        let is_video = sticker["is_video"] == Value::Bool(true);
        let r: anyhow::Result<String> = async {
            let fid = sticker["file_id"].as_str().ok_or_else(|| anyhow::anyhow!("missing file_id"))?;
            let url = save_sticker(&st, &work.path, &token, fid, is_video).await?;
            let emoji_name = format!("{safe_name}_{:03}", i + 1);
            let taken = sql!("SELECT id FROM custom_emojis WHERE name = ", &emoji_name, " LIMIT 1").fetch_optional(&st.db).await?.is_some();
            let final_name = if taken { format!("{emoji_name}_{}", crate::util::random_hex(3)) } else { emoji_name };
            sql!("INSERT INTO custom_emojis (name, url, pack_id) VALUES (", &final_name, ", ", &url, ", ", pack_id, ")").execute(&st.db).await?;
            Ok(final_name)
        }
        .await;
        match r {
            Ok(n) => {
                imported += 1;
                tracing::info!("[STICKER PACK] Imported sticker {}/{}: {n} ({})", i + 1, stickers.len(), if is_video { "video" } else { "static" });
            }
            Err(e) => {
                tracing::error!("[STICKER PACK] Failed sticker {}: {e}", i + 1);
                errors.push(format!("Sticker {}: {e}", i + 1));
                failed += 1;
            }
        }
        if i + 1 < stickers.len() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let _ = sql!("UPDATE sticker_packs SET sticker_count = ", imported as i32, " WHERE id = ", pack_id).execute(&st.db).await;
    notify_emojis(&st).await;
    tracing::info!("[STICKER PACK] Done. Imported: {imported}, Failed: {failed}");
    let mut pack = new_pack;
    pack.insert("sticker_count".into(), json!(imported));
    send(json!({ "success": true, "pack": pack, "imported": imported, "failed": failed, "errors": errors }), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_slugs() {
        assert_eq!(pack_slug("My Cool Pack", true), "my_cool_pack");
        assert_eq!(pack_slug("  Hi!! there  ", true), "hi_there");
        assert_eq!(pack_slug("Ünïcode", true), "n_code");
        // rename: only the first of leading/trailing underscore goes (old regex without /g)
        assert_eq!(pack_slug("_abc_", false), "abc_");
        assert_eq!(pack_slug("abc!", false), "abc");
        assert_eq!(pack_slug("!!!", true), "");
    }

    #[test]
    fn rename_naming() {
        assert_eq!(renamed_emoji("old_pack_7", "new").as_deref(), Some("new_007"));
        assert_eq!(renamed_emoji("old_pack_1234", "new").as_deref(), Some("new_1234"));
        assert_eq!(renamed_emoji("old_pack_007", "new").as_deref(), Some("new_007"));
        assert_eq!(renamed_emoji("custom", "new"), None);
        assert_eq!(renamed_emoji("old_x7", "new"), None);
    }

    #[test]
    fn tg_names() {
        assert_eq!(tg_pack_name("https://t.me/addstickers/Foo_Bar"), "Foo_Bar");
        assert_eq!(tg_pack_name("HTTP://T.ME/addstickers/x"), "x");
        assert_eq!(tg_pack_name(" plain "), "plain");
        assert_eq!(file_ext("stickers/file_1.WEBM"), "webm");
        assert_eq!(file_ext("noext"), "webp");
    }

    #[test]
    fn grouping() {
        let row = |v: Value| v.as_object().unwrap().clone();
        let rows = vec![
            row(json!({"id": 1, "name": "a", "url": "/a", "pack_id": null})),
            row(json!({"id": 2, "name": "b", "url": "/b", "pack_id": 5, "pack_name": "", "tg_title": "T", "tg_name": "t", "pack_thumb_url": null})),
            row(json!({"id": 3, "name": "c", "url": "/c", "pack_id": 9, "pack_name": null, "tg_title": null})),
        ];
        let (flat, packs) = group_emojis(&rows);
        assert_eq!(flat.len(), 3);
        assert_eq!(packs.len(), 3);
        assert_eq!(packs[0]["name"], "Custom Emojis");
        assert!(packs[0].get("thumb_url").is_none());
        assert_eq!(packs[1]["name"], "T");
        assert_eq!(packs[2]["name"], "Pack #9");
        let (_, packs) = group_emojis(&rows[1..]);
        assert_eq!(packs.len(), 2, "empty standalone group is dropped");
    }
}
