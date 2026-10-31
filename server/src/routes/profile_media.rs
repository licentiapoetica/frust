//! Profile media upload handlers registered as bypass routes like in index.mjs:
//! `/api/v2/settings/uploadAvatar` (src/avatar_handler.mjs), `/api/v2/settings/uploadBanner`
//! (src/banner_handler.mjs) and `/admin/brand_image/{upload,delete}` (src/brand_image_handler.mjs).
//!
//! The session middleware has already run (the old handlers looked the session up themselves), so
//! bans, anonymous-access rules etc. apply. The CSRF middleware skips these paths: the token is
//! accepted only from the `X-CSRF-Token` header and checked before the body is read.
//!
//! Changes from the old handlers (security):
//! * The body is streamed to temp files with a hard cap instead of buffering up to 150 MB.
//! * The banner's `file_orig` is content-checked like `file` before ImageMagick sees it, and the
//!   detected format is forced on ImageMagick (`png:/path`) so it never guesses a coder.
//! * `banner_position` / `banner_size` / `banner_repeat` are rendered into `style` attributes (and
//!   into HTML built by comments.js): only plain CSS keywords/percentages are stored now.
//! * Anonymous (passkey) sessions are refused like the `registeredUser` guard does; the old check
//!   only caught legacy `anon_` users.
//! * Error responses no longer echo internal error text.
//! * SVG brand images with scripts, event handlers or external references are refused.

use crate::db::RowExt;
use crate::http::{Req, Res, Router, json_raw};
use crate::media::multipart::{self, FilePart, MpError};
use crate::session::Session;
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use axum::http::Method;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MB: u64 = 1024 * 1024;
const MAX_IMAGE: u64 = 5 * MB;
const DECLARED: &[&str] = &["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp"];
const DETECTED: &[&str] = &["image/gif", "image/jpeg", "image/png", "image/webp"];

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/settings\/uploadAvatar$", None, avatar_upload);
    r.pre(Method::DELETE, r"^\/api\/v2\/settings\/uploadAvatar$", None, avatar_delete);
    r.pre(Method::POST, r"^\/admin\/brand_image\/upload$", None, brand_upload);
    r.pre(Method::POST, r"^\/admin\/brand_image\/delete$", None, brand_delete);
    r.pre(Method::POST, r"^\/api\/v2\/settings\/uploadBanner$", None, banner_upload);
    r.pre(Method::DELETE, r"^\/api\/v2\/settings\/uploadBanner$", None, banner_delete);
}

/// `sendJson(res, data, code)`: `application/json` without charset.
fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

fn fail(code: u16, msg: &str) -> Res {
    send(json!({ "success": false, "msg": msg }), code)
}

/// Session + registered-account + CSRF checks shared by the avatar and banner handlers.
fn registered_session(req: &Req, what: &str) -> Result<Session, Res> {
    let Some(s) = req.session.clone() else {
        tracing::info!("[{what} HANDLER] Unauthorized - No valid session found");
        return Err(fail(401, "Unauthorized"));
    };
    if s.is_anon || s.user.starts_with("anon_") {
        return Err(fail(403, "Action requires a registered account"));
    }
    if s.csrf_token.as_deref().is_some_and(|t| !t.is_empty()) && !crate::routes::uploads::csrf_header_ok(req, &s) {
        tracing::warn!("[CSRF] Blocked {} for user {}. Invalid token.", what.to_lowercase(), s.user);
        return Err(fail(403, "Invalid CSRF token"));
    }
    Ok(s)
}

fn refresh_session(st: &AppState, req: &Req) {
    if let Some(c) = req.cookie("session") {
        st.invalidate_session_cache(&crate::util::sha256(c));
    }
}

async fn notify(st: &AppState, channel: &str, payload: Value) {
    if let Err(e) = sql!("select pg_notify(", channel, ", ", payload.to_string(), ")").execute(&st.db).await {
        tracing::error!("[PROFILE MEDIA] pg_notify {channel} failed: {e}");
    }
}

/// Same shape as the old `genuuid()` (48 hex characters).
fn genuuid() -> String {
    crate::util::random_hex(24)
}

