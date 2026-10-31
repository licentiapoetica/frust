//! Link previews for external chat clients (port of src/chat_preview_handler.mjs).
//!
//! The server fetches the linked page instead of the client, so people posting links cannot learn
//! chat participants' IP addresses. Preview images are proxied through signed URLs.
//!
//!   GET /api/chat/preview?url=...                 — page metadata, X-API-Key (upload key) required
//!   GET /api/chat/preview/image?url=...&sig=...   — proxied preview image, signature required
//!
//! SSRF guard: only http(s); the host name is resolved by our own resolver, which drops private,
//! loopback, link-local, CGNAT, multicast and other special-purpose addresses, and the connection
//! goes to exactly the addresses it returned (no re-resolution → no DNS rebinding). IP-literal
//! hosts are checked the same way. Redirects are followed by hand (at most 4) and every hop is
//! checked again. No proxy from the environment is used. Responses are size and time limited.
//!
//! Changes from the old handler: error messages from the network stack are no longer passed
//! through (`Fetch failed` instead), the IPv6 checks also cover NAT64/6to4/Teredo/documentation
//! ranges and IPv4-compatible addresses, and only the charsets UTF-8 and Latin-1/Windows-1252 are
//! decoded (others fall back to lossy UTF-8).

use crate::http::{Req, Res, Router, json_raw, reply};
use axum::http::{HeaderValue, Method, header};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::state::AppState;

const USER_AGENT: &str = "Mozilla/5.0 (compatible; f0ckm-linkpreview/1.0)";
const TIMEOUT: Duration = Duration::from_secs(6);
const MAX_HTML: usize = 768 * 1024;
const MAX_IMAGE: usize = 5 * 1024 * 1024;
const MAX_REDIRECTS: usize = 4;
const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);
const FAIL_TTL: Duration = Duration::from_secs(30 * 60);
const CACHE_MAX: usize = 2000;

pub fn register(r: &mut Router) {
    r.pre(Method::GET, r"^\/api\/chat\/preview$", None, preview);
    r.pre(Method::GET, r"^\/api\/chat\/preview\/image$", None, preview_image);
}

fn is_enabled(st: &AppState) -> bool {
    let cfg = st.cfg();
    cfg.enabled("websrv.chat_uploads") && cfg.enabled("websrv.chat_link_previews")
}

fn send(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

// ── Signing ─────────────────────────────────────────────────────────────────

/// Per-process secret: image URLs are only valid for the process that signed them.
static SECRET: Lazy<Vec<u8>> = Lazy::new(|| crate::util::random_bytes(32));

fn sign(url: &str) -> String {
    use base64::Engine;
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(&SECRET).expect("hmac key");
    mac.update(url.as_bytes());
    let full = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    full[..32].to_string()
}

// ── Address guard ───────────────────────────────────────────────────────────

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0 || a == 10 || a == 127 || a >= 224
        || (a == 100 && (64..=127).contains(&b))   // CGNAT
        || (a == 169 && b == 254)                  // link-local
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // IETF protocol assignments, TEST-NET-1
        || (a == 192 && b == 88 && c == 99)        // 6to4 relay anycast
        || (a == 198 && (b == 18 || b == 19))      // benchmarking
        || (a == 198 && b == 51 && c == 100)       // TEST-NET-2
        || (a == 203 && b == 0 && c == 113)        // TEST-NET-3
}

fn private_v6(ip: Ipv6Addr) -> bool {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return private_v4(v4);
    }
    let seg = ip.segments();
    // NAT64 64:ff9b::/96 and 64:ff9b:1::/48 embed an IPv4 address
    if seg[0] == 0x64 && seg[1] == 0xff9b {
        let v4 = Ipv4Addr::new((seg[6] >> 8) as u8, seg[6] as u8, (seg[7] >> 8) as u8, seg[7] as u8);
        return seg[2..6].iter().all(|s| *s == 0) && private_v4(v4) || seg[2] == 1;
    }
    // IPv4-compatible ::a.b.c.d (deprecated, but routes to the IPv4 host on some stacks)
    if seg[..6].iter().all(|s| *s == 0) {
        return true;
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] & 0xfe00) == 0xfc00   // unique local
        || (seg[0] & 0xffc0) == 0xfe80   // link-local
        || (seg[0] & 0xffc0) == 0xfec0   // site-local (deprecated)
        || seg[0] == 0x2002              // 6to4 (embeds IPv4)
        || (seg[0] == 0x2001 && seg[1] == 0)           // Teredo
        || (seg[0] == 0x2001 && seg[1] == 0x0db8)      // documentation
        || seg[0] == 0x0100 && seg[1..4].iter().all(|s| *s == 0) // discard-only
}

pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => private_v6(v6),
    }
}

/// reqwest resolver that only hands out public addresses.
struct PublicResolver;

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::time::timeout(TIMEOUT, tokio::net::lookup_host((host.as_str(), 0)))
                .await
                .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> { "DNS timeout".into() })??
                .filter(|a| !is_private_address(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(FetchError::Private) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .dns_resolver(Arc::new(PublicResolver))
        .connect_timeout(TIMEOUT)
        .read_timeout(TIMEOUT)
        .build()
        .expect("http client")
});

#[derive(Debug, thiserror::Error)]
enum FetchError {
    #[error("Invalid URL")]
    InvalidUrl,
    #[error("Unsupported protocol")]
    Protocol,
    #[error("Refusing private address")]
    Private,
    #[error("Too many redirects")]
    Redirects,
    #[error("HTTP {0}")]
    Status(u16),
    #[error("Timed out")]
    Timeout,
    #[error("Not an HTML page")]
    NotHtml,
    #[error("No preview data")]
    NoData,
    #[error("Fetch failed")]
    Other,
}

/// Was the request refused by our resolver? (reqwest wraps resolver errors.)
fn refused_private(e: &reqwest::Error) -> bool {
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(s) = src {
        if s.downcast_ref::<FetchError>().is_some_and(|f| matches!(f, FetchError::Private)) {
            return true;
        }
        src = s.source();
    }
    false
}

fn check_url(raw: &str) -> Result<url::Url, FetchError> {
    let u = url::Url::parse(raw).map_err(|_| FetchError::InvalidUrl)?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return Err(FetchError::Protocol);
    }
    match u.host() {
        Some(url::Host::Ipv4(ip)) if private_v4(ip) => Err(FetchError::Private),
        Some(url::Host::Ipv6(ip)) if private_v6(ip) => Err(FetchError::Private),
        Some(_) => Ok(u),
        None => Err(FetchError::InvalidUrl),
    }
}

/// GET with redirects, each hop validated. Returns the 200 response and its final URL.
async fn safe_get(raw: &str, accept: &str) -> Result<(reqwest::Response, url::Url), FetchError> {
    let mut url = check_url(raw)?;
    for hop in 0..=MAX_REDIRECTS {
        let fut = CLIENT.get(url.clone()).header("Accept", accept).header("Accept-Language", "en,*;q=0.5").send();
        let res = match tokio::time::timeout(TIMEOUT, fut).await {
            Err(_) => return Err(FetchError::Timeout),
            Ok(Err(e)) if refused_private(&e) => return Err(FetchError::Private),
            Ok(Err(e)) if e.is_timeout() => return Err(FetchError::Timeout),
            Ok(Err(e)) => {
                tracing::debug!("[CHAT_PREVIEW] fetch {url} failed: {e}");
                return Err(FetchError::Other);
            }
            Ok(Ok(r)) => r,
        };
        let status = res.status().as_u16();
        if (300..400).contains(&status) {
            if let Some(loc) = res.headers().get(header::LOCATION).and_then(|l| l.to_str().ok()) {
                if hop >= MAX_REDIRECTS {
                    return Err(FetchError::Redirects);
                }
                let next = url.join(loc).map_err(|_| FetchError::InvalidUrl)?;
                url = check_url(next.as_str())?;
                continue;
            }
        }
        if status != 200 {
            return Err(FetchError::Status(status));
        }
        return Ok((res, url));
    }
    Err(FetchError::Redirects)
}

/// Read up to `max` bytes (stopping early once `stop_at` was seen); partial data on timeout.
async fn read_limited(mut res: reqwest::Response, max: usize, stop_at: Option<&str>) -> Result<Vec<u8>, FetchError> {
    let mut buf: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, res.chunk()).await {
            Err(_) => return if buf.is_empty() { Err(FetchError::Timeout) } else { Ok(buf) },
            Ok(Err(_)) => return if buf.is_empty() { Err(FetchError::Other) } else { Ok(buf) },
            Ok(Ok(None)) => break,
            Ok(Ok(Some(chunk))) => {
                let hit = stop_at.is_some_and(|s| latin1_lower(&chunk).contains(s));
                buf.extend_from_slice(&chunk);
                if buf.len() > max || hit {
                    break;
                }
            }
        }
    }
    buf.truncate(max);
    Ok(buf)
}

