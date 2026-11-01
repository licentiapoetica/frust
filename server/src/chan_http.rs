//! Outgoing 4chan requests (`src/inc/chan_http.mjs`): API JSON, media proxying and rehost downloads.
//!
//! Every call gets a randomized browser identity:
//!   * If curl-impersonate is installed (wrapper binaries like `curl_chrome116`, `curl_ff117`, …), a
//!     random profile is used. These reproduce a real browser's TLS + HTTP/2 handshake and send
//!     matching headers, so the User-Agent is not overridden.
//!   * Otherwise plain curl is used with a random, current User-Agent and a matching Accept-Language.
//!
//! config `main.curl_impersonate`: unset/true → auto-detect wrappers on PATH, false → never,
//! "<dir>" → also look in that directory.
//!
//! Only the 4chan hosts in [`ALLOWED_HOSTS`] are ever contacted: URLs can only be built through
//! [`ChanUrl`], which validates scheme, host and path, and curl runs with `--proto =https` and
//! without following redirects, so a response can't bounce the request to another host.

use crate::config::Config;
use anyhow::{Result, anyhow, bail};
use bytes::Bytes;
use rand::RngExt;
use rand::seq::IndexedRandom;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// The only hosts outgoing 4chan requests may go to: the JSON API, the media CDN and the old
/// media host the rehost endpoint accepted.
pub const API_HOST: &str = "a.4cdn.org";
pub const MEDIA_HOST: &str = "i.4cdn.org";
pub const LEGACY_MEDIA_HOST: &str = "boards.4cdn.org";
pub const ALLOWED_HOSTS: [&str; 3] = [API_HOST, MEDIA_HOST, LEGACY_MEDIA_HOST];

/// Upper bound for API JSON responses (a full catalog is around 1 MB).
pub const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
/// Upper bound for proxied media (4chan's own limits are a few MB per file).
pub const MAX_PROXY_BYTES: u64 = 32 * 1024 * 1024;
/// Total time a proxied media response may take.
pub const PROXY_MAX_SECS: u64 = 60;

// ── URLs ────────────────────────────────────────────────────────────────────

/// A validated `https://<allowed host>/...` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChanUrl(String);

/// Board names as the routes accept them (`[a-z0-9]+`), with a sane length cap.
pub fn valid_board(b: &str) -> bool {
    !b.is_empty() && b.len() <= 16 && b.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Media file names: `<name>.<ext>` from a safe charset (4chan uses `<tim>.<ext>` / `<tim>s.jpg`).
pub fn valid_media_file(f: &str) -> bool {
    let Some((name, ext)) = f.rsplit_once('.') else { return false };
    !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        && !ext.is_empty()
        && ext.len() <= 8
        && ext.bytes().all(|c| c.is_ascii_alphanumeric())
}

impl ChanUrl {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `https://a.4cdn.org/<board>/thread/<tid>.json`
    pub fn api_thread(board: &str, tid: u64) -> Option<ChanUrl> {
        valid_board(board).then(|| ChanUrl(format!("https://{API_HOST}/{board}/thread/{tid}.json")))
    }

    /// `https://a.4cdn.org/<board>/catalog.json`
    pub fn api_catalog(board: &str) -> Option<ChanUrl> {
        valid_board(board).then(|| ChanUrl(format!("https://{API_HOST}/{board}/catalog.json")))
    }

    /// `https://<media host>/<board>/<file>`
    pub fn media_on(host: &str, board: &str, file: &str) -> Option<ChanUrl> {
        (matches!(host, MEDIA_HOST | LEGACY_MEDIA_HOST) && valid_board(board) && valid_media_file(file))
            .then(|| ChanUrl(format!("https://{host}/{board}/{file}")))
    }

    pub fn media(board: &str, file: &str) -> Option<ChanUrl> {
        Self::media_on(MEDIA_HOST, board, file)
    }

    pub fn host(&self) -> &str {
        self.0.strip_prefix("https://").and_then(|r| r.split('/').next()).unwrap_or("")
    }
}

/// What `POST /api/v2/scroller/rehost` will download for a user-supplied URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RehostSource {
    /// URL that is fetched (always https on an allowed media host).
    pub download: ChanUrl,
    /// Value stored in `items.src` (what the thread/catalog views look up).
    pub src: String,
    pub board: String,
}

