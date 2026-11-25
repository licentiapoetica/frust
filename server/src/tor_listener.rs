//! The .onion service's own port (`websrv.tor_port`, default 1338): Tor sends every connection with
//! a PROXY protocol line naming the circuit it came on (torrc `HiddenServiceExportCircuitID
//! haproxy`), so .onion visitors can be told apart like clearnet ones by their IP (scraper guard,
//! rate limits) instead of all sharing the Tor service's address.
//!
//! Requests from here carry [`Circuit`]: http::Req::tor_circuit. They count as .onion whatever their
//! Host header says, and forwarding headers (X-Real-IP, ...) are never believed: they come from
//! the visitor. Only connections from loopback / the private network (the Tor container) are
//! taken. A connection without the PROXY line (a torrc without HiddenServiceExportCircuitID) still
//! works, as one without a circuit.

use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// The Tor circuit a request came on (its 32-bit global id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Circuit(pub u32);

/// Tor's addresses for circuits: fc00:dead:beef:4dad::<id high 16 bits>:<id low 16 bits>
const PREFIX: [u16; 4] = [0xfc00, 0xdead, 0xbeef, 0x4dad];

/// An address Tor made up for a circuit (see [`PREFIX`]).
pub fn is_circuit_addr(ip: Ipv6Addr) -> bool {
    ip.segments()[..4] == PREFIX
}

/// Longest PROXY v1 line (spec: 107 bytes with CRLF).
const MAX_LINE: usize = 107;

/// Parses a PROXY v1 line (without CRLF) from Tor: the circuit and the address to use as the
/// client's. `None` for anything else.
pub fn parse_line(line: &str) -> Option<(Circuit, SocketAddr)> {
    let f: Vec<&str> = line.split(' ').collect();
    if f.len() != 6 || f[0] != "PROXY" || f[1] != "TCP6" {
        return None;
    }
    let src: Ipv6Addr = f[2].parse().ok()?;
    let port: u16 = f[4].parse().ok()?;
    let s = src.segments();
    if s[..4] != PREFIX {
        return None;
    }
    let id = (u32::from(s[6]) << 16) | u32::from(s[7]);
    Some((Circuit(id), SocketAddr::new(IpAddr::V6(src), port)))
}

/// Reads the PROXY line off the connection if it starts with one (peeking first, so a plain HTTP
/// request is left untouched).
async fn read_header(stream: &mut TcpStream) -> Option<(Circuit, SocketAddr)> {
    let mut buf = [0u8; MAX_LINE];
    for _ in 0..200 {
        let n = stream.peek(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        let seen = &buf[..n];
        if !b"PROXY ".starts_with(&seen[..n.min(6)]) {
            return None;
        }
        if let Some(end) = seen.windows(2).position(|w| w == b"\r\n") {
            let mut line = vec![0u8; end + 2];
            stream.read_exact(&mut line).await.ok()?;
            return parse_line(std::str::from_utf8(&line[..end]).ok()?);
        }
        if n == MAX_LINE {
            return None;
        }
        // only part of the line is there yet
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    None
}

/// Accept loop for the Tor port.
pub async fn serve(listener: tokio::net::TcpListener, app: axum::Router) {
    loop {
        let (mut stream, remote) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("[TOR] accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if !crate::http::is_proxy_addr(remote.ip()) {
            tracing::warn!("[TOR] refused connection from {remote}: the Tor port only takes the Tor service");
            continue;
        }
        let app = app.clone();
        tokio::spawn(async move {
            let header = tokio::time::timeout(Duration::from_secs(5), read_header(&mut stream)).await.ok().flatten();
            let (circuit, addr) = match header {
                Some((c, a)) => (Some(c), a),
                None => (None, remote),
            };
            crate::serve_connection(stream, addr, circuit, app).await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tor_lines() {
        let (c, a) = parse_line("PROXY TCP6 fc00:dead:beef:4dad::1:2 ::1 65535 42").unwrap();
        assert_eq!(c, Circuit(0x0001_0002));
        assert_eq!(a.to_string(), "[fc00:dead:beef:4dad::1:2]:65535");
        assert_eq!(parse_line("PROXY TCP6 fc00:dead:beef:4dad::ffff:ffff ::1 1 80").unwrap().0, Circuit(u32::MAX));
        // not Tor's prefix, not TCP6, garbage
        assert!(parse_line("PROXY TCP6 2001:db8::1 ::1 1 80").is_none());
        assert!(parse_line("PROXY TCP4 10.0.0.1 10.0.0.2 1 80").is_none());
        assert!(parse_line("GET / HTTP/1.1").is_none());
        assert!(parse_line("PROXY TCP6 fc00:dead:beef:4dad::1:2 ::1 x 80").is_none());
    }
}
