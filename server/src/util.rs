//! General helpers (port of src/inc/lib.mjs, minus the route guards which live in http::guard).

use crate::config::Config;
use rand::RngCore;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#039;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand::rngs::OsRng.fill_bytes(&mut b);
    b
}

pub fn random_hex(n: usize) -> String {
    hex::encode(random_bytes(n))
}

/// Random slug from `[a-zA-Z0-9_-]`.
pub fn generate_slug(len: usize) -> String {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_-";
    random_bytes(len).iter().map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// `crypto.randomBytes(16).toString("hex") + Date.now().toString(24)`
pub fn create_id() -> String {
    format!("{}{}", random_hex(16), to_radix(now_ms() as u64, 24))
}

pub fn generate_token() -> String {
    random_hex(32)
}

fn to_radix(mut n: u64, radix: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if n == 0 {
        return "0".into();
    }
    let mut v = Vec::new();
    while n > 0 {
        v.push(DIGITS[(n % radix) as usize]);
        n /= radix;
    }
    v.reverse();
    String::from_utf8(v).unwrap_or_default()
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Seconds since the epoch (`~~(Date.now() / 1e3)`).
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn sha256(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

pub fn sha256_bytes(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

pub fn md5(s: &str) -> String {
    use md5::Md5;
    hex::encode(Md5::digest(s.as_bytes()))
}

pub fn hmac_sha256_hex(key: &[u8], data: &[u8]) -> String {
    use hmac::{Hmac, Mac};
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(data);
    hex::encode(mac.finalize().into_bytes())
}

/// Constant-time string comparison.
pub fn ct_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// JavaScript `Number.prototype.toFixed(2) * 1` style: round to 2 decimals, drop trailing zeros.
pub fn js_num(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.fract() == 0.0 && f.abs() < 1e21 {
        return format!("{}", f as i64);
    }
    let s = format!("{f}");
    s
}

pub fn format_size_f(size: f64) -> String {
    if size.is_nan() || size <= 0.0 {
        return "0 B".into();
    }
    let i = ((size.ln() / 1024f64.ln()).floor() as i32).clamp(0, 4);
    let v = size / 1024f64.powi(i);
    let rounded = (v * 100.0).round() / 100.0;
    format!("{} {}", js_num(rounded), ["B", "kB", "MB", "GB", "TB"][i as usize])
}

/// lib.formatSize — accepts numbers, numeric strings, or already formatted sizes.
pub fn format_size(v: &Value) -> String {
    match v {
        Value::Null => "0 B".into(),
        Value::Number(n) => format_size_f(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                return "0 B".into();
            }
            static RE: once_cell::sync::Lazy<regex::Regex> =
                once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)^\d+(\.\d+)?\s*(B|kB|KB|MB|GB|TB)$").unwrap());
            if RE.is_match(t) {
                return t.to_string();
            }
            t.parse::<f64>().map(format_size_f).unwrap_or_else(|_| "0 B".into())
        }
        _ => "0 B".into(),
    }
}

pub fn calc_speed(bytes: f64, secs: f64) -> f64 {
    ((bytes * 8.0 / secs / 1e6) * 1e4).round() / 1e4
}

/// lib.timeAgo
pub fn time_ago(i18n: &crate::i18n::Translator, seconds_ago: i64) -> String {
    const EPOCHS: [(&str, i64); 6] =
        [("year", 31536000), ("month", 2592000), ("day", 86400), ("hour", 3600), ("minute", 60), ("second", 1)];
    for (name, secs) in EPOCHS {
        let interval = seconds_ago / secs;
        if interval >= 1 {
            let key = if interval == 1 { format!("timeago.{name}") } else { format!("timeago.{name}s") };
            let t = i18n.t_with(&key, &[("n", &interval.to_string())]);
            return i18n.t_with("timeago.ago", &[("t", &t)]);
        }
    }
    i18n.t("timeago.just_now")
}

/// lib.expiresIn
pub fn expires_in(expires_at: &Value) -> Option<String> {
    let n = crate::config::as_int(expires_at)?;
    let diff = n - now_secs();
    if diff <= 0 {
        return Some("expiring now".into());
    }
    let plural = |n: i64| if n == 1 { "" } else { "s" };
    Some(if diff < 60 {
        format!("in {diff}s")
    } else if diff < 3600 {
        let m = diff / 60;
        format!("in {m} minute{}", plural(m))
    } else if diff < 86400 {
        let h = diff / 3600;
        let m = (diff % 3600) / 60;
        if m > 0 { format!("in {h}h {m}m") } else { format!("in {h} hour{}", plural(h)) }
    } else {
        let d = diff / 86400;
        let h = (diff % 86400) / 3600;
        if h > 0 { format!("in {d}d {h}h") } else { format!("in {d} day{}", plural(d)) }
    })
}

/// lib.getMode — SQL condition for a feed mode. Only ever built from integers.
pub fn get_mode(cfg: &Config, mode: i64) -> String {
    let nsfl = cfg.nsfl_tag_id();
    match mode {
        1 => "items.id in (select item_id from tags_assign where tag_id = 2)".into(),
        2 => format!("not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl}))"),
        3 => "1 = 1".into(),
        4 => {
            if cfg.enable_nsfl() {
                format!("items.id in (select item_id from tags_assign where tag_id = {nsfl})")
            } else {
                "1 = 0".into()
            }
        }
        _ => format!(
            "(items.id in (select item_id from tags_assign where tag_id = 1) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))"
        ),
    }
}

/// lib.getMultiRatingMode
pub fn get_multi_rating_mode(cfg: &Config, ratings: &[String]) -> Option<String> {
    let valid = ["sfw", "nsfw", "nsfl", "untagged"];
    let f: Vec<&str> = ratings.iter().map(String::as_str).filter(|r| valid.contains(r)).collect();
    if f.is_empty() {
        return None;
    }
    let has = |r: &str| f.contains(&r);
    if has("sfw") && has("nsfw") && has("untagged") && (!cfg.enable_nsfl() || has("nsfl")) {
        return Some("1 = 1".into());
    }
    let nsfl = cfg.nsfl_tag_id();
    let mut parts = Vec::new();
    if has("sfw") {
        parts.push("items.id in (select item_id from tags_assign where tag_id = 1)".to_string());
    }
    if has("nsfw") {
        parts.push("items.id in (select item_id from tags_assign where tag_id = 2)".to_string());
    }
    if has("nsfl") && cfg.enable_nsfl() {
        parts.push(format!("items.id in (select item_id from tags_assign where tag_id = {nsfl})"));
    }
    if has("untagged") {
        parts.push(format!("not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl}))"));
    }
    if parts.is_empty() { None } else { Some(format!("({})", parts.join(" OR "))) }
}

/// JavaScript `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
    const SET: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')');
    utf8_percent_encode(s, SET).to_string()
}