const PROXY_PREFIX: &str = "/api/v2/scroller/external/4chan/";

/// Accepts the two forms the frontend sends: a 4chan media URL
/// (`http(s)://i.4cdn.org/<board>/<file>`, also `boards.4cdn.org`) or this site's media proxy path
/// (`/api/v2/scroller/external/4chan/<board>/media/<file>`, relative or absolute). The proxy form
/// is resolved to the CDN URL directly; its host is never contacted.
///
/// The old check was an unanchored regex, so any URL containing the proxy path
/// (`http://10.0.0.1/api/v2/scroller/external/4chan/b/media/x`) was handed to curl.
pub fn rehost_source(raw: &str) -> Option<RehostSource> {
    let raw = raw.trim();
    let lower = raw.to_ascii_lowercase();
    let (path, absolute) = if lower.starts_with("http://") || lower.starts_with("https://") {
        let u = url::Url::parse(raw).ok()?;
        if !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
            return None;
        }
        let host = u.host_str()?.to_ascii_lowercase();
        if host == MEDIA_HOST || host == LEGACY_MEDIA_HOST {
            if u.port().is_some() {
                return None;
            }
            let mut segs = u.path_segments()?;
            let (board, file) = (segs.next()?, segs.next()?);
            if segs.next().is_some() {
                return None;
            }
            let download = ChanUrl::media_on(&host, board, file)?;
            return Some(RehostSource { download, src: raw.to_string(), board: board.to_string() });
        }
        (u.path().to_string(), true)
    } else if raw.starts_with('/') && !raw.starts_with("//") {
        if raw.contains(['?', '#']) {
            return None;
        }
        (raw.to_string(), false)
    } else {
        return None;
    };
    let rest = path.strip_prefix(PROXY_PREFIX)?;
    let mut parts = rest.split('/');
    let (board, media, file) = (parts.next()?, parts.next()?, parts.next()?);
    if media != "media" || parts.next().is_some() {
        return None;
    }
    let download = ChanUrl::media(board, file)?;
    // Absolute proxy URLs are stored in their relative form (the form the views look up)
    let src = if absolute { format!("{PROXY_PREFIX}{board}/media/{file}") } else { raw.to_string() };
    Some(RehostSource { download, src, board: board.to_string() })
}

// ── Browser identity ────────────────────────────────────────────────────────

/// Browser major version derived from the date so the pool never goes stale
/// (Chrome 100 shipped 2022-03-29, Firefox 100 on 2022-05-03; both release every 4 weeks).
fn major_since(base: i64, y: i32, m: u32, d: u32) -> i64 {
    let start = chrono::NaiveDate::from_ymd_opt(y, m, d).and_then(|d| d.and_hms_opt(0, 0, 0)).map(|d| d.and_utc().timestamp_millis()).unwrap_or(0);
    base + (crate::util::now_ms() - start).div_euclid(28 * 86_400_000)
}

pub fn random_user_agent() -> String {
    let mut rng = rand::rng();
    let chrome = major_since(100, 2022, 3, 29) - rng.random_range(0..3);
    let firefox = major_since(100, 2022, 5, 3) - rng.random_range(0..3);
    let templates = [
        format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36"),
        format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36"),
        format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36"),
        format!("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36"),
        format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.36 Edg/{chrome}.0.0.0"),
        format!("Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:{firefox}.0) Gecko/20100101 Firefox/{firefox}.0"),
        format!("Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:{firefox}.0) Gecko/20100101 Firefox/{firefox}.0"),
        format!("Mozilla/5.0 (X11; Linux x86_64; rv:{firefox}.0) Gecko/20100101 Firefox/{firefox}.0"),
    ];
    templates.choose(&mut rng).cloned().unwrap_or_default()
}

const ACCEPT_LANGUAGES: [&str; 5] = [
    "en-US,en;q=0.9",
    "en-US,en;q=0.8",
    "en-GB,en;q=0.9,en-US;q=0.8",
    "en-US,en;q=0.9,de;q=0.8",
    "de-DE,de;q=0.9,en-US;q=0.8,en;q=0.7",
];

