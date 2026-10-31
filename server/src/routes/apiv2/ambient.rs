//! YouTube "ambient" colour timeline (port of src/inc/routes/apiv2/ambient.mjs): a few colours
//! sampled from the video's storyboard (or its thumbnail), cached in `t/ambient_<id>.json`.

use crate::config::Config;
use crate::http::{Req, Res, Router, json_raw, with_header};
use crate::proc::{Opts, spawn_with};
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

pub fn register(r: &mut Router) {
    r.get(r"^\/api\/v2\/ambient\/yt\/([a-zA-Z0-9_-]+)", ambient);
}

/// One lock per video id, so concurrent requests run yt-dlp only once.
static PENDING: Lazy<parking_lot::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = Lazy::new(Default::default);

fn valid_id(id: &str) -> bool {
    (6..=15).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn opts(secs: u64) -> Opts {
    Opts { timeout: Duration::from_secs(secs), quiet: true, ..Default::default() }
}

/// `getOrExtractYoutubeAmbient(videoId)` → `{ videoId, duration, colors: [[r, g, b], ...] }`
pub async fn get_or_extract_youtube_ambient(cfg: &Config, video_id: &str) -> anyhow::Result<Value> {
    if !valid_id(video_id) {
        anyhow::bail!("Invalid YouTube video ID");
    }
    let t_dir = tokio::fs::canonicalize(&cfg.paths.t).await.unwrap_or_else(|_| cfg.paths.t.clone());
    let cache_file = t_dir.join(format!("ambient_{video_id}.json"));
    let read_cache = || async {
        let text = tokio::fs::read_to_string(&cache_file).await.ok()?;
        serde_json::from_str::<Value>(&text).ok()
    };
    if let Some(v) = read_cache().await {
        return Ok(v);
    }
    let lock = PENDING.lock().entry(video_id.to_string()).or_default().clone();
    let _guard = lock.lock().await;
    let r = async {
        if let Some(v) = read_cache().await {
            return Ok(v);
        }
        let v = extract(cfg, video_id).await?;
        if let Err(e) = tokio::fs::write(&cache_file, serde_json::to_string(&v)?).await {
            tracing::warn!("[AMBIENT] Failed to write cache for {video_id}: {e}");
        }
        Ok(v)
    }
    .await;
    drop(_guard);
    PENDING.lock().remove(video_id);
    r
}

/// Download `url` and scale it to `w`x`h` RGB pixels.
async fn sample(cfg: &Config, url: &str, size: &str, proxy: Option<&str>) -> anyhow::Result<Vec<Value>> {
    let work = crate::media::WorkDir::new(cfg, "ambient").await?;
    let img = work.join("sb.jpg").to_string_lossy().into_owned();
    let rgb = work.join("sb.rgb");
    let mut args: Vec<&str> = vec!["-s", "-L", "--proto", "=http,https", "--proto-redir", "=http,https", "--max-filesize", "20971520", "-o", &img];
    if let Some(p) = proxy {
        args.extend(["--proxy", p]);
    }
    args.extend(["--", url]);
    spawn_with("curl", &args, opts(10)).await?;
    let out = format!("rgb:{}", rgb.to_string_lossy());
    spawn_with("magick", &[&img, "-scale", size, &out], opts(5)).await?;
    let buf = tokio::fs::read(&rgb).await?;
    Ok(buf.chunks(3).map(|c| json!([c[0], c.get(1).copied(), c.get(2).copied()])).collect())
}

async fn extract(cfg: &Config, video_id: &str) -> anyhow::Result<Value> {
    let proxy = crate::media::proxy_url(cfg);
    let yt_url = format!("https://www.youtube.com/watch?v={video_id}");
    let mut args: Vec<&str> = vec!["-j", "--skip-download", "--no-playlist"];
    if let Some(p) = &proxy {
        args.extend(["--proxy", p.as_str()]);
    }
    args.extend(["--", &yt_url]);
    let out = spawn_with("yt-dlp", &args, opts(15)).await?;
    let info: Value = serde_json::from_slice(&out.stdout)?;
    let duration = info.get("duration").and_then(|d| match d {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => crate::social::helpers::js_number(s),
        _ => None,
    });
    let duration = crate::f0cklib::recommend::jnum(duration.filter(|d| d.is_finite() && *d != 0.0).unwrap_or(0.0));

    let empty = vec![];
    let sb: Vec<&Value> = info
        .get("formats")
        .and_then(Value::as_array)
        .unwrap_or(&empty)
        .iter()
        .filter(|f| f.get("format_id").and_then(Value::as_str).is_some_and(|id| id.starts_with("sb")))
        .collect();
    let by_id = |id: &str| sb.iter().find(|f| f["format_id"] == id).copied();
    let chosen = by_id("sb3").or_else(|| by_id("sb2")).or_else(|| sb.last().copied());
    let mut colors: Vec<Value> = vec![];
    if let Some(f) = chosen {
        let frag = f.get("fragments").and_then(|a| a.get(0)).and_then(|x| x.get("url")).and_then(Value::as_str).filter(|u| !u.is_empty());
        let img_url = frag.map(str::to_string).or_else(|| f.get("url").and_then(Value::as_str).filter(|u| !u.is_empty()).map(|u| u.replacen("$M", "0", 1)));
        if let Some(u) = img_url {
            colors = sample(cfg, &u, "10x10!", proxy.as_deref()).await?;
        }
    }
    if colors.is_empty() {
        // no storyboard: a 3x3 palette of the thumbnail
        let thumb = format!("https://img.youtube.com/vi/{video_id}/hqdefault.jpg");
        colors = match sample(cfg, &thumb, "3x3!", None).await {
            Ok(c) if !c.is_empty() => c,
            _ => vec![json!([30, 30, 35])],
        };
    }
    Ok(json!({ "videoId": video_id, "duration": duration, "colors": colors }))
}

async fn ambient(st: crate::state::AppState, req: Req) -> Res {
    let video_id = req.path.split('/').nth(5).unwrap_or("").to_string();
    if video_id.is_empty() {
        return Ok(json_raw(&json!({ "error": "Missing video ID" }), 400));
    }
    match get_or_extract_youtube_ambient(&st.cfg(), &video_id).await {
        Ok(v) => Ok(with_header(json_raw(&v, 200), "cache-control", "public, max-age=86400, stale-while-revalidate=604800")),
        Err(e) => {
            tracing::error!("[AMBIENT ERROR] Failed for {video_id}: {e}");
            // the old code sent the raw error (including tool output); only the validation message is kept
            let msg = if e.to_string() == "Invalid YouTube video ID" { "Invalid YouTube video ID" } else { "Ambient extraction failed" };
            Ok(json_raw(&json!({ "error": msg }), 500))
        }
    }
}
