//! Admin hall manager (src/hall_image_handler.mjs): create, rename/update, delete and the custom
//! hall image. Only active while `websrv.halls_enabled !== false`, like the old bypass middleware.

use super::{read_form, send};
use crate::http::{Req, Res, Router, not_found_route};
use crate::routes::halls::{cache_dir, custom_dir};
use crate::session::Session;
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/admin\/halls\/(?<slug>[^/]+)\/image$", None, image_upload);
    r.pre(Method::DELETE, r"^\/api\/v2\/admin\/halls\/(?<slug>[^/]+)\/image$", None, image_delete);
    r.pre(Method::POST, r"^\/api\/v2\/admin\/halls$", None, create);
    r.pre(Method::DELETE, r"^\/api\/v2\/admin\/halls\/(?<slug>[^/]+)$", None, delete);
    r.pre(Method::PATCH, r"^\/api\/v2\/admin\/halls\/(?<slug>[^/]+)$", None, update);
}

fn msg(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "msg": m }), code)
}

/// Halls enabled, admin or moderator, CSRF token from the header or `?csrf_token=`.
/// `Err(None)` means "halls disabled": fall through to a 404 like the old router.
fn guard(st: &AppState, req: &Req) -> Result<Session, Option<Res>> {
    if st.cfg().is_false("websrv.halls_enabled") {
        return Err(None);
    }
    let Some(s) = req.session().filter(|s| s.is_admin_or_mod()).cloned() else {
        return Err(Some(msg(403, "Unauthorized")));
    };
    let token = req.header("x-csrf-token").or_else(|| req.query("csrf_token"));
    let ok = match (token, s.csrf_token.as_deref().filter(|t| !t.is_empty())) {
        (Some(t), Some(expected)) => crate::util::ct_eq(t, expected),
        _ => false,
    };
    if !ok {
        return Err(Some(msg(403, "Invalid CSRF token")));
    }
    Ok(s)
}

macro_rules! guarded {
    ($st:expr, $req:expr) => {
        match guard(&$st, &$req) {
            Ok(s) => s,
            Err(Some(r)) => return r,
            Err(None) => return Ok(not_found_route(&$req.method, &$req.path)),
        }
    };
}

/// Decoded slug from the URL.
fn slug_param(req: &Req) -> String {
    crate::util::decode_lossy(req.param("slug").unwrap_or(""))
}

/// The slug when it can be used as a file name (the URL slug is user input).
fn file_slug(slug: &str) -> Option<&str> {
    crate::media::is_plain_name(slug).then_some(slug)
}

/// `name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '')`
pub(crate) fn slugify_hall(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Remove the cached mosaics of a hall (`md5(slug + '_' + mode).webp` for modes 0–2).
async fn clear_hall_cache(st: &AppState, slug: &str) {
    let dir = cache_dir(st);
    for mode in 0..=2 {
        let h = crate::util::md5(&format!("{slug}_{mode}"));
        let _ = tokio::fs::remove_file(dir.join(format!("{h}.webp"))).await;
    }
}

const MAX_IMAGE: u64 = 10 * 1024 * 1024;

async fn image_upload(st: AppState, mut req: Req) -> Res {
    let _session = guarded!(st, req);
    let slug = slug_param(&req);
    if slug.is_empty() {
        return msg(400, "Missing hall slug");
    }
    if sql!("SELECT id FROM halls WHERE slug = ", &slug, " LIMIT 1").fetch_optional(&st.db).await?.is_none() {
        return msg(404, "Hall not found");
    }
    let Some(fslug) = file_slug(&slug).map(str::to_string) else { return msg(400, "Invalid hall slug") };
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_IMAGE + 64 * 1024, "msg").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let Some(file) = form.file("file").filter(|f| f.size > 0) else { return msg(400, "No file provided") };
    if file.size > MAX_IMAGE {
        return msg(400, "File too large (10 MB max)");
    }
    let part_mime = file.content_type.to_lowercase().trim().to_string();
    if !["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp"].contains(&part_mime.as_str()) {
        return msg(400, &format!("Invalid file type: {part_mime}"));
    }
    let actual = crate::media::mime::detect(&file.path).await?;
    if !["image/gif", "image/jpeg", "image/png", "image/webp"].contains(&actual.as_str()) {
        return msg(400, &format!("Invalid file type: {actual}"));
    }
    let dir = custom_dir(&st);
    tokio::fs::create_dir_all(&dir).await?;
    let final_name = format!("{fslug}.webp");
    let (src, out) = (file.path.display().to_string(), dir.join(&final_name).display().to_string());
    // -coalesce composites animated GIF frames before resizing
    if let Err(e) = super::magick(&cfg, &[&src, "-coalesce", "-resize", "600x300^", "-gravity", "center", "-background", "none", "-extent", "600x300", "-quality", "85", &out]).await {
        tracing::error!("[HALL IMG] Magick error: {e}");
        return msg(500, "Failed to process image");
    }
    crate::media::make_public(std::path::Path::new(&out)).await;
    sql!("UPDATE halls SET custom_image = ", &final_name, " WHERE slug = ", &slug).execute(&st.db).await?;
    clear_hall_cache(&st, &fslug).await;
    tracing::info!("[HALL IMG] Uploaded custom image for: {slug}");
    send(json!({ "success": true, "msg": "Image uploaded", "file": final_name }), 200)
}

