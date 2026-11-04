//! Köpfe uploads (src/koepfe_handler.mjs). The navbar brand image lives in routes/profile_media.rs.

use super::{admin_session, csrf_header_ok, read_form, send};
use crate::http::{Req, Res, Router};
use crate::state::AppState;
use axum::http::Method;
use serde_json::json;

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/admin\/koepfe\/upload$", None, koepfe_upload);
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
}
