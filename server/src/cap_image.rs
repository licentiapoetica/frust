//! Image captcha for visitors without JavaScript: Cap (cap.rs) is a proof of work that runs in the
//! browser, so the login / register modals show this instead when JavaScript is off.
//!
//! The page carries a random id (`nojs_captcha_id`, tpl.rs) in a hidden `cap-image-id` field and
//! in the URL of a small frame, GET /api/cap/image/<id>/frame (views/cap-image-frame.html), which
//! shows GET /api/cap/image/<id>. That picks the characters for the id on first request and draws
//! them; the form sends them back as `cap-image-answer`, checked once by [`check`] (through
//! `cap::verify_form`). Only the answer and a drawing seed are kept, in memory, so asking for the
//! same id again gives the same picture (no averaging over many distortions).
//!
//! Clicking the picture loads the frame with `?new`, which drops the id's characters, so the image
//! comes with new ones. Only the frame reloads: what the visitor typed in the form stays, and the
//! form's id stays valid.
//!
//! The frame only loads without JavaScript: it sits in `.lm-imgcap`, hidden (f0ckm.css) when
//! JavaScript runs, and is `loading="lazy"`, which browsers don't fetch while hidden.
//!
//! IPs with repeated failed logins / registrations (`security::needs_hard_captcha`) get 7
//! characters instead of 5 and need such an image where a hard Cap challenge is needed.

use crate::cap::Captcha;
use crate::http::{Req, Res, Router, raw};
use crate::state::AppState;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use std::sync::atomic::{AtomicI64, Ordering};

const TTL_MS: i64 = 10 * 60 * 1000;
/// Upper bound on open images, so a flood can't grow memory without limit.
const MAX_PENDING: usize = 20_000;
/// New images one IP may request per minute.
const IMAGES_PER_IP_MINUTE: u32 = 20;
/// No look-alikes (0/O, 1/I/L, 2/Z, 5/S, 6/G, 8/B)
const CHARS: &[u8] = b"ACDEFHJKMNPRTUVWXY345679";
const W: usize = 340;
const H: usize = 100;

struct Pending {
    answer: String,
    seed: u64,
    hard: bool,
    expires: i64,
}

static PENDING: Lazy<DashMap<String, Pending>> = Lazy::new(DashMap::new);
static IP_HITS: Lazy<DashMap<String, (i64, u32)>> = Lazy::new(DashMap::new);
static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);

pub fn register(r: &mut Router) {
    r.get(r"^\/api\/cap\/image\/(?<id>[0-9a-f]{16,64})\/?$", image);
    r.get(r"^\/api\/cap\/image\/(?<id>[0-9a-f]{16,64})\/frame\/?$", frame);
}

/// The picture as a link to itself with `?new` (see the module docs).
async fn frame(st: AppState, req: Req) -> Res {
    if !crate::cap::enabled(&st.cfg()) {
        return Ok(raw(404, ""));
    }
    let id = req.param("id").unwrap_or("").to_string();
    if req.query("new").is_some() {
        PENDING.remove(&id);
    }
    // In both URLs, so neither the frame nor the picture comes from the browser's cache
    let v = crate::util::random_hex(4);
    let mut r = crate::http::html(crate::tpl::render(&st, Some(&req), "cap-image-frame", serde_json::json!({ "id": id, "v": v })));
    r.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(r)
}

/// A fresh id for a page's image captcha.
pub fn new_id() -> String {
    crate::util::random_hex(16)
}

/// Check the image captcha of a form once (the entry is used up either way). `need_hard`: only an
/// image drawn for a hard captcha passes.
pub fn check(id: Option<&str>, answer: Option<&str>, need_hard: bool) -> Captcha {
    let answer: String = answer.unwrap_or("").chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let Some(id) = id.map(str::trim).filter(|i| !i.is_empty()) else { return Captcha::Missing };
    if answer.is_empty() {
        return Captcha::Missing;
    }
    match PENDING.remove(id) {
        Some((_, p)) if p.expires > crate::util::now_ms() && (p.hard || !need_hard) && crate::util::ct_eq(&p.answer, &answer) => Captcha::Ok,
        _ => Captcha::Failed,
    }
}

