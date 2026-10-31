//! Small helpers the social routes need from modules other agents are porting.
//!
//! TODO(port): `compute_base_mode`, `resolve_mime_sql` and `resolve_numeric_item_id` duplicate
//! f0cklib.mjs helpers (feed port), `audit_log` is `audit.mjs`, `apply_word_filter` is
//! `wordfilter.mjs`, `log_anon_activity`/`resolve_audit_ip` are from `anon_auth.mjs`.
//! Once those modules exist, replace these with calls to them.

use crate::config::Config;
use crate::db::{RowExt, Sql};
use crate::http::Req;
use crate::session::Session;
use crate::settings::{anon_allowed_mimes, anon_allowed_modes, can_anon_do, is_anon_session, session_owner_name};
use crate::sql;
use crate::state::AppState;
use serde_json::Value;

/// `resolveNumericItemId` — numeric id, or the id behind an item/album slug.
pub async fn resolve_numeric_item_id(st: &AppState, id_or_slug: &str) -> Option<i64> {
    if id_or_slug.is_empty() {
        return None;
    }
    if id_or_slug.bytes().all(|b| b.is_ascii_digit()) {
        return id_or_slug.parse::<i64>().ok().filter(|n| *n != 0);
    }
    let row = sql!("SELECT id FROM items WHERE slug = ", id_or_slug, " LIMIT 1").fetch_optional(&st.db).await.ok()?;
    if let Some(id) = row.as_ref().and_then(|r| r.i64("id")) {
        return Some(id);
    }
    let row = sql!("SELECT item_id FROM album_items WHERE slug = ", id_or_slug, " LIMIT 1").fetch_optional(&st.db).await.ok()?;
    row.and_then(|r| r.i64("item_id")).filter(|n| *n != 0)
}

/// `audit.log(userId, action, targetType, targetId, details)` — errors are only logged.
pub async fn audit_log(st: &AppState, user_id: i64, action: &str, target_type: &str, target_id: &str, details: Option<Value>) {
    let details = details.unwrap_or_else(|| Value::Object(Default::default()));
    let q = sql!(
        "INSERT INTO audit_log (user_id, action, target_type, target_id, details) VALUES (",
        user_id as i32,
        ", ",
        action,
        ", ",
        target_type,
        ", ",
        target_id,
        ", ",
        details,
        ")"
    );
    match q.execute(&st.db).await {
        Ok(_) => tracing::info!("[AUDIT] User {user_id} performed {action} on {target_type}:{target_id}"),
        Err(e) => tracing::error!("[AUDIT] Failed to write log: {e}"),
    }
}

/// `applyWordFilter(content)` — case-preserving replacements from the `wordfilter` table.
pub async fn apply_word_filter(st: &AppState, content: &str) -> String {
    if content.is_empty() {
        return String::new();
    }
    let filters = match sql!("SELECT word, replacement FROM wordfilter").fetch_all(&st.db).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("[WORDFILTER] Error applying filter: {e}");
            return content.to_string();
        }
    };
    let mut filtered = content.to_string();
    for f in filters {
        let word = f.string("word");
        let replacement = f.string("replacement");
        if word.is_empty() {
            continue;
        }
        let Ok(re) = regex::RegexBuilder::new(&regex::escape(&word)).case_insensitive(true).build() else { continue };
        filtered = re
            .replace_all(&filtered, |caps: &regex::Captures| {
                let m = &caps[0];
                let upper = m.to_uppercase();
                let lower = m.to_lowercase();
                if m == upper && m != lower {
                    return replacement.to_uppercase();
                }
                let first: String = m.chars().take(1).collect();
                if first == first.to_uppercase() && first != first.to_lowercase() {
                    let mut chars = replacement.chars();
                    return match chars.next() {
                        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    };
                }
                if m == lower {
                    return replacement.to_lowercase();
                }
                replacement.clone()
            })
            .into_owned();
    }
    filtered
}

/// `resolveAuditIP(req)`
pub fn resolve_audit_ip(cfg: &Config, req: &Req) -> Option<String> {
    crate::security::storable_ip(cfg, &req.real_ip())
}