/// ImageMagick coder prefix for a detected MIME type.
fn coder(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/gif" => "gif",
        "image/jpeg" => "jpeg",
        "image/png" => "png",
        "image/webp" => "webp",
        _ => return None,
    })
}

/// Run `magick` with resource limits (decompression bombs) and a timeout.
async fn magick(args: &[&str]) -> anyhow::Result<()> {
    let mut all: Vec<&str> = vec!["-limit", "memory", "512MiB", "-limit", "map", "1GiB", "-limit", "width", "32KP", "-limit", "height", "32KP"];
    all.extend_from_slice(args);
    let opts = crate::proc::Opts { timeout: Duration::from_secs(120), ..Default::default() };
    crate::proc::spawn_with("magick", &all, opts).await?;
    Ok(())
}

/// Content check of an uploaded image; `Ok(coder)` or the 400 response.
async fn check_image(file: &FilePart) -> Result<&'static str, Res> {
    let actual = match crate::media::mime::detect(&file.path).await {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("[PROFILE MEDIA] file(1) failed: {e}");
            return Err(fail(500, "Failed to process image"));
        }
    };
    match coder(&actual).filter(|_| DETECTED.contains(&actual.as_str())) {
        Some(c) => Ok(c),
        None => Err(fail(400, &format!("Invalid file type detected: {actual}"))),
    }
}

fn declared_mime(file: &FilePart) -> String {
    file.content_type.to_lowercase().split(';').next().unwrap_or("").trim().to_string()
}

fn too_large(size: u64) -> String {
    format!("File too large. Maximum size is 5MB, got {:.2}MB", size as f64 / 1024.0 / 1024.0)
}

/// Parse the multipart body with a hard cap; `Err` is the response to send.
async fn read_form(st: &AppState, req: &mut Req, max: u64, no_boundary_msg: &str, too_large_msg: &str) -> Result<multipart::Form, Res> {
    let ct = req.content_type().to_string();
    if multipart::boundary(&ct).is_none() {
        return Err(fail(400, no_boundary_msg));
    }
    let tmp = st.cfg().paths.tmp.clone();
    let _ = tokio::fs::create_dir_all(&tmp).await;
    let body = req.take_body().unwrap_or_else(Body::empty);
    match multipart::parse_small(body, &ct, crate::routes::uploads::declared_len(req), &tmp, max).await {
        Ok(f) => Ok(f),
        Err(MpError::TooLarge) => Err(fail(413, too_large_msg)),
        Err(e) => {
            tracing::warn!("[PROFILE MEDIA] Malformed body: {e}");
            Err(fail(400, "No file provided"))
        }
    }
}

/// Remove a file of ours in `dir` named by a DB value (only plain file names).
async fn remove_in(dir: &Path, name: &str) {
    if crate::media::is_plain_name(name) {
        let _ = tokio::fs::remove_file(dir.join(name)).await;
    }
}

// ── avatar ──────────────────────────────────────────────────────────────────