async fn image(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !crate::cap::enabled(&cfg) {
        return Ok(raw(404, ""));
    }
    let id = req.param("id").unwrap_or("").to_string();
    let now = crate::util::now_ms();
    sweep(now);
    let existing = PENDING.get(&id).filter(|p| p.expires > now).map(|p| (p.answer.clone(), p.seed));
    let (answer, seed) = match existing {
        Some(a) => a,
        None => {
            if rate_limited(&req.real_ip(), now) || PENDING.len() >= MAX_PENDING {
                return Ok(raw(429, ""));
            }
            let hard = crate::security::needs_hard_captcha(&st, &req.real_ip()).await;
            let bytes = crate::util::random_bytes(if hard { 7 } else { 5 } + 8);
            let (chars, seed) = bytes.split_at(bytes.len() - 8);
            let answer: String = chars.iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect();
            let seed = u64::from_le_bytes(seed.try_into().unwrap_or([7; 8])) | 1;
            PENDING.insert(id, Pending { answer: answer.clone(), seed, hard, expires: now + TTL_MS });
            (answer, seed)
        }
    };
    let mut r = raw(200, encode_png(&draw(&answer, seed), W, H));
    let h = r.headers_mut();
    h.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("image/png"));
    h.insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(r)
}

fn sweep(now: i64) {
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if now - last < 60_000 || LAST_SWEEP.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    PENDING.retain(|_, p| p.expires > now);
    IP_HITS.retain(|_, (start, _)| now - *start < 60_000);
}

fn rate_limited(ip: &str, now: i64) -> bool {
    let mut e = IP_HITS.entry(ip.to_string()).or_insert((now, 0));
    if now - e.0 >= 60_000 {
        *e = (now, 0);
    }
    e.1 += 1;
    e.1 > IMAGES_PER_IP_MINUTE
}

// ── Drawing ─────────────────────────────────────────────────────────────────

/// xorshift64*, seeded per image (drawing only; the answer comes from the OS RNG)
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// 0.0 .. 1.0
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// 5x7 bitmaps of [`CHARS`], rows top to bottom, bit 4 = left column
fn glyph(c: u8) -> [u8; 7] {
    match c {
        b'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'C' => [0x0f, 0x10, 0x10, 0x10, 0x10, 0x10, 0x0f],
        b'D' => [0x1e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1e],
        b'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        b'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        b'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        b'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        b'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        b'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        b'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1b, 0x11],
        b'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        b'Y' => [0x11, 0x11, 0x0a, 0x04, 0x04, 0x04, 0x04],
        b'3' => [0x1e, 0x01, 0x01, 0x0e, 0x01, 0x01, 0x1e],
        b'4' => [0x11, 0x11, 0x11, 0x1f, 0x01, 0x01, 0x01],
        b'5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        b'6' => [0x0e, 0x10, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        b'7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x01, 0x0e],
        _ => [0; 7],
    }
}

