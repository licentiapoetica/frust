//! Minimal RFC 6455 websocket client (wss only) for Slack Socket Mode.
//!
//! TODO(port): replace with `tokio-tungstenite` once that crate is available offline; this is a
//! small, tested implementation of what we need: handshake, masked client frames, text/binary,
//! fragmentation, ping/pong and close.

use super::net::{self, BoxStream};
use anyhow::{Context, Result, bail};
use base64::Engine;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt, ReadHalf, WriteHalf};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const MAX_MESSAGE: usize = 16 * 1024 * 1024;

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BINARY: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// `Sec-WebSocket-Accept` for a key.
pub fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// Encode one final client frame (always masked).
pub fn encode_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut f = Vec::with_capacity(payload.len() + 14);
    f.push(0x80 | (opcode & 0x0F));
    let len = payload.len();
    if len < 126 {
        f.push(0x80 | len as u8);
    } else if len <= 0xFFFF {
        f.push(0x80 | 126);
        f.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        f.push(0x80 | 127);
        f.extend_from_slice(&(len as u64).to_be_bytes());
    }
    f.extend_from_slice(&mask);
    f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    f
}

pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

/// Read one frame. Server frames must not be masked.
pub async fn read_frame<R: tokio::io::AsyncRead + Unpin>(r: &mut R) -> Result<Frame> {
    let mut h = [0u8; 2];
    r.read_exact(&mut h).await?;
    let fin = h[0] & 0x80 != 0;
    let opcode = h[0] & 0x0F;
    if h[1] & 0x80 != 0 {
        bail!("masked frame from server");
    }
    let len = match h[1] & 0x7F {
        126 => r.read_u16().await? as u64,
        127 => r.read_u64().await?,
        n => n as u64,
    };
    if len > MAX_MESSAGE as u64 {
        bail!("frame too large ({len} bytes)");
    }
    let mut payload = vec![0u8; len as usize];
    r.read_exact(&mut payload).await?;
    Ok(Frame { fin, opcode, payload })
}

pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// Reads complete messages (reassembling fragments).
pub struct WsReader<R> {
    inner: R,
}

impl<R: tokio::io::AsyncRead + Unpin> WsReader<R> {
    pub fn new(inner: R) -> Self {
        WsReader { inner }
    }

    pub async fn next(&mut self) -> Result<WsMessage> {
        let mut buf: Vec<u8> = Vec::new();
        let mut first_op: Option<u8> = None;
        loop {
            let f = read_frame(&mut self.inner).await?;
            match f.opcode {
                OP_PING => return Ok(WsMessage::Ping(f.payload)),
                OP_PONG => return Ok(WsMessage::Pong),
                OP_CLOSE => return Ok(WsMessage::Close),
                OP_TEXT | OP_BINARY | OP_CONT => {
                    if f.opcode != OP_CONT {
                        first_op = Some(f.opcode);
                        buf.clear();
                    }
                    if buf.len() + f.payload.len() > MAX_MESSAGE {
                        bail!("message too large");
                    }
                    buf.extend_from_slice(&f.payload);
                    if f.fin {
                        return Ok(match first_op {
                            Some(OP_BINARY) => WsMessage::Binary(buf),
                            _ => WsMessage::Text(String::from_utf8_lossy(&buf).into_owned()),
                        });
                    }
                }
                op => bail!("unknown opcode {op}"),
            }
        }
    }
}

pub struct WsWriter {
    inner: WriteHalf<BoxStream>,
}

impl WsWriter {
    pub async fn send(&mut self, opcode: u8, payload: &[u8]) -> Result<()> {
        let mask: [u8; 4] = rand::random();
        self.inner.write_all(&encode_frame(opcode, payload, mask)).await?;
        self.inner.flush().await?;
        Ok(())
    }
}

/// Parse the handshake response head; returns the status code and lower-cased headers.
pub fn parse_response_head(head: &str) -> Result<(u16, Vec<(String, String)>)> {
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let code = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).context("bad status line")?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    Ok((code, headers))
}

