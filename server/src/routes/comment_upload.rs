//! Comment attachments (port of src/comment_upload_handler.mjs), registered as bypass routes
//! like in index.mjs:
//!   POST   /api/v2/comments/upload       — multipart upload, staged with `comment_id = NULL`
//!   DELETE /api/v2/comments/upload/:id   — the user removes a staged attachment
//! Posting the comment (routes/comments.rs) links the staged rows to it.
//!
//! The session middleware has already run (bans, anonymous access rules apply). The upload path
//! is exempt from the global CSRF check; the token is taken from the `X-CSRF-Token` header and
//! checked before the body is read. DELETE goes through the global CSRF check.
//!
//! Changes from the old handler:
//! * The body is streamed to temp files with a hard cap instead of buffering up to 10× the file
//!   limit in memory; every file is checked (size, declared and detected type) before any is
//!   stored, so a rejected batch no longer leaves orphaned attachments behind.
//! * `fileupload_comments_max` (files per comment) is enforced.
//! * ImageMagick gets the detected format forced (`png:/path`) and resource limits; audio cover
//!   art is decoded instead of stream-copied into a `.png` name.
//! * The stored original file name is reduced to its last path component without control chars.
//! * Cancelling an attachment keeps the file when another attachment still uses it (identical
//!   uploads are stored once and symlinked).

use crate::perms::{self, P};
use crate::db::RowExt;
use crate::http::{Req, Res, Router, json_raw};
use crate::media::multipart::{self, FilePart, MpError, Part};
use crate::media::{WorkDir, make_public, move_file};
use crate::session::Session;
use crate::sql;
use crate::state::AppState;
use axum::body::Body;
use axum::http::Method;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Comment images are scaled down to this size.
const IMG_MAX_DIM: u32 = 1280;
/// GIFs up to this size become WebP, larger ones WebM.
const GIF_WEBM_THRESHOLD: u64 = 5 * 1024 * 1024;
const THUMB_SIZE: u32 = 512;

pub fn register(r: &mut Router) {
    r.pre(Method::POST, r"^\/api\/v2\/comments\/upload$", None, upload);
    r.pre(Method::DELETE, r"^\/api\/v2\/comments\/upload\/(?<id>\d+)$", None, cancel);
}

fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

fn fail(code: u16, msg: &str) -> Res {
    send(json!({ "success": false, "msg": msg }), code)
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// `fileupload_comments_mimes` categories (default image/video/audio) resolved against `cfg.mimes`.
fn allowed_mimes(st: &AppState) -> Vec<String> {
    let cfg = st.cfg();
    let cats = cfg
        .get("websrv.fileupload_comments_mimes")
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_lowercase)).collect::<Vec<_>>())
        .unwrap_or_else(|| vec!["image".into(), "video".into(), "audio".into()]);
    let all: Vec<String> = cfg.mimes().keys().cloned().collect();
    crate::media::mime::allowed_from(&all, Some(cats))
}

/// ImageMagick coder for a detected image type (forced, so ImageMagick never guesses).
fn coder(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpeg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/avif" => "avif",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/tiff" => "tiff",
        _ => return None,
    })
}

async fn magick(args: &[&str]) -> anyhow::Result<crate::proc::Output> {
    let mut all: Vec<&str> = vec!["-limit", "memory", "512MiB", "-limit", "map", "1GiB", "-limit", "width", "32KP", "-limit", "height", "32KP"];
    all.extend_from_slice(args);
    let opts = crate::proc::Opts { timeout: Duration::from_secs(300), quiet: true, ..Default::default() };
    crate::proc::spawn_with("magick", &all, opts).await
}

async fn tool(cmd: &str, args: &[&str]) -> anyhow::Result<crate::proc::Output> {
    crate::proc::spawn_with(cmd, args, crate::proc::Opts { quiet: true, ..Default::default() }).await
}

async fn file_size(p: &Path) -> u64 {
    tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0)
}

