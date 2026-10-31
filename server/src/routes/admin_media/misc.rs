//! Köpfe uploads (src/koepfe_handler.mjs) and the navbar brand image (src/brand_image_handler.mjs).

use super::{admin_session, csrf_header_ok, file_size, magick, read_form, send};
use crate::http::{Req, Res, Router};
use crate::state::AppState;
use axum::http::Method;
use serde_json::json;

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/admin\/koepfe\/upload$", None, koepfe_upload);
    r.pre(Method::POST, r"^\/admin\/brand_image\/upload$", None, brand_upload);
    r.pre(Method::POST, r"^\/admin\/brand_image\/delete$", None, brand_delete);
}

fn msg(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "msg": m }), code)
}

// ── Köpfe ────────────────────────────────────────────────────────────────────

const KOEPFE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif"];
const MAX_KOEPFE: u64 = 20 * 1024 * 1024 + 64 * 1024;

/// `path.extname(name).toLowerCase()` (a leading dot alone is no extension).
pub(crate) fn extname(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    match base.rfind('.') {
        Some(i) if i > 0 => base[i..].to_lowercase(),
        _ => String::new(),
    }
}

async fn koepfe_upload(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Forbidden") };
    let cfg = st.cfg();
    // The admin page sends the token as a header; the old handler read a `csrf_token` form field.
    let header_ok = csrf_header_ok(&req, &session);
    let form = match read_form(&mut req, &cfg, MAX_KOEPFE, "msg").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let field_ok = match (form.text("csrf_token"), session.csrf_token.as_deref().filter(|t| !t.is_empty())) {
        (Some(t), Some(expected)) => crate::util::ct_eq(t, expected),
        _ => false,
    };
    if !header_ok && !field_ok {
        return msg(403, "Invalid CSRF");
    }
    let r: anyhow::Result<String> = async {
        let file = form.file("file").filter(|f| f.size > 0).ok_or_else(|| anyhow::anyhow!("No file data"))?;
        let ext = extname(if file.filename.is_empty() { ".png" } else { &file.filename });
        if !KOEPFE_EXTS.contains(&ext.as_str()) {
            anyhow::bail!("Unsupported format");
        }
        let mime = crate::media::mime::detect(&file.path).await?;
        if !super::RASTER_IMAGES.contains(&mime.as_str()) {
            anyhow::bail!("Unsupported format");
        }
        tokio::fs::create_dir_all(&cfg.paths.koepfe).await?;
        let name = format!("{}{ext}", crate::util::random_hex(8));
        let dest = cfg.paths.koepfe.join(&name);
        tokio::fs::copy(&file.path, &dest).await?;
        crate::media::make_public(&dest).await;
        Ok(format!("/s/koepfe/{name}"))
    }
    .await;
    match r {
        Ok(url) => send(json!({ "success": true, "file": url }), 200),
        Err(e) => {
            tracing::error!("[KOEPFE] {e:#}");
            msg(500, &super::err_msg(&e))
        }
    }
}

// ── Brand image ──────────────────────────────────────────────────────────────

const MAX_BRAND: u64 = 5 * 1024 * 1024;

/// Admin session with a valid `X-CSRF-Token` header (`getAdminSession`).
fn brand_session(req: &Req) -> Result<crate::session::Session, Res> {
    let Some(s) = admin_session(req) else { return Err(msg(401, "Unauthorized")) };
    if !csrf_header_ok(req, &s) {
        tracing::warn!("[CSRF] Blocked brand image request for user {}. Invalid token.", s.user);
        return Err(msg(403, "Invalid CSRF token"));
    }
    Ok(s)
}

/// SVGs are served as-is from `/s/img/navbar/`, so refuse anything that could run script when the
/// file is opened directly (defense in depth; only admins can upload).
pub(crate) fn svg_is_safe(svg: &str) -> bool {
    let lower = svg.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return false;
    }
    if ["<script", "javascript:", "<foreignobject", "<iframe", "<embed", "<object"].iter().any(|b| lower.contains(b)) {
        return false;
    }
    // on…= event handler attributes (onload=, onclick = …)
    let b = lower.as_bytes();
    let mut i = 0;
    while i + 2 < b.len() {
        if b[i] == b'o' && b[i + 1] == b'n' && (i == 0 || b[i - 1].is_ascii_whitespace() || b[i - 1] == b'/' || b[i - 1] == b'"' || b[i - 1] == b'\'') {
            let mut j = i + 2;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-' || b[j] == b'_') {
                j += 1;
            }
            let mut k = j;
            while k < b.len() && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if j > i + 2 && k < b.len() && b[k] == b'=' {
                return false;
            }
        }
        i += 1;
    }
    true
}

/// Remove the file behind the current brand image URL (only our own `/s/img/navbar/brand.*` files).
async fn delete_old_brand_file(st: &AppState) {
    let current = st.settings.brand_image_url();
    let path = current.split('?').next().unwrap_or("");
    if !path.starts_with("/s/img/navbar/brand.") {
        return;
    }
    let Some(name) = path.rsplit('/').next().filter(|n| crate::media::is_plain_name(n)) else { return };
    let _ = tokio::fs::remove_file(st.cfg().paths.s.join("img/navbar").join(name)).await;
}

async fn persist_brand(st: &AppState, url: &str) -> anyhow::Result<()> {
    st.settings.update(|s| s.brand_image_url = url.to_string());
    crate::settings::save(st, "brand_image_url", url).await
}