/// JavaScript `encodeURI`.
pub fn encode_uri(s: &str) -> String {
    use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
    const SET: &AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'!')
        .remove(b'~')
        .remove(b'*')
        .remove(b'\'')
        .remove(b'(')
        .remove(b')')
        .remove(b';')
        .remove(b',')
        .remove(b'/')
        .remove(b'?')
        .remove(b':')
        .remove(b'@')
        .remove(b'&')
        .remove(b'=')
        .remove(b'+')
        .remove(b'$')
        .remove(b'#');
    utf8_percent_encode(s, SET).to_string()
}

/// JavaScript `decodeURIComponent`; `None` on malformed input.
pub fn decode_uri_component(s: &str) -> Option<String> {
    percent_encoding::percent_decode_str(s).decode_utf8().ok().map(|c| c.into_owned())
}

/// Lenient decode: returns the input on failure.
pub fn decode_lossy(s: &str) -> String {
    decode_uri_component(s).unwrap_or_else(|| s.to_string())
}

/// lib.genLink
pub struct LinkEnv<'a> {
    pub tag: Option<&'a str>,
    pub hall: Option<&'a str>,
    pub user: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub path: Option<&'a str>,
    pub strict: bool,
    pub tagger: Option<&'a str>,
}

pub fn gen_link(env: &LinkEnv) -> Value {
    let mut link: Vec<String> = Vec::new();
    if let Some(t) = env.tag {
        link.push("tag".into());
        link.push(encode_uri_component(t));
    }
    if let Some(h) = env.hall {
        link.push("h".into());
        link.push(encode_uri_component(h));
    }
    if let Some(u) = env.user {
        link.push("user".into());
        link.push(encode_uri_component(u));
        link.push(env.kind.unwrap_or("uploads").to_string());
    } else if env.kind == Some("favs") {
        link.push("favs".into());
    }
    let mut tmp = if link.is_empty() { "/".to_string() } else { link.join("/") };
    if !tmp.ends_with('/') {
        tmp.push('/');
    }
    if !tmp.starts_with('/') {
        tmp.insert(0, '/');
    }
    let mut suffix = if env.strict { "?strict=1".to_string() } else { String::new() };
    if let Some(tagger) = env.tagger.filter(|t| !t.is_empty()) {
        suffix.push(if suffix.is_empty() { '?' } else { '&' });
        suffix.push_str("tagger=");
        suffix.push_str(&encode_uri_component(tagger));
    }
    let display = decode_uri_component(&tmp).unwrap_or_else(|| tmp.clone());
    json!({ "main": tmp, "mainDisplay": display, "path": env.path.unwrap_or(""), "suffix": suffix })
}