/// Connect to a `wss://` URL and perform the opening handshake.
pub async fn connect(url: &str) -> Result<(WsReader<ReadHalf<BoxStream>>, WsWriter)> {
    let u = url::Url::parse(url).context("invalid websocket url")?;
    if u.scheme() != "wss" {
        bail!("only wss:// is supported");
    }
    let host = u.host_str().context("websocket url without host")?.to_string();
    let port = u.port().unwrap_or(443);
    let mut path = u.path().to_string();
    if let Some(q) = u.query() {
        path.push('?');
        path.push_str(q);
    }
    let mut stream = net::connect(&host, port, true, false).await?;
    let key = base64::engine::general_purpose::STANDARD.encode(rand::random::<[u8; 16]>());
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
    );
    stream.write_all(req.as_bytes()).await?;
    // read the response head byte-wise so no frame data is consumed
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        if head.len() > 16 * 1024 {
            bail!("handshake response too large");
        }
        let b = stream.read_u8().await.context("handshake")?;
        head.push(b);
    }
    let (code, headers) = parse_response_head(&String::from_utf8_lossy(&head))?;
    if code != 101 {
        bail!("websocket handshake failed with HTTP {code}");
    }
    let accept = headers.iter().find(|(k, _)| k == "sec-websocket-accept").map(|(_, v)| v.as_str());
    if accept != Some(accept_key(&key).as_str()) {
        bail!("invalid Sec-WebSocket-Accept");
    }
    let (rd, wr) = tokio::io::split(stream);
    Ok((WsReader::new(rd), WsWriter { inner: wr }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc_accept_key() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn encodes_masked_frames() {
        let f = encode_frame(OP_TEXT, b"Hello", [0x37, 0xfa, 0x21, 0x3d]);
        // RFC 6455 5.7 example
        assert_eq!(f, vec![0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]);
        let big = vec![0u8; 300];
        let f = encode_frame(OP_BINARY, &big, [0; 4]);
        assert_eq!(&f[..4], &[0x82, 0xFE, 0x01, 0x2C]);
        assert_eq!(f.len(), 4 + 4 + 300);
        let huge = vec![0u8; 70_000];
        let f = encode_frame(OP_BINARY, &huge, [0; 4]);
        assert_eq!(f[1], 0xFF);
        assert_eq!(&f[2..10], &70_000u64.to_be_bytes());
    }

    #[tokio::test]
    async fn reads_fragmented_messages() {
        // "Hel" (text, not fin) + ping + "lo" (cont, fin)
        let data: Vec<u8> = vec![0x01, 0x03, b'H', b'e', b'l', 0x89, 0x00, 0x80, 0x02, b'l', b'o'];
        let mut r = WsReader::new(&data[..]);
        assert!(matches!(r.next().await.unwrap(), WsMessage::Ping(p) if p.is_empty()));
        assert!(matches!(r.next().await.unwrap(), WsMessage::Text(t) if t == "Hello"));
        // 16-bit length + close
        let mut data = vec![0x81, 126, 0x00, 0x80];
        data.extend(std::iter::repeat_n(b'a', 128));
        data.extend([0x88, 0x00]);
        let mut r = WsReader::new(&data[..]);
        assert!(matches!(r.next().await.unwrap(), WsMessage::Text(t) if t.len() == 128));
        assert!(matches!(r.next().await.unwrap(), WsMessage::Close));
        // masked server frames are refused
        let data = vec![0x81, 0x81, 0, 0, 0, 0, b'a'];
        assert!(WsReader::new(&data[..]).next().await.is_err());
    }

    #[test]
    fn parses_handshake_head() {
        let (code, h) = parse_response_head("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: abc=\r\n\r\n").unwrap();
        assert_eq!(code, 101);
        assert!(h.contains(&("sec-websocket-accept".into(), "abc=".into())));
    }
}