async fn probe_duration(p: &Path) -> Option<f64> {
    let out = tool("ffprobe", &["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", &s(p)]).await.ok()?;
    out.stdout_str().trim().parse::<f64>().ok().filter(|d| *d > 0.0)
}

// ── Upload ──────────────────────────────────────────────────────────────────

/// Comment files per user in [`UPLOAD_RATE_WINDOW_MS`] (staff unlimited). Files are public under /c/
/// at once, so without a limit the endpoint is free file hosting that can fill the disk.
const UPLOAD_RATE_LIMIT: usize = 30;
const UPLOAD_RATE_WINDOW_MS: i64 = 10 * 60_000;
static UPLOAD_RATE: once_cell::sync::Lazy<parking_lot::Mutex<std::collections::HashMap<i64, Vec<i64>>>> =
    once_cell::sync::Lazy::new(Default::default);

fn upload_rate_limited(user_id: i64) -> bool {
    let now = crate::util::now_ms();
    let start = now - UPLOAD_RATE_WINDOW_MS;
    let mut map = UPLOAD_RATE.lock();
    if map.len() > 5000 {
        map.retain(|_, ts| ts.iter().any(|t| *t > start));
    }
    let ts = map.entry(user_id).or_default();
    ts.retain(|t| *t > start);
    if ts.len() >= UPLOAD_RATE_LIMIT {
        return true;
    }
    ts.push(now);
    false
}

async fn upload(st: AppState, mut req: Req) -> Res {
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    if !crate::routes::uploads::csrf_header_ok(&req, &session) {
        return fail(403, "Invalid CSRF token");
    }
    let unlimited = perms::can(&st, Some(&session), P::UploadUnlimited);
    if !unlimited && upload_rate_limited(session.id) {
        return fail(429, "Too many uploads, please wait a few minutes.");
    }
    if !crate::cap::allowed(&st.cfg(), &req, &session, crate::cap::Scope::Comment) {
        return send(json!({ "success": false, "msg": "Please complete the captcha.", "captcha_required": "comment" }), 403);
    }
    let cfg = st.cfg();
    for p in [P::Comment, P::CommentFiles] {
        if !perms::can(&st, Some(&session), p) {
            return Ok(perms::denied(p));
        }
    }
    if !cfg.truthy("websrv.allow_fileupload_comments") {
        return fail(403, "Comment file uploads are disabled");
    }
    let ct = req.content_type().to_string();
    let Some(boundary) = multipart::boundary(&ct).filter(|_| ct.contains("multipart/form-data")) else {
        return fail(400, "Invalid content type");
    };

    let mut max_file = cfg.f64("websrv.fileupload_comments_size").filter(|v| *v > 0.0).unwrap_or(10.0 * 1024.0 * 1024.0);
    if perms::can(&st, Some(&session), P::UploadLarge) {
        max_file = (max_file * st.settings.upload_multiplier(&cfg)).floor();
    }
    let max_file = max_file as u64;

    let tmp = cfg.paths.tmp.clone();
    let body = req.take_body().unwrap_or_else(Body::empty);
    let limits = multipart::Limits { max_total: max_file.saturating_mul(10), max_text: 64 * 1024, max_parts: 64 };
    let form = match multipart::parse(body, &boundary, crate::routes::uploads::declared_len(&req), &tmp, limits).await {
        Ok(f) => f,
        Err(MpError::TooLarge) => return fail(413, "Request body too large"),
        Err(e) => {
            tracing::warn!("[COMMENT_UPLOAD] Malformed body: {e}");
            return fail(400, "No files provided");
        }
    };
    let files: Vec<FilePart> = form.entries.iter().filter_map(|(_, p)| if let Part::File(f) = p { Some(f.clone()) } else { None }).collect();
    if files.is_empty() {
        return fail(400, "No files provided");
    }
    let multi = cfg.truthy("websrv.fileupload_comments_multifile");
    if !multi && files.len() > 1 {
        return fail(400, "Only one file allowed per comment");
    }
    if let Some(max_n) = cfg.int("websrv.fileupload_comments_max").filter(|n| *n > 0) {
        if files.len() as i64 > max_n {
            return fail(400, &format!("Too many files (max {max_n})"));
        }
    }

    // Check everything before storing anything.
    let allowed = allowed_mimes(&st);
    for f in &files {
        if f.size > max_file {
            return fail(413, &format!("File \"{}\" exceeds the maximum size limit", f.filename));
        }
        if !allowed.contains(&f.content_type) {
            return fail(400, &format!("Invalid file type: {}", f.content_type));
        }
    }
    let mut detected: Vec<String> = Vec::with_capacity(files.len());
    for f in &files {
        let m = crate::media::mime::detect(&f.path).await.unwrap_or_default();
        if !allowed.contains(&m) {
            return fail(400, &format!("Invalid file type detected: {m}"));
        }
        detected.push(m);
    }
    // Files waiting to be attached count against a per-user quota until a comment uses them (or
    // purge_unattached removes them): /c/ is public at once, so it can't serve as free file hosting
    if !unlimited {
        let pending: i64 = sql!("SELECT COALESCE(SUM(size), 0)::bigint FROM comment_files WHERE user_id = ", session.id as i32, " AND comment_id IS NULL")
            .fetch_scalar(&st.db)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
        let incoming: u64 = files.iter().map(|f| f.size).sum();
        if pending.max(0) as u64 + incoming > UNATTACHED_QUOTA_BYTES {
            return fail(413, "Too many files waiting to be posted. Post your comment or wait a day for unused uploads to expire.");
        }
    }
    for d in [&cfg.paths.c, &cfg.paths.tmp, &cfg.paths.t] {
        let _ = tokio::fs::create_dir_all(d).await;
    }

    let mut results: Vec<Value> = Vec::new();
    for (f, mime) in files.iter().zip(detected) {
        match process_file(&st, &session, f, mime).await {
            Ok(v) => results.push(v),
            Err(e) => {
                tracing::error!("[COMMENT_UPLOAD] Error: {e:#}");
                return fail(500, "Upload failed");
            }
        }
    }
    drop(form);
    send(json!({ "success": true, "files": results }), 200)
}