async fn avatar_upload(st: AppState, mut req: Req) -> Res {
    tracing::info!("[AVATAR HANDLER] Upload started");
    let s = match registered_session(&req, "AVATAR") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let form = match read_form(&st, &mut req, MAX_IMAGE + 64 * 1024, "Invalid content type", "File too large (5 MB max for avatars)").await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(file) = form.file("file").filter(|f| f.size > 0) else { return fail(400, "No file provided") };
    if file.size > MAX_IMAGE {
        return fail(400, &too_large(file.size));
    }
    let declared = declared_mime(file);
    if !DECLARED.contains(&declared.as_str()) {
        return fail(400, &format!("Invalid file type. Allowed: gif, jpg, jpeg, png, webp. Got: {declared}"));
    }
    let coder = match check_image(file).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let cfg = st.cfg();
    let a_dir = cfg.paths.a.clone();
    let _ = tokio::fs::create_dir_all(&a_dir).await;
    let final_name = format!("{}.webp", genuuid());
    let final_path = a_dir.join(&final_name);
    let current = sql!("select avatar_file from user_options where user_id = ", s.id as i32)
        .fetch_optional(&st.db)
        .await?
        .and_then(|r| r.str("avatar_file").map(str::to_string));

    let input = format!("{coder}:{}", file.path.to_string_lossy());
    let out = final_path.to_string_lossy().into_owned();
    if let Err(e) = magick(&[&input, "-coalesce", "-resize", "256x256^", "-gravity", "center", "-background", "none", "-extent", "256x256", "-quality", "50", &out]).await {
        tracing::error!("[AVATAR HANDLER] Magick error: {e}");
        let _ = tokio::fs::remove_file(&final_path).await;
        return fail(500, "Failed to process image");
    }
    crate::media::make_public(&final_path).await;
    drop(form);

    if let Some(old) = current.filter(|c| !c.is_empty() && c != "default.png") {
        remove_in(&a_dir, &old).await;
    }
    // Clear the item-based avatar so the custom file takes priority
    let r = sql!("update user_options set avatar_file = ", final_name.clone(), ", avatar = NULL where user_id = ", s.id as i32).execute(&st.db).await;
    if let Err(e) = r {
        let _ = tokio::fs::remove_file(&final_path).await;
        return fail(500, &crate::util::log_error(&e, "Avatar Upload failed"));
    }
    refresh_session(&st, &req);
    notify(&st, "profile_update", json!({ "user_id": s.id, "user": s.user, "avatar_file": final_name, "avatar": null })).await;
    tracing::info!("[AVATAR HANDLER] Upload complete: {final_name}");
    send(json!({ "success": true, "avatar_file": final_name, "msg": "Avatar uploaded successfully" }), 200)
}

async fn avatar_delete(st: AppState, req: Req) -> Res {
    let s = match registered_session(&req, "AVATAR") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let r: anyhow::Result<()> = async {
        let current = sql!("select avatar_file from user_options where user_id = ", s.id as i32)
            .fetch_optional(&st.db)
            .await?
            .and_then(|r| r.str("avatar_file").map(str::to_string));
        if let Some(old) = current.filter(|c| !c.is_empty() && c != "default.png") {
            remove_in(&st.cfg().paths.a, &old).await;
        }
        sql!("update user_options set avatar_file = null where user_id = ", s.id as i32).execute(&st.db).await?;
        refresh_session(&st, &req);
        notify(&st, "profile_update", json!({ "user_id": s.id, "user": s.user, "avatar_file": null })).await;
        Ok(())
    }
    .await;
    match r {
        Ok(()) => send(json!({ "success": true, "msg": "Custom avatar removed" }), 200),
        Err(e) => {
            tracing::error!("[AVATAR DELETE ERROR] {e}");
            fail(500, "Failed to remove avatar")
        }
    }
}

// ── banner ──────────────────────────────────────────────────────────────────

/// A banner layout value safe inside a `style` attribute: CSS keywords, numbers and percentages.
fn css_value(v: Option<&str>, dflt: &str) -> String {
    match v {
        Some(s) if !s.is_empty() && s.len() <= 50 && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b' ' | b'.' | b'%' | b'-')) => s.to_string(),
        _ => dflt.to_string(),
    }
}

fn banner_disabled(st: &AppState) -> bool {
    st.cfg().is_false("websrv.user_banner_enabled")
}

/// `banner_x.webp` → `banner_x_orig.webp` (`replace('.webp', '_orig.webp')`).
fn orig_name(banner: &str) -> String {
    banner.replacen(".webp", "_orig.webp", 1)
}