fn latin1_lower(b: &[u8]) -> String {
    b.iter().map(|&c| (c as char).to_ascii_lowercase()).collect()
}

fn decode_body(body: &[u8], charset: &str) -> String {
    match charset {
        "iso-8859-1" | "latin1" | "latin-1" | "l1" | "iso8859-1" | "windows-1252" | "cp1252" | "us-ascii" | "ascii" => body.iter().map(|&c| c as char).collect(),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}

// ── Metadata parsing ────────────────────────────────────────────────────────

fn decode_entities(s: &str) -> String {
    static DEC: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"&#(\d+);").unwrap());
    static HEX: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)&#x([0-9a-f]+);").unwrap());
    let cp = |n: Option<u32>| n.and_then(char::from_u32).map(String::from).unwrap_or_else(|| "\u{FFFD}".into());
    let s = DEC.replace_all(s, |c: &regex::Captures| cp(c[1].parse::<u32>().ok()));
    let s = HEX.replace_all(&s, |c: &regex::Captures| cp(u32::from_str_radix(&c[1], 16).ok()));
    s.replace("&quot;", "\"").replace("&#39;", "'").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
}

fn clean(s: Option<&str>, max: usize) -> String {
    let Some(s) = s.filter(|s| !s.is_empty()) else { return String::new() };
    let t = decode_entities(s).split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > max {
        let mut out: String = t.chars().take(max - 1).collect();
        out.push('…');
        out
    } else {
        t
    }
}

/// `parsePreview(html, pageUrl)`: OpenGraph / Twitter / `<title>` metadata from `<head>`.
pub fn parse_preview(html: &str, page_url: &url::Url) -> Map<String, Value> {
    static HEAD_END: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)</head>").unwrap());
    static META: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?is)<meta\s+([^>]+?)/?>").unwrap());
    static ATTR: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r#"([a-zA-Z:_-]+)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap());
    static TITLE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    static COLOR: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)^#[0-9a-f]{3,8}$").unwrap());

    let head = HEAD_END.split(html).next().unwrap_or("");
    let mut meta: HashMap<String, String> = HashMap::new();
    for m in META.captures_iter(head) {
        let mut a: HashMap<String, String> = HashMap::new();
        for c in ATTR.captures_iter(&m[1]) {
            let v = c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)).map(|x| x.as_str()).unwrap_or("");
            a.insert(c[1].to_lowercase(), v.to_string());
        }
        let key = a.get("property").filter(|v| !v.is_empty()).or_else(|| a.get("name")).cloned().unwrap_or_default().to_lowercase();
        if !key.is_empty() {
            if let Some(content) = a.get("content") {
                meta.entry(key).or_insert_with(|| content.clone());
            }
        }
    }
    let get = |k: &str| meta.get(k).map(String::as_str).filter(|v| !v.is_empty());
    let first = |keys: &[&str]| keys.iter().find_map(|k| get(k));
    let title_tag = TITLE.captures(head).map(|c| c[1].to_string());
    let abs = |u: &str| -> String {
        match page_url.join(&decode_entities(u)) {
            Ok(x) if x.scheme() == "http" || x.scheme() == "https" => x.to_string(),
            _ => String::new(),
        }
    };
    let image = first(&["og:image:secure_url", "og:image", "twitter:image", "twitter:image:src"]).map(abs).unwrap_or_default();
    let color = get("theme-color").filter(|c| COLOR.is_match(c)).unwrap_or("").to_string();
    let host = page_url.host_str().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host).to_string();
    let width = meta.get("og:image:width").map(|w| w.trim().parse::<f64>().unwrap_or(if w.trim().is_empty() { 0.0 } else { f64::NAN })).unwrap_or(f64::NAN);

    let mut out = Map::new();
    out.insert("title".into(), clean(first(&["og:title", "twitter:title"]).or(title_tag.as_deref()), 200).into());
    out.insert("description".into(), clean(first(&["og:description", "twitter:description", "description"]), 400).into());
    out.insert("site".into(), clean(get("og:site_name").or(Some(host.as_str())), 80).into());
    out.insert("image".into(), image.into());
    out.insert("large".into(), Value::Bool(meta.get("twitter:card").map(String::as_str) == Some("summary_large_image") || width >= 600.0));
    out.insert("color".into(), color.into());
    out
}

