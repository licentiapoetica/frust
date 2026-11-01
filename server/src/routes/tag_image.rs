//! Tag preview images (`/tag_image/:tag`) and their cache — port of src/inc/routes/tag_image.mjs.
//!
//! Cache files live in `public/tag_cache/<md5(tag)>_<mode>.webp`. The file name is derived only
//! from the md5 hex digest and a normalized mode (0 = sfw, 1 = nsfw, 3 = everything), so no
//! request value reaches the path.

use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, json_raw, reply};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use std::path::PathBuf;

const CACHE_MAX_AGE: u32 = 3600;
const MODES: [i64; 3] = [0, 1, 3];

pub fn register(r: &mut Router) {
    r.get(r"^\/tag_image\/(?<tag>.+)$", tag_image);
    r.exact(Method::GET, "/admin/tag_image/regenerate_all", Some(Guard::Auth), regenerate_all);
    r.get_g(r"^\/admin\/tag_image\/regenerate\/(?<tag>.+)$", Guard::Auth, regenerate_one);
}

fn cache_dir(st: &AppState) -> PathBuf {
    // path.join(cfg.paths.s, '../tag_cache')
    st.cfg().paths.s.parent().map(|p| p.join("tag_cache")).unwrap_or_else(|| PathBuf::from("public/tag_cache"))
}

/// Only 0 (sfw) and 1 (nsfw) filter the images; every other mode shows all items and shares
/// the `3` cache entry (the old code kept one file per arbitrary `?m=` value, unbounded).
fn norm_mode(mode: i64) -> i64 {
    match mode {
        0 | 1 => mode,
        _ => 3,
    }
}

fn cache_path(st: &AppState, tag: &str, mode: i64) -> PathBuf {
    cache_dir(st).join(format!("{}_{}.webp", crate::util::md5(tag), norm_mode(mode)))
}

/// `regenerateTagImage(tag, mode)` — up to 3 random thumbnails side by side, 600×300.
pub async fn regenerate_tag_image(st: &AppState, tag: &str, mode: i64) -> Option<PathBuf> {
    let cfg = st.cfg();
    let mode = norm_mode(mode);
    let target = cache_path(st, tag, mode);
    let join = match mode {
        0 => "JOIN tags_assign ta_sfw ON ta_sfw.item_id = i.id AND ta_sfw.tag_id = 1",
        1 => "JOIN tags_assign ta_nsfw ON ta_nsfw.item_id = i.id AND ta_nsfw.tag_id = 2",
        _ => "",
    };
    let mut q = crate::db::Sql::new("SELECT i.id FROM items i JOIN tags_assign ta ON ta.item_id = i.id JOIN tags t ON t.id = ta.tag_id ");
    q.push(join)
        .push(" WHERE (t.tag = ")
        .bind(tag)
        .push(" OR t.normalized = ")
        .bind(tag)
        .push(") AND i.active = true AND i.is_deleted = false AND COALESCE(i.visibility, 0) = 0 ORDER BY RANDOM() LIMIT 3");
    let rows = match q.fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[TAG_IMAGE] Failed to generate image for tag \"{tag}\" (mode {mode}): {e}");
            return None;
        }
    };
    let mut inputs: Vec<String> = Vec::new();
    for r in &rows {
        let Some(id) = r.i64("id") else { continue };
        let p = cfg.paths.t.join(format!("{id}.webp"));
        if tokio::fs::metadata(&p).await.is_ok() {
            inputs.push(format!("{}[0]", p.to_string_lossy()));
        }
    }
    if inputs.is_empty() {
        return None;
    }
    if let Some(dir) = target.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    let out = target.to_string_lossy().to_string();
    let mut args: Vec<&str> = inputs.iter().map(String::as_str).collect();
    args.extend(["+append", "-background", "none", "-resize", "600x300^", "-gravity", "center", "-extent", "600x300", &out]);
    match crate::proc::spawn("magick", &args).await {
        Ok(_) => Some(target),
        Err(e) => {
            tracing::error!("[TAG_IMAGE] Failed to generate image for tag \"{tag}\" (mode {mode}): {e}");
            None
        }
    }
}