// ── Unattached files ────────────────────────────────────────────────────────

/// Bytes of uploaded but not yet attached comment files a user may have (staff unlimited).
const UNATTACHED_QUOTA_BYTES: u64 = 200 * 1024 * 1024;
/// Hours an upload may wait for its comment before [`purge_unattached`] removes it.
const UNATTACHED_HOURS: i32 = 24;
/// Rows looked at per run (each one checks the comment and chat texts).
const PURGE_BATCH: i64 = 100;

/// Remove comment files that were uploaded but never attached to a comment within
/// [`UNATTACHED_HOURS`]: row, file in c/ and thumbnail t/cf_<name>.webp. Kept when a comment or
/// a global chat message mentions the file name (pasted link), and when another file in c/ is a
/// symlink to it (identical or similar later uploads link instead of copying, see `process_file`).
/// A file that is itself a symlink is only unlinked. Returns how many rows were removed.
pub async fn purge_unattached(st: &AppState) -> u64 {
    let cfg = st.cfg();
    let rows = sql!(
        "SELECT cf.id, cf.dest FROM comment_files cf WHERE cf.comment_id IS NULL AND cf.created_at < now() - make_interval(hours => ",
        UNATTACHED_HOURS,
        ") AND NOT EXISTS (SELECT 1 FROM comments c WHERE strpos(c.content, cf.dest) > 0)
           AND NOT EXISTS (SELECT 1 FROM global_chat g WHERE strpos(g.message, cf.dest) > 0)
         ORDER BY cf.id LIMIT ",
        PURGE_BATCH
    )
    .fetch_all(&st.db)
    .await;
    let rows = match rows {
        Ok(r) if !r.is_empty() => r,
        Ok(_) => return 0,
        Err(e) => {
            tracing::error!("[COMMENT_UPLOAD] Unattached lookup failed: {e}");
            return 0;
        }
    };
    let link_targets = symlink_targets(&cfg.paths.c).await;
    let mut removed = 0;
    for r in rows {
        let (id, dest) = (r.i64("id").unwrap_or(0), r.string("dest"));
        if !crate::media::is_plain_name(&dest) {
            continue;
        }
        let path = cfg.paths.c.join(&dest);
        match tokio::fs::symlink_metadata(&path).await {
            Ok(m) if m.file_type().is_symlink() => {
                let _ = tokio::fs::remove_file(&path).await;
            }
            Ok(_) => {
                let real = tokio::fs::canonicalize(&path).await.unwrap_or_else(|_| path.clone());
                if link_targets.contains(&real) {
                    continue; // another comment file links here
                }
                let _ = tokio::fs::remove_file(&path).await;
            }
            Err(_) => {} // already gone: just the row
        }
        let stem = dest.split('.').next().unwrap_or("");
        let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("cf_{stem}.webp"))).await;
        match sql!("DELETE FROM comment_files WHERE id = ", id as i32, " AND comment_id IS NULL").execute(&st.db).await {
            Ok(n) => removed += n,
            Err(e) => tracing::error!("[COMMENT_UPLOAD] Removing unattached file {id} failed: {e}"),
        }
    }
    removed
}

