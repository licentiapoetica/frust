//! PWA web app manifest (/manifest.json), built from the `manifest:` section of config.yaml.
//!
//! Every key under `manifest:` is copied into the manifest as-is (standard Web App Manifest keys,
//! https://developer.mozilla.org/docs/Web/Manifest), overriding the defaults below.
//!
//! Installable PWAs need 192x192 and 512x512 PNG icons, so they are made from the favicon with
//! ImageMagick (`public/s/img/manifest/icon-<size>.png`, served by the /s/ route) and remade
//! whenever the favicon changes. Not done when `manifest.icons` is set in config.

use crate::config::Config;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_THEME_COLOR: &str = "#0096ff";

fn icon_mime(src: &str) -> &'static str {
    let s = src.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    match s.rsplit('.').next().unwrap_or("") {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    }
}

/// `favicon`: the site's favicon (admin panel upload or `websrv.custom_favicon`), empty for the
/// built-in one.
/// `icons`: the generated PNG icons (`ensure_icons`), listed before the favicon.
pub fn manifest(cfg: &Config, favicon: &str, icons: Option<&IconSet>) -> Value {
    let domain = cfg.domain();
    let favicon = if favicon.is_empty() { BUILTIN_FAVICON } else { favicon };
    let mut icon_list: Vec<Value> = icons
        .map(|set| {
            ICON_SIZES
                .iter()
                .map(|s| json!({ "src": format!("{ICON_URL}/icon-{s}.png?v={}", set.version), "sizes": format!("{s}x{s}"), "type": "image/png", "purpose": "any" }))
                .collect()
        })
        .unwrap_or_default();
    icon_list.push(json!({ "src": favicon, "sizes": "any", "type": icon_mime(favicon), "purpose": "any" }));
    let mut m = json!({
        "id": "/",
        "name": domain,
        "short_name": domain,
        "description": cfg.str_or("websrv.description", ""),
        "start_url": "/",
        "scope": "/",
        "display": "standalone",
        "background_color": "#000000",
        "theme_color": DEFAULT_THEME_COLOR,
        "icons": icon_list,
        "categories": ["entertainment", "social", "memes"],
    });
    if let (Some(out), Value::Object(over)) = (m.as_object_mut(), cfg.get("manifest")) {
        for (k, v) in over {
            if v.is_null() {
                out.remove(k);
            } else {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    m
}

/// `<meta name="theme-color">`: follows `manifest.theme_color`.
pub fn theme_color(cfg: &Config) -> &str {
    cfg.str_or("manifest.theme_color", DEFAULT_THEME_COLOR)
}

const BUILTIN_FAVICON: &str = "/s/img/favicon.gif";
pub const ICON_SIZES: [u32; 2] = [192, 512];
/// Public URL of the generated icons; on disk `paths.s/img/manifest`.
const ICON_URL: &str = "/s/img/manifest";

/// The generated icons are up to date for the current favicon.
pub struct IconSet {
    /// Changes with the favicon, for `?v=` cache busting.
    pub version: String,
}

fn icon_dir(cfg: &Config) -> PathBuf {
    cfg.paths.s.join("img").join("manifest")
}

/// The favicon as a local file under `public/s/` (external URLs can't be converted).
fn favicon_file(cfg: &Config, favicon: &str) -> Option<PathBuf> {
    let url = if favicon.is_empty() { BUILTIN_FAVICON } else { favicon };
    let rel = url.split(['?', '#']).next()?.strip_prefix("/s/")?;
    crate::http::files::safe_join(&cfg.paths.s, rel).filter(|p| p.is_file())
}

/// What the icons were made from: favicon URL + file size + mtime.
fn source_stamp(favicon: &str, src: &Path) -> Option<String> {
    let meta = std::fs::metadata(src).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(format!("{favicon}\n{}\n{mtime}\n", meta.len()))
}

fn short_hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(s.as_bytes());
    h.iter().take(5).map(|b| format!("{b:02x}")).collect()
}

/// Make sure `icon-192.png` / `icon-512.png` exist for the current favicon, (re)making them
/// when it changed. `None` when `manifest.icons` is configured, the favicon isn't a local file,
/// or ImageMagick failed (the manifest then only lists the favicon).
pub async fn ensure_icons(cfg: &Config, favicon: &str) -> Option<IconSet> {
    if !cfg.get("manifest.icons").is_null() {
        return None;
    }
    let src = favicon_file(cfg, favicon)?;
    let stamp = source_stamp(favicon, &src)?;
    let dir = icon_dir(cfg);
    let stamp_file = dir.join("source");
    let up_to_date = |dir: &Path| {
        std::fs::read_to_string(dir.join("source")).is_ok_and(|s| s == stamp)
            && ICON_SIZES.iter().all(|s| dir.join(format!("icon-{s}.png")).is_file())
    };
    let set = IconSet { version: short_hash(&stamp) };
    if up_to_date(&dir) {
        return Some(set);
    }
    // one conversion at a time; whoever waited finds the icons done
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _guard = LOCK.lock().await;
    if up_to_date(&dir) {
        return Some(set);
    }
    // a failed attempt isn't retried on every request
    static FAILED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
    if FAILED.lock().ok().is_some_and(|f| f.as_deref() == Some(stamp.as_str())) {
        return None;
    }
    match make_icons(&src, &dir).await {
        Ok(()) => {
            if let Err(e) = tokio::fs::write(&stamp_file, &stamp).await {
                tracing::warn!("[MANIFEST] can't write {}: {e}", stamp_file.display());
            }
            tracing::info!("[MANIFEST] icons made from {favicon}", favicon = if favicon.is_empty() { BUILTIN_FAVICON } else { favicon });
            Some(set)
        }
        Err(e) => {
            tracing::warn!("[MANIFEST] can't make icons from {}: {e}", src.display());
            if let Ok(mut f) = FAILED.lock() {
                *f = Some(stamp);
            }
            None
        }
    }
}

/// `magick [sub] <limits> args…` (a subcommand like `identify` has to come before the limits).
async fn magick(sub: Option<&str>, args: &[&str]) -> anyhow::Result<crate::proc::Output> {
    let mut all: Vec<&str> = sub.into_iter().collect();
    all.extend(["-limit", "memory", "512MiB", "-limit", "map", "1GiB", "-limit", "width", "32KP", "-limit", "height", "32KP"]);
    all.extend_from_slice(args);
    let opts = crate::proc::Opts { timeout: Duration::from_secs(60), ..Default::default() };
    crate::proc::spawn_with("magick", &all, opts).await
}

/// The largest frame (an .ico holds several sizes, a GIF is animated): `(index, width, height)`.
async fn largest_frame(input: &str) -> anyhow::Result<(usize, u32, u32)> {
    let out = magick(Some("identify"), &["-format", "%w %h\n", input]).await?.stdout_str();
    out.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let mut it = l.split_whitespace().map(|n| n.parse::<u32>().ok());
            Some((i, it.next()??, it.next()??))
        })
        .max_by_key(|&(i, w, h)| (w.max(h), std::cmp::Reverse(i)))
        .ok_or_else(|| anyhow::anyhow!("no image frames"))
}