async fn build_preview(url: &str) -> Result<Map<String, Value>, FetchError> {
    let (res, final_url) = safe_get(url, "text/html,application/xhtml+xml;q=0.9,*/*;q=0.1").await?;
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let lc = ct.to_lowercase();
    if !(lc.contains("text/html") || lc.contains("application/xhtml")) {
        return Err(FetchError::NotHtml);
    }
    static CHARSET: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)charset=([\w-]+)").unwrap());
    let charset = CHARSET.captures(&ct).map(|c| c[1].to_lowercase()).unwrap_or_else(|| "utf-8".into());
    let body = read_limited(res, MAX_HTML, Some("</head>")).await?;
    let html = decode_body(&body, &charset);
    let mut p = parse_preview(&html, &final_url);
    let empty = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").is_empty();
    if empty("title") && empty("description") {
        return Err(FetchError::NoData);
    }
    p.insert("url".into(), Value::String(final_url.to_string()));
    Ok(p)
}

// ── Cache ───────────────────────────────────────────────────────────────────

static CACHE: Lazy<Mutex<HashMap<String, (Value, Instant)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn cache_get(url: &str) -> Option<Value> {
    let c = CACHE.lock();
    c.get(url).filter(|(_, exp)| *exp > Instant::now()).map(|(v, _)| v.clone())
}

fn cache_set(url: &str, v: Value, ttl: Duration) {
    let mut c = CACHE.lock();
    c.insert(url.to_string(), (v, Instant::now() + ttl));
    if c.len() > CACHE_MAX {
        let now = Instant::now();
        c.retain(|_, (_, exp)| *exp > now);
        while c.len() > CACHE_MAX {
            let Some(oldest) = c.iter().min_by_key(|(_, (_, exp))| *exp).map(|(k, _)| k.clone()) else { break };
            c.remove(&oldest);
        }
    }
}

// ── Routes ──────────────────────────────────────────────────────────────────

async fn preview(st: AppState, req: Req) -> Res {
    if !is_enabled(&st) {
        return send(json!({ "success": false, "msg": "Not found" }), 404);
    }
    if let Err(r) = crate::routes::chat_upload::auth_key(&st, &req).await {
        return r;
    }
    let url: String = req.query("url").unwrap_or("").chars().take(2048).collect();
    static HTTP: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)^https?://").unwrap());
    if !HTTP.is_match(&url) {
        return send(json!({ "success": false, "msg": "Invalid URL" }), 400);
    }
    let result = match cache_get(&url) {
        Some(v) => v,
        None => {
            let v = match build_preview(&url).await {
                Ok(p) => {
                    let mut m = Map::new();
                    m.insert("success".into(), Value::Bool(true));
                    m.extend(p);
                    Value::Object(m)
                }
                Err(e) => json!({ "success": false, "msg": e.to_string() }),
            };
            let ok = v["success"] == Value::Bool(true);
            cache_set(&url, v.clone(), if ok { CACHE_TTL } else { FAIL_TTL });
            v
        }
    };
    if result["success"] != Value::Bool(true) {
        return send(result, 200);
    }
    let mut out = result;
    let image = out["image"].as_str().unwrap_or("").to_string();
    if !image.is_empty() {
        let base = st.cfg().str_or("main.url.full", "").trim_end_matches('/').to_string();
        out["image"] = Value::String(format!("{base}/api/chat/preview/image?url={}&sig={}", crate::util::encode_uri_component(&image), sign(&image)));
    }
    send(out, 200)
}

fn img_fail(code: u16) -> Res {
    Ok(reply(code, "text/plain", "Not available"))
}

