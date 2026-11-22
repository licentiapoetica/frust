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

/// Two-part extensions of compressed tar archives.
const TAR_EXTS: [&str; 3] = ["tar.gz", "tar.bz2", "tar.xz"];

/// The extension to store a file under: [`ext_for`], except that a compressed tar keeps its full
/// `.tar.gz` / `.tar.bz2` / `.tar.xz`. Decided from the content, so it works on every upload route.
pub async fn stored_ext(cfg: &Config, path: &Path, mime: &str) -> Option<String> {
    let ext = ext_for(cfg, mime)?;
    let wrapped = match mime {
        "application/gzip" => Some(("zcat", TAR_EXTS[0])),
        "application/x-bzip2" => Some(("bzcat", TAR_EXTS[1])),
        "application/x-xz" => Some(("xzcat", TAR_EXTS[2])),
        _ => None,
    };
    if let Some((tool, tar_ext)) = wrapped {
        if holds_tar(path, tool).await {
            return Some(tar_ext.to_string());
        }
    }
    Some(ext)
}

/// Does the compressed file start with a tar header? Only the first block is decompressed: the
/// tool is killed when `child` drops, so a decompression bomb costs nothing.
async fn holds_tar(path: &Path, tool: &str) -> bool {
    use tokio::io::AsyncReadExt;
    let Ok(file) = tokio::fs::File::open(path).await else { return false };
    let Ok(mut child) = tokio::process::Command::new(tool)
        .stdin(file.into_std().await)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
    else {
        return false;
    };
    let Some(mut out) = child.stdout.take() else { return false };
    let mut block = [0u8; 512];
    let read = tokio::time::timeout(std::time::Duration::from_secs(10), out.read_exact(&mut block)).await;
    matches!(read, Ok(Ok(_))) && block[257..262] == *b"ustar"
}

/// A stored file name without its extension, two-part tar extensions included.
pub fn file_stem(name: &str) -> &str {
    TAR_EXTS
        .iter()
        .find_map(|e| name.strip_suffix(e)?.strip_suffix('.'))
        .unwrap_or_else(|| name.rsplit_once('.').map_or(name, |(stem, _)| stem))
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
    fn mp3_is_stored_as_mp3() {
        for stored in ["mpg", "mp3"] {
            let cfg = crate::media::test_cfg(&format!("mimes: {{ audio/mpeg: {stored}, video/mpeg: mp4 }}"));
            assert_eq!(ext_for(&cfg, "audio/mpeg").as_deref(), Some("mp3"), "config says {stored}");
            assert_eq!(ext_for(&cfg, "video/mpeg").as_deref(), Some("mp4"));
        }
    }

    #[test]
    fn stems_drop_the_whole_extension() {
        assert_eq!(file_stem("abc.tar.gz"), "abc");
        assert_eq!(file_stem("abc.tar.bz2"), "abc");
        assert_eq!(file_stem("abc.tar.xz"), "abc");
        assert_eq!(file_stem("abc.gz"), "abc");
        assert_eq!(file_stem("abc.webp"), "abc");
        assert_eq!(file_stem("abc"), "abc");
    }

    #[tokio::test]
    async fn compressed_tars_keep_their_full_extension() {
        let dir = std::env::temp_dir().join(format!("f0ckm_tar_test_{}", crate::util::random_hex(8)));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        let script = "tar czf t.gz a.txt && tar cjf t.bz2 a.txt && tar cJf t.xz a.txt && \
                      gzip -c a.txt > p.gz && bzip2 -c a.txt > p.bz2 && xz -c a.txt > p.xz";
        let made = std::process::Command::new("sh").args(["-c", script]).current_dir(&dir).status().unwrap();
        assert!(made.success(), "needs tar, gzip, bzip2 and xz");
        let cfg = crate::media::test_cfg("mimes: { application/gzip: gz, application/x-bzip2: bz2, application/x-xz: xz, image/png: png }");
        for (ext, mime) in [("gz", "application/gzip"), ("bz2", "application/x-bzip2"), ("xz", "application/x-xz")] {
            assert_eq!(stored_ext(&cfg, &dir.join(format!("t.{ext}")), mime).await, Some(format!("tar.{ext}")));
            assert_eq!(stored_ext(&cfg, &dir.join(format!("p.{ext}")), mime).await.as_deref(), Some(ext), "not a tar");
        }
        assert_eq!(stored_ext(&cfg, &dir.join("a.txt"), "image/png").await.as_deref(), Some("png"));
        assert_eq!(stored_ext(&cfg, &dir.join("a.txt"), "text/plain").await, None);
        std::fs::remove_dir_all(&dir).unwrap();
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