/// Real paths that symlinks in `dir` point to.
async fn symlink_targets(dir: &Path) -> std::collections::HashSet<PathBuf> {
    let mut out = std::collections::HashSet::new();
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return out };
    while let Ok(Some(e)) = rd.next_entry().await {
        if e.file_type().await.is_ok_and(|t| t.is_symlink()) {
            if let Ok(real) = tokio::fs::canonicalize(e.path()).await {
                out.insert(real);
            }
        }
    }
    out
}

/// Symlink `dest` to the real file behind `existing` (relative link).
async fn link_to(existing: &Path, dest: &Path) -> bool {
    let r: std::io::Result<()> = async {
        let real = tokio::fs::canonicalize(existing).await?;
        let rel = crate::media::relative_path(dest.parent().unwrap_or(Path::new("/")), &real);
        tokio::fs::symlink(&rel, dest).await
    }
    .await;
    match r {
        Ok(()) => true,
        Err(e) => {
            tracing::error!("[COMMENT_UPLOAD] Symlink failed: {e}");
            false
        }
    }
}

/// Existing media file of ours named by a DB value, if it is a plain file name.
fn existing_in(dir: &Path, dest: &str) -> Option<PathBuf> {
    crate::media::is_plain_name(dest).then(|| dir.join(dest))
}