/// `logAnonActivity(req, { action, targetId, details })`
pub async fn log_anon_activity(st: &AppState, req: &Req, action: &str, target_id: Option<i64>, details: Option<Value>) {
    let cfg = st.cfg();
    let Some(s) = req.session() else { return };
    if s.id == 0 {
        return;
    }
    let raw_ip = req.real_ip();
    let ip = crate::security::storable_ip(&cfg, &raw_ip);
    let fingerprint = s.fingerprint.clone().or_else(|| s.anon_fingerprint.clone());
    let target: Option<i32> = target_id.filter(|t| *t != 0).and_then(|t| i32::try_from(t).ok());
    let mut q = Sql::new("INSERT INTO anon_activity_log (user_id, fingerprint, hw_fingerprint, ip, action, target_id, details) VALUES (");
    q.bind(s.id as i32).push(", ").bind(fingerprint).push(", ").bind(None::<String>).push(", ").bind(ip).push(", ").bind(action).push(", ").bind(target).push(", ");
    match details {
        Some(d) => q.bind(d),
        None => q.push("NULL"),
    };
    q.push(")");
    if let Err(e) = q.execute(&st.db).await {
        tracing::error!("[ANON_ACTIVITY_LOG] Failed to log activity: {e}");
        return;
    }
    crate::security::log_user_ip(st, s.id, &raw_ip).await;
}

fn has_user(session: Option<&Session>) -> bool {
    session.map(|s| !s.user.is_empty()).unwrap_or(false)
}

/// `f0cklib.computeBaseMode(mode, ratings, session)` — SQL condition (built only from integers
/// and the validated shadow-login name).
pub fn compute_base_mode(cfg: &Config, mode: Option<i64>, ratings: Option<&[String]>, session: Option<&Session>) -> String {
    let mut eff_mode = mode.unwrap_or(0);
    let ratings_arr: Option<Vec<String>> = ratings.filter(|r| !r.is_empty()).map(<[String]>::to_vec);
    let mut safe = ratings_arr;
    let nsfl = cfg.nsfl_tag_id();
    let mode_names = crate::settings::MODE_NAMES;

    if !has_user(session) {
        eff_mode = 0;
        safe = Some(vec!["sfw".into()]);
    } else if is_anon_session(session) {
        let allowed = anon_allowed_modes(cfg);
        if !can_anon_do(cfg, "filter") {
            safe = None;
            eff_mode = 0;
        } else if let Some(s) = safe.take() {
            let f: Vec<String> = s.into_iter().filter(|r| allowed.contains(r)).collect();
            safe = if f.is_empty() {
                if allowed.len() == 1 && !allowed.iter().any(|m| m == "all") {
                    Some(vec![allowed[0].clone()])
                } else if allowed.len() < 5 {
                    Some(allowed.clone())
                } else {
                    None
                }
            } else {
                Some(f)
            };
        }
        let current = mode_names.get(eff_mode as usize).copied().unwrap_or("sfw");
        if eff_mode == 3 {
            if allowed.len() < 5 {
                safe = Some(allowed.clone());
            }
        } else if !allowed.iter().any(|m| m == current) {
            let fallback = allowed.first().map(String::as_str).unwrap_or("sfw");
            eff_mode = mode_names.iter().position(|m| *m == fallback).map(|p| p as i64).unwrap_or(0);
        }
    }

    let multi = |r: &Option<Vec<String>>| r.as_ref().and_then(|r| crate::util::get_multi_rating_mode(cfg, r));
    let mut base = if eff_mode == 2 {
        match &safe {
            Some(r) if r.iter().any(|x| x == "untagged") && r.len() > 1 => multi(&safe).unwrap_or_default(),
            _ => crate::util::get_mode(cfg, 2),
        }
    } else if eff_mode == 3 {
        match &safe {
            Some(r) if r.len() > 1 => multi(&safe).unwrap_or_default(),
            _ => crate::util::get_mode(cfg, 3),
        }
    } else {
        multi(&safe).unwrap_or_else(|| crate::util::get_mode(cfg, eff_mode))
    };

    match session {
        None => {
            let few = safe.as_ref().map(|r| r.len() <= 1).unwrap_or(true);
            let public_nsfw = cfg.truthy("websrv.public_nsfw");
            let public_untagged = cfg.truthy("websrv.public_untagged");
            if (eff_mode == 0 || eff_mode == 3 || mode.is_none()) && few {
                base = match (public_nsfw, public_untagged) {
                    (true, true) => format!("(items.id in (select item_id from tags_assign where tag_id in (1, 2)) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))"),
                    (true, false) => "items.id in (select item_id from tags_assign where tag_id in (1, 2))".into(),
                    (false, true) => format!("(items.id in (select item_id from tags_assign where tag_id = 1) or not exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl})))"),
                    (false, false) => "items.id in (select item_id from tags_assign where tag_id = 1)".into(),
                };
            } else if eff_mode == 2 {
                if !public_untagged {
                    base = "items.id in (select item_id from tags_assign where tag_id = 1)".into();
                }
            } else if eff_mode == 1 {
                if !public_nsfw {
                    base = "items.id in (select item_id from tags_assign where tag_id = 1)".into();
                }
            } else if eff_mode == 4 {
                base = "1 = 0".into();
            }
        }
        Some(_) if is_anon_session(session) => {
            let allowed = anon_allowed_modes(cfg);
            let has = |m: &str| allowed.iter().any(|x| x == m);
            if !has("nsfl") {
                base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = {nsfl})");
            }
            if !has("nsfw") {
                base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = 2)");
            }
            if !has("untagged") {
                base = format!("({base}) and exists (select 1 from tags_assign where item_id = items.id and tag_id in (1, 2, {nsfl}))");
            }
            if !has("sfw") {
                base = format!("({base}) and not exists (select 1 from tags_assign where item_id = items.id and tag_id = 1)");
            }
            if let Some(owner) = session_owner_name(session) {
                let plain = !owner.is_empty() && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
                if !has("untagged") && plain {
                    base = format!(
                        "(({base}) or items.id in (select own.id from items own where lower(own.username) = '{}' and not exists (select 1 from tags_assign ota where ota.item_id = own.id and ota.tag_id in (1, 2, {nsfl}))))",
                        owner.to_lowercase()
                    );
                }
            }
        }
        _ => {}
    }
    base
}

