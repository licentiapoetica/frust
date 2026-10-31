//! Meme templates: the admin manager (src/inc/routes/meme-manager.mjs), template upload/edit
//! (src/meme_upload_handler.mjs) and the meme creator pages (src/inc/routes/meme.mjs).

use super::{admin_session, csrf_header_ok, err_msg, file_size, pg_code, read_form, send, trimmed};
use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, html, raw};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/memes\/?$", Guard::Auth, admin_page);
    r.exact(Method::GET, "/api/v2/memes", None, list_memes);
    r.post(r"\/api\/v2\/admin\/memes\/(?<id>\d+)\/delete", delete_meme);
    // meme.mjs
    r.get_g(r"^\/meme$", Guard::UserAuth, select_page);
    r.get_g(r"^\/meme\/custom$", Guard::UserAuth, custom_page);
    r.get_g(r"^\/meme\/(?<id>[a-z0-9-]+)$", Guard::UserAuth, creator_page);
    // index.mjs bypass middleware
    r.pre(Method::POST, r"^\/api\/v2\/admin\/memes$", None, upload_meme);
    r.pre(Method::POST, r"^\/api\/v2\/admin\/memes\/(?<id>\d+)\/edit$", None, edit_meme);
}

fn msg(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "message": m }), code)
}

async fn admin_page(st: AppState, req: Req) -> Res {
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/memes", json!({ "session": req.session_value(), "tmp": null }))))
}

async fn list_memes(st: AppState, _req: Req) -> Res {
    match sql!("SELECT id, template_id, name, url, category FROM meme_templates ORDER BY name ASC").fetch_all(&st.db).await {
        Ok(memes) => send(json!({ "success": true, "memes": memes }), 200),
        Err(e) => {
            tracing::error!("[MEMES] list failed: {e}");
            msg(500, "Database error")
        }
    }
}

async fn delete_meme(st: AppState, req: Req) -> Res {
    let Some(s) = admin_session(&req) else { return msg(403, "Forbidden") };
    if !csrf_header_ok(&req, &s) {
        return msg(403, "Invalid CSRF token");
    }
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return msg(500, "Database error") };
    match sql!("DELETE FROM meme_templates WHERE id = ", id).execute(&st.db).await {
        Ok(_) => send(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[MEMES] delete failed: {e}");
            msg(500, "Database error")
        }
    }
}

// ── Creator pages (meme.mjs) ─────────────────────────────────────────────────

fn creator_enabled(st: &AppState) -> bool {
    st.cfg().truthy("websrv.meme_creator")
}

fn page_meta(st: &AppState, title: &str, description: &str, path: &str) -> Value {
    let domain = st.cfg().str_or("main.url.domain", "").to_string();
    json!({ "title": title, "description": description, "url": format!("https://{domain}{path}") })
}

/// `['All', ...new Set(categories)].sort()` (JS sorts by UTF-16 code units).
pub(crate) fn categories(cats: impl Iterator<Item = String>) -> Vec<String> {
    let mut out = vec!["All".to_string()];
    for c in cats {
        if !out.contains(&c) {
            out.push(c);
        }
    }
    out.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
    out
}

async fn select_page(st: AppState, req: Req) -> Res {
    if !creator_enabled(&st) {
        return Ok(raw(404, "Not Found"));
    }
    let templates = sql!("SELECT template_id as id, name, url, category, sub_category FROM meme_templates ORDER BY created_at DESC").fetch_all(&st.db).await?;
    let cats = categories(templates.iter().map(|t| t.str("category").filter(|c| !c.is_empty()).unwrap_or("General").to_string()));
    let body = crate::tpl::render(
        &st,
        Some(&req),
        "meme-select",
        json!({
            "templates": templates,
            "categories": cats,
            "page_meta": page_meta(&st, "Meme Creator - Select Template", "Select a template to create your meme", "/meme"),
        }),
    );
    Ok(html(body))
}

const CUSTOM_PLACEHOLDER: &str = r#"data:image/svg+xml;utf8,<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600" viewBox="0 0 800 600"><rect width="800" height="600" fill="%231a1a1b"/><text x="50%25" y="50%25" fill="%23888888" font-family="sans-serif" font-size="24" dominant-baseline="middle" text-anchor="middle">Click %22Choose Image%22 or Drag and Drop here</text></svg>"#;