async fn remove_cached(st: &AppState, tag: &str) {
    for m in MODES {
        let _ = tokio::fs::remove_file(cache_path(st, tag, m)).await;
    }
}

/// Background listener: when an item's tags change (`pg_notify('tags', …)`), drop and rebuild
/// the cached images of all its tags.
pub fn spawn_listener(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        loop {
            let mut listener = match sqlx::postgres::PgListener::connect_with(&st.db).await {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("[TAG_IMAGE] DB Listen error: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    continue;
                }
            };
            if let Err(e) = listener.listen("tags").await {
                tracing::error!("[TAG_IMAGE] DB Listen error: {e}");
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                continue;
            }
            loop {
                match listener.recv().await {
                    Ok(n) => {
                        let st = st.clone();
                        let payload = n.payload().to_string();
                        tokio::spawn(async move { on_tags_changed(&st, &payload).await });
                    }
                    Err(e) => {
                        tracing::error!("[TAG_IMAGE] DB Listen error: {e}");
                        break;
                    }
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}

async fn on_tags_changed(st: &AppState, payload: &str) {
    let Ok(data) = serde_json::from_str::<serde_json::Value>(payload) else { return };
    let Some(item_id) = crate::config::as_int(&data["item_id"]).filter(|&i| i != 0).and_then(|i| i32::try_from(i).ok()) else { return };
    let current: Vec<String> = match sql!("SELECT DISTINCT t.tag FROM tags t JOIN tags_assign ta ON t.id = ta.tag_id WHERE ta.item_id = ", item_id)
        .fetch_scalar_list::<String, _>(&st.db)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[TAG_IMAGE] Background listener error: {e}");
            return;
        }
    };
    let mut names: Vec<String> = Vec::new();
    let from_payload = data["tags"].as_array().map(|a| a.iter().filter_map(|t| t["tag"].as_str().filter(|s| !s.is_empty()).map(str::to_string)).collect::<Vec<_>>());
    for t in current.into_iter().chain(from_payload.unwrap_or_default()) {
        if !names.contains(&t) {
            names.push(t);
        }
    }
    if names.is_empty() {
        return;
    }
    tracing::info!("[TAG_IMAGE] Tag change on item {item_id}, regenerating cache for {} tag(s)", names.len());
    for tag in names {
        remove_cached(st, &tag).await;
        for m in MODES {
            let (st, tag) = (st.clone(), tag.clone());
            tokio::spawn(async move {
                regenerate_tag_image(&st, &tag, m).await;
            });
        }
    }
}

/// Deterministic SVG placeholder (gradient + tag name).
fn fallback_svg(tag: &str) -> String {
    let hash = crate::util::md5(tag);
    let esc: String = tag
        .chars()
        .map(|c| match c {
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '&' => "&amp;".to_string(),
            '\'' => "&apos;".to_string(),
            '"' => "&quot;".to_string(),
            c => c.to_string(),
        })
        .collect();
    let c1 = &hash[0..6];
    let c2 = &hash[6..12];
    let c3 = &hash[12..18];
    let n1 = i64::from_str_radix(&hash[18..20], 16).unwrap_or(0);
    let n2 = i64::from_str_radix(&hash[20..22], 16).unwrap_or(0);
    let r1 = crate::util::js_num((n1 + n2) as f64 / 2.0);
    let r2 = crate::util::js_num((n1 + n2) as f64 / 1.5);
    format!(
        r##"<svg width="600" height="300" viewBox="0 0 600 300" xmlns="http://www.w3.org/2000/svg">
      <defs>
        <linearGradient id="grad" x1="0%" y1="0%" x2="100%" y2="100%">
          <stop offset="0%" stop-color="#{c1}" stop-opacity="1" />
          <stop offset="100%" stop-color="#{c2}" stop-opacity="1" />
        </linearGradient>
      </defs>
      <rect width="600" height="300" fill="url(#grad)" />
      <circle cx="{n1}%" cy="{n2}%" r="{r1}" fill="#{c3}" fill-opacity="0.25" />
      <circle cx="{}%" cy="{}%" r="{r2}" fill="#{c3}" fill-opacity="0.15" />
      <text x="50%" y="50%" dominant-baseline="middle" text-anchor="middle" font-family="monospace, sans-serif" font-size="36" fill="#fff" fill-opacity="0.95" font-weight="bold">{esc}</text>
    </svg>"##,
        100 - n1,
        100 - n2
    )
}

fn image_response(content_type: &'static str, body: impl Into<axum::body::Body>) -> axum::response::Response {
    let mut r = axum::response::Response::new(body.into());
    let h = r.headers_mut();
    h.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static(content_type));
    if let Ok(v) = axum::http::HeaderValue::from_str(&format!("public, max-age={CACHE_MAX_AGE}")) {
        h.insert(axum::http::header::CACHE_CONTROL, v);
    }
    r
}