/// Grayscale pixels (W x H): light, rotated, wobbling characters with crossing curves and specks
/// on a noisy dark ground.
fn draw(text: &str, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed);
    let mut px: Vec<u8> = (0..W * H).map(|_| 18 + (rng.next() % 14) as u8).collect();
    let plot = |px: &mut Vec<u8>, x: i32, y: i32, v: u8| {
        if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
            px[y as usize * W + x as usize] = v;
        }
    };

    let wave_amp = rng.range(3.0, 6.0);
    let wave_freq = rng.range(0.03, 0.06);
    let wave_phase = rng.range(0.0, 6.28);
    let n = text.len().max(1);
    let step = (W as f32 - 40.0) / n as f32;
    for (i, c) in text.bytes().enumerate() {
        let bits = glyph(c);
        let cx = 20.0 + step * (i as f32 + 0.5) + rng.range(-4.0, 4.0);
        let cy = H as f32 / 2.0 + rng.range(-10.0, 10.0);
        let angle = rng.range(-0.45, 0.45);
        let (sin, cos) = angle.sin_cos();
        let sx = rng.range(6.0, 7.5);
        let sy = rng.range(6.5, 8.0);
        let shade = 190 + (rng.next() % 60) as u8;
        for y in (cy as i32 - 45)..(cy as i32 + 45) {
            for x in (cx as i32 - 40)..(cx as i32 + 40) {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy + wave_amp * (x as f32 * wave_freq + wave_phase).sin();
                let u = (dx * cos + dy * sin) / sx + 2.5;
                let v = (-dx * sin + dy * cos) / sy + 3.5;
                // Thickened a little, so diagonal strokes (cells touching at corners) stay joined
                let on = |u: f32, v: f32| u >= 0.0 && v >= 0.0 && (u as usize) < 5 && (v as usize) < 7 && bits[v as usize] & (0x10 >> u as usize) != 0;
                const R: f32 = 0.3;
                if on(u, v) || on(u - R, v - R) || on(u + R, v - R) || on(u - R, v + R) || on(u + R, v + R) {
                    plot(&mut px, x, y, shade);
                }
            }
        }
    }

    // Curves through the text, about as bright as it
    for _ in 0..3 {
        let (amp, freq, phase) = (rng.range(8.0, 25.0), rng.range(0.01, 0.04), rng.range(0.0, 6.28));
        let base = rng.range(25.0, H as f32 - 25.0);
        let tilt = rng.range(-0.15, 0.15);
        let shade = 150 + (rng.next() % 80) as u8;
        for x in 0..W as i32 {
            let y = base + tilt * x as f32 + amp * (x as f32 * freq + phase).sin();
            plot(&mut px, x, y as i32, shade);
            plot(&mut px, x, y as i32 + 1, shade);
        }
    }
    // Specks
    for _ in 0..W * H / 25 {
        let (x, y) = ((rng.next() % W as u64) as i32, (rng.next() % H as u64) as i32);
        let v = if rng.next() % 2 == 0 { 30 + (rng.next() % 40) as u8 } else { 120 + (rng.next() % 100) as u8 };
        plot(&mut px, x, y, v);
    }
    px
}

// ── PNG (8-bit grayscale, uncompressed deflate: ~34 KB, no image crate needed) ──

fn encode_png(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut rows = Vec::with_capacity((w + 1) * h);
    for row in px.chunks(w) {
        rows.push(0); // filter: none
        rows.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = rows.chunks(65_535).collect();
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let len = b.len() as u16;
        z.extend_from_slice(&len.to_le_bytes());
        z.extend_from_slice(&(!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&rows).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(w as u32).to_be_bytes());
    ihdr.extend_from_slice(&(h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 0, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [(b"IHDR", &ihdr[..]), (b"IDAT", &z[..]), (b"IEND", &[][..])] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let crc = crc32(&out[start..]);
        out.extend_from_slice(&crc.to_be_bytes());
    }
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &d in data {
        a = (a + d as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &d in data {
        crc ^= d as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_checksums() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
        let png = encode_png(&draw("AC3", 42), W, H);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(png.ends_with(&[0xae, 0x42, 0x60, 0x82]));
    }

    #[test]
    fn same_seed_same_picture() {
        assert_eq!(draw("AC3", 42), draw("AC3", 42));
        assert_ne!(draw("AC3", 42), draw("AC3", 43));
    }

    #[test]
    fn answer_works_once() {
        let exp = crate::util::now_ms() + 60_000;
        PENDING.insert("aa".into(), Pending { answer: "AC3K7".into(), seed: 1, hard: false, expires: exp });
        assert!(matches!(check(Some("aa"), Some(" ac3 k7 "), false), Captcha::Ok));
        assert!(matches!(check(Some("aa"), Some("AC3K7"), false), Captcha::Failed));
        PENDING.insert("bb".into(), Pending { answer: "AC3K7".into(), seed: 1, hard: false, expires: exp });
        assert!(matches!(check(Some("bb"), Some("AC3K7"), true), Captcha::Failed));
        assert!(matches!(check(Some("cc"), Some(""), false), Captcha::Missing));
        assert!(matches!(check(None, Some("X"), false), Captcha::Missing));
    }

    #[test]
    fn every_char_has_a_glyph() {
        assert!(CHARS.iter().all(|c| glyph(*c).iter().any(|r| *r != 0)));
    }

    /// `F0CKM_DUMP_CAPTCHA=out.png cargo test cap_image` writes a sample
    #[test]
    fn dump_sample() {
        if let Ok(out) = std::env::var("F0CKM_DUMP_CAPTCHA") {
            std::fs::write(out, encode_png(&draw("KX7MP", 99), W, H)).unwrap();
        }
    }
}