// ── curl-impersonate detection (cached after the first lookup) ──────────────

/// Only the newest desktop profiles per browser: an old fingerprint stands out as much as plain curl.
const PROFILES_PER_BROWSER: usize = 3;

/// `curl_chrome146` → ("chrome", 146); desktop profiles only (`_android`, `_ios`, `133a` are skipped).
fn parse_impersonate_name(name: &str) -> Option<(&'static str, u32)> {
    let rest = name.strip_prefix("curl_")?;
    let split = rest.find(|c: char| c.is_ascii_digit())?;
    let (fam, tail) = rest.split_at(split);
    if !tail.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let family = match fam.to_ascii_lowercase().as_str() {
        "chrome" => "chrome",
        "edge" => "edge",
        "firefox" | "ff" => "firefox",
        "safari" => "safari",
        _ => return None,
    };
    Some((family, tail.parse().ok()?))
}

/// Pick the newest profiles from (family, version, path) candidates.
fn select_profiles(mut found: Vec<(&'static str, u32, PathBuf)>) -> Vec<PathBuf> {
    // Edge shares Chrome's version numbers; drop Edge profiles far behind the newest Chrome.
    let newest_chrome = found.iter().filter(|p| p.0 == "chrome").map(|p| p.1).max().unwrap_or(0);
    if newest_chrome > 0 {
        found.retain(|p| p.0 != "edge" || p.1 + 10 >= newest_chrome);
    }
    let mut out = Vec::new();
    for fam in ["chrome", "edge", "firefox", "safari"] {
        let mut list: Vec<_> = found.iter().filter(|p| p.0 == fam).collect();
        list.sort_by_key(|p| std::cmp::Reverse(p.1));
        out.extend(list.into_iter().take(PROFILES_PER_BROWSER).map(|p| p.2.clone()));
    }
    out
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

fn find_impersonate_bins(cfg: &Config) -> &'static [PathBuf] {
    static BINS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    BINS.get_or_init(|| {
        let setting = cfg.get("main.curl_impersonate");
        if setting == &Value::Bool(false) {
            return vec![];
        }
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        if let Some(d) = setting.as_str().map(str::trim).filter(|d| !d.is_empty()) {
            dirs.insert(0, PathBuf::from(d));
        }
        let mut seen = std::collections::HashSet::new();
        let mut found = Vec::new();
        for dir in dirs {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                let Some((fam, ver)) = parse_impersonate_name(&name) else { continue };
                if seen.contains(&name) {
                    continue;
                }
                let full = dir.join(&name);
                if !is_executable(&full) {
                    continue;
                }
                seen.insert(name);
                found.push((fam, ver, full));
            }
        }
        let bins = select_profiles(found);
        if bins.is_empty() {
            tracing::info!("[BOOT] 4chan requests: curl-impersonate not found, using curl with randomized User-Agent");
        } else {
            let names: Vec<String> = bins.iter().filter_map(|b| b.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
            tracing::info!("[BOOT] 4chan requests: curl-impersonate enabled ({})", names.join(", "));
        }
        bins
    })
}

/// The static curl-impersonate build looks for CAs at the Debian path only; point it at the local
/// bundle on other distros.
fn ca_args() -> &'static [String] {
    static CA: OnceLock<Vec<String>> = OnceLock::new();
    CA.get_or_init(|| {
        if Path::new("/etc/ssl/certs/ca-certificates.crt").exists() {
            return vec![];
        }
        ["/etc/ssl/ca-bundle.pem", "/etc/pki/tls/certs/ca-bundle.crt", "/etc/ssl/cert.pem"]
            .iter()
            .find(|f| Path::new(f).exists())
            .map(|f| vec!["--cacert".to_string(), f.to_string()])
            .unwrap_or_default()
    })
}

/// A curl invocation (binary + arguments, no shell).
#[derive(Debug, Clone)]
pub struct CurlCmd {
    pub bin: String,
    pub args: Vec<String>,
}

impl CurlCmd {
    pub fn arg_refs(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }
}