async fn banner_upload(st: AppState, mut req: Req) -> Res {
    if banner_disabled(&st) {
        return send(json!({ "success": false, "msg": "Banner feature is currently disabled" }), 403);
    }
    let s = match registered_session(&req, "BANNER") {
        Ok(s) => s,
        Err(r) => return r,
    };
    // The cropped banner is ≤ 5 MB; the optional uncropped original may be bigger.
    let form = match read_form(&st, &mut req, 40 * MB, "Invalid content type", "File too large (5 MB max for banners)").await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(file) = form.file("file").filter(|f| f.size > 0) else { return fail(400, "No file provided") };
    if file.size > MAX_IMAGE {
        return fail(400, &too_large(file.size));
    }
    let declared = declared_mime(file);
    if !DECLARED.contains(&declared.as_str()) {
        return fail(400, &format!("Invalid file type. Allowed: gif, jpg, jpeg, png, webp. Got: {declared}"));
    }
    let coder = match check_image(file).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let cfg = st.cfg();
    let a_dir = cfg.paths.a.clone();
    let _ = tokio::fs::create_dir_all(&a_dir).await;
    let uuid = genuuid();
    let final_name = format!("banner_{uuid}.webp");
    let final_path = a_dir.join(&final_name);
    let input = format!("{coder}:{}[0]", file.path.to_string_lossy());
    if let Err(e) = magick(&[&input, "-resize", "3840x3840>", "-quality", "85", &final_path.to_string_lossy()]).await {
        tracing::error!("[BANNER HANDLER] Magick error: {e}");
        let _ = tokio::fs::remove_file(&final_path).await;
        return fail(500, "Failed to process image");
    }
    crate::media::make_public(&final_path).await;

    let mut orig_path: Option<PathBuf> = None;
    if let Some(orig) = form.file("file_orig").filter(|f| f.size > 0) {
        // The old code fed the original to ImageMagick unchecked
        match check_image(orig).await {
            Ok(c) => {
                let p = a_dir.join(format!("banner_{uuid}_orig.webp"));
                let input = format!("{c}:{}[0]", orig.path.to_string_lossy());
                match magick(&[&input, "-resize", "3840x3840>", "-quality", "85", &p.to_string_lossy()]).await {
                    Ok(()) => {
                        crate::media::make_public(&p).await;
                        orig_path = Some(p);
                    }
                    Err(e) => {
                        tracing::error!("[BANNER HANDLER] Magick error on original: {e}");
                        let _ = tokio::fs::remove_file(&p).await;
                    }
                }
            }
            Err(_) => tracing::warn!("[BANNER HANDLER] Ignoring original with unsupported content for user {}", s.user),
        }
    }
    let position = css_value(form.text("banner_position"), "center");
    let size = css_value(form.text("banner_size"), "cover");
    let repeat = css_value(form.text("banner_repeat"), "repeat");
    drop(form);

    let current = match sql!("select banner_file from user_options where user_id = ", s.id as i32).fetch_optional(&st.db).await {
        Ok(r) => r.and_then(|r| r.str("banner_file").map(str::to_string)),
        Err(e) => {
            tracing::error!("[BANNER HANDLER] Could not fetch current banner: {e}");
            None
        }
    };
    if let Some(old) = current.filter(|c| !c.is_empty()) {
        remove_in(&a_dir, &old).await;
        remove_in(&a_dir, &orig_name(&old)).await;
    }

    let r = sql!(
        "update user_options set banner_file = ", final_name.clone(), ", banner_position = ", position.clone(), ", banner_size = ", size.clone(),
        ", banner_repeat = ", repeat.clone(), " where user_id = ", s.id as i32
    )
    .execute(&st.db)
    .await;
    if let Err(e) = r {
        tracing::error!("[BANNER HANDLER] DB update failed: {e}");
        let _ = tokio::fs::remove_file(&final_path).await;
        if let Some(p) = orig_path {
            let _ = tokio::fs::remove_file(p).await;
        }
        return fail(500, "Banner upload failed");
    }
    refresh_session(&st, &req);
    notify(
        &st,
        "profile_update",
        json!({ "user_id": s.id, "user": s.user, "banner_file": final_name, "banner_position": position, "banner_size": size, "banner_repeat": repeat }),
    )
    .await;
    send(json!({ "success": true, "banner_file": final_name, "msg": "Banner uploaded successfully" }), 200)
}

