//! MIME decisions for uploads: content detection (libmagic via `file`, like the old code), the
//! allowed list from config, archive classification and quick magic-byte sniffing.

use crate::config::Config;
use std::path::Path;

/// Content-based MIME type (`file --mime-type -b <path>`). The path is always one of our own
/// temp files; `--` keeps it from ever being read as an option.
pub async fn detect(path: &Path) -> anyhow::Result<String> {
    let p = path.to_string_lossy();
    let out = crate::proc::spawn("file", &["--mime-type", "-b", "--", &p]).await?;
    Ok(out.stdout_str().trim().to_string())
}

/// MIME types uploads may have: `cfg.mimes` filtered by `cfg.allowedMimes` categories
/// (`image` matches `image/*`, `image/png` matches exactly). No `allowedMimes` → all of `cfg.mimes`.
pub fn allowed_mimes(cfg: &Config) -> Vec<String> {
    let mimes: Vec<String> = cfg.mimes().keys().cloned().collect();
    allowed_from(&mimes, cfg.get("allowedMimes").as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_lowercase)).collect()))
}

pub fn allowed_from(mimes: &[String], cats: Option<Vec<String>>) -> Vec<String> {
    match cats {
        None => mimes.to_vec(),
        Some(cats) => mimes
            .iter()
            .filter(|m| cats.iter().any(|c| if c.contains('/') { *m == c } else { m.starts_with(&format!("{c}/")) }))
            .cloned()
            .collect(),
    }
}

/// Any `application/*` in `cfg.mimes` that isn't swf or pdf counts as an archive.
pub fn is_archive_mime(cfg: &Config, mime: &str) -> bool {
    mime.starts_with("application/") && cfg.mimes().get(mime).and_then(|e| e.as_str()).map(|e| e != "swf" && e != "pdf").unwrap_or(false)
}

pub fn is_flash(mime: &str) -> bool {
    mime == "application/x-shockwave-flash" || mime == "application/vnd.adobe.flash.movie"
}

/// What the upload forms list as accepted files: the allowed MIME types as the extensions people
/// pick (not `cfg.mimes`' stored extension: a .mov is stored as mp4), grouped by kind in the order
/// video, image, audio, flash, pdf, archive. Flash and archives only with `websrv.enable_swf` /
/// `websrv.enable_archive` on, PDF only when `pdf` (the admin setting), like the upload form's checks.
pub fn upload_types(cfg: &Config, pdf: bool) -> Vec<(&'static str, Vec<String>)> {
    let swf = cfg.truthy("websrv.enable_swf");
    let archive = cfg.truthy("websrv.enable_archive");
    let mut groups: Vec<(&'static str, Vec<String>)> =
        ["video", "image", "audio", "flash", "pdf", "archive"].into_iter().map(|k| (k, Vec::new())).collect();
    for m in allowed_mimes(cfg) {
        let kind = if is_flash(&m) {
            if !swf {
                continue;
            }
            "flash"
        } else if m == "application/pdf" {
            if !pdf {
                continue;
            }
            "pdf"
        } else if is_archive_mime(cfg, &m) {
            if !archive {
                continue;
            }
            "archive"
        } else {
            match m.split('/').next() {
                Some("video") => "video",
                Some("image") => "image",
                Some("audio") => "audio",
                _ => continue,
            }
        };
        let picked: Vec<String> = match m.as_str() {
            "image/jpeg" | "image/jpg" => vec!["jpg".into(), "jpeg".into()],
            "video/quicktime" => vec!["mov".into()],
            "video/x-m4v" => vec!["m4v".into()],
            "video/mpeg" => vec!["mpg".into(), "mpeg".into()],
            "audio/mpeg" | "audio/mp3" => vec!["mp3".into()],
            "audio/aac" => vec!["aac".into()],
            _ => ext_for(cfg, &m).into_iter().collect(),
        };
        let group = &mut groups.iter_mut().find(|(k, _)| *k == kind).expect("known kind").1;
        for e in picked {
            if !group.contains(&e) {
                group.push(e);
            }
        }
    }
    groups.retain(|(_, exts)| !exts.is_empty());
    groups
}

/// `file --mime-type` results that get re-encoded to MP4 (transcode.mjs).
pub fn needs_transcode(mime: &str) -> bool {
    mime == "video/mpeg"
}

/// Extension for a MIME type from `cfg.mimes`.
pub fn ext_for(cfg: &Config, mime: &str) -> Option<String> {
    cfg.mimes().get(mime).and_then(|e| e.as_str()).map(str::to_string)
}

/// Image detection from the first bytes (meta_extract_handler): JPEG, PNG, WebP, GIF, TIFF.
pub fn sniff_image(magic: &[u8]) -> bool {
    let b = |i: usize| magic.get(i).copied();
    let jpeg = b(0) == Some(0xFF) && b(1) == Some(0xD8);
    let png = b(0) == Some(0x89) && b(1) == Some(0x50);
    let webp = b(8) == Some(0x57) && b(9) == Some(0x45) && b(10) == Some(0x42) && b(11) == Some(0x50);
    let gif = b(0) == Some(0x47) && b(1) == Some(0x49);
    let tiff = (b(0) == Some(0x49) && b(1) == Some(0x49)) || (b(0) == Some(0x4D) && b(1) == Some(0x4D));
    jpeg || png || webp || gif || tiff
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn upload_types_lists_picked_extensions_of_enabled_kinds() {
        let mimes = "mimes: { video/webm: webm, video/quicktime: mp4, video/mp4: mp4, image/jpeg: jpeg, image/png: png, \
                     audio/mpeg: mpg, application/x-shockwave-flash: swf, application/pdf: pdf, application/zip: zip }";
        let names = |t: Vec<(&'static str, Vec<String>)>| t.into_iter().map(|(k, e)| format!("{k}: {}", e.join(" "))).collect::<Vec<_>>();
        let off = crate::media::test_cfg(mimes);
        assert_eq!(names(upload_types(&off, false)), ["video: webm mov mp4", "image: jpg jpeg png", "audio: mp3"]);
        let on = crate::media::test_cfg(&format!("{mimes}\nwebsrv: {{ enable_swf: true, enable_archive: true }}"));
        assert_eq!(names(upload_types(&on, true)), ["video: webm mov mp4", "image: jpg jpeg png", "audio: mp3", "flash: swf", "pdf: pdf", "archive: zip"]);
    }

    #[test]
    fn allowed_categories() {
        let mimes = s(&["image/png", "image/gif", "video/mp4", "audio/mpeg", "application/pdf", "application/zip"]);
        assert_eq!(allowed_from(&mimes, None), mimes);
        assert_eq!(allowed_from(&mimes, Some(s(&["image", "application/pdf"]))), s(&["image/png", "image/gif", "application/pdf"]));
        // a category must match the type prefix exactly, not a substring
        assert!(allowed_from(&mimes, Some(s(&["imag"]))).is_empty());
    }

    #[test]
    fn sniff() {
        assert!(sniff_image(&[0xFF, 0xD8, 0xFF, 0xE0]));
        assert!(sniff_image(b"\x89PNG\r\n\x1a\n"));
        assert!(sniff_image(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(sniff_image(b"GIF89a"));
        assert!(!sniff_image(b"\x1aE\xdf\xa3")); // webm
        assert!(!sniff_image(b""));
    }

    #[test]
    fn flags() {
        assert!(is_flash("application/x-shockwave-flash"));
        assert!(needs_transcode("video/mpeg"));
        assert!(!needs_transcode("video/mp4"));
    }
}