/// Build a curl command for a 4chan URL with a randomized browser identity (`chanCurl`).
/// `extra` are additional curl flags (e.g. `-o <file>`, `--max-time 300`).
pub fn chan_curl(cfg: &Config, url: &ChanUrl, extra: &[String]) -> CurlCmd {
    build_curl(url, extra, crate::media::socks(cfg).as_deref(), find_impersonate_bins(cfg))
}

fn build_curl(url: &ChanUrl, extra: &[String], socks: Option<&str>, impersonate: &[PathBuf]) -> CurlCmd {
    let mut rng = rand::rng();
    // No -L: 4chan's API and CDN answer directly, and a redirect must not lead anywhere else.
    let mut args: Vec<String> = ["-s", "-f", "--proto", "=https", "--max-redirs", "0"].iter().map(|s| s.to_string()).collect();
    args.extend(extra.iter().cloned());
    if let Some(s) = socks {
        let host = s.split_once("://").map(|(_, h)| h).unwrap_or(s);
        args.extend(["--socks5-hostname".to_string(), host.to_string()]);
    }
    let bin = match impersonate.choose(&mut rng) {
        // The wrapper supplies its own UA, header order and TLS/HTTP2 fingerprint
        Some(b) => {
            args.extend(ca_args().iter().cloned());
            b.to_string_lossy().into_owned()
        }
        None => {
            let lang = ACCEPT_LANGUAGES.choose(&mut rng).copied().unwrap_or(ACCEPT_LANGUAGES[0]);
            args.extend(["-A".to_string(), random_user_agent(), "-H".to_string(), format!("Accept-Language: {lang}")]);
            "curl".to_string()
        }
    };
    args.push("--".into());
    args.push(url.as_str().to_string());
    CurlCmd { bin, args }
}

// ── Requests ────────────────────────────────────────────────────────────────

/// Fetch and parse API JSON (`fetchWithProxy`): 30 s, size-capped, must look like JSON.
pub async fn fetch_json(cfg: &Config, url: &ChanUrl) -> Result<Value> {
    let extra = ["--max-time", "30", "--max-filesize", &MAX_JSON_BYTES.to_string()].map(String::from);
    let cmd = chan_curl(cfg, url, &extra);
    let out = crate::proc::spawn_with(&cmd.bin, &cmd.arg_refs(), crate::proc::Opts { timeout: Duration::from_secs(40), quiet: true, ..Default::default() })
        .await
        .map_err(|e| anyhow!("request to {} failed: {}", url.as_str(), first_line(&e.to_string())))?;
    if out.stdout.len() as u64 > MAX_JSON_BYTES {
        bail!("response from {} too large", url.as_str());
    }
    parse_json_body(&out.stdout).map_err(|e| anyhow!("{e} from {}", url.as_str()))
}

fn first_line(s: &str) -> &str {
    s.lines().next().unwrap_or("")
}

/// The body must start with `{` or `[` (4chan answers some errors with an HTML page).
pub fn parse_json_body(body: &[u8]) -> Result<Value> {
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if !text.starts_with('{') && !text.starts_with('[') {
        let head: String = text.chars().take(100).collect();
        bail!("Expected JSON but got non-JSON response ({head:?})");
    }
    Ok(serde_json::from_str(text)?)
}

/// Download a media file to `dest` (rehost): size and time limited.
pub async fn download(cfg: &Config, url: &ChanUrl, dest: &Path, max_bytes: u64) -> Result<()> {
    let extra = [
        "-o".to_string(),
        dest.to_string_lossy().into_owned(),
        "--max-filesize".to_string(),
        max_bytes.to_string(),
        "--connect-timeout".to_string(),
        "30".to_string(),
        "--max-time".to_string(),
        "300".to_string(),
    ];
    let cmd = chan_curl(cfg, url, &extra);
    crate::proc::spawn_with(&cmd.bin, &cmd.arg_refs(), crate::proc::Opts { timeout: Duration::from_secs(320), quiet: true, ..Default::default() }).await?;
    let len = tokio::fs::metadata(dest).await.map(|m| m.len()).unwrap_or(0);
    if len == 0 {
        bail!("empty download from {}", url.as_str());
    }
    if len > max_bytes {
        bail!("download from {} exceeds the size limit", url.as_str());
    }
    Ok(())
}

