//! Outgoing-request guard against SSRF.
//!
//! External tools (curl, yt-dlp) fetch user-supplied URLs. They are pointed at a small in-process
//! HTTP proxy (`--proxy`, see [`curl_args`] / [`ytdlp_args`]) that resolves every hostname itself,
//! refuses private, loopback, link-local and other special-purpose addresses, and connects to the
//! exact address it checked. That covers redirects and DNS rebinding, which a check of the URL
//! text alone cannot. The proxy listens on 127.0.0.1 and requires a random per-process token.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn private_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b)) // CGNAT
        || (a == 169 && b == 254) // link-local
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2)) // IETF protocol assignments, TEST-NET-1
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 198 && (b == 18 || b == 19)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // TEST-NET-2
        || (a == 203 && b == 0 && c == 113) // TEST-NET-3
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
    // IPv4-compatible ::a.b.c.d
    if seg[..6].iter().all(|s| *s == 0) {
        return true;
    }
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (seg[0] & 0xfe00) == 0xfc00 // unique local
        || (seg[0] & 0xffc0) == 0xfe80 // link-local
        || (seg[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
        || seg[0] == 0x2002 // 6to4 (embeds IPv4)
        || (seg[0] == 0x2001 && seg[1] == 0) // Teredo
        || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
        || seg[0] == 0x0100 && seg[1..4].iter().all(|s| *s == 0) // discard-only
}

/// Addresses outgoing requests must never reach.
pub fn is_private_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => private_v4(v4),
        IpAddr::V6(v6) => private_v6(v6),
    }
}

/// Syntactic check of a user-supplied URL: http(s) only, no local hostnames, no private IP literals.
/// Hostnames are checked again at connect time by the proxy.
pub fn validate_remote_url(raw: &str) -> Option<url::Url> {
    let u = url::Url::parse(raw.trim()).ok()?;
    if u.scheme() != "http" && u.scheme() != "https" {
        return None;
    }
    if !u.username().is_empty() || u.password().is_some() {
        return None;
    }
    let blocked = match u.host()? {
        url::Host::Domain(d) => {
            let d = d.trim_end_matches('.').to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost") || d.ends_with(".local") || d.ends_with(".internal") || !d.contains('.')
        }
        url::Host::Ipv4(ip) => private_v4(ip),
        url::Host::Ipv6(ip) => private_v6(ip),
    };
    (!blocked).then_some(u)
}

/// Resolve `host:port` and return the first public address; `None` if the name resolves to any
/// private address (mixed answers are refused too) or doesn't resolve.
pub async fn resolve_public(host: &str, port: u16) -> Option<SocketAddr> {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = host.parse::<IpAddr>() {
        return (!is_private_address(ip)).then_some(SocketAddr::new(ip, port));
    }
    let addrs: Vec<SocketAddr> = tokio::time::timeout(Duration::from_secs(10), tokio::net::lookup_host((host, port))).await.ok()?.ok()?.collect();
    if addrs.is_empty() || addrs.iter().any(|a| is_private_address(a.ip())) {
        return None;
    }
    addrs.into_iter().next()
}

struct Proxy {
    port: u16,
    token: String,
}

static PROXY: OnceLock<Proxy> = OnceLock::new();

/// Start the guard proxy (idempotent). Called at boot and by CLI commands that download.
pub async fn start() -> anyhow::Result<()> {
    if PROXY.get().is_some() {
        return Ok(());
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let port = listener.local_addr()?.port();
    let token = crate::util::random_hex(16);
    if PROXY.set(Proxy { port, token: token.clone() }).is_err() {
        return Ok(());
    }
    let expected = {
        use base64::Engine;
        format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(format!("f0ckm:{token}")))
    };
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else { continue };
            let expected = expected.clone();
            tokio::spawn(async move {
                if let Err(e) = handle(sock, &expected).await {
                    tracing::debug!("[NET GUARD] {e}");
                }
            });
        }
    });
    tracing::info!("[BOOT] outgoing request guard on 127.0.0.1:{port}");
    Ok(())
}

/// Proxy URL with credentials for external tools (`None` if the guard isn't running).
pub fn proxy_url() -> Option<String> {
    PROXY.get().map(|p| format!("http://f0ckm:{}@127.0.0.1:{}", p.token, p.port))
}

/// Arguments that make curl fetch through the guard. Refuses to run unguarded.
pub fn curl_args() -> Vec<String> {
    match proxy_url() {
        Some(p) => vec!["--proxy".into(), p, "--noproxy".into(), "".into()],
        // Unreachable proxy: curl fails instead of fetching unguarded.
        None => vec!["--proxy".into(), "http://127.0.0.1:9".into()],
    }
}

/// Arguments that make yt-dlp fetch through the guard.
pub fn ytdlp_args() -> Vec<String> {
    vec!["--proxy".into(), proxy_url().unwrap_or_else(|| "http://127.0.0.1:9".into())]
}

const MAX_HEAD: usize = 32 * 1024;

async fn reply(sock: &mut TcpStream, status: &str) {
    let _ = sock.write_all(format!("HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n").as_bytes()).await;
}