async fn process_file(st: &AppState, session: &Session, file: &FilePart, mut mime: String) -> anyhow::Result<Value> {
    let cfg = st.cfg();
    let uuid = crate::media::queue::genuuid();
    let work = WorkDir::new(&cfg, "cupload").await?;
    let tmp = work.join("in.tmp");
    move_file(&file.path, &tmp).await?;
    let tmp_s = s(&tmp);

    // Audio-only MP4 containers
    if mime == "video/mp4" || mime == "video/quicktime" {
        let ext = file.filename.rsplit('.').next().unwrap_or("").to_lowercase();
        if ext == "m4a" || ext == "aac" {
            mime = "audio/mp4".into();
        } else if let Ok(o) = tool("ffprobe", &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_type", "-of", "csv=p=0", &tmp_s]).await {
            if o.stdout_str().trim().is_empty() {
                mime = "audio/mp4".into();
            }
        }
    }
    let mut ext = crate::media::mime::stored_ext(&cfg, &tmp, &mime).await.filter(|e| !e.is_empty()).unwrap_or_else(|| "bin".into());

    // GIF → WebP (small) or WebM (large), when that is smaller
    let mut converted_gif = false;
    if mime == "image/gif" {
        let gif_size = file.size;
        let resize = format!("{IMG_MAX_DIM}x{IMG_MAX_DIM}>");
        if gif_size <= GIF_WEBM_THRESHOLD {
            let out = work.join("conv.webp");
            let input = format!("gif:{tmp_s}");
            let output = format!("webp:{}", s(&out));
            match magick(&[&input, "-coalesce", "-resize", &resize, "-quality", "40", "+repage", &output]).await {
                Ok(_) if file_size(&out).await > 0 && file_size(&out).await < gif_size => {
                    tokio::fs::rename(&out, &tmp).await?;
                    tracing::info!("[COMMENT_UPLOAD] GIF → WebP: {}", file.filename);
                    (mime, ext, converted_gif) = ("image/webp".into(), "webp".into(), true);
                }
                Ok(_) => tracing::info!("[COMMENT_UPLOAD] WebP larger than GIF, keeping original: {}", file.filename),
                Err(e) => tracing::warn!("[COMMENT_UPLOAD] GIF→WebP failed, keeping original: {e}"),
            }
        } else {
            let out = work.join("conv.webm");
            let vf = format!("scale='min({IMG_MAX_DIM},iw)':min'({IMG_MAX_DIM},ih)':force_original_aspect_ratio=decrease");
            let o = s(&out);
            let args = ["-y", "-i", &tmp_s, "-an", "-vf", &vf, "-c:v", "libvpx-vp9", "-pix_fmt", "yuva420p", "-auto-alt-ref", "0", "-crf", "30", "-b:v", "0", &o];
            match tool("ffmpeg", &args).await {
                Ok(_) if file_size(&out).await > 0 && file_size(&out).await < gif_size => {
                    tokio::fs::rename(&out, &tmp).await?;
                    tracing::info!("[COMMENT_UPLOAD] GIF → WebM: {}", file.filename);
                    (mime, ext, converted_gif) = ("video/webm".into(), "webm".into(), true);
                }
                Ok(_) => tracing::info!("[COMMENT_UPLOAD] WebM also larger, keeping original GIF: {}", file.filename),
                Err(e) => tracing::warn!("[COMMENT_UPLOAD] GIF→WebM failed, keeping original: {e}"),
            }
        }
    }

    // Downscale large still images
    if mime.starts_with("image/") && mime != "image/gif" {
        if let Some(c) = coder(&mime) {
            let first = format!("{c}:{tmp_s}[0]");
            match magick(&[&first, "-format", "%wx%h", "info:"]).await {
                Ok(o) => {
                    let dims = o.stdout_str();
                    let (w, h) = crate::media::queue::parse_wxh(dims.trim(), 'x').unwrap_or((0, 0));
                    if w as u32 > IMG_MAX_DIM || h as u32 > IMG_MAX_DIM {
                        let io = format!("{c}:{tmp_s}");
                        let resize = format!("{IMG_MAX_DIM}x{IMG_MAX_DIM}>");
                        magick(&[&io, "-resize", &resize, "-quality", "85", &io]).await?;
                        tracing::info!("[COMMENT_UPLOAD] Resized {w}x{h} → max {IMG_MAX_DIM}px: {}", file.filename);
                    }
                }
                Err(e) => tracing::warn!("[COMMENT_UPLOAD] Resize check failed: {e}"),
            }
        }
    }

    let filename = format!("{uuid}.{ext}");
    let checksum = crate::media::queue::sha256_file(&tmp).await?;
    let dest_path = cfg.paths.c.join(&filename);

    // Repost detection: identical comment file or item → symlink instead of a copy
    let mut linked = false;
    if let Some(dest) = sql!("SELECT dest FROM comment_files WHERE checksum = ", checksum.as_str(), " LIMIT 1").fetch_optional(&st.db).await?.map(|r| r.string("dest")) {
        if let Some(p) = existing_in(&cfg.paths.c, &dest) {
            linked = link_to(&p, &dest_path).await;
        }
    }
    if !linked {
        if let Some(dest) = sql!("SELECT dest FROM items WHERE checksum = ", checksum.as_str(), " LIMIT 1").fetch_optional(&st.db).await?.map(|r| r.string("dest")) {
            if let Some(p) = existing_in(&cfg.paths.b, &dest) {
                linked = link_to(&p, &dest_path).await;
            }
        }
    }
    let mut phash: Option<String> = None;
    if mime.starts_with("image/") || mime.starts_with("video/") {
        phash = crate::media::queue::generate_phash(&tmp).await;
        if phash.is_some() && !linked {
            if let Some(row) = crate::media::queue::check_comment_repost_phash(st, phash.as_deref()).await? {
                if let Some(p) = existing_in(&cfg.paths.c, &row.string("dest")) {
                    linked = link_to(&p, &dest_path).await;
                }
            }
            if !linked {
                if let Some(item) = crate::media::queue::check_repost_phash(st, phash.as_deref()).await? {
                    // Only a public, live item: the similar (not identical) upload is served as that
                    // item's original under /c/, which no private/unavailable gate covers
                    let public = sql!(
                        "SELECT dest FROM items WHERE id = ", item as i32,
                        " AND active = true AND is_deleted = false AND COALESCE(visibility, 0) = 0 LIMIT 1"
                    );
                    if let Some(dest) = public.fetch_optional(&st.db).await?.map(|r| r.string("dest")) {
                        if let Some(p) = existing_in(&cfg.paths.b, &dest) {
                            linked = link_to(&p, &dest_path).await;
                        }
                    }
                }
            }
        }
    }
    if !linked {
        tokio::fs::copy(&tmp, &dest_path).await?;
        make_public(&dest_path).await;
    }

    let thumb = cfg.paths.t.join(format!("cf_{uuid}.webp"));
    if let Err(e) = comment_thumbnail(&work, &dest_path, &mime, &thumb).await {
        tracing::warn!("[COMMENT_UPLOAD] Thumbnail generation failed for {filename}: {e}");
        let spec = format!("{THUMB_SIZE}x{THUMB_SIZE}");
        let _ = magick(&["-size", &spec, "xc:#1a1a1a", &format!("webp:{}", s(&thumb))]).await;
    }
    make_public(&thumb).await;

    let original = multipart::sanitize_filename(&file.filename);
    let row = sql!(
        "INSERT INTO comment_files (user_id, dest, mime, size, checksum, phash, original_filename) VALUES (",
        session.id as i32, ", ", filename.as_str(), ", ", mime.as_str(), ", ", file.size.min(i32::MAX as u64) as i32, ", ", checksum.as_str(), ", ", phash,
        ", ", (!original.is_empty()).then_some(original), ") RETURNING id, dest, mime"
    )
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("insert returned nothing"))?;
    Ok(json!({
        "id": row.val("id"),
        "dest": row.val("dest"),
        "mime": row.val("mime"),
        "thumbnail": format!("/t/cf_{uuid}.webp"),
        "converted_gif": converted_gif,
    }))
}