async fn image_delete(st: AppState, req: Req) -> Res {
    let _session = guarded!(st, req);
    let slug = slug_param(&req);
    if slug.is_empty() {
        return msg(400, "Missing hall slug");
    }
    let Some(hall) = sql!("SELECT id, custom_image FROM halls WHERE slug = ", &slug, " LIMIT 1").fetch_optional(&st.db).await? else {
        return msg(404, "Hall not found");
    };
    let dir = custom_dir(&st);
    if let Some(fs) = file_slug(&slug) {
        if let Err(e) = tokio::fs::remove_file(dir.join(format!("{fs}.webp"))).await {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::error!("[HALL IMG] Failed to delete file for {slug}: {e}");
            }
        }
    }
    // Also delete whatever the DB says (in case it differs)
    if let Some(ci) = hall.get("custom_image").and_then(Value::as_str) {
        if ci != format!("{slug}.webp") && crate::media::is_plain_name(ci) {
            let _ = tokio::fs::remove_file(dir.join(ci)).await;
        }
    }
    sql!("UPDATE halls SET custom_image = NULL WHERE slug = ", &slug).execute(&st.db).await?;
    clear_hall_cache(&st, &slug).await;
    send(json!({ "success": true, "msg": "Custom image removed" }), 200)
}

async fn delete(st: AppState, req: Req) -> Res {
    let session = guarded!(st, req);
    let slug = slug_param(&req);
    if slug.is_empty() {
        return msg(400, "Missing hall slug");
    }
    if let Err(e) = sql!("DELETE FROM halls_assign WHERE hall_id = (SELECT id FROM halls WHERE slug = ", &slug, ")").execute(&st.db).await {
        tracing::warn!("[HALL] halls_assign cleanup failed: {e}");
    }
    sql!("DELETE FROM halls WHERE slug = ", &slug).execute(&st.db).await?;
    if let Some(fs) = file_slug(&slug) {
        let _ = tokio::fs::remove_file(custom_dir(&st).join(format!("{fs}.webp"))).await;
    }
    clear_hall_cache(&st, &slug).await;
    st.update_halls_cache().await;
    crate::audit::log(&st, session.id, "delete_hall", "hall", slug.clone(), json!({ "slug": slug })).await;
    tracing::info!("[HALL] Deleted hall: {slug}");
    send(json!({ "success": true, "msg": "Hall deleted" }), 200)
}