fn split_host_port(hp: &str, default: u16) -> Option<(String, u16)> {
    if let Some(rest) = hp.strip_prefix('[') {
        let (h, after) = rest.split_once(']')?;
        let port = match after.strip_prefix(':') {
            Some(p) => p.parse().ok()?,
            None => default,
        };
        return Some((h.to_string(), port));
    }
    match hp.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => Some((h.to_string(), p.parse().ok()?)),
        _ => Some((hp.to_string(), default)),
    }
}

async fn handle(mut sock: TcpStream, expected_auth: &str) -> anyhow::Result<()> {
    // Read the request head
    let mut buf = Vec::with_capacity(4096);
    let head_end = loop {
        let mut chunk = [0u8; 4096];
        let n = tokio::time::timeout(Duration::from_secs(30), sock.read(&mut chunk)).await??;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        if buf.len() > MAX_HEAD {
            reply(&mut sock, "431 Request Header Fields Too Large").await;
            return Ok(());
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let (method, target, version) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or("HTTP/1.1"));
    let headers: Vec<&str> = lines.filter(|l| !l.is_empty()).collect();

    let authorized = headers.iter().any(|h| {
        h.split_once(':')
            .map(|(k, v)| k.trim().eq_ignore_ascii_case("proxy-authorization") && crate::util::ct_eq(v.trim(), expected_auth))
            .unwrap_or(false)
    });
    if !authorized {
        reply(&mut sock, "407 Proxy Authentication Required").await;
        return Ok(());
    }

    if method.eq_ignore_ascii_case("CONNECT") {
        let Some((host, port)) = split_host_port(target, 443) else {
            reply(&mut sock, "400 Bad Request").await;
            return Ok(());
        };
        let Some(addr) = resolve_public(&host, port).await else {
            tracing::warn!("[NET GUARD] blocked CONNECT to {host}:{port}");
            reply(&mut sock, "403 Forbidden").await;
            return Ok(());
        };
        let mut upstream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr)).await??;
        sock.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        if buf.len() > head_end {
            upstream.write_all(&buf[head_end..]).await?;
        }
        tokio::io::copy_bidirectional(&mut sock, &mut upstream).await?;
        return Ok(());
    }

    // Plain HTTP in absolute form: GET http://host/path HTTP/1.1
    let Ok(url) = url::Url::parse(target) else {
        reply(&mut sock, "400 Bad Request").await;
        return Ok(());
    };
    if url.scheme() != "http" {
        reply(&mut sock, "400 Bad Request").await;
        return Ok(());
    }
    let host = url.host_str().unwrap_or("").to_string();
    let port = url.port().unwrap_or(80);
    let Some(addr) = resolve_public(&host, port).await else {
        tracing::warn!("[NET GUARD] blocked request to {host}:{port}");
        reply(&mut sock, "403 Forbidden").await;
        return Ok(());
    };
    let mut upstream = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect(addr)).await??;
    let path = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_string(),
    };
    let mut out = format!("{method} {path} {version}\r\n");
    for h in &headers {
        let name = h.split(':').next().unwrap_or("").trim().to_ascii_lowercase();
        if name.starts_with("proxy-") {
            continue;
        }
        out.push_str(h);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    upstream.write_all(out.as_bytes()).await?;
    if buf.len() > head_end {
        upstream.write_all(&buf[head_end..]).await?;
    }
    tokio::io::copy_bidirectional(&mut sock, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for ip in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.0.1", "169.254.169.254", "100.64.0.1", "0.0.0.0", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1", "64:ff9b::7f00:1"] {
            assert!(is_private_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "192.0.78.9", "2606:4700::1111"] {
            assert!(!is_private_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn url_validation() {
        assert!(validate_remote_url("https://example.com/a.png").is_some());
        for bad in ["--exec=id", "file:///etc/passwd", "http://localhost:5454/", "http://127.0.0.1/", "http://10.1.2.3/", "http://[::1]/", "http://user:pw@example.com/", "http://intranet/", "gopher://example.com/"] {
            assert!(validate_remote_url(bad).is_none(), "{bad}");
        }
    }

    #[tokio::test]
    async fn proxy_blocks_private_and_requires_token() {
        start().await.unwrap();
        let p = PROXY.get().unwrap();
        // Without credentials
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).await.unwrap();
        s.write_all(b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n").await.unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).await.unwrap();
        assert!(r.starts_with("HTTP/1.1 407"), "{r}");
        // With credentials, to a private address
        use base64::Engine;
        let auth = base64::engine::general_purpose::STANDARD.encode(format!("f0ckm:{}", p.token));
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).await.unwrap();
        s.write_all(format!("CONNECT 127.0.0.1:5454 HTTP/1.1\r\nProxy-Authorization: Basic {auth}\r\n\r\n").as_bytes()).await.unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).await.unwrap();
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
        let mut s = TcpStream::connect(("127.0.0.1", p.port)).await.unwrap();
        s.write_all(format!("GET http://localhost:5454/ HTTP/1.1\r\nHost: localhost\r\nProxy-Authorization: Basic {auth}\r\n\r\n").as_bytes()).await.unwrap();
        let mut r = String::new();
        s.read_to_string(&mut r).await.unwrap();
        assert!(r.starts_with("HTTP/1.1 403"), "{r}");
    }
}