async fn preview_image(st: AppState, req: Req) -> Res {
    if !is_enabled(&st) {
        return img_fail(404);
    }
    let url = req.query("url").unwrap_or("").to_string();
    let sig = req.query("sig").unwrap_or("");
    let expected = sign(&url);
    if url.is_empty() || !crate::util::ct_eq(sig, &expected) {
        return img_fail(403);
    }
    let (res, _) = match safe_get(&url, "image/avif,image/webp,image/png,image/jpeg,image/gif;q=0.9").await {
        Ok(r) => r,
        Err(_) => return img_fail(502),
    };
    let ct = res.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim().to_lowercase();
    if !matches!(ct.as_str(), "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/avif") {
        return img_fail(415);
    }
    let body = match read_limited(res, MAX_IMAGE + 1, None).await {
        Ok(b) => b,
        Err(_) => return img_fail(502),
    };
    if body.len() > MAX_IMAGE {
        return img_fail(413);
    }
    let len = body.len();
    let mut r = crate::http::raw(200, body);
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&ct) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'none'; sandbox"));
    h.insert("access-control-allow-origin", HeaderValue::from_static("*"));
    h.insert("cross-origin-resource-policy", HeaderValue::from_static("cross-origin"));
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_addresses() {
        for p in [
            "127.0.0.1", "10.1.2.3", "172.16.0.1", "172.31.255.255", "192.168.1.1", "169.254.169.254", "100.64.0.1", "100.127.255.255", "0.0.0.0",
            "224.0.0.1", "255.255.255.255", "198.18.0.1", "192.0.0.8", "::1", "::", "fe80::1", "fc00::1", "fd12::1", "ff02::1", "::ffff:127.0.0.1",
            "::ffff:10.0.0.1", "64:ff9b::a00:1", "::127.0.0.1", "2002:7f00:1::", "2001:db8::1", "2001:0:4136:e378::1", "fec0::1",
        ] {
            assert!(is_private_address(ip(p)), "{p} should be private");
        }
        for p in ["1.1.1.1", "8.8.8.8", "100.63.0.1", "100.128.0.1", "172.32.0.1", "2a00:1450:4001::1", "::ffff:8.8.8.8", "64:ff9b::808:808"] {
            assert!(!is_private_address(ip(p)), "{p} should be public");
        }
    }

    #[test]
    fn url_checks() {
        assert!(matches!(check_url("ftp://example.com/"), Err(FetchError::Protocol)));
        assert!(matches!(check_url("http://127.0.0.1/"), Err(FetchError::Private)));
        assert!(matches!(check_url("http://[::1]:8080/"), Err(FetchError::Private)));
        assert!(matches!(check_url("http://0x7f000001/"), Err(FetchError::Private)));
        assert!(matches!(check_url("http://2130706433/"), Err(FetchError::Private)));
        assert!(check_url("https://example.com/x").is_ok());
    }

    #[tokio::test]
    async fn refuses_local_names() {
        assert!(matches!(safe_get("http://localhost:1/", "*/*").await, Err(FetchError::Private)));
    }

    #[test]
    fn parses_metadata() {
        let html = r##"<html><head><title> Fallback &amp; title </title>
            <meta property="og:title" content="OG &quot;Title&quot;">
            <meta property="og:title" content="second">
            <meta name='description' content='Desc &#x41;&#66;'>
            <meta property="og:image" content="/img/a.png">
            <meta property="og:image:width" content="1200">
            <meta name="theme-color" content="#ff0000">
            </head><body><meta property="og:site_name" content="late"></body></html>"##;
        let u = url::Url::parse("https://www.example.com/page").unwrap();
        let p = parse_preview(html, &u);
        assert_eq!(p["title"], "OG \"Title\"");
        assert_eq!(p["description"], "Desc AB");
        assert_eq!(p["site"], "example.com");
        assert_eq!(p["image"], "https://www.example.com/img/a.png");
        assert_eq!(p["large"], true);
        assert_eq!(p["color"], "#ff0000");
        let keys: Vec<&String> = p.keys().collect();
        assert_eq!(keys, ["title", "description", "site", "image", "large", "color"]);

        let p = parse_preview("<title>x</title><meta property='og:image' content='javascript:alert(1)'>", &u);
        assert_eq!(p["title"], "x");
        assert_eq!(p["image"], "");
        assert_eq!(p["large"], false);
    }

    #[test]
    fn signatures() {
        let s = sign("https://example.com/a.png");
        assert_eq!(s.len(), 32);
        assert_eq!(s, sign("https://example.com/a.png"));
        assert_ne!(s, sign("https://example.com/b.png"));
    }

    #[test]
    fn clean_truncates() {
        assert_eq!(clean(Some("  a \n b  "), 10), "a b");
        assert_eq!(clean(Some("abcdef"), 4), "abc…");
        assert_eq!(clean(None, 4), "");
    }
}
