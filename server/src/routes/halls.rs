//! Halls overview and hall thumbnails (port of src/inc/routes/halls.mjs).

use crate::http::{Req, Res, Router, html, html_code, raw};
use crate::social::halls;
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn register(r: &mut Router) {
    r.get(r"^\/lists$", halls_overview);
    // Halls are called lists now: the old addresses move there for good (path and query kept)
    r.get(r"^\/halls\/?$", old_hall_url);
    r.get(r"^\/h\/.+$", old_hall_url);
    r.get(r"^\/user\/[^/]+\/halls\/?$", old_hall_url);
    r.get(r"^\/user\/[^/]+\/hall\/.+$", old_hall_url);
    r.get(r"^\/list_image\/(?<hallSlug>.+)$", hall_image);
}

/// `301` from a hall address to the list one: `/halls` → `/lists`, `/h/<slug>…` → `/list/<slug>…`,
/// `/user/<name>/halls` → `/user/<name>/lists`, `/user/<name>/hall/<slug>…` → `/user/<name>/list/<slug>…`.
async fn old_hall_url(_st: AppState, req: Req) -> Res {
    Ok(crate::http::redirect_code(&list_url(&req.path, &req.raw_query), 301))
}

/// The list address of an old hall path (see [`old_hall_url`]); other paths come back unchanged.
pub fn list_url(path: &str, query: &str) -> String {
    let p = path.trim_end_matches('/');
    let new = if p == "/halls" {
        "/lists".to_string()
    } else if let Some(rest) = path.strip_prefix("/h/") {
        format!("/list/{rest}")
    } else if let Some(user) = path.strip_prefix("/user/") {
        match user.split_once('/') {
            Some((name, "halls" | "halls/")) => format!("/user/{name}/lists"),
            Some((name, rest)) => match rest.strip_prefix("hall/") {
                Some(r) => format!("/user/{name}/list/{r}"),
                None => path.to_string(),
            },
            None => path.to_string(),
        }
    } else {
        path.to_string()
    };
    if query.is_empty() { new } else { format!("{new}?{query}") }
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
    let mut list = halls::get_halls_overview(&st, req.mode, &excluded).await;
    for h in list.iter_mut() {
        let v = image_v(&custom_dir(&st).join(format!("{}.webp", basename(&crate::db::RowExt::string(h, "slug")))), h).await;
        h.insert("image_v".into(), json!(v));
    }
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
            "title": "Lists",
            "description": "Browse curated item collections",
            "url": format!("https://{}/lists", cfg.domain()),
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

/// The rating mode (0 sfw, 1 nsfw, other: everything) a mosaic may show this viewer. Mosaic URLs
/// don't pass the media gate, so `?m=` alone must not unlock what the listings hide: members any
/// mode, guests sfw (nsfw with `websrv.public_nsfw`), anon users what their modes include.
/// Anything not allowed becomes sfw.
pub fn mosaic_mode_for(cfg: &crate::config::Config, req: &Req, wanted: Option<i64>) -> i64 {
    let wanted = wanted.unwrap_or(0);
    let s = req.session().filter(|s| !s.user.is_empty());
    let allowed = match s {
        Some(s) if !s.is_anon && !s.user.starts_with("anon_") => true,
        Some(_) => {
            let modes = crate::settings::anon_allowed_modes(cfg);
            let has = |m: &str| modes.iter().any(|x| x == m);
            match wanted {
                0 => has("sfw"),
                1 => has("nsfw"),
                _ => has("sfw") && has("nsfw") && has("nsfl") && has("untagged"),
            }
        }
        None => match wanted {
            0 => true,
            1 => cfg.truthy("websrv.public_nsfw"),
            _ => false,
        },
    };
    if allowed { wanted } else { 0 }
}

/// Mosaics other than sfw stay out of shared caches: the URL is the same for everyone.
pub fn mosaic_cache(mut r: Response, mode: i64) -> Response {
    if mode != 0 {
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=3600"));
    }
    r
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

/// How long a built list picture is kept: then it is built again (other random f0cks, and a f0ck
/// that went away drops out). Changes to the list clear it at once ([`clear_site_list`],
/// [`clear_user_list`], [`clear_lists_of_item`]).
const MOSAIC_TTL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// Serve a cached mosaic if present and not older than [`MOSAIC_TTL`].
pub async fn serve_cached(path: &Path) -> Option<Response> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    if meta.modified().ok().and_then(|m| m.elapsed().ok()).is_none_or(|age| age > MOSAIC_TTL) {
        return None;
    }
    let body = tokio::fs::read(path).await.ok()?;
    Some(webp(200, body, "public, max-age=3600"))
}

/// The rating modes a list picture is kept for: 0 sfw, 1 nsfw, 3 everything (every other mode shows
/// everything too, and shares the `3` file; as tag pictures do).
pub fn norm_mode(mode: i64) -> i64 {
    match mode {
        0 | 1 => mode,
        _ => 3,
    }
}

/// The modes a list picture can be cached for (2 only from before [`norm_mode`]).
const CACHED_MODES: [i64; 4] = [0, 1, 2, 3];

/// The cached picture of a site list (`slug`: a file-safe slug) in a mode.
pub fn site_cache_path(st: &AppState, slug: &str, mode: i64) -> PathBuf {
    cache_dir(st).join(format!("{}.webp", crate::util::md5(&format!("{slug}_{mode}"))))
}

/// The cached picture of a user's list (`slug`: a file-safe slug) in a mode.
pub fn user_cache_path(st: &AppState, user_id: i64, slug: &str, mode: i64) -> PathBuf {
    cache_dir(st).join(format!("{}.webp", crate::util::md5(&format!("uh_{user_id}_{slug}_{mode}"))))
}

/// Drop a site list's cached pictures: its f0cks changed.
pub async fn clear_site_list(st: &AppState, slug: &str) {
    let slug = basename(slug);
    for m in CACHED_MODES {
        let _ = tokio::fs::remove_file(site_cache_path(st, &slug, m)).await;
    }
}

/// Drop a user list's cached pictures: its f0cks changed.
pub async fn clear_user_list(st: &AppState, user_id: i64, slug: &str) {
    let slug = basename(slug);
    for m in CACHED_MODES {
        let _ = tokio::fs::remove_file(user_cache_path(st, user_id, &slug, m)).await;
    }
}

/// Drop the cached pictures of every list an item is in (its tags or rating changed).
pub async fn clear_lists_of_item(st: &AppState, item_id: i64) {
    let Ok(id) = i32::try_from(item_id) else { return };
    let site: Vec<String> = sql!("SELECT h.slug FROM halls h JOIN halls_assign ha ON ha.hall_id = h.id WHERE ha.item_id = ", id)
        .fetch_scalar_list(&st.db)
        .await
        .unwrap_or_default();
    for slug in site {
        clear_site_list(st, &slug).await;
    }
    let mine = sql!("SELECT uh.user_id, uh.slug FROM user_halls uh JOIN user_halls_assign a ON a.hall_id = uh.id WHERE a.item_id = ", id)
        .fetch_all(&st.db)
        .await
        .unwrap_or_default();
    for r in mine {
        clear_user_list(st, crate::db::RowExt::i64(&r, "user_id").unwrap_or(0), &crate::db::RowExt::string(&r, "slug")).await;
    }
}

/// The picture of a list without f0cks to show: its name on a colour of its own, like a tag
/// without pictures (tag_image.rs `fallback_svg`).
pub fn list_fallback(name: &str) -> Response {
    let mut r = Response::new(axum::body::Body::from(crate::routes::tag_image::fallback_svg(name)));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=600"));
    r
}

/// `image_v` of a list for its picture URL (`?v=`): the own picture's time, else what decides the
/// built one (how many f0cks, the newest), so the URL changes with the picture and browsers don't
/// keep showing the old one.
pub async fn image_v(custom_file: &Path, list: &crate::db::Row) -> String {
    use crate::db::RowExt;
    if list.bool("custom_image") {
        if let Ok(m) = tokio::fs::metadata(custom_file).await.and_then(|m| m.modified()) {
            return m.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0).to_string();
        }
    }
    let n = list.val("total_items");
    let n = n.as_i64().or_else(|| n.as_str().and_then(|s| s.parse().ok())).unwrap_or(0);
    format!("{n}-{}", list.i64("latest_item_id").unwrap_or(0))
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
    let m = norm_mode(mosaic_mode_for(&st.cfg(), &req, mode_param(&req).0));
    let mode = Some(m);
    let mut name: Option<String> = None;
    let r: anyhow::Result<Option<Response>> = async {
        if let Some(r) = serve_custom(&req, &custom_dir(&st).join(format!("{slug}.webp"))).await {
            return Ok(Some(r));
        }
        name = sql!("SELECT name FROM halls WHERE slug = ", slug.as_str(), " LIMIT 1").fetch_optional(&st.db).await?.map(|r| crate::db::RowExt::string(&r, "name"));
        if name.is_none() {
            return Ok(None);
        }
        let cache_path = site_cache_path(&st, &slug, m);
        if let Some(r) = serve_cached(&cache_path).await {
            return Ok(Some(r));
        }
        let mut q = crate::db::Sql::new("SELECT i.id FROM items i JOIN halls_assign ha ON ha.item_id = i.id JOIN halls h ON h.id = ha.hall_id ");
        q.push(mosaic_mode_join(mode))
            .push(" WHERE h.slug = ")
            .bind(slug.as_str())
            .push(" AND i.active = true AND i.is_deleted = false AND COALESCE(i.visibility, 0) = 0 ORDER BY RANDOM() LIMIT 3");
        let ids: Vec<i64> = q.fetch_all(&st.db).await?.iter().filter_map(|r| crate::db::RowExt::i64(r, "id")).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        build_mosaic(&st, &ids, &cache_path).await
    }
    .await;
    match r {
        Ok(Some(resp)) => Ok(mosaic_cache(resp, m)),
        Ok(None) => Ok(name.map(|n| list_fallback(&n)).unwrap_or_else(placeholder)),
        Err(e) => {
            tracing::error!("[HALL_IMAGE] Error: {e:#}");
            Ok(placeholder())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::list_url;

    /// An empty list's picture: its name (escaped) on a colour of its own, as an SVG.
    #[test]
    fn empty_list_gets_its_name_as_picture() {
        let r = super::list_fallback("cats & <dogs>");
        assert_eq!(r.headers()["content-type"], "image/svg+xml");
        let svg = crate::routes::tag_image::fallback_svg("cats & <dogs>");
        assert!(svg.contains(">cats &amp; &lt;dogs&gt;</text>"));
        assert_eq!(svg, crate::routes::tag_image::fallback_svg("cats & <dogs>"), "the same colours every time");
        assert_eq!((super::norm_mode(0), super::norm_mode(1), super::norm_mode(2), super::norm_mode(4)), (0, 1, 3, 3));
    }

    #[test]
    fn old_hall_urls_move_to_lists() {
        assert_eq!(list_url("/halls", ""), "/lists");
        assert_eq!(list_url("/halls/", ""), "/lists");
        assert_eq!(list_url("/h/best", ""), "/list/best");
        assert_eq!(list_url("/h/best/video/p/2", "x=1"), "/list/best/video/p/2?x=1");
        assert_eq!(list_url("/h/best/4711", ""), "/list/best/4711");
        assert_eq!(list_url("/user/Al%20B/halls", ""), "/user/Al%20B/lists");
        assert_eq!(list_url("/user/al/hall/mine", ""), "/user/al/list/mine");
        assert_eq!(list_url("/user/al/hall/mine/p/3", ""), "/user/al/list/mine/p/3");
        assert_eq!(list_url("/user/al/hall/mine/4711", ""), "/user/al/list/mine/4711");
    }
}
