//! `POST /api/v2/items/:id/thumbnail` (`src/rethumb_handler.mjs`): custom thumbnail for Flash items.

use super::{csrf_header_ok, declared_len, fail, send};
use crate::db::RowExt;
use crate::http::{Req, Res};
use crate::media::multipart::{self, MpError};
use crate::media::{mime, queue};
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use serde_json::json;

const MAX: u64 = 5 * 1024 * 1024;
const DECLARED: &[&str] = &["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp"];
const DETECTED: &[&str] = &["image/gif", "image/jpeg", "image/png", "image/webp"];

pub async fn handle_rethumb_upload(st: AppState, mut req: Req) -> Res {
    let id_param = req.param("id").unwrap_or("").to_string();
    tracing::info!("[RETHUMB HANDLER] Upload started for item: {id_param}");
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    if session.csrf_token.as_deref().is_some_and(|t| !t.is_empty()) && !csrf_header_ok(&req, &session) {
        tracing::warn!("[CSRF] Blocked rethumb upload for user {}. Invalid token.", session.user);
        return fail(403, "Invalid CSRF token");
    }
    // `+itemId`: non-numeric ids never match an item
    let Ok(item_id) = id_param.parse::<i32>() else { return fail(404, "Item not found") };
    let item = match sql!("select id, username, mime, active from items where id = ", item_id, " limit 1").fetch_optional(&st.db).await {
        Ok(Some(r)) => r,
        Ok(None) => return fail(404, "Item not found"),
        Err(e) => {
            tracing::error!("[RETHUMB HANDLER] Database error: {e}");
            return fail(500, "Database error");
        }
    };
    let owner = crate::settings::session_owner_name(Some(&session)).unwrap_or_default();
    if !session.admin && !session.is_moderator && owner != item.string("username") {
        return fail(403, "Insufficient permissions");
    }
    if !mime::is_flash(&item.string("mime")) {
        return fail(400, "Only Flash uploads support custom thumbnails at this time");
    }

    let cfg = st.cfg();
    let ct = req.content_type().to_string();
    if multipart::boundary(&ct).is_none() {
        return fail(400, "Invalid content type");
    }
    let body = req.take_body().unwrap_or_else(Body::empty);
    // The multipart overhead is small; anything beyond 5 MB + 64 KB can't hold a valid file.
    let form = match multipart::parse_small(body, &ct, declared_len(&req), &cfg.paths.tmp, MAX + 64 * 1024).await {
        Ok(f) => f,
        Err(MpError::TooLarge) => return fail(413, "File too large (5 MB max)"),
        Err(e) => {
            tracing::warn!("[RETHUMB HANDLER] Malformed body: {e}");
            return fail(400, "No file provided");
        }
    };
    let Some(file) = form.file("file") else { return fail(400, "No file provided") };
    if file.size > MAX {
        return fail(400, &format!("File too large. Maximum size is 5MB, got {:.2}MB", file.size as f64 / 1024.0 / 1024.0));
    }
    let declared = file.content_type.to_lowercase();
    if !DECLARED.contains(&declared.as_str()) {
        return fail(400, &format!("Invalid file type. Allowed: gif, jpg, jpeg, png, webp. Got: {declared}"));
    }
    let actual = mime::detect(&file.path).await?;
    if !DETECTED.contains(&actual.as_str()) {
        return fail(400, &format!("Invalid file type detected: {actual}"));
    }

    let active = item.bool("active");
    let id = item.i64("id").unwrap_or(item_id as i64);
    let (_, t_dir, _) = queue::dirs(&cfg, !active).await;
    let final_path = t_dir.join(format!("{id}.webp"));
    let src = file.path.to_string_lossy().into_owned();
    let r = crate::proc::spawn("magick", &[&src, "-coalesce", "-resize", "512x512^", "-gravity", "center", "-crop", "512x512+0+0", "+repage", &final_path.to_string_lossy()]).await;
    if let Err(e) = r {
        tracing::error!("[RETHUMB HANDLER] Magick error: {e}");
        return fail(500, "Failed to process image");
    }
    queue::gen_blurred_thumbnail(&cfg, id, !active).await;
    if let Err(e) = sql!("SELECT pg_notify('rethumb', ", json!({ "item_id": id }).to_string(), ")").execute(&st.db).await {
        tracing::error!("[RETHUMB HANDLER] SSE notify error: {e}");
    }
    tracing::info!("[RETHUMB HANDLER] Custom thumbnail applied to item {id}");
    send(json!({ "success": true, "msg": "Thumbnail updated successfully" }), 200)
}