async fn brand_upload(st: AppState, mut req: Req) -> Res {
    let session = match brand_session(&req) {
        Ok(s) => s,
        Err(r) => return r,
    };
    let cfg = st.cfg();
    if crate::media::multipart::boundary(req.content_type()).is_none() {
        return msg(400, "Invalid content type — multipart boundary missing");
    }
    let form = match read_form(&mut req, &cfg, 10 * 1024 * 1024, "msg").await {
        Ok(f) => f,
        Err(r) if r.status().as_u16() == 413 => return msg(413, "File too large (5 MB max)"),
        Err(r) => return Ok(r),
    };
    let Some(file) = form.file("file").filter(|f| f.size > 0) else { return msg(400, "No file provided") };
    if file.size > MAX_BRAND {
        return msg(400, &format!("File too large. Maximum is 5 MB, got {:.2} MB", file.size as f64 / 1024.0 / 1024.0));
    }
    let mime = file.content_type.to_lowercase().split(';').next().unwrap_or("").trim().to_string();
    if !["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp", "image/svg+xml"].contains(&mime.as_str()) {
        return msg(400, &format!("Invalid file type. Allowed: gif, jpg, png, webp, svg. Got: {mime}"));
    }
    let r: anyhow::Result<Res> = async {
        let img_dir = cfg.paths.s.join("img/navbar");
        tokio::fs::create_dir_all(&img_dir).await?;
        let is_svg = mime == "image/svg+xml";
        let ts = crate::util::now_ms();
        let out_name = if is_svg { format!("brand.{ts}.svg") } else { format!("brand.{ts}.webp") };
        let final_path = img_dir.join(&out_name);
        if is_svg {
            let text = tokio::fs::read_to_string(&file.path).await.unwrap_or_default();
            if !svg_is_safe(&text) {
                return Ok(msg(400, "SVG contains scripts or event handlers"));
            }
            tokio::fs::copy(&file.path, &final_path).await?;
        } else {
            let actual = crate::media::mime::detect(&file.path).await?;
            if !["image/gif", "image/jpeg", "image/png", "image/webp"].contains(&actual.as_str()) {
                return Ok(msg(400, &format!("Invalid file type detected: {actual}")));
            }
            let (s, d) = (file.path.display().to_string(), final_path.display().to_string());
            if let Err(e) = magick(&cfg, &[&s, "-coalesce", "-quality", "85", &d]).await {
                tracing::error!("[BRAND UPLOAD] ImageMagick error: {e}");
                let _ = tokio::fs::remove_file(&final_path).await;
                return Ok(msg(500, "Failed to process image (ImageMagick required)"));
            }
        }
        if file_size(&final_path).await == 0 {
            let _ = tokio::fs::remove_file(&final_path).await;
            anyhow::bail!("output file is empty");
        }
        crate::media::make_public(&final_path).await;
        delete_old_brand_file(&st).await;
        let url = format!("/s/img/navbar/{out_name}");
        persist_brand(&st, &url).await?;
        let _ = crate::sql!("SELECT pg_notify('brand_image', ", json!({ "url": url }).to_string(), ")").execute(&st.db).await;
        crate::audit::log(&st, session.id, "update_brand_image", "system", "0".to_string(), json!({ "url": url })).await;
        tracing::info!("[BRAND UPLOAD] Done: {url}");
        Ok(send(json!({ "success": true, "url": url, "msg": "Brand image updated" }), 200))
    }
    .await;
    r.unwrap_or_else(|e| {
        tracing::error!("[BRAND UPLOAD ERROR] {e:#}");
        msg(500, &format!("Upload failed: {}", super::err_msg(&e)))
    })
}

async fn brand_delete(st: AppState, req: Req) -> Res {
    let session = match brand_session(&req) {
        Ok(s) => s,
        Err(r) => return r,
    };
    delete_old_brand_file(&st).await;
    if let Err(e) = persist_brand(&st, "").await {
        tracing::error!("[BRAND DELETE ERROR] {e:#}");
        return msg(500, &format!("Delete failed: {}", super::err_msg(&e)));
    }
    let _ = crate::sql!("SELECT pg_notify('brand_image', ", json!({ "url": null }).to_string(), ")").execute(&st.db).await;
    crate::audit::log(&st, session.id, "delete_brand_image", "system", "0".to_string(), json!({})).await;
    send(json!({ "success": true, "msg": "Brand image removed" }), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_names() {
        assert_eq!(extname("a.PNG"), ".png");
        assert_eq!(extname(".png"), "");
        assert_eq!(extname("noext"), "");
        assert_eq!(extname("x/y.tar.gz"), ".gz");
    }

    #[test]
    fn svg_check() {
        assert!(svg_is_safe(r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="10" height="10" fill="red"/></svg>"#));
        assert!(!svg_is_safe(r#"<svg><script>alert(1)</script></svg>"#));
        assert!(!svg_is_safe(r#"<svg onload="alert(1)"></svg>"#));
        assert!(!svg_is_safe(r#"<svg><rect ONCLICK = "x"/></svg>"#));
        assert!(!svg_is_safe(r#"<svg><a href="javascript:alert(1)">x</a></svg>"#));
        assert!(!svg_is_safe(r#"<svg><foreignObject></foreignObject></svg>"#));
        assert!(!svg_is_safe("not an svg"));
        // words containing "on" are fine
        assert!(svg_is_safe(r#"<svg><text font-family="monospace">only one</text></svg>"#));
    }
}
