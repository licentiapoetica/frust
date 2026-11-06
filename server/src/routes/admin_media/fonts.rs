//! Admin font manager (`/admin/fonts`): upload website fonts into the fonts directory and delete
//! uploaded ones. Built-in fonts (`public/s/fonts-builtin`) are listed but can't be deleted; an
//! upload with the same file name replaces one. Uploaded fonts show up right away in every font
//! picker (user settings, admin → branding), see crate::fonts.
//!
//! Plain forms that also work without JavaScript: the upload is a `pre` route (multipart; CSRF
//! from the `X-CSRF-Token` header or the form's `csrf_token` field) answering with a redirect to
//! `back`, the delete a normal route (CSRF middleware, `back` redirect by crate::nojs).

use super::{admin_session, csrf_header_ok, read_form, send};
use crate::http::{Guard, Req, Res, Router, html, reply};
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

/// Largest upload (all files of one post together).
const MAX_UPLOAD: u64 = 50 * 1024 * 1024;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/fonts\/?$", Guard::Auth, fonts_page);
    r.get_g(r"^\/admin\/fonts\/preview\.css$", Guard::Auth, preview_css);
    r.pre(Method::POST, r"^\/admin\/fonts\/upload$", None, fonts_upload);
    r.post_g(r"^\/admin\/fonts\/delete$", Guard::Auth, font_delete);
}

fn fail(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "msg": m }), code)
}

async fn fonts_page(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let fams = crate::fonts::admin_json(&cfg);
    // Version of the preview stylesheet: changes with the font list
    let css_v = crate::fonts::preview_key(&fams.to_string());
    let weights: Vec<Value> = crate::fonts::WEIGHTS.iter().map(|w| json!({ "weight": w, "name": crate::fonts::weight_name(*w) })).collect();
    let data = json!({
        "session": req.session_value(),
        "font_families": fams,
        "font_weights": weights,
        "preview_css": format!("/admin/fonts/preview.css?v={css_v}"),
        "brand_font": st.settings.get().brand_font,
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/fonts", data)))
}

async fn preview_css(st: AppState, _req: Req) -> Res {
    let css = crate::fonts::preview_css(&st.cfg());
    Ok(crate::http::with_header(reply(200, "text/css; charset=utf-8", css), "cache-control", "private, max-age=86400"))
}

async fn fonts_upload(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return fail(401, "Unauthorized") };
    let ajax = crate::auth::is_ajax(&req);
    let header_ok = csrf_header_ok(&req, &session);
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_UPLOAD, "msg").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let field_ok = match (form.text("csrf_token"), session.csrf_token.as_deref().filter(|t| !t.is_empty())) {
        (_, None) => true,
        (Some(t), Some(expected)) => crate::util::ct_eq(t, expected),
        _ => false,
    };
    if !header_ok && !field_ok {
        tracing::warn!("[CSRF] Blocked font upload for user {}. Invalid token.", session.user);
        return fail(403, "Invalid CSRF token");
    }
    let back = form.text("back").and_then(crate::nojs::safe_back);
    let resp = store(&st, &session, &form).await;
    drop(form);
    crate::routes::profile_media::finish_form(&st, ajax, back, resp).await
}

/// Check and store every uploaded `file`. All files are checked before any is written, so a
/// bad one leaves the fonts directory as it was.
async fn store(st: &AppState, s: &crate::session::Session, form: &crate::media::multipart::Form) -> Res {
    let cfg = st.cfg();
    let files: Vec<_> = form.files("file").into_iter().filter(|f| f.size > 0).collect();
    if files.is_empty() {
        return fail(400, "No file provided");
    }
    let family = form.text("family").map(str::trim).filter(|f| !f.is_empty());
    if family.is_some_and(|f| crate::fonts::clean_family(f).is_empty()) {
        return fail(400, "The family name needs letters or digits (A-Z, 0-9)");
    }
    // A chosen weight only for a single file; several files keep the weights in their names
    let weight = form.text("weight").and_then(|w| w.trim().parse::<u16>().ok()).filter(|w| crate::fonts::WEIGHTS.contains(w));
    let weight = if files.len() == 1 { weight } else { None };

    let mut plan: Vec<(&std::path::Path, String)> = Vec::new();
    for f in &files {
        let mut head = [0u8; 4];
        let ok = match tokio::fs::File::open(&f.path).await {
            Ok(mut h) => tokio::io::AsyncReadExt::read_exact(&mut h, &mut head).await.is_ok(),
            Err(_) => false,
        };
        let Some(ext) = crate::fonts::sniff(&head).filter(|_| ok) else {
            return fail(400, &format!("{}: not a font (ttf, otf, woff or woff2)", f.filename));
        };
        let Some(name) = crate::fonts::upload_name(&f.filename, family, weight, ext) else {
            return fail(400, &format!("{}: can't tell the family from the file name, enter one", f.filename));
        };
        if let Some(why) = crate::fonts::upload_conflict(&cfg, &name) {
            return fail(409, &why);
        }
        if plan.iter().any(|(_, n)| crate::fonts::same_face(n, &name)) {
            return fail(409, &format!("Two files for {name}"));
        }
        plan.push((&f.path, name));
    }

    if let Err(e) = tokio::fs::create_dir_all(&cfg.paths.fonts).await {
        tracing::error!("[FONTS] {e}");
        return fail(500, "Upload failed");
    }
    let mut stored = Vec::new();
    for (src, name) in &plan {
        let dest = cfg.paths.fonts.join(name);
        if let Err(e) = tokio::fs::copy(src, &dest).await {
            tracing::error!("[FONTS] {name}: {e}");
            return fail(500, "Upload failed");
        }
        crate::media::make_public(&dest).await;
        stored.push(name.clone());
    }
    crate::social::helpers::audit_log(st, s.id, "upload_font", "system", "0", Some(json!({ "files": stored }))).await;
    let msg = match stored.as_slice() {
        [one] => format!("Font uploaded: {one}"),
        many => format!("{} fonts uploaded", many.len()),
    };
    send(json!({ "success": true, "files": stored, "msg": msg }), 200)
}

/// `POST /admin/fonts/delete` (`file`): remove an uploaded font. A user or the brand text still
/// set to it falls back to the default font.
async fn font_delete(st: AppState, mut req: Req) -> Res {
    let Some(s) = req.session.clone() else { return fail(401, "Unauthorized") };
    let file = req.post_str("file").await.unwrap_or_default();
    let file = file.trim();
    let cfg = st.cfg();
    if !crate::fonts::is_uploaded(&cfg, file) {
        return fail(400, "Not an uploaded font");
    }
    if let Err(e) = tokio::fs::remove_file(cfg.paths.fonts.join(file)).await {
        tracing::error!("[FONTS] delete {file}: {e}");
        return fail(500, "Delete failed");
    }
    crate::social::helpers::audit_log(&st, s.id, "delete_font", "system", "0", Some(json!({ "file": file }))).await;
    send(json!({ "success": true, "msg": format!("Font deleted: {file}") }), 200)
}