async fn update(st: AppState, mut req: Req) -> Res {
    let session = guarded!(st, req);
    let slug = slug_param(&req);
    if slug.is_empty() {
        return msg(400, "Missing slug");
    }
    let body = req.post().await.cloned().unwrap_or(Value::Null);
    let name = body.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(str::to_string);
    let description = body.get("description").and_then(Value::as_str).map(str::to_string);
    let rating = body.get("rating").and_then(Value::as_str).filter(|r| ["sfw", "nsfw", "nsfl"].contains(r)).map(str::to_string);

    let apply = |target: String| {
        let (st, name, description, rating) = (st.clone(), name.clone(), description.clone(), rating.clone());
        async move {
            if let Some(n) = &name {
                sql!("UPDATE halls SET name = ", n, " WHERE slug = ", &target).execute(&st.db).await?;
            }
            if let Some(d) = &description {
                sql!("UPDATE halls SET description = ", d, " WHERE slug = ", &target).execute(&st.db).await?;
            }
            if let Some(r) = &rating {
                sql!("UPDATE halls SET rating = ", r, " WHERE slug = ", &target).execute(&st.db).await?;
            }
            anyhow::Ok(())
        }
    };

    if let Some(raw_slug) = body.get("slug").and_then(Value::as_str) {
        let new_slug = slugify_hall(raw_slug);
        if new_slug.is_empty() {
            return msg(400, "Slug cannot be empty");
        }
        if new_slug != slug {
            if sql!("SELECT id FROM halls WHERE slug = ", &new_slug, " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
                return msg(409, "A hall with that slug already exists");
            }
            let dir = custom_dir(&st);
            if let Some(old) = file_slug(&slug) {
                if let Err(e) = tokio::fs::rename(dir.join(format!("{old}.webp")), dir.join(format!("{new_slug}.webp"))).await {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::error!("[HALL IMG] Failed to rename image: {e}");
                    }
                }
            }
            sql!(
                "UPDATE halls SET slug = ", &new_slug, ", custom_image = CASE WHEN custom_image IS NOT NULL THEN ", format!("{new_slug}.webp"),
                " ELSE NULL END WHERE slug = ", &slug
            )
            .execute(&st.db)
            .await?;
            clear_hall_cache(&st, &slug).await;
            clear_hall_cache(&st, &new_slug).await;
            apply(new_slug.clone()).await?;
            st.update_halls_cache().await;
            crate::audit::log(
                &st,
                session.id,
                "rename_hall",
                "hall",
                new_slug.clone(),
                json!({ "old_slug": slug, "new_slug": new_slug, "name": name, "description": description }),
            )
            .await;
            return send(json!({ "success": true, "msg": "Hall updated", "newSlug": new_slug }), 200);
        }
    }
    apply(slug.clone()).await?;
    st.update_halls_cache().await;
    crate::audit::log(&st, session.id, "update_hall", "hall", slug.clone(), json!({ "name": name, "description": description })).await;
    send(json!({ "success": true, "msg": "Hall updated" }), 200)
}

async fn create(st: AppState, mut req: Req) -> Res {
    let session = guarded!(st, req);
    let body: Value = match req.body_bytes(64 * 1024).await.ok().and_then(|b| serde_json::from_slice(&b).ok()) {
        Some(v @ Value::Object(_)) => v,
        _ => return msg(400, "Invalid JSON body"),
    };
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let slug = slugify_hall(body.get("slug").and_then(Value::as_str).unwrap_or(""));
    let rating = body.get("rating").and_then(Value::as_str).filter(|r| ["sfw", "nsfw", "nsfl"].contains(r)).unwrap_or("sfw").to_string();
    if name.is_empty() {
        return msg(400, "Name is required");
    }
    if slug.is_empty() {
        return msg(400, "Could not derive a valid slug from the name");
    }
    if sql!("SELECT id FROM halls WHERE slug = ", &slug, " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
        return msg(409, "A hall with that slug already exists");
    }
    sql!("INSERT INTO halls (name, slug, description, rating) VALUES (", &name, ", ", &slug, ", '', ", &rating, ")").execute(&st.db).await?;
    st.update_halls_cache().await;
    crate::audit::log(&st, session.id, "create_hall", "hall", slug.clone(), json!({ "name": name, "slug": slug })).await;
    tracing::info!("[HALL] Created hall: {slug} - {name}");
    send(json!({ "success": true, "msg": "Hall created", "slug": slug }), 200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hall_slugs() {
        assert_eq!(slugify_hall("  My Hall! 2 "), "my-hall-2");
        assert_eq!(slugify_hall("--a__b--"), "a-b");
        assert_eq!(slugify_hall("Ärger"), "rger");
        assert_eq!(slugify_hall("../../etc"), "etc");
        assert_eq!(slugify_hall("!!!"), "");
        assert_eq!(file_slug("../x"), None);
        assert_eq!(file_slug("ok-slug"), Some("ok-slug"));
    }
}