async fn tag_image(st: AppState, req: Req) -> Res {
    let tag = crate::util::decode_lossy(req.param("tag").unwrap_or(""));
    let mode = match req.query("m").filter(|m| !m.is_empty()) {
        Some(m) => crate::social::helpers::js_parse_int(m).unwrap_or(3),
        None => req.mode,
    };
    let path = cache_path(&st, &tag, mode);
    if let Ok(meta) = tokio::fs::metadata(&path).await {
        if meta.len() > 0 {
            if let Ok(bytes) = tokio::fs::read(&path).await {
                return Ok(image_response("image/webp", bytes));
            }
        }
    }
    if let Some(p) = regenerate_tag_image(&st, &tag, mode).await {
        if let Ok(bytes) = tokio::fs::read(&p).await {
            return Ok(image_response("image/webp", bytes));
        }
    }
    Ok(image_response("image/svg+xml", fallback_svg(&tag)))
}

async fn regenerate_all(st: AppState, _req: Req) -> Res {
    let tags: Vec<String> = match sql!("SELECT DISTINCT tag FROM tags JOIN tags_assign ON tags.id = tags_assign.tag_id").fetch_scalar_list(&st.db).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[ADMIN] Failed to trigger full regeneration: {e}");
            return Ok(reply(500, "text/html", "Internal Server Error"));
        }
    };
    let n = tags.len();
    tracing::info!("[ADMIN] Triggering full tag image regeneration for {n} tags");
    let st2 = st.clone();
    tokio::spawn(async move {
        for batch in tags.chunks(10) {
            let futs = batch.iter().map(|t| {
                let st = st2.clone();
                async move {
                    for m in MODES {
                        let _ = tokio::fs::remove_file(cache_path(&st, t, m)).await;
                        regenerate_tag_image(&st, t, m).await;
                    }
                }
            });
            futures::future::join_all(futs).await;
        }
        tracing::info!("[ADMIN] Full tag image regeneration completed");
    });
    Ok(json_raw(&serde_json::json!({ "success": true, "message": format!("Started regeneration for {n} tags in background.") }), 200))
}

async fn regenerate_one(st: AppState, req: Req) -> Res {
    let tag = crate::util::decode_lossy(req.param("tag").unwrap_or(""));
    tracing::info!("[ADMIN] Regenerating images for tag: {tag}");
    for m in MODES {
        let _ = tokio::fs::remove_file(cache_path(&st, &tag, m)).await;
        regenerate_tag_image(&st, &tag, m).await;
    }
    Ok(json_raw(&serde_json::json!({ "success": true, "message": format!("Regenerated images for tag \"{tag}\".") }), 200))
}