async fn banner_delete(st: AppState, req: Req) -> Res {
    if banner_disabled(&st) {
        return send(json!({ "success": false, "msg": "Banner feature is currently disabled" }), 403);
    }
    let s = match registered_session(&req, "BANNER") {
        Ok(s) => s,
        Err(r) => return r,
    };
    let r: anyhow::Result<()> = async {
        let current = sql!("select banner_file from user_options where user_id = ", s.id as i32)
            .fetch_optional(&st.db)
            .await?
            .and_then(|r| r.str("banner_file").map(str::to_string));
        if let Some(old) = current.filter(|c| !c.is_empty()) {
            let a = st.cfg().paths.a.clone();
            remove_in(&a, &old).await;
            remove_in(&a, &orig_name(&old)).await;
        }
        sql!("update user_options set banner_file = null where user_id = ", s.id as i32).execute(&st.db).await?;
        refresh_session(&st, &req);
        notify(&st, "profile_update", json!({ "user_id": s.id, "user": s.user, "banner_file": null })).await;
        Ok(())
    }
    .await;
    match r {
        Ok(()) => send(json!({ "success": true, "msg": "Custom banner removed" }), 200),
        Err(e) => {
            tracing::error!("[BANNER DELETE ERROR] {e}");
            fail(500, "Failed to remove banner")
        }
    }
}

// ── brand image (admin) ─────────────────────────────────────────────────────

/// Admin session + CSRF header check (`getAdminSession`).
fn admin_session(req: &Req) -> Result<Session, Res> {
    let Some(s) = req.session.clone().filter(|s| s.admin) else { return Err(fail(401, "Unauthorized")) };
    if s.csrf_token.as_deref().is_some_and(|t| !t.is_empty()) && !crate::routes::uploads::csrf_header_ok(req, &s) {
        tracing::warn!("[CSRF] Blocked brand image request for user {}. Invalid token.", s.user);
        return Err(fail(403, "Invalid CSRF token"));
    }
    Ok(s)
}

fn navbar_dir(st: &AppState) -> PathBuf {
    st.cfg().paths.s.join("img").join("navbar")
}

/// Delete the file of the stored brand image URL, if it is one of ours.
async fn delete_old_brand_file(st: &AppState) {
    let current = st.settings.brand_image_url();
    let url_path = current.split('?').next().unwrap_or("");
    let Some(name) = url_path.strip_prefix("/s/img/navbar/") else { return };
    if !name.starts_with("brand.") || !crate::media::is_plain_name(name) {
        return;
    }
    let _ = tokio::fs::remove_file(navbar_dir(st).join(name)).await;
}

async fn persist_brand_image(st: &AppState, url: &str) -> anyhow::Result<()> {
    let u = url.to_string();
    st.settings.update(|s| s.brand_image_url = u);
    crate::settings::save(st, "brand_image_url", url).await
}

/// An SVG that can't run script or pull in other resources when opened directly.
fn svg_is_safe(text: &str) -> bool {
    static BAD: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
        regex::Regex::new(r#"(?i)<\s*script|<\s*foreignobject|<\s*iframe|<\s*embed|<\s*object|<!entity|<\?xml-stylesheet|javascript:|data:text/html|\son[a-z]+\s*=|@import|(?:href|src)\s*=\s*["']?\s*(?:https?:|//)"#).unwrap()
    });
    text.to_ascii_lowercase().contains("<svg") && !BAD.is_match(text)
}