async fn custom_page(st: AppState, req: Req) -> Res {
    if !creator_enabled(&st) {
        return Ok(raw(404, "Not Found"));
    }
    let body = crate::tpl::render(
        &st,
        Some(&req),
        "meme-creator",
        json!({
            "template": { "id": "custom", "name": "Custom Template", "url": CUSTOM_PLACEHOLDER, "category": "Custom", "sub_category": "" },
            "page_meta": page_meta(&st, "Create Meme - Custom Template", "Create a meme using your own custom template", "/meme/custom"),
        }),
    );
    Ok(html(body))
}

async fn creator_page(st: AppState, req: Req) -> Res {
    if !creator_enabled(&st) {
        return Ok(raw(404, "Not Found"));
    }
    let id = req.param("id").unwrap_or("").to_string();
    let Some(template) = sql!("SELECT template_id as id, name, url, category, sub_category FROM meme_templates WHERE template_id = ", &id, " LIMIT 1")
        .fetch_optional(&st.db)
        .await?
    else {
        return Ok(raw(404, "Template not found"));
    };
    let name = template.string("name");
    let body = crate::tpl::render(
        &st,
        Some(&req),
        "meme-creator",
        json!({
            "template": template,
            "page_meta": page_meta(&st, &format!("Create Meme - {name}"), &format!("Create a meme using the {name} template"), &format!("/meme/{id}")),
        }),
    );
    Ok(html(body))
}

// ── Upload / edit (meme_upload_handler.mjs) ──────────────────────────────────

const MAX_MEME_UPLOAD: u64 = 20 * 1024 * 1024 + 64 * 1024;

fn valid_template_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// File extension for an accepted template image, by content (never from the client's file name).
pub(crate) fn meme_ext(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        _ => return None,
    })
}

/// Move an uploaded template image into the memes dir; returns its URL.
async fn store_meme_file(st: &AppState, template_id: &str, file: &crate::media::multipart::FilePart) -> anyhow::Result<Result<String, String>> {
    let cfg = st.cfg();
    let mime = crate::media::mime::detect(&file.path).await?;
    let Some(ext) = meme_ext(&mime) else { return Ok(Err(format!("Unsupported file type: {mime}"))) };
    tokio::fs::create_dir_all(&cfg.paths.memes).await?;
    let filename = format!("{template_id}_{}.{ext}", crate::util::random_hex(4));
    let dest = cfg.paths.memes.join(&filename);
    tokio::fs::copy(&file.path, &dest).await?;
    if file_size(&dest).await == 0 {
        let _ = tokio::fs::remove_file(&dest).await;
        anyhow::bail!("File was written but verify failed (size 0 or not found)");
    }
    crate::media::make_public(&dest).await;
    tracing::info!("[MEME HANDLER] Wrote {} ({} bytes)", dest.display(), file.size);
    Ok(Ok(format!("/memes/{filename}")))
}

/// Delete a file we stored for a template (`/memes/<plain name>` only).
async fn remove_meme_file(st: &AppState, url: &str) {
    if let Some(name) = url.strip_prefix("/memes/").filter(|n| crate::media::is_plain_name(n)) {
        let _ = tokio::fs::remove_file(st.cfg().paths.memes.join(name)).await;
    }
}

struct MemeFields {
    template_id: String,
    name: String,
    category: String,
    url: String,
}

fn fields(form: &crate::media::multipart::Form) -> Result<MemeFields, Res> {
    let f = MemeFields {
        template_id: trimmed(form.text("template_id")).to_lowercase(),
        name: trimmed(form.text("name")),
        category: Some(trimmed(form.text("category"))).filter(|c| !c.is_empty()).unwrap_or_else(|| "General".into()),
        url: trimmed(form.text("url")),
    };
    if f.template_id.is_empty() || f.name.is_empty() {
        return Err(msg(400, "Template ID and Name are required"));
    }
    if !valid_template_id(&f.template_id) {
        return Err(msg(400, "Invalid ID. Use lowercase a-z, 0-9, - only."));
    }
    Ok(f)
}