pub fn slugify(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

/// Escape ILIKE wildcards in user input.
pub fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

// ── Password hashing ────────────────────────────────────────────────────────

const SCRYPT_LOG_N: u8 = 14; // Node default N=16384, r=8, p=1

fn scrypt_derive(password: &str, salt: &str) -> Option<Vec<u8>> {
    let params = scrypt::Params::new(SCRYPT_LOG_N, 8, 1, 64).ok()?;
    let mut out = vec![0u8; 64];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).ok()?;
    Some(out)
}

/// `$f0ck$<salt>:<scrypt hex>` — compatible with existing hashes.
pub async fn hash_password(password: String) -> String {
    tokio::task::spawn_blocking(move || {
        let salt = random_hex(16);
        let key = scrypt_derive(&password, &salt).unwrap_or_default();
        format!("$f0ck${salt}:{}", hex::encode(key))
    })
    .await
    .unwrap_or_default()
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        if let Some(rest) = hash.strip_prefix("$f0ck$") {
            let Some((salt, key)) = rest.split_once(':') else { return false };
            let Ok(key) = hex::decode(key) else { return false };
            let Some(derived) = scrypt_derive(&password, salt) else { return false };
            use subtle::ConstantTimeEq;
            return key.len() == derived.len() && bool::from(key.ct_eq(&derived));
        }
        // Legacy unsalted hashes
        if hash.len() == 32 {
            return ct_eq(&md5(&password), &hash);
        }
        if hash.len() == 64 {
            return ct_eq(&sha256(&password), &hash);
        }
        false
    })
    .await
    .unwrap_or(false)
}

/// lib.getBadge
pub fn get_badge(cfg: &Config, tag: &str, normalized: &str) -> &'static str {
    static CYR: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new("[а-яё]").unwrap());
    if tag.starts_with('>') {
        "badge-greentext badge-light"
    } else if normalized == "ukraine" {
        "badge-ukraine badge-light"
    } else if CYR.is_match(normalized) || normalized == "russia" {
        "badge-russia badge-light"
    } else if normalized == "german" {
        "badge-german badge-light"
    } else if normalized == "dutch" {
        "badge-dutch badge-light"
    } else if normalized == "sfw" {
        "badge-success"
    } else if normalized == "nsfw" {
        "badge-danger"
    } else if normalized == "nsfl" {
        if cfg.enable_nsfl() { "badge-nsfl" } else { "badge-light" }
    } else {
        "badge-light"
    }
}

/// Error reference for logs, returned to clients instead of internals.
pub fn log_error(err: &dyn std::fmt::Display, context: &str) -> String {
    let id = uuid::Uuid::new_v4();
    tracing::error!("[ERROR REF {id}] {context}: {err}");
    format!("Internal Error. Reference: {id}")
}

/// JSON safe to embed inside `<script>` and quoted HTML attributes: escapes `<`, `>`, `&`, `'`
/// and line separators (all only ever occur inside JSON strings, where `\uXXXX` is equivalent).
pub fn json_for_script(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "null".into());
    s.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\'', "\\u0027")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// `JSON.stringify` (plain).
pub fn json_string(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}
