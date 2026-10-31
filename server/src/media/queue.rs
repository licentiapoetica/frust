//! Media processing helpers (`src/inc/queue.mjs`): perceptual hashes, duplicate checks,
//! thumbnails, cover art, blurred thumbnails, metadata. External tools run via `crate::proc`
//! (argument vectors, never a shell).

use super::{WorkDir, is_plain_name, mime, proxy_url};
use crate::config::Config;
use crate::db::{Row, RowExt, Sql};
use crate::proc::{self, Opts};
use crate::sql;
use crate::state::AppState;
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

fn quiet() -> Opts {
    Opts { quiet: true, ..Opts::default() }
}

fn quiet_any_exit() -> Opts {
    Opts { quiet: true, ignore_exit_code: true, ..Opts::default() }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

// ── Perceptual hash ──────────────────────────────────────────────────────────

/// Gray 33x32 frame that is (nearly) one flat colour, too dark or too bright.
pub fn is_flat_frame(buf: &[u8]) -> bool {
    if buf.len() != 1056 {
        return true;
    }
    let (mut min, mut max, mut sum) = (255u8, 0u8, 0u64);
    for &v in buf {
        min = min.min(v);
        max = max.max(v);
        sum += v as u64;
    }
    let mean = sum as f64 / buf.len() as f64;
    if !(15.0..=240.0).contains(&mean) {
        return true;
    }
    let var = buf.iter().map(|&v| (v as f64 - mean).powi(2)).sum::<f64>() / buf.len() as f64;
    var < 10.0 || (max - min) < 15
}

/// dHash of a 33x32 gray frame: 1024 bits (left > right) as 256 hex chars.
pub fn dhash(buf: &[u8]) -> String {
    let mut out = String::with_capacity(256);
    let (mut cur, mut bits) = (0u8, 0);
    for y in 0..32 {
        for x in 0..32 {
            let bit = (buf[y * 33 + x] > buf[y * 33 + x + 1]) as u8;
            cur = (cur << 1) | bit;
            bits += 1;
            if bits == 8 {
                out.push_str(&format!("{cur:02x}"));
                cur = 0;
                bits = 0;
            }
        }
    }
    out
}

async fn probe_duration(path: &Path) -> Option<f64> {
    let p = s(path);
    let out = proc::spawn_with("ffprobe", &["-v", "error", "-show_entries", "format=duration", "-of", "default=noprint_wrappers=1:nokey=1", &p], quiet()).await.ok()?;
    out.stdout_str().trim().parse::<f64>().ok().filter(|d| *d > 0.0)
}

/// Temporal dHash: 3 frames (10/50/90 %) for videos, 1 for images, joined by `_`.
pub async fn generate_phash(source: &Path) -> Option<String> {
    if source.to_string_lossy().to_lowercase().ends_with(".pdf") {
        return None;
    }
    let duration = probe_duration(source).await;
    let timestamps: Vec<Option<f64>> = match duration {
        Some(d) => vec![Some(d * 0.1), Some(d * 0.5), Some(d * 0.9)],
        None => vec![None],
    };
    let src = s(source);
    let mut hashes = Vec::new();
    for ts in timestamps {
        let mut args: Vec<String> = Vec::new();
        if let Some(t) = ts {
            args.extend(["-ss".into(), crate::util::js_num(t)]);
        }
        let vf = if ts.is_some() { "thumbnail,scale=33:32,format=gray" } else { "scale=33:32,format=gray" };
        args.extend(["-v", "error", "-i", &src, "-vf", vf, "-frames:v", "1", "-f", "rawvideo", "pipe:1"].map(String::from));
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let buf = match proc::spawn_with("ffmpeg", &refs, quiet()).await {
            Ok(o) => o.stdout,
            Err(e) => {
                tracing::warn!("[PHASH] Failed to extract frame at {ts:?}s for {src}: {e}");
                continue;
            }
        };
        if buf.len() != 1056 {
            tracing::warn!("[PHASH] Invalid buffer length ({}) at {ts:?}s for {src}", buf.len());
            continue;
        }
        if is_flat_frame(&buf) {
            tracing::info!("[PHASH] Ignored flat/black frame at {ts:?}s for {src}");
            continue;
        }
        hashes.push(dhash(&buf));
    }
    if hashes.is_empty() { None } else { Some(hashes.join("_")) }
}

/// Up to three non-zero hash parts (`h1`, `h2`, `h3`, empty when missing).
pub fn phash_parts(hash: &str) -> Option<[String; 3]> {
    let parts: Vec<&str> = hash.split('_').filter(|p| !p.is_empty() && !p.starts_with("00000000")).collect();
    if parts.is_empty() {
        return None;
    }
    let g = |i: usize| parts.get(i).map(|p| p.to_string()).unwrap_or_default();
    Some([g(0), g(1), g(2)])
}

/// Number of matching frames (Hamming distance ≤ 15) between `phash` and the given parts.
fn match_count(h: &[String; 3]) -> Sql {
    let mut q = Sql::new("(");
    for (i, part) in h.iter().enumerate() {
        let n = i + 1;
        if i > 0 {
            q.push(" + ");
        }
        q.push(&format!("CASE WHEN split_part(phash, '_', {n}) != '' AND "))
            .bind(part.clone())
            .push(&format!(" != '' THEN bit_count(('x' || split_part(phash, '_', {n}))::bit(1024) # ('x' || "))
            .bind(part.clone())
            .push(")::bit(1024)) <= 15 ELSE false END::int");
    }
    q.push(")");
    q
}

/// Required matches: 2 when both sides have a 2nd/3rd frame, else 1.
fn required_matches(h: &[String; 3]) -> Sql {
    let mut q = Sql::new("(CASE WHEN split_part(phash, '_', 3) != '' AND ");
    q.bind(h[2].clone())
        .push(" != '' THEN 2 WHEN split_part(phash, '_', 2) != '' AND ")
        .bind(h[1].clone())
        .push(" != '' THEN 2 ELSE 1 END)");
    q
}

pub async fn check_repost_phash(st: &AppState, hash: Option<&str>) -> Result<Option<i64>> {
    let Some(h) = hash.and_then(phash_parts) else { return Ok(None) };
    let q = sql!(
        "SELECT id FROM items WHERE is_deleted = false AND phash IS NOT NULL AND phash != '' AND phash != 'ERROR' AND phash != 'MISSING' AND phash NOT LIKE '00000000%' AND ",
        @match_count(&h), " >= ", @required_matches(&h), " LIMIT 1"
    );
    Ok(q.fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

/// All items with at least one matching frame (`findallrepostphash`).
pub async fn find_all_repost_phash(st: &AppState, hash: Option<&str>, exclude_id: Option<i64>) -> Result<Vec<Row>> {
    let Some(h) = hash.and_then(phash_parts) else { return Ok(vec![]) };
    let mut q = sql!("SELECT id, username, stamp FROM items WHERE is_deleted = false AND phash IS NOT NULL AND phash != '' AND phash != 'ERROR' AND phash != 'MISSING' AND phash NOT LIKE '00000000%' ");
    if let Some(ex) = exclude_id.filter(|e| *e != 0) {
        q.push("AND id != ").bind(ex as i32).push(" ");
    }
    q.push("AND ").append(&match_count(&h)).push(" >= 1 ORDER BY id ASC");
    q.fetch_all(&st.db).await
}

pub async fn check_comment_repost_phash(st: &AppState, hash: Option<&str>) -> Result<Option<Row>> {
    let Some(h) = hash.and_then(phash_parts) else { return Ok(None) };
    let q = sql!(
        "SELECT id, dest FROM comment_files WHERE phash IS NOT NULL AND phash != '' AND phash NOT LIKE '00000000%' AND ",
        @match_count(&h), " >= ", @required_matches(&h), " LIMIT 1"
    );
    q.fetch_optional(&st.db).await
}

// ── Lookups ──────────────────────────────────────────────────────────────────

/// 48 random hex characters (the old code took them from two `gen_random_uuid()`s).
pub fn genuuid() -> String {
    crate::util::random_hex(24)
}

pub async fn check_repost_link(st: &AppState, link: &str) -> Result<Option<i64>> {
    Ok(sql!(r#"select id from "items" where src = "#, link, " limit 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

pub async fn check_repost_sum(st: &AppState, checksum: &str) -> Result<Option<i64>> {
    Ok(sql!(r#"select id from "items" where checksum = "#, checksum, " limit 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

pub async fn get_item_id(st: &AppState, dest: &str) -> Result<Option<i64>> {
    Ok(sql!(r#"select id from "items" where dest = "#, dest, " limit 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

/// SHA-256 of a file (hex), computed in-process instead of spawning `sha256sum`.
pub async fn sha256_file(path: &Path) -> Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut f = std::fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        Ok(hex::encode(h.finalize()))
    })
    .await?
}

// ── Thumbnails ───────────────────────────────────────────────────────────────

pub fn thumb_size(_tier: i64) -> u32 {
    512
}

/// Media dirs for live or pending items, symlinks resolved like `fs.realpath`.
pub async fn dirs(cfg: &Config, pending: bool) -> (PathBuf, PathBuf, PathBuf) {
    let (b, t, c) = if pending {
        (cfg.paths.pending.join("b"), cfg.paths.pending.join("t"), cfg.paths.pending.join("ca"))
    } else {
        (cfg.paths.b.clone(), cfg.paths.t.clone(), cfg.paths.ca.clone())
    };
    let real = |p: PathBuf| async move { tokio::fs::canonicalize(&p).await.unwrap_or(p) };
    (real(b).await, real(t).await, real(c).await)
}

async fn is_symlink(p: &Path) -> bool {
    tokio::fs::symlink_metadata(p).await.map(|m| m.file_type().is_symlink()).unwrap_or(false)
}

async fn file_size(p: &Path) -> u64 {
    tokio::fs::metadata(p).await.map(|m| m.len()).unwrap_or(0)
}

/// Config path like `/s/img/swf.png` → `<base>/public/s/img/swf.png`.
fn public_asset(cfg: &Config, p: &str) -> PathBuf {
    match p.strip_prefix('/') {
        Some(rel) => cfg.base.join("public").join(rel),
        None => PathBuf::from(p),
    }
}

async fn magick(args: &[&str]) -> Result<proc::Output> {
    proc::spawn("magick", args).await
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ThumbResult {
    /// Thumbnail generated (false: a placeholder/fallback was written instead).
    pub ok: bool,
    /// Audio cover art was found (the old `queue._lastCoverExtracted`).
    pub cover_extracted: bool,
}

/// `genThumbnail(filename, mime, itemid, link, pending, size)`: writes `t/<itemid>.webp`
/// (and `ca/<itemid>.webp` for audio cover art). Never fails; falls back to placeholders.
pub async fn gen_thumbnail(cfg: &Config, filename: &str, mime: &str, itemid: i64, link: &str, pending: bool, size: u32) -> ThumbResult {
    let (b_dir, t_dir, c_dir) = dirs(cfg, pending).await;
    let thumb_size = if size > 128 { size } else { 128 };
    let spec = format!("{thumb_size}x{thumb_size}");
    let out_path = t_dir.join(format!("{itemid}.webp"));
    if is_symlink(&out_path).await {
        let _ = tokio::fs::remove_file(&out_path).await;
    }
    let mut cover = false;
    let work = match WorkDir::new(cfg, "thumb").await {
        Ok(w) => Some(w),
        Err(e) => {
            tracing::error!("[QUEUE] genThumbnail: cannot create work dir: {e}");
            None
        }
    };
    let res = match &work {
        Some(w) => thumbnail_inner(cfg, w, filename, mime, itemid, link, &b_dir, &c_dir, &out_path, thumb_size, &spec, &mut cover).await,
        None => Err(anyhow::anyhow!("no work dir")),
    };
    match res {
        Ok(()) => ThumbResult { ok: true, cover_extracted: cover },
        Err(e) => {
            tracing::error!("[QUEUE] genThumbnail failed for item {itemid} ({mime}): {e:#}");
            if mime.starts_with("audio/") && gen_audio_placeholder(cfg, &out_path, thumb_size).await {
                tracing::warn!("[QUEUE] Used audio placeholder thumbnail for item {itemid}");
                return ThumbResult::default();
            }
            let fallback = s(&cfg.paths.s.join("img").join("404.gif"));
            let o = s(&out_path);
            let crop = format!("{spec}+0+0");
            let fill = format!("{spec}^");
            match magick(&[&fallback, "-resize", &fill, "-gravity", "center", "-crop", &crop, "+repage", &o]).await {
                Ok(_) => tracing::warn!("[QUEUE] Used 404.gif fallback thumbnail for item {itemid}"),
                Err(e) => tracing::error!("[QUEUE] Even fallback thumbnail failed for item {itemid}: {e}"),
            }
            ThumbResult::default()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn thumbnail_inner(
    cfg: &Config,
    work: &WorkDir,
    filename: &str,
    mime: &str,
    itemid: i64,
    link: &str,
    b_dir: &Path,
    c_dir: &Path,
    out_path: &Path,
    thumb_size: u32,
    spec: &str,
    cover: &mut bool,
) -> Result<()> {
    let tmp_file = work.join("thumb.png");
    let tmp_jpg = work.join("cover.jpg");
    let (tf, tj) = (s(&tmp_file), s(&tmp_jpg));
    let ca_out = c_dir.join(format!("{itemid}.webp"));
    let ca = s(&ca_out);

    let source = if mime == "video/youtube" {
        PathBuf::new()
    } else {
        if !is_plain_name(filename) {
            bail!("refusing unsafe file name {filename:?}");
        }
        let p = b_dir.join(filename);
        if is_symlink(&p).await {
            let real = tokio::fs::canonicalize(&p).await.unwrap_or(p.clone());
            tracing::info!("[QUEUE] Resolved symlink for thumbnailing: {filename} -> {}", real.display());
            real
        } else {
            p
        }
    };
    let src = s(&source);
    let src0 = format!("{src}[0]");

    if mime == "video/youtube" {
        let video_id = filename.strip_prefix("yt:").filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
        if let Some(video_id) = video_id {
            let proxy = proxy_url(cfg);
            let mut fetched = false;
            for quality in ["maxresdefault", "sddefault", "hqdefault"] {
                let url = format!("https://img.youtube.com/vi/{video_id}/{quality}.jpg");
                let mut args = vec!["-s", "-L", url.as_str(), "-o", tf.as_str()];
                if let Some(p) = &proxy {
                    args.extend(["--proxy", p.as_str()]);
                }
                if proc::spawn("curl", &args).await.is_ok() && file_size(&tmp_file).await > 5000 {
                    fetched = true;
                    break;
                }
            }
            if !fetched {
                tracing::error!("[QUEUE] YouTube thumbnail extraction failed for {itemid}: all quality levels failed or returned placeholder");
            }
            if fetched {
                // pre-warm the ambient timeline in the background
                let (cfg2, vid) = (cfg.clone(), video_id.to_string());
                tokio::spawn(async move {
                    let _ = crate::routes::apiv2::ambient::get_or_extract_youtube_ambient(&cfg2, &vid).await;
                });
            }
        }
    } else if mime.starts_with("video/") || mime == "image/gif" {
        let ff_size = thumb_size.max(512).to_string();
        for seek in ["20%", "40%", "60%", "80%"] {
            if let Err(e) = proc::spawn_with("ffmpegthumbnailer", &["-i", &src, "-s", &ff_size, "-t", seek, "-o", &tf], quiet()).await {
                tracing::warn!("[QUEUE] ffmpegthumbnailer failed at {seek} for {itemid}, trying ffmpeg fallback: {e}");
                let pct: f64 = seek.trim_end_matches('%').parse::<f64>().unwrap_or(0.0) / 100.0;
                let seek_secs = match probe_duration(&source).await {
                    Some(d) => d * pct,
                    None => match seek {
                        "20%" => 2.0,
                        "40%" => 5.0,
                        "60%" => 8.0,
                        _ => 10.0,
                    },
                };
                let vf = format!("scale={ff_size}:{ff_size}:force_original_aspect_ratio=increase,crop={ff_size}:{ff_size}");
                let ss = crate::util::js_num(seek_secs);
                proc::spawn_with("ffmpeg", &["-y", "-ss", &ss, "-color_trc", "1", "-i", &src, "-frames:v", "1", "-update", "1", "-vf", &vf, &tf], quiet()).await?;
            }
            match magick(&[&tf, "-colorspace", "Gray", "-format", "%[fx:mean]", "info:"]).await {
                Ok(o) => {
                    if o.stdout_str().trim().parse::<f64>().map(|m| m > 0.05).unwrap_or(false) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    } else if mime.starts_with("image/") {
        if mime == "image/avif" {
            if proc::spawn_with("ffmpeg", &["-i", &src, "-frames:v", "1", "-update", "1", &tf], quiet()).await.is_err() {
                magick(&[&src0, "-auto-orient", &tf]).await?;
            }
        } else {
            magick(&[&src0, "-auto-orient", &tf]).await?;
        }
    } else if mime.starts_with("audio/") {
        if !link.is_empty() && link.contains("soundcloud") {
            let proxy = proxy_url(cfg);
            let mut args: Vec<&str> = Vec::new();
            if let Some(p) = &proxy {
                args.extend(["--proxy", p.as_str()]);
            }
            args.extend(["--get-thumbnail", "--", link]);
            let cover_url = match proc::spawn_with("yt-dlp", &args, quiet()).await {
                Ok(o) => o.stdout_str().lines().map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_string),
                Err(e) => {
                    tracing::warn!("[QUEUE] yt-dlp thumbnail fetch failed for SoundCloud track ({link}): {e}");
                    None
                }
            };
            if let Some(url) = cover_url.filter(|u| !u.contains("default_avatar") && (u.starts_with("https://") || u.starts_with("http://"))) {
                static SIZE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"-(large|original)\.").unwrap());
                let url = SIZE.replace(&url, "-t500x500.").into_owned();
                let mut cargs = vec!["-s", "-L", url.as_str(), "-o", tj.as_str()];
                if let Some(p) = &proxy {
                    cargs.extend(["--proxy", p.as_str()]);
                }
                if proc::spawn("curl", &cargs).await.is_ok() && file_size(&tmp_jpg).await > 0 && magick(&[&tj, &tf]).await.is_ok() && magick(&[&tj, &ca]).await.is_ok() {
                    *cover = true;
                }
            }
            if !*cover {
                let ok = proc::spawn_with("ffmpeg", &["-i", &src, "-an", "-vcodec", "copy", "-frames:v", "1", "-update", "1", &tj], quiet()).await.is_ok();
                if ok && file_size(&tmp_jpg).await > 0 && magick(&[&tj, &tf]).await.is_ok() && magick(&[&tj, &ca]).await.is_ok() {
                    *cover = true;
                }
            }
        } else {
            // Embedded cover art (video stream in the audio file)
            if proc::spawn_with("ffmpeg", &["-y", "-i", &src, "-an", "-vcodec", "webp", "-frames:v", "1", &ca], quiet()).await.is_ok()
                && file_size(&ca_out).await > 0
                && magick(&[&format!("{ca}[0]"), &tf]).await.is_ok()
            {
                *cover = true;
            }
            if !*cover {
                let ok = proc::spawn_with("ffmpeg", &["-y", "-i", &src, "-an", "-vcodec", "copy", "-frames:v", "1", "-update", "1", &tj], quiet()).await.is_ok();
                if ok && file_size(&tmp_jpg).await > 0 && magick(&[&tj, &tf]).await.is_ok() && magick(&[&tj, &ca]).await.is_ok() {
                    *cover = true;
                }
            }
        }
        // Cover art saved earlier (e.g. a beatmap background)
        if !*cover && file_size(&ca_out).await > 0 && magick(&[&ca, &tf]).await.is_ok() {
            *cover = true;
        }
        if !*cover {
            gen_audio_placeholder(cfg, &tmp_file, thumb_size).await;
        }
    } else if mime::is_flash(mime) {
        let mut used_custom = false;
        if let Some(custom) = cfg.str("websrv.swf_thumb").filter(|c| !c.is_empty()) {
            let p = public_asset(cfg, custom);
            if file_size(&p).await > 0 && magick(&[&s(&p), &tf]).await.is_ok() {
                used_custom = true;
            }
        }
        if !used_custom {
            let _ = magick(&["-size", spec, "xc:#1a1a2e", "-gravity", "center", "-fill", "#e040fb", "-pointsize", "48", "-annotate", "0", "SWF", &tf]).await;
        }
    } else if mime == "application/pdf" {
        let out_arg = format!("-sOutputFile={tf}");
        let gs = proc::spawn_with(
            "gs",
            &["-q", "-dSAFER", "-dNOPAUSE", "-dBATCH", "-sDEVICE=png16m", "-r150", "-dTextAlphaBits=4", "-dGraphicsAlphaBits=4", "-dLastPage=1", &out_arg, &src],
            quiet(),
        )
        .await;
        if gs.is_err() {
            tracing::warn!("[QUEUE] PDF extraction failed for {itemid}, using fallback icon.");
            let fallback = cfg.paths.s.join("img").join("pdf.webp");
            if tokio::fs::copy(&fallback, &tmp_file).await.is_err() {
                let _ = magick(&["-size", spec, "xc:#d32f2f", "-gravity", "center", "-fill", "white", "-pointsize", "60", "-annotate", "0", "PDF", &tf]).await;
            }
        }
    } else if mime::is_archive_mime(cfg, mime) {
        let mut used_custom = false;
        if let Some(custom) = cfg.str("websrv.archive_thumb").filter(|c| !c.is_empty()) {
            let p = public_asset(cfg, custom);
            if file_size(&p).await > 0 && magick(&[&s(&p), &tf]).await.is_ok() {
                used_custom = true;
            }
        }
        if !used_custom {
            let label = mime::ext_for(cfg, mime).unwrap_or_else(|| "arc".into()).to_uppercase();
            let _ = magick(&["-size", spec, "xc:#1a2e1a", "-gravity", "center", "-fill", "#66bb6a", "-pointsize", "48", "-annotate", "0", &label, &tf]).await;
        }
    }

    let out = s(out_path);
    let fill = format!("{spec}^");
    let crop = format!("{spec}+0+0");
    if matches!(mime, "image/png" | "image/webp" | "image/avif" | "image/gif") {
        // Grey/white checkerboard behind transparent images
        let (r1, r2, tile, bg, rs) = (s(&work.join("r1.png")), s(&work.join("r2.png")), s(&work.join("tile.png")), s(&work.join("bg.png")), s(&work.join("rs.png")));
        magick(&["-size", "16x16", "xc:white", "-size", "16x16", "xc:#cccccc", "+append", &r1]).await?;
        magick(&[&r1, "-flop", &r2]).await?;
        magick(&[&r1, &r2, "-append", &tile]).await?;
        magick(&["-size", spec, &format!("tile:{tile}"), "-define", "png:color-type=2", &bg]).await?;
        magick(&[&tf, "-auto-orient", "-colorspace", "sRGB", "-resize", &fill, "-gravity", "center", "-crop", &crop, "+repage", &rs]).await?;
        magick(&[&bg, &rs, "-composite", &out]).await?;
    } else {
        magick(&[&tf, "-auto-orient", "-resize", &fill, "-gravity", "center", "-crop", &crop, "+repage", &out]).await?;
    }
    Ok(())
}

/// Music-note placeholder (matches `.sidebar-media-placeholder.audio`).
pub async fn gen_audio_placeholder(cfg: &Config, target: &Path, thumb_size: u32) -> bool {
    let spec = format!("{thumb_size}x{thumb_size}");
    let font = s(&cfg.paths.s.join("fa").join("webfonts").join("fa-solid-900.ttf"));
    let pt = ((thumb_size as f64) * 0.35).round().to_string();
    let t = s(target);
    let r = magick(&[
        "-size", &spec, "radial-gradient:#2c2c2c-#181818",
        "(", "+clone", "-font", &font, "-pointsize", &pt, "-gravity", "center", "-fill", "rgba(255,255,255,0.3)", "-annotate", "0", "\u{f001}", "-blur", "0x12", ")",
        "-composite",
        "-font", &font, "-pointsize", &pt, "-gravity", "center", "-fill", "rgba(240,240,240,0.88)", "-annotate", "0", "\u{f001}",
        &t,
    ])
    .await;
    if r.is_ok() {
        return true;
    }
    if tokio::fs::copy(cfg.paths.s.join("img").join("audio.webp"), target).await.is_ok() {
        return true;
    }
    let _ = magick(&["-size", &spec, "radial-gradient:#2c2c2c-#181818", "-gravity", "center", "-fill", "rgba(240,240,240,0.88)", "-pointsize", "60", "-annotate", "0", "♪", &t]).await;
    false
}

/// `t/<itemid>_blur.webp` from `t/<itemid>.webp`.
pub async fn gen_blurred_thumbnail(cfg: &Config, itemid: i64, pending: bool) -> bool {
    let (_, t_dir, _) = dirs(cfg, pending).await;
    let src = t_dir.join(format!("{itemid}.webp"));
    let dst = t_dir.join(format!("{itemid}_blur.webp"));
    if is_symlink(&dst).await {
        let _ = tokio::fs::remove_file(&dst).await;
    }
    match magick(&[&s(&src), "-blur", "0x48", &s(&dst)]).await {
        Ok(_) => true,
        Err(e) => {
            tracing::error!("[QUEUE] Failed to generate blurred thumbnail for {itemid}: {e}");
            false
        }
    }
}

/// Format tags via ffprobe (`getVideoMetadata`).
pub async fn get_video_metadata(source: &Path) -> Option<Map<String, Value>> {
    let p = s(source);
    let out = proc::spawn_with("ffprobe", &["-v", "quiet", "-show_entries", "format_tags", "-of", "json", &p], quiet_any_exit()).await.ok()?;
    let text = out.stdout_str();
    if text.trim().is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("format")?.get("tags")?.as_object().cloned()
}

/// Pixel size: `magick identify` for images, ffprobe for videos.
pub async fn dimensions(path: &Path, mime: &str) -> Option<(i32, i32)> {
    let p = s(path);
    if mime.starts_with("image/") {
        let out = proc::spawn_with("magick", &["identify", "-format", "%wx%h\n", &format!("{p}[0]")], quiet_any_exit()).await.ok()?;
        return parse_wxh(out.stdout_str().trim().lines().next().unwrap_or(""), 'x');
    }
    if mime.starts_with("video/") && mime != "video/youtube" {
        let out = proc::spawn_with("ffprobe", &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "csv=p=0", &p], quiet_any_exit()).await.ok()?;
        return parse_wxh(out.stdout_str().trim().lines().next().unwrap_or(""), ',');
    }
    None
}

pub fn parse_wxh(line: &str, sep: char) -> Option<(i32, i32)> {
    let mut it = line.trim().split(sep);
    let w: i32 = it.next()?.trim().parse().ok()?;
    let h: i32 = it.next()?.trim().parse().ok()?;
    (w > 0 && h > 0).then_some((w, h))
}

// ── Tags & notifications ─────────────────────────────────────────────────────

pub async fn tag_sfw(st: &AppState, itemid: i64) -> Result<u64> {
    sql!(r#"insert into "tags_assign" (item_id, tag_id, user_id) values ("#, itemid as i32, ", 1, 1)").execute(&st.db).await
}

pub async fn tag_nsfw(st: &AppState, itemid: i64) -> Result<u64> {
    sql!(r#"insert into "tags_assign" (item_id, tag_id, user_id) values ("#, itemid as i32, ", 2, 1)").execute(&st.db).await
}

/// `admin_pending` notification for every admin and moderator.
pub async fn notify_admins(st: &AppState, itemid: i64) {
    let q = sql!(
        r#"INSERT INTO notifications (user_id, type, reference_id, item_id) SELECT id, 'admin_pending', 0, "#,
        itemid as i32,
        r#" FROM "user" WHERE admin = true OR is_moderator = true ON CONFLICT DO NOTHING"#
    );
    if let Err(e) = q.execute(&st.db).await {
        tracing::error!("[QUEUE] Failed to notify admins/mods: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(1056);
        for y in 0..32 {
            for x in 0..33 {
                v.push(f(x, y));
            }
        }
        v
    }

    #[test]
    fn flat_frames() {
        assert!(is_flat_frame(&[]));
        assert!(is_flat_frame(&frame(|_, _| 0)));
        assert!(is_flat_frame(&frame(|_, _| 128)));
        assert!(!is_flat_frame(&frame(|x, y| ((x * 7 + y * 13) % 200 + 20) as u8)));
    }

    #[test]
    fn dhash_bits() {
        // strictly decreasing rows → every bit 1
        let h = dhash(&frame(|x, _| (200 - x * 5) as u8));
        assert_eq!(h.len(), 256);
        assert!(h.chars().all(|c| c == 'f'));
        let h = dhash(&frame(|x, _| (20 + x * 5) as u8));
        assert!(h.chars().all(|c| c == '0'));
    }

    #[test]
    fn phash_split() {
        assert!(phash_parts("").is_none());
        assert!(phash_parts("00000000abc").is_none());
        let p = phash_parts("aa_00000000ff_bb").unwrap();
        assert_eq!(p, ["aa".to_string(), "bb".to_string(), String::new()]);
    }

    #[test]
    fn match_sql_binds_each_part_twice() {
        let h = ["a".to_string(), "b".to_string(), "c".to_string()];
        let q = match_count(&h).sql();
        assert_eq!(q.matches("bit_count").count(), 3);
        assert!(q.contains("$6"));
    }

    #[test]
    fn wxh() {
        assert_eq!(parse_wxh("640x480", 'x'), Some((640, 480)));
        assert_eq!(parse_wxh("1920,1080", ','), Some((1920, 1080)));
        assert_eq!(parse_wxh("0x10", 'x'), None);
        assert_eq!(parse_wxh("garbage", 'x'), None);
    }

    #[test]
    fn uuid_shape() {
        let u = genuuid();
        assert_eq!(u.len(), 48);
        assert!(u.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