/// `f0cklib.resolveMimeSQL(rawMime, session, alias)` — returns the `and (...)` fragment (or empty).
pub fn resolve_mime_sql(cfg: &Config, raw_mime: Option<&str>, session: Option<&Session>, alias: &str) -> Sql {
    const KNOWN: [&str; 5] = ["video", "audio", "image", "flash", "pdf"];
    let mut parts: Vec<String> = raw_mime.unwrap_or("").split(',').filter(|m| KNOWN.contains(m)).map(str::to_string).collect();
    if is_anon_session(session) {
        let allowed = anon_allowed_mimes(cfg);
        if !can_anon_do(cfg, "filter") || allowed.len() == 1 {
            parts = if allowed.len() < 5 { allowed.clone() } else { vec![] };
        } else if !parts.is_empty() {
            parts.retain(|m| allowed.contains(m));
            if parts.is_empty() {
                parts = if allowed.len() < 5 { allowed.clone() } else { vec![] };
            }
        } else if allowed.len() < 5 {
            parts = allowed.clone();
        }
    }
    let mut q = Sql::empty();
    if parts.is_empty() {
        return q;
    }
    let col = if alias == "i" { "i.mime" } else { "items.mime" };
    let flash_mimes: Vec<String> = cfg.mimes().iter().filter(|(_, ext)| ext.as_str() == Some("swf")).map(|(m, _)| m.clone()).collect();
    q.push("and (");
    for (i, m) in parts.iter().enumerate() {
        if i > 0 {
            q.push(" or ");
        }
        match m.as_str() {
            "flash" => {
                if flash_mimes.is_empty() {
                    q.push("false");
                } else {
                    for (j, fm) in flash_mimes.iter().enumerate() {
                        if j > 0 {
                            q.push(" or ");
                        }
                        q.push(col).push(" = ").bind(fm.clone());
                    }
                }
            }
            "pdf" => {
                q.push(col).push(" = 'application/pdf'");
            }
            other => {
                q.push(col).push(" ilike ").bind(format!("{other}/%"));
            }
        }
    }
    q.push(")");
    q
}