async fn make_icons(src: &Path, dir: &Path) -> anyhow::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    let path = src.to_string_lossy();
    let is_svg = path.to_ascii_lowercase().ends_with(".svg");
    let (frame, w, h) = largest_frame(&path).await?;
    let input = format!("{path}[{frame}]");
    for size in ICON_SIZES {
        let dims = format!("{size}x{size}");
        let tmp = dir.join(format!("icon-{size}.png.tmp"));
        let out = format!("png32:{}", tmp.to_string_lossy());
        // SVGs are rasterised right at the target size (identify reads them at 96 dpi)
        let density = (96 * size).div_ceil(w.max(h).max(1)).clamp(10, 9600).to_string();
        let mut args: Vec<&str> = vec!["-background", "none"];
        if is_svg {
            args.extend(["-density", &density]);
        }
        args.push(&input);
        // small pixel-art favicons stay crisp instead of blurring
        if !is_svg && w.max(h) * 2 < size {
            args.extend(["-filter", "Point"]);
        }
        args.extend(["-resize", &dims, "-gravity", "center", "-extent", &dims, "-strip", &out]);
        if let Err(e) = magick(None, &args).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
        let dest = dir.join(format!("icon-{size}.png"));
        tokio::fs::rename(&tmp, &dest).await?;
        crate::media::make_public(&dest).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(raw: Value) -> Config {
        let base = std::path::PathBuf::from(".");
        Config { raw, paths: crate::config::resolve_paths(&base), base }
    }

    #[test]
    fn defaults_from_config() {
        let m = manifest(&cfg(json!({ "main": { "url": { "domain": "f0ck.me" } }, "websrv": { "description": "d" } })), "/s/img/x.webp", None);
        assert_eq!(m["name"], "f0ck.me");
        assert_eq!(m["description"], "d");
        assert_eq!(m["icons"][0]["type"], "image/webp");
    }

    #[test]
    fn generated_icons_listed_first() {
        let set = IconSet { version: "abc".into() };
        let m = manifest(&cfg(json!({})), "", Some(&set));
        assert_eq!(m["icons"][0]["src"], "/s/img/manifest/icon-192.png?v=abc");
        assert_eq!(m["icons"][1]["sizes"], "512x512");
        assert_eq!(m["icons"][2]["src"], "/s/img/favicon.gif");
    }

    #[test]
    fn overrides_and_removal() {
        let c = cfg(json!({ "manifest": { "name": "F", "theme_color": "#f00", "categories": null, "icons": [] } }));
        let m = manifest(&c, "", Some(&IconSet { version: "x".into() }));
        assert_eq!(m["name"], "F");
        assert_eq!(m["icons"], json!([]));
        assert!(m.get("categories").is_none());
        assert_eq!(theme_color(&c), "#f00");
    }

    #[tokio::test]
    async fn icons_from_favicon() {
        if std::process::Command::new("magick").arg("-version").output().is_err() {
            return; // no ImageMagick here
        }
        let tmp = std::env::temp_dir().join(format!("f0ckm-pwa-{}", std::process::id()));
        let s_dir = tmp.join("s");
        std::fs::create_dir_all(s_dir.join("img")).unwrap();
        let repo = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/s/img");
        std::fs::copy(repo.join("favicon.gif"), s_dir.join("img/favicon.gif")).unwrap();
        std::fs::copy(repo.join("ghost_fav.svg"), s_dir.join("img/ghost.svg")).unwrap();
        let mut c = cfg(json!({}));
        c.paths.s = s_dir.clone();

        let a = ensure_icons(&c, "").await.expect("icons from the built-in gif");
        for size in ICON_SIZES {
            let out = std::process::Command::new("magick").args(["identify", "-format", "%w %h %m"]).arg(s_dir.join(format!("img/manifest/icon-{size}.png"))).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), format!("{size} {size} PNG"));
        }
        assert_eq!(ensure_icons(&c, "").await.unwrap().version, a.version);
        let b = ensure_icons(&c, "/s/img/ghost.svg?x=1").await.expect("icons from an svg");
        assert_ne!(a.version, b.version);
        assert!(ensure_icons(&c, "https://example.com/f.png").await.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
