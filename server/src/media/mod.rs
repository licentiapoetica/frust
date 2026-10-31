//! Media upload & processing pipeline (old `src/inc/queue.mjs`, `multipart.mjs`, `transcode.mjs`,
//! `lib_delete.mjs`, `autotagger.mjs`).

pub mod autotagger;
pub mod delete;
pub mod mime;
pub mod multipart;
pub mod queue;
pub mod transcode;

use crate::config::Config;
use std::path::{Path, PathBuf};

/// `path.relative(from_dir, to)` for absolute paths.
pub fn relative_path(from_dir: &Path, to: &Path) -> PathBuf {
    let f: Vec<_> = from_dir.components().collect();
    let t: Vec<_> = to.components().collect();
    let common = f.iter().zip(t.iter()).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..f.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    out
}

/// Rename, falling back to copy + remove across file systems.
pub async fn move_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if tokio::fs::rename(src, dst).await.is_ok() {
        return Ok(());
    }
    tokio::fs::copy(src, dst).await?;
    let _ = tokio::fs::remove_file(src).await;
    Ok(())
}

/// Media files are served to everyone: 0644 (upload temp files are created 0600).
pub async fn make_public(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).await;
    }
}

/// Move a file that may be a relative symlink (repost) so the link still resolves at `dst`.
pub async fn move_safe(src: &Path, dst: &Path) {
    let r = async {
        let meta = tokio::fs::symlink_metadata(src).await?;
        if meta.file_type().is_symlink() {
            let target = tokio::fs::read_link(src).await?;
            let abs = delete::resolve_link(src, &target);
            let rel = relative_path(dst.parent().unwrap_or(Path::new("/")), &abs);
            tokio::fs::symlink(&rel, dst).await?;
            let _ = tokio::fs::remove_file(src).await;
            Ok(())
        } else {
            move_file(src, dst).await
        }
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[UPLOAD MOVE ERROR] Failed to move {} to {}: {e}", src.display(), dst.display());
    }
}

/// A private scratch directory under `paths.tmp`, removed on drop. Replaces the old fixed names in
/// the shared `/tmp` (`os.tmpdir()/<itemid>.png`), which other local users could pre-create.
pub struct WorkDir {
    pub path: PathBuf,
}

impl WorkDir {
    pub async fn new(cfg: &Config, prefix: &str) -> std::io::Result<WorkDir> {
        let path = cfg.paths.tmp.join(format!("{prefix}_{}", crate::util::random_hex(12)));
        tokio::fs::create_dir_all(&cfg.paths.tmp).await?;
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        b.create(&path)?;
        Ok(WorkDir { path })
    }