/// `generateCommentThumbnail`: square WebP thumbnail of a comment file.
async fn comment_thumbnail(work: &WorkDir, source: &Path, mime: &str, thumb: &Path) -> anyhow::Result<()> {
    let real = tokio::fs::canonicalize(source).await.unwrap_or_else(|_| source.to_path_buf());
    let src = s(&real);
    let tmp_png = work.join("thumb.png");
    let tf = s(&tmp_png);
    let spec = format!("{THUMB_SIZE}x{THUMB_SIZE}");

    if mime.starts_with("video/") || mime == "image/gif" {
        let ff = THUMB_SIZE.max(512).to_string();
        for seek in ["20%", "40%", "60%", "80%"] {
            if let Err(e) = tool("ffmpegthumbnailer", &["-i", &src, "-s", &ff, "-t", seek, "-o", &tf]).await {
                tracing::warn!("[COMMENT_UPLOAD] ffmpegthumbnailer failed at {seek}, trying ffmpeg fallback: {e}");
                let pct = seek.trim_end_matches('%').parse::<f64>().unwrap_or(0.0) / 100.0;
                let secs = match probe_duration(&real).await {
                    Some(d) => d * pct,
                    None => match seek {
                        "20%" => 2.0,
                        "40%" => 5.0,
                        "60%" => 8.0,
                        _ => 10.0,
                    },
                };
                let vf = format!("scale={ff}:{ff}:force_original_aspect_ratio=increase,crop={ff}:{ff}");
                let ss = crate::util::js_num(secs);
                tool("ffmpeg", &["-y", "-ss", &ss, "-color_trc", "1", "-i", &src, "-frames:v", "1", "-update", "1", "-vf", &vf, &tf]).await?;
            }
            match magick(&[&format!("png:{tf}"), "-colorspace", "Gray", "-format", "%[fx:mean]", "info:"]).await {
                Ok(o) if o.stdout_str().trim().parse::<f64>().map(|m| m > 0.05).unwrap_or(false) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    } else if mime.starts_with("image/") {
        let c = coder(mime).ok_or_else(|| anyhow::anyhow!("no thumbnailer for {mime}"))?;
        magick(&[&format!("{c}:{src}[0]"), &format!("png:{tf}")]).await?;
    } else if mime.starts_with("audio/") {
        let cover = tool("ffmpeg", &["-y", "-i", &src, "-an", "-frames:v", "1", "-update", "1", &tf]).await.is_ok() && file_size(&tmp_png).await > 0;
        if !cover {
            magick(&["-size", &spec, "xc:#1a1a1a", "-gravity", "center", "-fill", "#ffffff", "-pointsize", "48", "-annotate", "0", "♪", &format!("png:{tf}")]).await?;
        }
    }

    let fill = format!("{spec}^");
    let crop = format!("{spec}+0+0");
    let out = format!("webp:{}", s(thumb));
    if magick(&[&format!("png:{tf}"), "-resize", &fill, "-gravity", "center", "-crop", &crop, "+repage", "-quality", "85", &out]).await.is_err() {
        magick(&["-size", &spec, "xc:#1a1a1a", &out]).await?;
    }
    Ok(())
}

// ── Cancel ──────────────────────────────────────────────────────────────────

async fn cancel(st: AppState, req: Req) -> Res {
    let Some(session) = req.session.clone() else { return fail(401, "Unauthorized") };
    let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()).filter(|i| *i != 0) else { return fail(400, "Invalid file ID") };
    let r: anyhow::Result<()> = async {
        let Some(row) = sql!("SELECT id, dest, checksum FROM comment_files WHERE id = ", id, " AND user_id = ", session.id as i32, " AND comment_id IS NULL")
            .fetch_optional(&st.db)
            .await?
        else {
            // Unknown, someone else's or already linked — silently OK
            return Ok(());
        };
        let dest = row.string("dest");
        sql!("DELETE FROM comment_files WHERE id = ", id).execute(&st.db).await?;
        let cfg = st.cfg();
        if let Some(path) = existing_in(&cfg.paths.c, &dest) {
            let link = tokio::fs::read_link(&path).await.ok();
            let shared = sql!("SELECT id FROM comment_files WHERE checksum = ", row.string("checksum"), " LIMIT 1").fetch_optional(&st.db).await?.is_some();
            match (link, shared) {
                // Our own copy that other attachments link to: keep it
                (None, true) => {}
                (None, false) => {
                    let _ = tokio::fs::remove_file(&path).await;
                }
                (Some(target), shared) => {
                    let _ = tokio::fs::remove_file(&path).await;
                    // Last user of a comment file we linked to (its own row is gone): remove it too.
                    // Item files (b/) are never touched.
                    let abs = crate::media::delete::resolve_link(&path, &target);
                    if !shared && abs.parent() == Some(cfg.paths.c.as_path()) {
                        let _ = tokio::fs::remove_file(&abs).await;
                    }
                }
            }
            let uuid = dest.split('.').next().unwrap_or("");
            let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("cf_{uuid}.webp"))).await;
        }
        tracing::info!("[COMMENT_UPLOAD] Cancelled (user-removed) attachment deleted: {dest}");
        Ok(())
    }
    .await;
    match r {
        Ok(()) => send(json!({ "success": true }), 200),
        Err(e) => {
            tracing::error!("[COMMENT_UPLOAD] Cancel error: {e}");
            fail(500, "Delete failed")
        }
    }
}
