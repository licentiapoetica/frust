//! Halls overview and hall thumbnails (port of src/inc/routes/halls.mjs).

use crate::http::{Req, Res, Router, html, html_code, raw};
use crate::social::halls;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn register(r: &mut Router) {
    r.get(r"^\/halls$", halls_overview);
    r.get(r"^\/hall_image\/(?<hallSlug>.+)$", hall_image);
}

/// `tpl.render('error', { message, tmp: null })` with a status code.
pub fn error_page(st: &AppState, req: &Req, message: &str, code: u16) -> Response {
    html_code(crate::tpl::render(st, Some(req), "error", json!({ "message": message, "tmp": null })), code)
}

async fn halls_overview(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if cfg.is_false("websrv.halls_enabled") {
        return Ok(error_page(&st, &req, "Not found", 404));
    }
    let excluded: Vec<i64> = req.session().map(|s| s.excluded_tags.clone()).unwrap_or_default();
    let list = halls::get_halls_overview(&st, req.mode, &excluded).await;
    let session = match req.session() {
        Some(s) if !s.user.is_empty() => req.session_value(),
        _ => Value::Bool(false),
    };
    let data = json!({
        "hallsList": list,
        "tmp": null,
        "hidePagination": true,
        "session": session,
        "page_meta": {
            "title": "Halls",
            "description": "Browse curated item collections",
            "url": format!("https://{}/halls", cfg.domain()),
        }
    });
    let view = if req.is_xhr() { "halls-partial" } else { "halls" };
    Ok(html(crate::tpl::render(&st, Some(&req), view, data)))
}

/// `path.basename(p)` (POSIX).
pub fn basename(p: &str) -> String {
    p.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
}

/// `+req.url.qs.m` rendered like JS would in a template string (`NaN` for garbage).
pub fn mode_param(req: &Req) -> (Option<i64>, String) {
    let n = crate::social::helpers::js_number(req.query("m").unwrap_or("0"));
    match n {
        Some(f) if f.is_finite() && f.fract() == 0.0 => (Some(f as i64), (f as i64).to_string()),
        Some(f) => (None, crate::util::js_num(f)),
        None => (None, "NaN".into()),
    }
}

pub fn custom_dir(st: &AppState) -> PathBuf {
    st.cfg().paths.s.join("../hall_custom")
}

pub fn cache_dir(st: &AppState) -> PathBuf {
    st.cfg().paths.s.join("../hall_cache")
}

fn webp(code: u16, body: Vec<u8>, cache: &'static str) -> Response {
    let mut r = raw(code, body);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/webp"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    r
}

pub fn placeholder() -> Response {
    let mut r = raw(302, "");
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/s/img/favicon.gif"));
    r
}

/// Serve an uploaded custom hall image with an mtime/size ETag (304 when unchanged).
pub async fn serve_custom(req: &Req, path: &Path) -> Option<Response> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let mtime_ms = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
    let etag = format!("\"{mtime_ms:x}-{:x}\"", meta.len());
    if req.header("if-none-match") == Some(etag.as_str()) {
        return Some(raw(304, ""));
    }
    let body = tokio::fs::read(path).await.ok()?;
    let mut r = webp(200, body, "no-cache");
    if let Ok(v) = HeaderValue::from_str(&etag) {
        r.headers_mut().insert(header::ETAG, v);
    }
    Some(r)
}

/// Serve a cached mosaic if present.
pub async fn serve_cached(path: &Path) -> Option<Response> {
    let body = tokio::fs::read(path).await.ok()?;
    Some(webp(200, body, "public, max-age=3600"))
}

/// Build a 600x300 mosaic from up to three item thumbnails and serve it.
pub async fn build_mosaic(st: &AppState, item_ids: &[i64], cache_path: &Path) -> anyhow::Result<Option<Response>> {
    let cfg = st.cfg();
    let mut inputs: Vec<String> = Vec::new();
    for id in item_ids {
        let p = cfg.paths.t.join(format!("{id}.webp"));
        if tokio::fs::metadata(&p).await.is_ok() {
            inputs.push(format!("{}[0]", p.display()));
        }
    }
    if inputs.is_empty() {
        return Ok(None);
    }
    if let Some(dir) = cache_path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let out = cache_path.display().to_string();
    let mut args: Vec<&str> = inputs.iter().map(String::as_str).collect();
    args.extend(["+append", "-background", "none", "-resize", "600x300^", "-gravity", "center", "-extent", "600x300", out.as_str()]);
    crate::proc::spawn("magick", &args).await?;
    let body = tokio::fs::read(cache_path).await?;
    Ok(Some(webp(200, body, "public, max-age=3600")))
}

/// Mode join used by the mosaics (SFW / NSFW tag), none for other modes.
pub fn mosaic_mode_join(mode: Option<i64>) -> &'static str {
    match mode {
        Some(0) => "JOIN tags_assign ta_sfw ON ta_sfw.item_id = i.id AND ta_sfw.tag_id = 1",
        Some(1) => "JOIN tags_assign ta_nsfw ON ta_nsfw.item_id = i.id AND ta_nsfw.tag_id = 2",
        _ => "",
    }
}

async fn hall_image(st: AppState, req: Req) -> Res {
    let slug = basename(&crate::util::decode_lossy(req.param("hallSlug").unwrap_or("")));
    let (mode, mode_str) = mode_param(&req);
    let r: anyhow::Result<Option<Response>> = async {
        if let Some(r) = serve_custom(&req, &custom_dir(&st).join(format!("{slug}.webp"))).await {
            return Ok(Some(r));
        }
        let hash = crate::util::md5(&format!("{slug}_{mode_str}"));
        let cache_path = cache_dir(&st).join(format!("{hash}.webp"));
        if let Some(r) = serve_cached(&cache_path).await {
            return Ok(Some(r));
        }
        let mut q = crate::db::Sql::new("SELECT i.id FROM items i JOIN halls_assign ha ON ha.item_id = i.id JOIN halls h ON h.id = ha.hall_id ");
        q.push(mosaic_mode_join(mode)).push(" WHERE h.slug = ").bind(slug.as_str()).push(" AND i.active = true ORDER BY RANDOM() LIMIT 3");
        let ids: Vec<i64> = q.fetch_all(&st.db).await?.iter().filter_map(|r| crate::db::RowExt::i64(r, "id")).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        build_mosaic(&st, &ids, &cache_path).await
    }
    .await;
    match r {
        Ok(Some(resp)) => Ok(resp),
        Ok(None) => Ok(placeholder()),
        Err(e) => {
            tracing::error!("[HALL_IMAGE] Error: {e:#}");
            Ok(placeholder())
        }
    }
}
