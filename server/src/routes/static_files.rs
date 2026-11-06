//! Media and asset files (port of src/inc/routes/static.mjs).

use crate::http::files::{not_found, safe_join, serve_dir, serve_file};
use crate::http::{Req, Res, Router, reply};
use crate::sql;
use crate::state::AppState;
use axum::response::Response;

pub fn register(r: &mut Router) {
    r.get(r"^\/b\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.b, "/b/").await) });
    r.get(r"^\/c\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.c, "/c/").await) });
    r.get(r"^\/s\/emojis\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.emojis, "/s/emojis/").await) });
    r.get(r"^\/s\/koepfe\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.koepfe, "/s/koepfe/").await) });
    // Uploaded fonts, falling back to the built-in ones (public/s/fonts-builtin)
    r.get(r"^\/s\/fonts\/", |st: AppState, req: Req| async move {
        let cfg = st.cfg();
        let root = crate::fonts::serve_root(&cfg, req.path.strip_prefix("/s/fonts/").unwrap_or(""));
        Ok(serve_dir(&req, &root, "/s/fonts/").await)
    });
    // /s/ruffle/ is served from public/s/ruffle by the /s/ route (npm run vendor installs it)
    r.get(r"^\/s\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.s, "/s/").await) });
    r.get(r"^\/t\/(?<file>.+)$", thumb);
    r.get(r"^\/ca\/(?<file>.+)$", cover);
    r.get(r"^\/memes\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.memes, "/memes/").await) });
    r.get(r"^\/a\/", |st: AppState, req: Req| async move { Ok(serve_dir(&req, &st.cfg().paths.a, "/a/").await) });
    r.get(r"^\/robots\.txt$", |st: AppState, _req: Req| async move { public_file(&st, "robots.txt", "text/plain").await });
    r.get(r"^\/manifest\.json$", |st: AppState, _req: Req| async move {
        let (cfg, favicon) = (st.cfg(), st.settings.favicon_url());
        let icons = crate::pwa::ensure_icons(&cfg, &favicon).await;
        Ok(reply(200, "application/manifest+json", crate::pwa::manifest(&cfg, &favicon, icons.as_ref()).to_string()))
    });
    // The navbar brand font (admin panel → branding), see crate::fonts::brand_css
    r.get(r"^\/brand-font\.css$", |st: AppState, _req: Req| async move {
        let font = st.settings.get().brand_font;
        let css = crate::fonts::brand_css(&st.cfg(), &font);
        Ok(crate::http::with_header(reply(200, "text/css; charset=utf-8", css), "cache-control", "public, max-age=86400"))
    });
    // A visitor's chosen font as 'CustomUserFont' (#user-font-css in f0ckm and w0bm; settings.js
    // swaps it live), see crate::fonts::user_css.
    r.get(r"^\/user-font\.css$", |st: AppState, req: Req| async move {
        let weight = req.query("w").and_then(|w| w.parse::<u16>().ok());
        let css = crate::fonts::user_css(&st.cfg(), req.query("f").unwrap_or(""), weight);
        // Short: the admin can replace a family's files (font manager)
        Ok(crate::http::with_header(reply(200, "text/css; charset=utf-8", css), "cache-control", "public, max-age=300"))
    });
    r.get(r"^\/sw\.js$", |st: AppState, _req: Req| async move { public_file(&st, "sw.js", "application/javascript").await });
}

async fn public_file(st: &AppState, name: &str, ct: &str) -> Res {
    let p = st.cfg().paths.s.join("..").join(name);
    let body = tokio::fs::read_to_string(p).await?;
    Ok(reply(200, ct, body))
}

fn img_mime(f: &str) -> &'static str {
    if f.ends_with(".webp") {
        "image/webp"
    } else if f.ends_with(".png") {
        "image/png"
    } else if f.ends_with(".gif") {
        "image/gif"
    } else if f.ends_with(".jpg") || f.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

fn not_found_plain() -> Response {
    reply(404, "text/plain", "404 - file not found.")
}

/// `<id>.webp` / `<id>_blur.webp` of an item that isn't live is never public.
async fn is_hidden_media(st: &AppState, file: &str) -> bool {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"^(\d+)(?:_blur)?\.webp$").unwrap());
    match RE.captures(file).and_then(|c| c[1].parse::<i64>().ok()) {
        Some(id) => crate::hidden_items::is_hidden(st, id).await,
        None => false,
    }
}

/// Cache-Control comes from the middleware (public 1 h, or private for gated/protected files).
async fn image(req: &Req, path: &std::path::Path, file: &str) -> Response {
    serve_file(req, path, Some(img_mime(file))).await
}

async fn cover(st: AppState, req: Req) -> Res {
    let file = req.param("file").unwrap_or("").to_string();
    if is_hidden_media(&st, &file).await {
        return Ok(not_found_plain());
    }
    match safe_join(&st.cfg().paths.ca, &file) {
        Some(p) if p.is_file() => Ok(image(&req, &p, &file).await),
        _ => Ok(not_found_plain()),
    }
}

async fn thumb(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let file = req.param("file").unwrap_or("").to_string();
    if is_hidden_media(&st, &file).await {
        return Ok(not_found_plain());
    }
    let Some(path) = safe_join(&cfg.paths.t, &file) else { return Ok(not_found()) };
    if path.is_file() {
        return Ok(image(&req, &path, &file).await);
    }

    // Album entries: generate the thumbnail on demand, or fall back to the album's own.
    let base = {
        static EXT: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"\.[^.]+$").unwrap());
        let b = EXT.replace(&file, "").into_owned();
        b.strip_suffix("_blur").map(str::to_string).unwrap_or(b)
    };
    if base.contains('/') || base.contains("..") {
        return Ok(not_found_plain());
    }
    let like = format!("{}.%", crate::util::escape_like(&base));
    let sub = sql!("SELECT id, item_id, dest, mime FROM album_items WHERE dest LIKE ", like, " LIMIT 1").fetch_optional(&st.db).await;
    if let Ok(Some(sub)) = sub {
        use crate::db::RowExt;
        let thumb_dest = cfg.paths.t.join(format!("{base}.webp"));
        if let Some(src) = safe_join(&cfg.paths.b, &sub.string("dest")) {
            let ok = tokio::fs::metadata(&src).await.map(|m| m.len() > 0).unwrap_or(false);
            if ok {
                let mime = sub.string("mime");
                let (s, d) = (src.to_string_lossy().into_owned(), thumb_dest.to_string_lossy().into_owned());
                if mime.starts_with("image/") {
                    let first = format!("{s}[0]");
                    let _ = crate::proc::spawn("magick", &[&first, "-resize", "256x256^", "-gravity", "center", "-crop", "256x256+0+0", "+repage", &d]).await;
                } else if mime.starts_with("video/") && crate::proc::spawn("ffmpegthumbnailer", &["-i", &s, "-s", "256", "-o", &d]).await.is_err() {
                    let _ = crate::proc::spawn(
                        "ffmpeg",
                        &["-y", "-ss", "00:00:01", "-i", &s, "-vframes", "1", "-vf", "scale=256:256:force_original_aspect_ratio=increase,crop=256:256", &d],
                    )
                    .await;
                }
                if tokio::fs::metadata(&thumb_dest).await.map(|m| m.len() > 0).unwrap_or(false) {
                    return Ok(image(&req, &thumb_dest, "x.webp").await);
                }
            }
        }
        let parent = cfg.paths.t.join(format!("{}.webp", sub.i64("item_id").unwrap_or(0)));
        if tokio::fs::metadata(&parent).await.map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(image(&req, &parent, "x.webp").await);
        }
    }
    Ok(not_found_plain())
}
