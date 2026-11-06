//! Admin media: emojis and sticker packs (src/inc/routes/emojis.mjs, emoji_upload_handler.mjs,
//! sticker_pack_handler.mjs), meme templates (meme-manager.mjs, meme.mjs, meme_upload_handler.mjs),
//! Köpfe uploads (koepfe_handler.mjs) and the admin hall manager (hall_image_handler.mjs). The
//! navbar brand image is in routes/profile_media.rs. The font manager (`/admin/fonts`) is new.
//!
//! The upload handlers were "bypass" middleware in index.mjs that read the raw body and looked the
//! session up themselves; here they are `pre` routes that use the session the middleware loaded
//! (which also applies bans). They check the `X-CSRF-Token` header before reading the body.
//!
//! Changes from the old code:
//! * JSON answers go out as `application/json`. The old `res.reply({ headers, body })` ignored
//!   `headers`, so they were sent as `text/html` (a content-sniffing risk for admin-supplied names).
//! * Uploaded files are content-checked with `file --mime-type` (the meme and emoji handlers stored
//!   whatever arrived) and file names never come from user input.
//! * Hall slugs from the URL are only used for file paths when they are plain names (the old hall
//!   delete unlinked `<custom dir>/<decoded slug>.webp` without checking, so `..%2F` escaped it).

mod emojis;
mod fonts;
mod halls;
mod memes;
mod misc;

use crate::config::Config;
use crate::http::{Req, Res, Router, json_raw};
use crate::session::Session;
use serde_json::Value;
use std::path::Path;

pub fn register(r: &mut Router) {
    emojis::register(r);
    fonts::register(r);
    memes::register(r);
    halls::register(r);
    misc::register(r);
}

/// `sendJson(res, data, code)`: `application/json` without charset.
pub(crate) fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

/// `X-CSRF-Token` header equals the session's token (a session without a token never passes).
pub(crate) fn csrf_header_ok(req: &Req, session: &Session) -> bool {
    match (session.csrf_token.as_deref().filter(|t| !t.is_empty()), req.header("x-csrf-token")) {
        (Some(expected), Some(got)) => crate::util::ct_eq(got, expected),
        _ => false,
    }
}

/// The session when it belongs to an admin.
pub(crate) fn admin_session(req: &Req) -> Option<Session> {
    req.session().filter(|s| s.admin).cloned()
}

/// `Content-Length` when present and numeric.
pub(crate) fn declared_len(req: &Req) -> Option<u64> {
    req.header("content-length").and_then(|v| v.trim().parse().ok())
}

/// Postgres error code of a failed query (`err.code` in postgres.js). `db::Sql` turns sqlx errors
/// into message strings, so a unique violation is also recognised by its message.
pub(crate) fn pg_code(e: &anyhow::Error) -> Option<String> {
    let code = e
        .chain()
        .find_map(|c| c.downcast_ref::<sqlx::Error>())
        .and_then(|e| e.as_database_error())
        .and_then(|d| d.code().map(|c| c.into_owned()));
    code.or_else(|| e.to_string().contains("duplicate key value violates unique constraint").then(|| "23505".to_string()))
}

/// Error text for the client: the first line only (`db::Sql` errors append the query).
pub(crate) fn err_msg(e: &anyhow::Error) -> String {
    e.to_string().lines().next().unwrap_or("").to_string()
}

/// `magick` with the project's policy (`config/magick-policy` raises list-length for big GIFs).
pub(crate) async fn magick(cfg: &Config, args: &[&str]) -> anyhow::Result<crate::proc::Output> {
    let policy = cfg.base.join("config/magick-policy");
    let opts = crate::proc::Opts { env: vec![("MAGICK_CONFIGURE_PATH".into(), policy.display().to_string())], ..Default::default() };
    crate::proc::spawn_with("magick", args, opts).await
}

/// Image types the converters accept (checked by content, so ImageMagick never sees SVG/MVG/etc.).
pub(crate) const RASTER_IMAGES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif", "image/bmp", "image/apng"];

/// Size of a file, 0 when missing.
pub(crate) async fn file_size(p: &Path) -> u64 {
    tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0)
}

/// Convert an image to WebP (`magick <src> -coalesce -quality 80 <dst>`) after checking its content
/// type. Errors mean nothing usable was written.
pub(crate) async fn convert_to_webp(cfg: &Config, src: &Path, dst: &Path) -> anyhow::Result<()> {
    let mime = crate::media::mime::detect(src).await?;
    if !RASTER_IMAGES.contains(&mime.as_str()) {
        anyhow::bail!("Unsupported file type: {mime}");
    }
    let (s, d) = (src.display().to_string(), dst.display().to_string());
    if mime == "image/webp" {
        tokio::fs::copy(src, dst).await?;
    } else if let Err(e) = magick(cfg, &[&s, "-coalesce", "-quality", "80", &d]).await {
        let _ = tokio::fs::remove_file(dst).await;
        return Err(e);
    }
    if file_size(dst).await == 0 {
        let _ = tokio::fs::remove_file(dst).await;
        anyhow::bail!("File write/conversion verification failed");
    }
    crate::media::make_public(dst).await;
    Ok(())
}

/// `path.basename(url)` of a local `/s/emojis/` URL, only when it is a plain file name.
pub(crate) fn local_emoji_file(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("/s/emojis/")?;
    let name = rest.rsplit('/').next()?;
    crate::media::is_plain_name(name).then_some(name)
}

/// Parse a multipart upload into temp files under `paths.tmp`. `Err` is the response to send:
/// `{ success: false, <key>: msg }` with `key` = `message` or `msg` like the handler's old answers.
pub(crate) async fn read_form(req: &mut Req, cfg: &Config, max: u64, key: &str) -> Result<crate::media::multipart::Form, axum::response::Response> {
    use crate::media::multipart::{self, MpError};
    let ct = req.content_type().to_string();
    if multipart::boundary(&ct).is_none() {
        return Err(json_raw(&serde_json::json!({ "success": false, key: "Invalid content type" }), 400));
    }
    let _ = tokio::fs::create_dir_all(&cfg.paths.tmp).await;
    let len = declared_len(req);
    let body = req.take_body().unwrap_or_else(axum::body::Body::empty);
    match multipart::parse_small(body, &ct, len, &cfg.paths.tmp, max).await {
        Ok(f) => Ok(f),
        Err(MpError::TooLarge) => Err(json_raw(&serde_json::json!({ "success": false, key: format!("File too large ({} MB max)", max / (1024 * 1024)) }), 413)),
        Err(e) => {
            tracing::warn!("[ADMIN MEDIA] Malformed multipart body: {e}");
            Err(json_raw(&serde_json::json!({ "success": false, key: "Invalid multipart body" }), 400))
        }
    }
}

/// JS `String.prototype.trim` for an optional JSON/form string.
pub(crate) fn trimmed(v: Option<&str>) -> String {
    v.unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emoji_file_names() {
        assert_eq!(local_emoji_file("/s/emojis/abc.webp"), Some("abc.webp"));
        assert_eq!(local_emoji_file("/s/emojis/x/../y.webp"), Some("y.webp"));
        assert_eq!(local_emoji_file("/s/emojis/.."), None);
        assert_eq!(local_emoji_file("/s/emojis/"), None);
        assert_eq!(local_emoji_file("https://x/s/emojis/a.png"), None);
    }
}