// ── Media proxy ─────────────────────────────────────────────────────────────

/// Content type for the proxied file, decided by its first bytes; only these image/video types
/// are ever passed through.
pub fn sniff_media_type(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[0..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        Some("video/webm")
    } else if head.len() >= 12 && &head[4..8] == b"ftyp" {
        Some("video/mp4")
    } else {
        None
    }
}

/// Bytes needed to decide [`sniff_media_type`].
const SNIFF_LEN: usize = 12;

pub enum ProxyStart {
    /// Upstream answered with a recognised media type; the stream yields the whole body.
    Ok { content_type: &'static str, body: axum::body::Body },
    /// curl failed before sending data (`http_error` = HTTP status ≥ 400, e.g. pruned media).
    Failed { http_error: bool },
    /// Upstream sent something that is not an allowed image/video.
    BadType,
}

/// Start streaming a media file: the response starts as soon as the first bytes arrive.
/// The stream is cut after [`MAX_PROXY_BYTES`] or [`PROXY_MAX_SECS`]; curl is killed when the
/// client disconnects (the body, which owns the child, is dropped).
pub async fn proxy_media(cfg: &Config, url: &ChanUrl) -> Result<ProxyStart> {
    use tokio::io::AsyncReadExt;
    let extra = ["--max-time", &PROXY_MAX_SECS.to_string(), "--max-filesize", &MAX_PROXY_BYTES.to_string()].map(String::from);
    let cmd = chan_curl(cfg, url, &extra);
    let mut child = tokio::process::Command::new(&cmd.bin)
        .args(&cmd.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow!("failed to start {}: {e}", cmd.bin))?;
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(PROXY_MAX_SECS + 5);

    // Read until the type can be decided (or EOF)
    let mut head = Vec::with_capacity(64 * 1024);
    let mut buf = vec![0u8; 64 * 1024];
    while head.len() < SNIFF_LEN {
        let n = match tokio::time::timeout_at(deadline, stdout.read(&mut buf)).await {
            Ok(r) => r?,
            Err(_) => return Ok(ProxyStart::Failed { http_error: false }),
        };
        if n == 0 {
            break;
        }
        head.extend_from_slice(&buf[..n]);
    }
    if head.is_empty() {
        let status = tokio::time::timeout_at(deadline, child.wait()).await.ok().and_then(Result::ok);
        tracing::debug!("[CHAN_HTTP] media fetch {} failed: {status:?}", url.as_str());
        // curl exit 22: HTTP error (-f)
        return Ok(ProxyStart::Failed { http_error: status.and_then(|s| s.code()) == Some(22) });
    }
    let Some(content_type) = sniff_media_type(&head) else { return Ok(ProxyStart::BadType) };

    struct State {
        child: tokio::process::Child,
        stdout: tokio::process::ChildStdout,
        first: Option<Bytes>,
        sent: u64,
        done: bool,
    }
    let state = State { child, stdout, first: Some(Bytes::from(head)), sent: 0, done: false };
    let stream = futures::stream::unfold(state, move |mut s| async move {
        if s.done {
            return None;
        }
        if let Some(first) = s.first.take() {
            s.sent += first.len() as u64;
            return Some((Ok::<Bytes, std::io::Error>(first), s));
        }
        let mut buf = vec![0u8; 64 * 1024];
        match tokio::time::timeout_at(deadline, s.stdout.read(&mut buf)).await {
            Ok(Ok(0)) => {
                let _ = s.child.wait().await;
                None
            }
            Ok(Ok(n)) => {
                s.sent += n as u64;
                if s.sent > MAX_PROXY_BYTES {
                    s.done = true;
                    let _ = s.child.start_kill();
                    return Some((Err(std::io::Error::other("media proxy size limit exceeded")), s));
                }
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), s))
            }
            Ok(Err(e)) => {
                s.done = true;
                Some((Err(e), s))
            }
            Err(_) => {
                s.done = true;
                let _ = s.child.start_kill();
                Some((Err(std::io::Error::other("media proxy timed out")), s))
            }
        }
    });
    Ok(ProxyStart::Ok { content_type, body: axum::body::Body::from_stream(stream) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_only_on_allowed_hosts() {
        assert_eq!(ChanUrl::api_thread("wsg", 123).unwrap().as_str(), "https://a.4cdn.org/wsg/thread/123.json");
        assert_eq!(ChanUrl::api_catalog("b").unwrap().as_str(), "https://a.4cdn.org/b/catalog.json");
        assert_eq!(ChanUrl::media("gif", "1700000000123456.webm").unwrap().as_str(), "https://i.4cdn.org/gif/1700000000123456.webm");
        assert_eq!(ChanUrl::media("gif", "1700000000123456s.jpg").unwrap().host(), "i.4cdn.org");
        assert!(ChanUrl::api_catalog("B").is_none());
        assert!(ChanUrl::api_catalog("b/../x").is_none());
        assert!(ChanUrl::api_catalog("").is_none());
        assert!(ChanUrl::media("b", "../x.jpg").is_none());
        assert!(ChanUrl::media("b", "x").is_none());
        assert!(ChanUrl::media("b", "a@evil.com.jpg").is_none());
        assert!(ChanUrl::media("b", "x.jpg?y").is_none());
        assert!(ChanUrl::media_on("evil.com", "b", "1.jpg").is_none());
        for u in [ChanUrl::api_thread("b", 1).unwrap(), ChanUrl::media_on(LEGACY_MEDIA_HOST, "b", "1.png").unwrap()] {
            assert!(ALLOWED_HOSTS.contains(&u.host()));
        }
    }

    #[test]
    fn rehost_sources() {
        let r = rehost_source("https://i.4cdn.org/wsg/1700000000123456.webm").unwrap();
        assert_eq!(r.download.as_str(), "https://i.4cdn.org/wsg/1700000000123456.webm");
        assert_eq!(r.src, "https://i.4cdn.org/wsg/1700000000123456.webm");
        assert_eq!(r.board, "wsg");
        // http is upgraded for the download, src stays as given
        let r = rehost_source("http://i.4cdn.org/b/1.jpg").unwrap();
        assert_eq!(r.download.as_str(), "https://i.4cdn.org/b/1.jpg");
        assert_eq!(r.src, "http://i.4cdn.org/b/1.jpg");
        let r = rehost_source("https://boards.4cdn.org/b/1.png").unwrap();
        assert_eq!(r.download.as_str(), "https://boards.4cdn.org/b/1.png");
        // proxy path → CDN
        let r = rehost_source("/api/v2/scroller/external/4chan/gif/1700000000123456.mp4").map(|r| r.download);
        assert!(r.is_none(), "missing /media/ segment");
        let r = rehost_source("/api/v2/scroller/external/4chan/gif/media/1700000000123456.mp4").unwrap();
        assert_eq!(r.download.as_str(), "https://i.4cdn.org/gif/1700000000123456.mp4");
        assert_eq!(r.src, "/api/v2/scroller/external/4chan/gif/media/1700000000123456.mp4");
        let r = rehost_source("https://f0ck.example/api/v2/scroller/external/4chan/gif/media/1.gif").unwrap();
        assert_eq!(r.download.as_str(), "https://i.4cdn.org/gif/1.gif");
        assert_eq!(r.src, "/api/v2/scroller/external/4chan/gif/media/1.gif");
        // SSRF attempts and junk
        for bad in [
            "http://127.0.0.1/api/v2/scroller/external/4chan/b/media/../../x.jpg",
            "http://10.0.0.1/x/api/v2/scroller/external/4chan/b/media/1.jpg",
            "https://i.4cdn.org.evil.com/b/1.jpg",
            "https://evil.com/i.4cdn.org/b/1.jpg",
            "https://user@i.4cdn.org/b/1.jpg",
            "https://i.4cdn.org:8443/b/1.jpg",
            "https://i.4cdn.org/b/1.jpg?x=1",
            "https://i.4cdn.org/b/c/1.jpg",
            "https://i.4cdn.org/b/",
            "file:///etc/passwd",
            "ftp://i.4cdn.org/b/1.jpg",
            "//i.4cdn.org/b/1.jpg",
            "-o/tmp/x",
            "",
            "/api/v2/scroller/external/4chan/b/media/1.jpg?x",
            "/api/v2/scroller/external/4chan/b/media/1.jpg/x",
        ] {
            assert!(rehost_source(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn impersonate_names() {
        assert_eq!(parse_impersonate_name("curl_chrome146"), Some(("chrome", 146)));
        assert_eq!(parse_impersonate_name("curl_ff117"), Some(("firefox", 117)));
        assert_eq!(parse_impersonate_name("curl_safari2601"), Some(("safari", 2601)));
        assert_eq!(parse_impersonate_name("curl_chrome131_android"), None);
        assert_eq!(parse_impersonate_name("curl_chrome133a"), None);
        assert_eq!(parse_impersonate_name("curl_tor145"), None);
        assert_eq!(parse_impersonate_name("curl-config"), None);
        let p = |f, v| (f, v, PathBuf::from(format!("/bin/curl_{f}{v}")));
        let sel = select_profiles(vec![p("chrome", 100), p("chrome", 146), p("chrome", 142), p("chrome", 145), p("edge", 101), p("firefox", 147)]);
        let names: Vec<_> = sel.iter().map(|b| b.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["curl_chrome146", "curl_chrome145", "curl_chrome142", "curl_firefox147"]);
    }

    #[test]
    fn curl_args() {
        let u = ChanUrl::api_catalog("b").unwrap();
        let c = build_curl(&u, &["--max-time".into(), "30".into()], Some("socks5://127.0.0.1:9050"), &[]);
        assert_eq!(c.bin, "curl");
        assert_eq!(&c.args[..6], ["-s", "-f", "--proto", "=https", "--max-redirs", "0"]);
        assert!(!c.args.iter().any(|a| a == "-L"));
        let i = c.args.iter().position(|a| a == "--socks5-hostname").unwrap();
        assert_eq!(c.args[i + 1], "127.0.0.1:9050");
        assert!(c.args.iter().any(|a| a == "-A"));
        assert_eq!(&c.args[c.args.len() - 2..], ["--", "https://a.4cdn.org/b/catalog.json"]);
        let c = build_curl(&u, &[], None, &[PathBuf::from("/usr/bin/curl_chrome146")]);
        assert_eq!(c.bin, "/usr/bin/curl_chrome146");
        assert!(!c.args.iter().any(|a| a == "-A"), "impersonate keeps its own UA");
        assert_eq!(c.args.last().unwrap(), "https://a.4cdn.org/b/catalog.json");
    }

    #[test]
    fn user_agent_is_current() {
        let ua = random_user_agent();
        assert!(ua.starts_with("Mozilla/5.0"));
        let ver: i64 = ua.split(['/', 'v']).filter_map(|p| p.split('.').next()?.parse().ok()).filter(|&n| n >= 100).max().unwrap();
        assert!(ver >= 140, "{ua}");
    }

    #[test]
    fn sniffing() {
        assert_eq!(sniff_media_type(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0, 0, 0, 0, 0]), Some("image/jpeg"));
        assert_eq!(sniff_media_type(b"\x89PNG\r\n\x1a\n\0\0\0\0"), Some("image/png"));
        assert_eq!(sniff_media_type(b"GIF89a......"), Some("image/gif"));
        assert_eq!(sniff_media_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_media_type(&[0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3, 4, 5, 6, 7, 8]), Some("video/webm"));
        assert_eq!(sniff_media_type(b"\0\0\0\x18ftypmp42"), Some("video/mp4"));
        assert_eq!(sniff_media_type(b"<html><script>"), None);
        assert_eq!(sniff_media_type(b"<svg xmlns="), None);
    }

    #[test]
    fn json_bodies() {
        assert!(parse_json_body(b"  {\"posts\":[]}\n").is_ok());
        assert!(parse_json_body(b"[{\"page\":1}]").is_ok());
        assert!(parse_json_body(b"<!DOCTYPE html>").is_err());
        assert!(parse_json_body(b"").is_err());
    }
}