fn db_fail(e: anyhow::Error) -> Res {
    if pg_code(&e).as_deref() == Some("23505") {
        return msg(400, "Template ID is already in use by another template");
    }
    tracing::error!("[MEME HANDLER ERROR] {e:#}");
    msg(500, &err_msg(&e))
}

async fn upload_meme(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        tracing::warn!("[CSRF] Blocked meme upload for user {}. Invalid token.", session.user);
        return msg(403, "Invalid CSRF token");
    }
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_MEME_UPLOAD, "message").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let mut f = match fields(&form) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let r: anyhow::Result<Res> = async {
        if let Some(file) = form.file("file").filter(|x| x.size > 0) {
            match store_meme_file(&st, &f.template_id, file).await? {
                Ok(u) => f.url = u,
                Err(m) => return Ok(msg(400, &m)),
            }
        }
        if f.url.is_empty() {
            return Ok(msg(400, "Either image URL or File is required"));
        }
        let row = sql!(
            "INSERT INTO meme_templates (template_id, name, url, category) VALUES (", &f.template_id, ", ", &f.name, ", ", &f.url, ", ", &f.category,
            ") RETURNING id, template_id, name, url, category"
        )
        .fetch_optional(&st.db)
        .await;
        let row = match row {
            Ok(r) => r,
            Err(e) => {
                if form.file("file").is_some_and(|x| x.size > 0) {
                    remove_meme_file(&st, &f.url).await;
                }
                return Err(e);
            }
        };
        Ok(send(json!({ "success": true, "meme": row }), 200))
    }
    .await;
    r.unwrap_or_else(db_fail)
}

async fn edit_meme(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return msg(403, "Unauthorized") };
    if !csrf_header_ok(&req, &session) {
        tracing::warn!("[CSRF] Blocked meme edit for user {}. Invalid token.", session.user);
        return msg(403, "Invalid CSRF token");
    }
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) else { return msg(404, "Meme template not found") };
    let current = match sql!("SELECT * FROM meme_templates WHERE id = ", id).fetch_optional(&st.db).await {
        Ok(Some(c)) => c,
        Ok(None) => return msg(404, "Meme template not found"),
        Err(e) => return db_fail(e),
    };
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_MEME_UPLOAD, "message").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let mut f = match fields(&form) {
        Ok(f) => f,
        Err(r) => return r,
    };
    let r: anyhow::Result<Res> = async {
        if sql!("SELECT id FROM meme_templates WHERE template_id = ", &f.template_id, " AND id != ", id).fetch_optional(&st.db).await?.is_some() {
            return Ok(msg(400, "Template ID is already in use by another template"));
        }
        let mut replaced_file = false;
        if let Some(file) = form.file("file").filter(|x| x.size > 0) {
            match store_meme_file(&st, &f.template_id, file).await? {
                Ok(u) => f.url = u,
                Err(m) => return Ok(msg(400, &m)),
            }
            replaced_file = true;
        }
        if f.url.is_empty() {
            f.url = current.string("url");
        }
        sql!(
            "UPDATE meme_templates SET template_id = ", &f.template_id, ", name = ", &f.name, ", category = ", &f.category, ", url = ", &f.url, " WHERE id = ", id
        )
        .execute(&st.db)
        .await?;
        // The old handler left the replaced upload behind
        if replaced_file && current.str("url") != Some(f.url.as_str()) {
            remove_meme_file(&st, current.str("url").unwrap_or("")).await;
        }
        Ok(send(json!({ "success": true }), 200))
    }
    .await;
    r.unwrap_or_else(db_fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_list() {
        let c = categories(["General", "Zoo", "Anime", "General", "anime"].into_iter().map(String::from));
        assert_eq!(c, vec!["All", "Anime", "General", "Zoo", "anime"]);
        assert!(valid_template_id("drake-2"));
        assert!(!valid_template_id("Drake"));
        assert!(!valid_template_id("../x"));
        assert_eq!(meme_ext("image/svg+xml"), None);
        assert_eq!(meme_ext("image/jpeg"), Some("jpg"));
    }
}