async fn brand_upload(st: AppState, mut req: Req) -> Res {
    let s = match admin_session(&req) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let form = match read_form(&st, &mut req, 10 * MB, "Invalid content type — multipart boundary missing", "File too large (5 MB max)").await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let Some(file) = form.file("file").filter(|f| f.size > 0) else { return send(json!({ "success": false, "msg": "No file provided" }), 400) };
    if file.size > MAX_IMAGE {
        let msg = format!("File too large. Maximum is 5 MB, got {:.2} MB", file.size as f64 / 1024.0 / 1024.0);
        return send(json!({ "success": false, "msg": msg }), 400);
    }
    let mime = declared_mime(file);
    if !["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp", "image/svg+xml"].contains(&mime.as_str()) {
        return send(json!({ "success": false, "msg": format!("Invalid file type. Allowed: gif, jpg, png, webp, svg. Got: {mime}") }), 400);
    }
    let dir = navbar_dir(&st);
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        tracing::error!("[BRAND UPLOAD ERROR] {e}");
        return send(json!({ "success": false, "msg": "Upload failed" }), 500);
    }
    let is_svg = mime == "image/svg+xml";
    let ts = crate::util::now_ms();
    let out_name = if is_svg { format!("brand.{ts}.svg") } else { format!("brand.{ts}.webp") };
    let final_path = dir.join(&out_name);

    if !is_svg {
        // file(1) is required here (the old code skipped the check when it was missing)
        let coder = match check_image(file).await {
            Ok(c) => c,
            Err(r) => return r,
        };
        let input = format!("{coder}:{}", file.path.to_string_lossy());
        if let Err(e) = magick(&[&input, "-coalesce", "-quality", "85", &final_path.to_string_lossy()]).await {
            tracing::error!("[BRAND UPLOAD] ImageMagick error: {e}");
            let _ = tokio::fs::remove_file(&final_path).await;
            return send(json!({ "success": false, "msg": "Failed to process image (ImageMagick required)" }), 500);
        }
    } else {
        let text = tokio::fs::read(&file.path).await.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        if !svg_is_safe(&text) {
            return send(json!({ "success": false, "msg": "SVG contains scripts or external references" }), 400);
        }
        if let Err(e) = tokio::fs::copy(&file.path, &final_path).await {
            tracing::error!("[BRAND UPLOAD ERROR] {e}");
            return send(json!({ "success": false, "msg": "Upload failed" }), 500);
        }
    }
    crate::media::make_public(&final_path).await;
    drop(form);

    delete_old_brand_file(&st).await;
    let public_url = format!("/s/img/navbar/{out_name}");
    if let Err(e) = persist_brand_image(&st, &public_url).await {
        tracing::error!("[BRAND UPLOAD ERROR] {e}");
        return send(json!({ "success": false, "msg": "Upload failed" }), 500);
    }
    notify(&st, "brand_image", json!({ "url": public_url })).await;
    crate::social::helpers::audit_log(&st, s.id, "update_brand_image", "system", "0", Some(json!({ "url": public_url }))).await;
    send(json!({ "success": true, "url": public_url, "msg": "Brand image updated" }), 200)
}

async fn brand_delete(st: AppState, req: Req) -> Res {
    let s = match admin_session(&req) {
        Ok(s) => s,
        Err(r) => return r,
    };
    delete_old_brand_file(&st).await;
    if let Err(e) = persist_brand_image(&st, "").await {
        tracing::error!("[BRAND DELETE ERROR] {e}");
        return send(json!({ "success": false, "msg": "Delete failed" }), 500);
    }
    notify(&st, "brand_image", json!({ "url": null })).await;
    crate::social::helpers::audit_log(&st, s.id, "delete_brand_image", "system", "0", Some(json!({}))).await;
    send(json!({ "success": true, "msg": "Brand image removed" }), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_values() {
        assert_eq!(css_value(Some("center top"), "center"), "center top");
        assert_eq!(css_value(Some("50.5% 20%"), "center"), "50.5% 20%");
        assert_eq!(css_value(Some("100% auto"), "cover"), "100% auto");
        assert_eq!(css_value(Some("center; background:url(x)"), "center"), "center");
        assert_eq!(css_value(Some("\"><img src=x onerror=alert(1)>"), "cover"), "cover");
        assert_eq!(css_value(None, "repeat"), "repeat");
        assert_eq!(css_value(Some(""), "repeat"), "repeat");
    }

    #[test]
    fn svg_check() {
        assert!(svg_is_safe(r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#));
        assert!(svg_is_safe(r##"<svg><use href="#a"/></svg>"##));
        assert!(!svg_is_safe("<svg><script>alert(1)</script></svg>"));
        assert!(!svg_is_safe(r#"<svg onload="alert(1)"></svg>"#));
        assert!(!svg_is_safe(r#"<svg><image href="https://evil/x.png"/></svg>"#));
        assert!(!svg_is_safe(r#"<svg><a xlink:href="javascript:alert(1)">x</a></svg>"#));
        assert!(!svg_is_safe("<html>not svg</html>"));
    }

    #[test]
    fn orig_names() {
        assert_eq!(orig_name("banner_ab.webp"), "banner_ab_orig.webp");
    }
}
