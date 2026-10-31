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

/// scrypt parameters of the Node server's `$f0ck$` hashes (verification only).
const SCRYPT_LOG_N: u8 = 14; // Node default N=16384, r=8, p=1

fn scrypt_derive(password: &str, salt: &str) -> Option<Vec<u8>> {
    let params = scrypt::Params::new(SCRYPT_LOG_N, 8, 1, 64).ok()?;
    let mut out = vec![0u8; 64];
    scrypt::scrypt(password.as_bytes(), salt.as_bytes(), &params, &mut out).ok()?;
    Some(out)
}

/// Argon2id (default parameters), PHC string format.
pub async fn hash_password(password: String) -> String {
    tokio::task::spawn_blocking(move || {
        use argon2::password_hash::{PasswordHasher, SaltString};
        let salt = SaltString::generate(&mut rand::rngs::OsRng);
        argon2::Argon2::default().hash_password(password.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Verify a password against an Argon2 PHC string, or a scrypt `$f0ck$<salt>:<hex>` hash of the
/// Node server (kept for existing accounts; rehashed on login, see `password_needs_rehash`).
pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        if hash.starts_with("$argon2") {
            use argon2::password_hash::{PasswordHash, PasswordVerifier};
            let Ok(parsed) = PasswordHash::new(&hash) else { return false };
            return argon2::Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok();
        }
        if let Some(rest) = hash.strip_prefix("$f0ck$") {
            let Some((salt, key)) = rest.split_once(':') else { return false };
            let Ok(key) = hex::decode(key) else { return false };
            let Some(derived) = scrypt_derive(&password, salt) else { return false };
            use subtle::ConstantTimeEq;
            return !key.is_empty() && key.len() == derived.len() && bool::from(key.ct_eq(&derived));
        }
        false
    })
    .await
    .unwrap_or(false)
}

/// Whether a stored hash should be replaced by a fresh `hash_password` result after a successful login.
pub fn password_needs_rehash(hash: &str) -> bool {
    !hash.starts_with("$argon2id$")
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

// ── lib.mjs: tags & favorites (added with the feed port) ───────────────────

/// lib.userHasFavorited(session, favorites)
pub fn user_has_favorited(session: Option<&crate::session::Session>, favorites: &[Value]) -> bool {
    let Some(s) = session else { return false };
    if favorites.is_empty() {
        return false;
    }
    let sess_id = Some(s.id).filter(|&i| i != 0);
    let sess_user = s.user.to_lowercase();
    let sess_login = s.login.to_lowercase();
    let sess_anon = s.anon_login.as_deref().unwrap_or("").to_lowercase();
    favorites.iter().any(|f| {
        let f_id = crate::config::as_int(&f["user_id"]).filter(|&i| i != 0).or_else(|| crate::config::as_int(&f["id"]).filter(|&i| i != 0));
        if sess_id.is_some() && f_id.is_some() && f_id == sess_id {
            return true;
        }
        let f_user = f["user"].as_str().unwrap_or("").to_lowercase();
        let f_login = f["login"].as_str().unwrap_or("").to_lowercase();
        if !sess_user.is_empty() && sess_user != "anonymous" && (f_user == sess_user || f_login == sess_user) {
            return true;
        }
        if !sess_login.is_empty() && (f_user == sess_login || f_login == sess_login) {
            return true;
        }
        !sess_anon.is_empty() && (f_user == sess_anon || f_login == sess_anon)
    })
}

/// lib.getTags(itemid, session, subf0ckId) — the item's tags (rating first) with badge and
/// exclusion flags; tagger names only for regular sessions.
pub async fn get_tags(
    st: &crate::state::AppState,
    itemid: i64,
    session: Option<&crate::session::Session>,
    subf0ck_id: Option<&str>,
) -> anyhow::Result<Vec<Value>> {
    use crate::db::{RowExt, Sql};
    let cfg = st.cfg();
    let anonymized = crate::settings::is_anonymize_session(&cfg, session);
    let has_session = !anonymized && session.map(|s| !s.is_anon && (s.id != 0 || !s.user.is_empty())).unwrap_or(false);
    let itemid32 = itemid as i32;

    let mut album_item_id: Option<i32> = None;
    if let Some(sub) = subf0ck_id.filter(|s| !s.is_empty()) {
        if let Some(n) = sub.trim().parse::<i64>().ok().filter(|&n| n > 0) {
            album_item_id = crate::sql!(
                "SELECT id FROM album_items WHERE (id = ", n as i32, " OR (item_id = ", itemid32, " AND order_index = ", n as i32,
                ")) AND item_id = ", itemid32, " LIMIT 1"
            )
            .fetch_scalar(&st.db)
            .await?;
        }
        if album_item_id.is_none() {
            album_item_id = crate::sql!("SELECT id FROM album_items WHERE slug = ", sub, " AND item_id = ", itemid32, " LIMIT 1")
                .fetch_scalar(&st.db)
                .await?;
        }
    }

    const ORDER: &str = r#" order by (case when "tags".id = 1 then 0 when "tags".id = 2 then 1 when "tags".normalized = 'nsfl' then 2 else 3 end) asc"#;
    let cols = |assign: &str| -> Sql {
        let mut q = Sql::new(r#"select "tags".id, "tags".tag, "tags".normalized"#);
        if has_session {
            q.push(r#", "user".user, uo.display_name"#);
        }
        q.push(&format!(r#" from "{assign}" left join "tags" on "tags".id = "{assign}".tag_id "#));
        if has_session {
            q.push(&format!(r#"left join "user" on "user".id = "{assign}".user_id left join user_options uo on uo.user_id = "user".id "#));
        }
        q
    };
    let mut tags: Vec<crate::db::Row> = match album_item_id {
        Some(aid) => {
            let mut q = cols("album_items_tags_assign");
            q.push(r#"where "album_items_tags_assign".album_item_id = "#).bind(aid).push(ORDER).push(r#", "tags".id asc"#);
            let mut tags = q.fetch_all(&st.db).await?;
            let has_sub_rating = tags.iter().any(|t| {
                matches!(t.str("normalized"), Some("sfw" | "nsfw" | "nsfl")) || matches!(t.i64("id"), Some(1 | 2))
            });
            if !has_sub_rating {
                let mut q = cols("tags_assign");
                q.push(r#"where "tags_assign".item_id = "#)
                    .bind(itemid32)
                    .push(r#" and ("tags".id in (1, 2) or "tags".normalized in ('sfw', 'nsfw', 'nsfl'))"#)
                    .push(ORDER)
                    .push(" limit 1");
                if let Some(parent) = q.fetch_all(&st.db).await?.into_iter().next() {
                    tags.insert(0, parent);
                }
            }
            tags
        }
        None => {
            let mut q = cols("tags_assign");
            q.push(r#"where "tags_assign".item_id = "#).bind(itemid32).push(ORDER).push(r#", "tags".id asc"#);
            q.fetch_all(&st.db).await?
        }
    };

    let excluded: &[i64] = session.map(|s| s.excluded_tags.as_slice()).unwrap_or(&[]);
    let can_exclude = match session {
        Some(s) if !s.is_anon => true,
        _ => crate::settings::can_anon_do(&cfg, "exclude_tags"),
    };
    let mut has_rating = false;
    let mut out = Vec::with_capacity(tags.len());
    for mut t in tags.drain(..) {
        let is_rating = matches!(t.str("normalized"), Some("sfw" | "nsfw" | "nsfl"));
        if is_rating {
            if has_rating {
                continue;
            }
            has_rating = true;
        }
        let badge = get_badge(&cfg, t.str("tag").unwrap_or(""), t.str("normalized").unwrap_or(""));
        t.insert("badge".into(), json!(badge));
        let excl = !is_rating && t.i64("id").map(|id| excluded.contains(&id)).unwrap_or(false);
        t.insert("is_excluded".into(), json!(excl));
        t.insert("can_exclude".into(), json!(can_exclude));
        if !has_session {
            t.remove("user");
            t.remove("display_name");
        }
        out.push(Value::Object(t));
    }
    Ok(out)
}

#[cfg(test)]
mod password_tests {
    use super::*;

    #[tokio::test]
    async fn argon2_and_legacy_scrypt() {
        let h = hash_password("correct horse battery staple!".into()).await;
        assert!(h.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple!".into(), h.clone()).await);
        assert!(!verify_password("wrong".into(), h.clone()).await);
        assert!(!password_needs_rehash(&h));

        let salt = "0123456789abcdef";
        let legacy = format!("$f0ck${salt}:{}", hex::encode(scrypt_derive("pw", salt).unwrap()));
        assert!(verify_password("pw".into(), legacy.clone()).await);
        assert!(!verify_password("px".into(), legacy.clone()).await);
        assert!(password_needs_rehash(&legacy));

        // unsalted legacy hashes and disabled passwords never verify
        assert!(!verify_password("pw".into(), sha256("pw")).await);
        assert!(!verify_password("pw".into(), md5("pw")).await);
        assert!(!verify_password("".into(), "!".into()).await);
    }
}

/// `s.substring(0, n)` — the first `n` UTF-16 code units (a split surrogate pair is dropped).
pub fn utf16_prefix(s: &str, n: usize) -> String {
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        units += c.len_utf16();
        if units > n {
            break;
        }
        out.push(c);
    }
    out
}