    pub fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A temp file path that is deleted on drop unless [`TempPath::keep`] was called.
pub struct TempPath {
    pub path: PathBuf,
    keep: bool,
}

impl TempPath {
    pub fn new(path: PathBuf) -> TempPath {
        TempPath { path, keep: false }
    }

    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        std::mem::take(&mut self.path)
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        if !self.keep && !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

impl AsRef<Path> for TempPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

/// `cfg.main.socks`, unless empty or the string "undefined".
pub fn socks(cfg: &Config) -> Option<String> {
    cfg.str("main.socks").map(str::trim).filter(|s| !s.is_empty() && *s != "undefined").map(str::to_string)
}

/// Configured upstream proxy (`main.socks`): as configured when it has a scheme, else `socks5h://host`.
pub fn configured_proxy(cfg: &Config) -> Option<String> {
    socks(cfg).map(|s| if s.contains("://") { s } else { format!("socks5h://{s}") })
}

/// Proxy for curl/yt-dlp `--proxy` when fetching user-supplied URLs: the configured SOCKS proxy
/// (e.g. Tor, which refuses internal addresses itself), otherwise the SSRF guard
/// (`crate::net_guard`). Always `Some` once the guard runs, so tools never fetch unguarded.
pub fn proxy_url(cfg: &Config) -> Option<String> {
    configured_proxy(cfg).or_else(crate::net_guard::proxy_url)
}

/// `calculateExpiresAt` (routes/apiv2/upload.mjs): expiry value → unix timestamp or `None`.
pub fn calculate_expires_at(cfg: &Config, expiry: Option<&str>, base: i64) -> Option<i64> {
    if !crate::settings::enable_expiring_uploads(cfg) {
        return None;
    }
    let raw = expiry?;
    if raw.is_empty() || raw == "permanent" || raw == "never" || raw == "0" {
        return None;
    }
    let v = raw.trim().to_lowercase();
    let add = match v.as_str() {
        "30minutes" | "30m" | "1800" => 1800,
        "1hour" | "1h" | "3600" => 3600,
        "24hours" | "24h" | "1day" | "86400" => 86400,
        "1week" | "1w" | "7days" | "604800" => 604800,
        "1month" | "1m" | "30days" | "2592000" => 2592000,
        _ => {
            let n = crate::config::as_int(&serde_json::Value::String(v.clone()))?;
            if n <= 0 {
                return None;
            }
            return Some(if n > 2_000_000_000 { n } else { base + n });
        }
    };
    Some(base + add)
}

/// The item-id-or-file name must be a plain file name inside a media directory.
pub fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\') && !name.contains('\0')
}

/// Config built from YAML for unit tests (storage paths point into the system temp dir).
#[cfg(test)]
pub fn test_cfg(yaml: &str) -> Config {
    let raw: serde_json::Value = serde_yaml::from_str(yaml).unwrap();
    let d = std::env::temp_dir().join("f0ckm_media_tests");
    let p = |s: &str| d.join(s);
    let paths = crate::config::Paths {
        a: p("a"), b: p("b"), c: p("c"), t: p("t"), ca: p("ca"), s: p("s"), emojis: p("emojis"), koepfe: p("koepfe"),
        fonts: p("fonts"), memes: p("memes"), e: p("e"), cu: p("cu"), pending: p("pending"), deleted: p("deleted"),
        logs: p("logs"), tmp: p("tmp"), import: p("import"),
    };
    Config { raw, paths, base: d }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(yaml: &str) -> Config {
        test_cfg(yaml)
    }

    #[test]
    fn expiry() {
        let c = cfg("main: {}");
        assert_eq!(calculate_expires_at(&c, None, 1000), None);
        assert_eq!(calculate_expires_at(&c, Some("permanent"), 1000), None);
        assert_eq!(calculate_expires_at(&c, Some("1h"), 1000), Some(4600));
        assert_eq!(calculate_expires_at(&c, Some(" 1Week "), 0), Some(604800));
        assert_eq!(calculate_expires_at(&c, Some("120"), 1000), Some(1120));
        assert_eq!(calculate_expires_at(&c, Some("2100000000"), 1000), Some(2100000000));
        assert_eq!(calculate_expires_at(&c, Some("-5"), 1000), None);
        assert_eq!(calculate_expires_at(&c, Some("soon"), 1000), None);
        let off = cfg("enable_expiring_uploads: false");
        assert_eq!(calculate_expires_at(&off, Some("1h"), 1000), None);
    }

    #[test]
    fn relative() {
        assert_eq!(relative_path(Path::new("/d/pending/b"), Path::new("/d/b/x.webm")), PathBuf::from("../../b/x.webm"));
        assert_eq!(relative_path(Path::new("/d/b"), Path::new("/d/b/x.webm")), PathBuf::from("x.webm"));
    }

    #[test]
    fn plain_names() {
        assert!(is_plain_name("abc.webm"));
        assert!(!is_plain_name("../x"));
        assert!(!is_plain_name("a/b"));
        assert!(!is_plain_name(".."));
    }

    #[test]
    fn proxies() {
        assert_eq!(configured_proxy(&cfg("main: { socks: '127.0.0.1:9050' }")).as_deref(), Some("socks5h://127.0.0.1:9050"));
        assert_eq!(configured_proxy(&cfg("main: { socks: 'socks5://h:1' }")).as_deref(), Some("socks5://h:1"));
        assert_eq!(configured_proxy(&cfg("main: { socks: 'undefined' }")), None);
    }
}

/// JavaScript runtime yt-dlp uses for YouTube (`--js-runtimes`): `main.ytdlp_js_runtime`, default
/// `deno` (installed in the Docker image; set `node`, `bun` or `quickjs` for hosts without deno).
pub fn ytdlp_js_runtime(cfg: &Config) -> String {
    cfg.str_or("main.ytdlp_js_runtime", "deno").to_string()
}