/// The nsfp filter used by the comment lists (`globalfilter` in comments.mjs): returns tag ids.
pub fn comment_globalfilter_tags(st: &AppState, session: Option<&Session>) -> Vec<i64> {
    let cfg = st.cfg();
    let mut tags = st.settings.nsfp_ids();
    if session.is_some() && is_anon_session(session) {
        let allowed = anon_allowed_modes(&cfg);
        if allowed.iter().any(|m| m == "nsfw") {
            tags.retain(|t| *t != 2);
        }
        let nsfl = cfg.nsfl_tag_id();
        if allowed.iter().any(|m| m == "nsfl") {
            tags.retain(|t| *t != nsfl);
        }
    } else if cfg.truthy("websrv.public_nsfw") {
        tags.retain(|t| *t != 2);
    }
    tags
}

/// `ratings` cookie → list of valid ratings (`null` when the cookie is missing).
pub fn ratings_from_cookie(req: &Req) -> Option<Vec<String>> {
    let raw = req.cookie("ratings").filter(|r| !r.is_empty())?;
    let decoded = crate::util::decode_lossy(raw);
    Some(
        decoded
            .split(['|', ','])
            .filter(|r| ["sfw", "nsfw", "nsfl", "untagged"].contains(r))
            .map(str::to_string)
            .collect(),
    )
}

/// `/* <mode-override> */`: `?mode=0..3` beats the session mode.
pub fn mode_override(req: &Req) -> i64 {
    match req.query("mode") {
        Some(m @ ("0" | "1" | "2" | "3")) => m.parse().unwrap_or(req.mode),
        _ => req.mode,
    }
}

/// `pg_notify(channel, payload)` — errors are logged (the old code didn't await these).
pub async fn notify(st: &AppState, channel: &str, payload: &Value) {
    let text = crate::util::json_string(payload);
    if let Err(e) = sql!("select pg_notify(", channel, ", ", text, ")").execute(&st.db).await {
        tracing::error!("[NOTIFY] {channel}: {e}");
    }
}

/// `Number(x)` for JSON values coming from postgres.js rows (strings for bigints).
pub fn num(v: &Value) -> Option<i64> {
    crate::config::as_int(v)
}

/// A request-body value as JS would stringify it (`undefined`/`null` → `None`).
pub fn js_str(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(crate::util::js_num(n.as_f64().unwrap_or(0.0))),
        Value::Array(a) => Some(a.iter().map(|x| js_str(Some(x)).unwrap_or_default()).collect::<Vec<_>>().join(",")),
        Value::Object(_) => Some("[object Object]".into()),
    }
}

/// JS truthiness of an optional body value.
pub fn js_truthy(v: Option<&Value>) -> bool {
    v.map(crate::config::truthy).unwrap_or(false)
}

/// `parseInt(s, 10)` — leading whitespace, optional sign, then digits; `None` for NaN.
pub fn js_parse_int(s: &str) -> Option<i64> {
    let t = s.trim_start();
    let (neg, rest) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    Some(if neg { -n } else { n })
}

/// `parseFloat(s)` — longest numeric prefix; `None` for NaN.
pub fn js_parse_float(s: &str) -> Option<f64> {
    let t = s.trim_start();
    let b = t.as_bytes();
    let mut end = 0;
    let mut best = None;
    while end < b.len() {
        end += 1;
        if let Ok(f) = t[..end].parse::<f64>() {
            if !t[..end].chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') {
                best = Some(f);
            }
        }
    }
    if best.is_none() && t.starts_with("Infinity") {
        return Some(f64::INFINITY);
    }
    best
}

/// `+s` (Number(s)) for query strings: `None` for NaN.
pub fn js_number(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return Some(0.0);
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(h, 16).ok().map(|n| n as f64);
    }
    if t.chars().any(|c| c.is_ascii_alphabetic() && c != 'e' && c != 'E') && t != "Infinity" && t != "-Infinity" {
        return None;
    }
    match t {
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => t.parse::<f64>().ok(),
    }
}

/// A value usable as SQL LIMIT/OFFSET (non-negative integer); `None` where postgres would fail.
pub fn sql_count(f: Option<f64>) -> Option<i64> {
    let f = f?;
    (f.is_finite() && f.fract() == 0.0 && f >= 0.0).then_some(f as i64)
}
