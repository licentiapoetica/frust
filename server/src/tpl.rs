//! Template engine (MiniJinja) and page rendering.
//!
//! Templates in views/ use Jinja syntax and are HTML-autoescaped. Values print with JavaScript
//! semantics so output matches the old engine: `null`/`undefined` print nothing, whole floats
//! print without `.0`, arrays print comma-joined.
//!
//! Helpers available in templates (besides the MiniJinja builtins and Python-style methods):
//!   functions: t(key, data?), mtime(path), now(), floor(x), ceil(x), max(a, b), min(a, b),
//!              parse_int(x), encode_uri_component(x)
//!   filters:   locale_string(lang?), locale_date(lang?), locale_time(lang?), locale_number,
//!              format_size, fixed(n), encode_uri_component, encode_uri, time_ago,
//!              expires_in, js (JS string literal), b64

use crate::config::Config;
use crate::http::Req;
use crate::i18n::I18n;
use crate::state::AppState;
use chrono::{DateTime, Local, TimeZone};
use minijinja::value::Rest;
use minijinja::{AutoEscape, Environment, Error, ErrorKind, Output, State, UndefinedBehavior, Value as JValue};
use parking_lot::RwLock;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct Templates {
    env: RwLock<Environment<'static>>,
    dirty: Arc<AtomicBool>,
    _watcher: Option<notify::RecommendedWatcher>,
}

fn write_escaped(out: &mut Output, s: &str) -> std::fmt::Result {
    let mut last = 0;
    for (i, c) in s.char_indices() {
        let rep = match c {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&#39;",
            _ => continue,
        };
        out.write_str(&s[last..i])?;
        out.write_str(rep)?;
        last = i + 1;
    }
    out.write_str(&s[last..])
}

/// JavaScript `String(value)` for template values.
pub fn js_to_string(v: &JValue) -> String {
    use minijinja::value::ValueKind;
    match v.kind() {
        ValueKind::Undefined | ValueKind::None => String::new(),
        ValueKind::Bool => if v.is_true() { "true".into() } else { "false".into() },
        ValueKind::Number => {
            if let Some(i) = v.as_i64() {
                return i.to_string();
            }
            let f = f64::try_from(v.clone()).unwrap_or(f64::NAN);
            crate::util::js_num(f)
        }
        ValueKind::String => v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
        ValueKind::Seq | ValueKind::Iterable => match v.try_iter() {
            Ok(it) => it.map(|x| js_to_string(&x)).collect::<Vec<_>>().join(","),
            Err(_) => String::new(),
        },
        ValueKind::Map => "[object Object]".into(),
        _ => v.to_string(),
    }
}

fn js_formatter(out: &mut Output, state: &State, value: &JValue) -> Result<(), Error> {
    if value.is_undefined() || value.is_none() {
        return Ok(());
    }
    let s = js_to_string(value);
    let r = if value.is_safe() || state.auto_escape() == AutoEscape::None { out.write_str(&s) } else { write_escaped(out, &s) };
    r.map_err(|_| Error::new(ErrorKind::WriteFailure, "write failed"))
}

/// Parse a JS `new Date(x)` argument (ms number, ISO string, date string).
fn to_datetime(v: &JValue) -> Option<DateTime<Local>> {
    if let Some(s) = v.as_str() {
        if let Ok(d) = DateTime::parse_from_rfc3339(s) {
            return Some(d.with_timezone(&Local));
        }
        if let Ok(n) = s.trim().parse::<f64>() {
            return Local.timestamp_millis_opt(n as i64).single();
        }
        for fmt in ["%Y-%m-%d %H:%M:%S%.f%#z", "%Y-%m-%dT%H:%M:%S%.f"] {
            if let Ok(d) = DateTime::parse_from_str(s, fmt) {
                return Some(d.with_timezone(&Local));
            }
            if let Ok(nd) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
                return Local.from_local_datetime(&nd).single();
            }
        }
        if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Some(nd.and_hms_opt(0, 0, 0)?.and_utc().with_timezone(&Local));
        }
        return None;
    }
    let ms = f64::try_from(v.clone()).ok()?;
    Local.timestamp_millis_opt(ms as i64).single()
}

fn locale_of(lang: Option<&str>) -> &'static str {
    match lang.map(|l| l.split(['-', '_']).next().unwrap_or("").to_lowercase()) {
        Some(l) if l == "de" => "de",
        Some(l) if l == "nl" => "nl",
        _ => "en",
    }
}

/// `Date.prototype.toLocaleDateString(lang)`
pub fn locale_date_str(d: &DateTime<Local>, lang: Option<&str>) -> String {
    match locale_of(lang) {
        "de" => d.format("%-d.%-m.%Y").to_string(),
        "nl" => d.format("%-d-%-m-%Y").to_string(),
        _ => d.format("%-m/%-d/%Y").to_string(),
    }
}

/// `Date.prototype.toLocaleTimeString(lang)`
pub fn locale_time_str(d: &DateTime<Local>, lang: Option<&str>) -> String {
    match locale_of(lang) {
        "de" | "nl" => d.format("%H:%M:%S").to_string(),
        _ => d.format("%-I:%M:%S %p").to_string(),
    }
}

/// `Date.prototype.toLocaleString(lang)`
pub fn locale_string_str(d: &DateTime<Local>, lang: Option<&str>) -> String {
    format!("{}, {}", locale_date_str(d, lang), locale_time_str(d, lang))
}

/// `Number.prototype.toLocaleString()` (en-US grouping, up to 3 fraction digits).
pub fn locale_number(f: f64) -> String {
    let neg = f < 0.0;
    let rounded = (f.abs() * 1000.0).round() / 1000.0;
    let int = rounded.trunc() as u64;
    let frac = rounded - rounded.trunc();
    let digits = int.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    if frac > 0.0 {
        let f = format!("{frac:.3}");
        grouped.push_str(f.trim_start_matches('0').trim_end_matches('0'));
    }
    if neg { format!("-{grouped}") } else { grouped }
}

fn opt_lang(state: &State, lang: Option<String>) -> Option<String> {
    lang.or_else(|| state.lookup("__date_lang").and_then(|v| v.as_str().map(str::to_string)))
}

fn build_env(views: PathBuf, i18n: Arc<I18n>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader(views));
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(|name| if name.ends_with(".html") { AutoEscape::Html } else { AutoEscape::None });
    env.set_formatter(js_formatter);
    env.set_keep_trailing_newline(true);
    minijinja_contrib::add_to_environment(&mut env);
    env.set_unknown_method_callback(minijinja_contrib::pycompat::unknown_method_callback);

    // t(key, data?) — translation in the page language. Translations are trusted HTML;
    // interpolated values are escaped.
    let i18n_t = i18n.clone();
    env.add_function("t", move |state: &State, key: String, data: Option<JValue>| -> Result<JValue, Error> {
        let lang = state.lookup("lang").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "en".into());
        let tr = i18n_t.translator(&lang);
        let mut repl = Map::new();
        if let Some(d) = data.filter(|d| d.kind() == minijinja::value::ValueKind::Map) {
            if let Ok(keys) = d.try_iter() {
                for k in keys {
                    let v = d.get_item(&k).unwrap_or_default();
                    let s = js_to_string(&v);
                    let s = if v.is_safe() { s } else { crate::util::escape_html(&s) };
                    repl.insert(js_to_string(&k), Value::String(s));
                }
            }
        }
        match tr.t_value(&key, Some(&repl)) {
            Value::String(s) => Ok(JValue::from_safe_string(s)),
            other => Ok(JValue::from_serialize(&other)),
        }
    });

    env.add_function("mtime", |path: String| -> i64 {
        let p = std::env::current_dir().unwrap_or_default().join(path.trim_start_matches('/'));
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });
    env.add_function("now", || crate::util::now_ms());
    env.add_function("floor", |x: f64| x.floor() as i64);
    env.add_function("ceil", |x: f64| x.ceil() as i64);
    env.add_function("max", |a: JValue, b: JValue| if f64::try_from(a.clone()).unwrap_or(f64::NAN) >= f64::try_from(b.clone()).unwrap_or(f64::NAN) { a } else { b });
    env.add_function("min", |a: JValue, b: JValue| if f64::try_from(a.clone()).unwrap_or(f64::NAN) <= f64::try_from(b.clone()).unwrap_or(f64::NAN) { a } else { b });
    env.add_function("parse_int", |v: JValue| -> JValue {
        let s = js_to_string(&v);
        match crate::config::as_int(&Value::String(s)) {
            Some(n) => JValue::from(n),
            None => JValue::from(f64::NAN),
        }
    });
    env.add_function("encode_uri_component", |v: JValue| crate::util::encode_uri_component(&js_to_string(&v)));

    env.add_filter("encode_uri_component", |v: JValue| crate::util::encode_uri_component(&js_to_string(&v)));
    env.add_filter("encode_uri", |v: JValue| crate::util::encode_uri(&js_to_string(&v)));
    env.add_filter("format_size", |v: JValue| {
        let j: Value = serde_json::to_value(&v).unwrap_or(Value::Null);
        crate::util::format_size(&j)
    });
    env.add_filter("fixed", |v: JValue, n: Option<usize>| {
        let f = f64::try_from(v).unwrap_or(f64::NAN);
        format!("{:.*}", n.unwrap_or(0), f)
    });
    env.add_filter("locale_number", |v: JValue| locale_number(f64::try_from(v).unwrap_or(0.0)));
    env.add_filter("locale_string", |state: &State, v: JValue, lang: Option<String>| {
        to_datetime(&v).map(|d| locale_string_str(&d, opt_lang(state, lang).as_deref())).unwrap_or_else(|| "Invalid Date".into())
    });
    env.add_filter("locale_date", |state: &State, v: JValue, lang: Option<String>| {
        to_datetime(&v).map(|d| locale_date_str(&d, opt_lang(state, lang).as_deref())).unwrap_or_else(|| "Invalid Date".into())
    });
    env.add_filter("locale_time", |state: &State, v: JValue, lang: Option<String>| {
        to_datetime(&v).map(|d| locale_time_str(&d, opt_lang(state, lang).as_deref())).unwrap_or_else(|| "Invalid Date".into())
    });
    env.add_filter("iso_date", |v: JValue| to_datetime(&v).map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()).unwrap_or_default());
    let i18n_ago = i18n.clone();
    env.add_filter("time_ago", move |state: &State, v: JValue| {
        let lang = state.lookup("lang").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_else(|| "en".into());
        let Some(d) = to_datetime(&v) else { return String::new() };
        let secs = (chrono::Utc::now().timestamp_millis() - d.timestamp_millis()) / 1000;
        crate::util::time_ago(&i18n_ago.translator(&lang), secs)
    });
    env.add_filter("expires_in", |v: JValue| {
        let j: Value = serde_json::to_value(&v).unwrap_or(Value::Null);
        crate::util::expires_in(&j).unwrap_or_default()
    });
    // JS string literal (for inline scripts)
    env.add_filter("js", |v: JValue| JValue::from_safe_string(crate::util::json_for_script(&Value::String(js_to_string(&v)))));
    env.add_filter("b64", |v: JValue| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(js_to_string(&v))
    });
    // `@each(x as v)` -> `{% for key, v in x|entries %}`: (index, value) for lists, (key, value) for maps.
    env.add_filter("entries", |v: JValue| -> Vec<JValue> {
        use minijinja::value::ValueKind;
        match v.kind() {
            ValueKind::Map => v
                .try_iter()
                .map(|it| it.map(|k| JValue::from(vec![k.clone(), v.get_item(&k).unwrap_or_default()])).collect())
                .unwrap_or_default(),
            ValueKind::Seq | ValueKind::Iterable => v
                .try_iter()
                .map(|it| it.enumerate().map(|(i, x)| JValue::from(vec![JValue::from(i), x])).collect())
                .unwrap_or_default(),
            _ => vec![],
        }
    });
    env.add_filter("values_of", |v: JValue| -> Vec<JValue> {
        v.try_iter().map(|it| it.map(|k| v.get_item(&k).unwrap_or_default()).collect()).unwrap_or_default()
    });
    env.add_filter("year", |v: JValue| to_datetime(&v).map(|d| chrono::Datelike::year(&d) as i64).unwrap_or(0));
    // Variadic helper kept for templates that join several values.
    env.add_function("concat", |args: Rest<JValue>| args.iter().map(js_to_string).collect::<String>());
    env
}

impl Templates {
    /// Compile every template; returns (file, error) for each failure.
    pub fn check_all(&self, views: &std::path::Path) -> Vec<(String, String)> {
        let env = self.env.read();
        let mut errors = Vec::new();
        let mut stack = vec![views.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|x| x == "html").unwrap_or(false) {
                    let rel = p.strip_prefix(views).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                    if let Err(err) = env.get_template(&rel) {
                        let mut msg = format!("{err}");
                        if let Some(d) = err.display_debug_info().to_string().lines().find(|l| l.contains(">")) {
                            msg.push_str(&format!(" | {}", d.trim()));
                        }
                        errors.push((rel, msg));
                    }
                }
            }
        }
        errors.sort();
        errors
    }

    pub fn new(views: PathBuf, i18n: Arc<I18n>) -> Templates {
        let dirty = Arc::new(AtomicBool::new(false));
        let watcher = {
            use notify::Watcher;
            let d = dirty.clone();
            let w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    d.store(true, Ordering::Relaxed);
                }
            });
            match w {
                Ok(mut w) => match w.watch(&views, notify::RecursiveMode::Recursive) {
                    Ok(()) => Some(w),
                    Err(e) => {
                        tracing::warn!("[TPL] cannot watch {}: {e}", views.display());
                        None
                    }
                },
                Err(e) => {
                    tracing::warn!("[TPL] cannot watch views: {e}");
                    None
                }
            }
        };
        Templates { env: RwLock::new(build_env(views, i18n)), dirty, _watcher: watcher }
    }

    /// Render `views/<name>.html`. Errors are logged and rendered as a short message.
    pub fn render(&self, name: &str, ctx: &Value) -> String {
        if self.dirty.swap(false, Ordering::Relaxed) {
            self.env.write().clear_templates();
        }
        let env = self.env.read();
        let file = if name.ends_with(".html") { name.to_string() } else { format!("{name}.html") };
        let result = env.get_template(&file).and_then(|t| t.render(ctx));
        match result {
            Ok(s) => collapse_blank_lines(&s),
            Err(e) => {
                let mut msg = format!("{e}");
                let mut src = std::error::Error::source(&e);
                while let Some(s) = src {
                    msg.push_str(&format!(": {s}"));
                    src = s.source();
                }
                tracing::error!("[TPL] {file}: {msg}");
                format!("{} in {file}", crate::util::escape_html(&msg))
            }
        }
    }
}

/// flummpress: `.replace(/\n\s*\n/g, "\n")`
fn collapse_blank_lines(s: &str) -> String {
    static RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"\n\s*\n").unwrap());
    RE.replace_all(s, "\n").into_owned()
}

// ── Page rendering (globals + per-request values) ───────────────────────────

fn list_dir(dir: &std::path::Path, re: &str) -> Vec<String> {
    let re = regex::Regex::new(re).expect("static regex");
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| re.is_match(f)).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn json_script(v: &Value) -> String {
    crate::util::json_for_script(v)
}

/// Site-wide template values (the old `tpl.globals`).
pub fn globals(st: &AppState, cfg: &Config) -> Map<String, Value> {
    let s = st.settings.get();
    let w = |k: &str| cfg.get(&format!("websrv.{k}")).clone();
    let mimes = cfg.mimes();
    let mut exts: Vec<String> = Vec::new();
    for v in mimes.values() {
        if let Some(e) = v.as_str() {
            if !exts.iter().any(|x| x == e) {
                exts.push(e.to_string());
            }
        }
    }
    let allowed_mimes = mimes.keys().cloned().chain(exts.iter().map(|e| format!(".{e}"))).collect::<Vec<_>>().join(",");
    let b64 = |t: &str| {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(t)
    };
    let onara_cfg_val = if cfg.get("onara").is_null() { cfg.get("websrv.onara").clone() } else { cfg.get("onara").clone() };
    let privacy_ip_mode = if !cfg.truthy("websrv.log_user_ips") {
        "off"
    } else if cfg.truthy("websrv.hash_user_ips") {
        "hashed"
    } else {
        "raw"
    };
    let default_upload_visibility = cfg
        .get("default_upload_visibility")
        .as_i64()
        .or_else(|| cfg.get("websrv.default_upload_visibility").as_i64())
        .unwrap_or(0);
    let scroller_mime_cats: Vec<Value> = match cfg.get("allowedMimes").as_array() {
        Some(a) => a.iter().filter(|c| matches!(c.as_str(), Some("video" | "image" | "audio"))).cloned().collect(),
        None => vec![json!("video"), json!("image"), json!("audio")],
    };
    let audio_tuner = [cfg.get("audio_tuner"), cfg.get("audio_visualizer"), cfg.get("websrv.audio_tuner")]
        .into_iter()
        .find(|v| crate::config::truthy(v))
        .cloned()
        .unwrap_or(Value::Null);
    let koepfe = {
        let files = list_dir(&cfg.paths.koepfe, r"(?i)\.(png|jpe?g|gif|webp|avif)$");
        if cfg.paths.koepfe.exists() && !files.is_empty() {
            Value::from(files.into_iter().map(|f| format!("/s/koepfe/{f}")).collect::<Vec<_>>())
        } else {
            match cfg.get("websrv.koepfe") {
                Value::Array(a) => Value::Array(a.clone()),
                _ => json!([]),
            }
        }
    };
    let brand = s.brand_image_url.clone();
    let brand_images = if brand.is_empty() {
        json!([])
    } else {
        match cfg.get("websrv.custom_brand_image") {
            Value::Array(a) if a.len() > 1 && a.iter().any(|x| x.as_str() == Some(&brand)) => Value::Array(a.clone()),
            _ => json!([brand]),
        }
    };
    let fonts: Vec<Value> = list_dir(&cfg.paths.fonts, r"(?i)\.(ttf|otf|woff2?)$")
        .into_iter()
        .map(|f| json!({ "name": f.split('.').next().unwrap_or(""), "file": f }))
        .collect();
    let thumb = cfg.str_or("websrv.default_thumb_size", "").to_lowercase();
    let allowed_comment_images = match w("allowed_comment_images") {
        Value::Array(a) => Value::Array(a),
        _ => json!([]),
    };

    let mut g = Map::new();
    let mut put = |k: &str, v: Value| {
        g.insert(k.to_string(), v);
    };
    put("lul", w("lul"));
    put("themes", w("themes"));
    put("default_theme", json!(cfg.str_or("websrv.theme", "f0ck")));
    put("modes", cfg.get("allowedModes").clone());
    put("api", w("api"));
    put("site", cfg.get("main.url.full").clone());
    put("domain", cfg.get("main.url.domain").clone());
    put("hide_comments_from_public", cfg.get("main.hide_comments_from_public").clone());
    put("guest_anonymize", json!(cfg.truthy("main.guest_anonymize")));
    put("anon_anonymize", json!(false));
    put("is_anonymized", json!(false));
    put("git_hash", json!(st.git_hash));
    put("motd", json!(s.motd));
    put("manual_approval", json!(s.manual_approval));
    put("min_tags", json!(s.min_tags));
    put("registration_open", json!(st.settings.registration_open(cfg)));
    put("hw_fingerprint_enabled", json!(crate::settings::hw_fingerprint_enabled(cfg)));
    put("privacy_ip_mode", json!(privacy_ip_mode));
    put("registration_web_toggle_enabled", json!(cfg.enabled("websrv.open_registration_web_toggle")));
    put("registration_require_mail_andor_token", json!(cfg.truthy("websrv.open_registration_require_mail_andor_token")));
    put("trusted_uploads", json!(s.trusted_uploads));
    put("shitpost_mode", json!(crate::settings::shitpost_mode(cfg)));
    put("shitpost_require_rating", json!(cfg.truthy("websrv.shitpost_require_rating")));
    put("shitpost_min_tags", json!(cfg.int("websrv.shitpost_min_tags").unwrap_or(0)));
    put("about_text", json!(s.about_text));
    put("rules_text", json!(s.rules_text));
    put("terms_text", json!(s.terms_text));
    put("about_text_b64", json!(b64(&s.about_text)));
    put("rules_text_b64", json!(b64(&s.rules_text)));
    put("terms_text_b64", json!(b64(&s.terms_text)));
    put("halls", st.halls());
    put("halls_enabled", json!(cfg.enabled("websrv.halls_enabled")));
    put("userhalls_enabled", json!(cfg.enabled("websrv.userhalls_enabled")));
    put("enable_userhall_image_upload", json!(cfg.enabled("websrv.enable_userhall_image_upload")));
    put("enable_oc", json!(cfg.enabled("websrv.enable_oc")));
    put("abyss_enabled", json!(cfg.enabled("websrv.abyss_enabled")));
    put("smtp_enabled", json!(cfg.truthy("smtp") && cfg.truthy("smtp.enabled") && cfg.truthy("smtp.mail_reset_password")));
    put("recaptcha_enabled", json!(cfg.truthy("recaptcha.enabled") && cfg.truthy("recaptcha.site_key")));
    put("recaptcha_site_key", json!(cfg.str_or("recaptcha.site_key", "")));
    put("show_background_cfg", json!(cfg.enabled("websrv.background")));
    put("allowed_mimes", json!(allowed_mimes));
    put("mimes_json", json!(json_script(&Value::Object(mimes))));
    put("development", json!(cfg.development()));
    put("show_mime_picker", json!(cfg.enabled("websrv.show_mime_picker")));
    put("private_society", if cfg.truthy("websrv.private_society") { w("private_society") } else { json!(false) });
    put("private_society_gate", json!(cfg.str_or("websrv.private_society_gate", "")));
    put("private_society_gate_location", json!(cfg.str_or("websrv.private_society_gate_location", "Frankfurt")));
    put("show_content_warning", json!(cfg.enabled("websrv.show_content_warning")));
    put("web_url_upload", json!(cfg.truthy("websrv.web_url_upload")));
    put("enable_youtube_upload", json!(cfg.enabled("websrv.enable_youtube_upload")));
    put("upload_limit", if cfg.get("main.upload_limit").is_null() { json!(69) } else { cfg.get("main.upload_limit").clone() });
    put("meme_creator", json!(cfg.truthy("websrv.meme_creator")));
    put("custom_favicon", json!(cfg.str_or("websrv.custom_favicon", "")));
    put("custom_brand_image", json!(brand));
    put("custom_navbar_brand_text", json!(cfg.str_or("websrv.custom_navbar_brand_text", "")));
    put("default_font", json!(cfg.str_or("websrv.default_font", "")));
    put("site_description", json!(cfg.str_or("websrv.description", "The webs dumpster")));
    put("enable_nsfl", json!(cfg.enable_nsfl()));
    put("enable_comments", json!(cfg.enabled("enable_comments")));
    put("public_nsfw", json!(cfg.truthy("websrv.public_nsfw")));
    put("public_untagged", json!(cfg.truthy("websrv.public_untagged")));
    put("onara", json!(crate::config::truthy(&onara_cfg_val)));
    put("onara_cfg", Value::Null);
    put("is_onara_item", json!(false));
    put("can_extract_meta", json!(false));
    put("enable_private_uploads", json!(cfg.enabled("enable_private_uploads")));
    put("enable_expiring_uploads", json!(crate::settings::enable_expiring_uploads(cfg)));
    put("enable_item_slugs", json!(crate::settings::enable_item_slugs(cfg)));
    put("enable_anonymous_access", json!(crate::settings::enable_anonymous_access(cfg)));
    let perms = Value::Object(crate::settings::anon_permissions(cfg));
    put("anon_permissions_json", json!(json_script(&perms)));
    put("anon_permissions", perms);
    put("default_upload_visibility", json!(default_upload_visibility));
    put("allow_user_upload_visibility", json!(cfg.enabled("allow_user_upload_visibility") && cfg.enabled("websrv.allow_user_upload_visibility")));
    put("nsfl_tag_id", json!(cfg.int("nsfl_tag_id").filter(|n| *n != 0).unwrap_or(3)));
    put("scroller_mime_cats", Value::Array(scroller_mime_cats));
    put("themes_json", json!(json_script(&match w("themes") {
        Value::Null => json!([]),
        v => v,
    })));
    put("audio_tuner_json", json!(json_script(&audio_tuner)));
    put("enable_profile_description", json!(cfg.truthy("websrv.enable_profile_description")));
    put("expose_repost_links_to_guests", json!(cfg.truthy("websrv.expose_repost_links_to_guests") || cfg.truthy("websrv.expose_repost_links")));
    put("private_messages", json!(s.private_messages));
    put("dm_attachments", json!(s.dm_attachments));
    put("dm_unencrypted", json!(s.dm_unencrypted));
    put("allow_comment_deletion", json!(crate::settings::allow_comment_deletion(cfg)));
    put("enable_pdf", json!(s.enable_pdf));
    put("enable_cleanup", json!(st.settings.enable_cleanup(cfg)));
    put("cleanup_start_date", json!(s.cleanup_start_date));
    put("cleanup_end_date", json!(s.cleanup_end_date));
    put("cleanup_include_engaged", json!(s.cleanup_include_engaged));
    put("matrix_enabled", json!(cfg.client_enabled("matrix")));
    put("telegram_enabled", json!(cfg.client_enabled("tg")));
    put("ts", json!(crate::util::now_ms()));
    put("default_layout", json!(s.default_layout));
    put("show_koepfe", json!(cfg.truthy("websrv.show_koepfe")));
    put("hide_sidebar_default", json!(cfg.truthy("websrv.hide_sidebar_default")));
    put("allow_language_change", json!(cfg.enabled("websrv.allow_language_change")));
    put("enable_xd_score", json!(cfg.truthy("websrv.enable_xd_score")));
    put("enable_dynamic_thumbs", json!(cfg.truthy("websrv.enable_dynamic_thumbs")));
    put("default_thumb_size", json!(if ["s", "m", "l", "xl"].contains(&thumb.as_str()) { thumb.as_str() } else { "m" }));
    put("comment_max_length", cfg.get("main.comment_max_length").clone());
    put("enable_swf", json!(cfg.truthy("websrv.enable_swf")));
    put("enable_archive", json!(cfg.truthy("websrv.enable_archive")));
    put("enable_danmaku", json!(cfg.enabled("websrv.enable_danmaku")));
    put("enable_item_title", json!(cfg.enabled("websrv.enable_item_title")));
    put("enable_global_chat", json!(cfg.truthy("websrv.enable_global_chat")));
    put("embed_youtube_in_comments", json!(cfg.enabled("websrv.embed_youtube_in_comments")));
    put("koepfe_json", json!(json_script(&koepfe)));
    put("custom_brand_images_json", json!(json_script(&brand_images)));
    put("allowed_comment_images_json", json!(json_script(&allowed_comment_images)));
    put("allowed_comment_images", allowed_comment_images);
    put("paths_images", json!(cfg.str_or("websrv.paths.images", "/b")));
    put("default_comment_display_mode", if crate::config::truthy(&w("default_comment_display_mode")) { w("default_comment_display_mode") } else { json!(0) });
    put("allow_fileupload_comments", if crate::config::truthy(&w("allow_fileupload_comments")) { w("allow_fileupload_comments") } else { json!(false) });
    put("fileupload_comments_multifile", if crate::config::truthy(&w("fileupload_comments_multifile")) { w("fileupload_comments_multifile") } else { json!(false) });
    put("fileupload_comments_size", if crate::config::truthy(&w("fileupload_comments_size")) { w("fileupload_comments_size") } else { json!(10 * 1024 * 1024) });
    put("fileupload_comments_max", if crate::config::truthy(&w("fileupload_comments_max")) { w("fileupload_comments_max") } else { json!(5) });
    put("fileupload_comments_mode", json!(cfg.str_or("websrv.fileupload_comments_mode", "attachment")));
    put("fileupload_comments_mimes", match w("fileupload_comments_mimes") {
        Value::Array(a) => Value::Array(a),
        _ => json!(["image", "video", "audio"]),
    });
    put("enable_comment_polls", if crate::config::truthy(&w("enable_comment_polls")) { w("enable_comment_polls") } else { json!(false) });
    put("fonts", Value::Array(fonts));
    g
}

fn bool_or(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

/// Render a page: globals, then `data`, then per-request values (the old patched `tpl.render`).
pub fn render(st: &AppState, req: Option<&Req>, view: &str, data: Value) -> String {
    let cfg = st.cfg();
    let mut data = match data {
        Value::Object(m) => m,
        _ => Map::new(),
    };

    // Per-request language
    let lang = match req {
        Some(r) => {
            let raw = r
                .session
                .as_ref()
                .and_then(|s| s.language.as_deref())
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .or_else(|| r.cookie("language").map(str::trim).filter(|l| !l.is_empty()))
                .map(str::to_string)
                .unwrap_or_else(|| cfg.str_or("websrv.language", "en").to_string());
            if crate::i18n::VALID_LANGS.contains(&raw.as_str()) { raw } else { cfg.str_or("websrv.language", "en").to_string() }
        }
        None => data.get("lang").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| cfg.str_or("websrv.language", "en").to_string()),
    };

    let default_recaptcha = cfg.truthy("recaptcha.enabled") && cfg.truthy("recaptcha.site_key");
    let recaptcha = match req {
        Some(r) if r.is_onion() || r.is_localhost() => false,
        _ => bool_or(data.get("recaptcha_enabled")).unwrap_or(default_recaptcha),
    };

    let session = req.and_then(|r| r.session.as_ref());
    let use_alt_infobox = session
        .and_then(|s| s.use_alternative_infobox)
        .or_else(|| bool_or(data.get("user_alternative_infobox")))
        .unwrap_or(cfg.enabled("websrv.user_alternative_infobox"));
    let use_alt_steuerung = session
        .and_then(|s| s.use_alternative_steuerung)
        .or_else(|| bool_or(data.get("user_alternative_steuerung")))
        .unwrap_or(cfg.enabled("websrv.user_alternative_steuerung"));

    let is_anonymized = crate::settings::is_anonymize_session(&cfg, session);
    let can_chan = crate::settings::can_use_chan(&cfg, session);

    if let Some(Value::Object(item)) = data.get_mut("item") {
        if item.get("size").map(Value::is_number).unwrap_or(false) {
            let fs = crate::util::format_size(&item["size"]);
            item.insert("size".into(), Value::String(fs));
        }
    }

    let mut ctx = globals(st, &cfg);
    let item_has_dims = data
        .get("item")
        .map(|i| crate::config::truthy(&i["width"]) && crate::config::truthy(&i["height"]))
        .unwrap_or(false);
    let can_rate_item = data.get("can_rate_item").filter(|v| !v.is_null()).or_else(|| data.get("can_manage_item")).map(crate::config::truthy).unwrap_or(false);
    let is_mod_area = bool_or(data.get("is_mod_area"))
        .unwrap_or_else(|| req.map(|r| r.path == "/mod" || r.path.starts_with("/mod/")).unwrap_or(false));
    let comment_display_mode = session
        .and_then(|s| s.comment_display_mode)
        .map(Value::from)
        .or_else(|| data.get("comment_display_mode").filter(|v| v.is_number()).cloned())
        .unwrap_or_else(|| {
            let v = cfg.get("websrv.default_comment_display_mode");
            if crate::config::truthy(v) { v.clone() } else { json!(0) }
        });

    let session_value = data.get("session").cloned();
    for (k, v) in data {
        ctx.insert(k, v);
    }
    // Session object as seen by templates, with can_chan resolved
    match req {
        Some(r) => {
            let mut sv = session_value.unwrap_or_else(|| r.session_value());
            if let Value::Object(m) = &mut sv {
                m.insert("can_chan".into(), json!(can_chan));
            }
            ctx.insert("session".into(), sv);
        }
        None => {
            if let Some(sv) = session_value {
                ctx.insert("session".into(), sv);
            }
        }
    }

    ctx.insert("lang".into(), json!(lang));
    ctx.insert("__date_lang".into(), Value::Null);
    ctx.insert("recaptcha_enabled".into(), json!(recaptcha));
    ctx.insert("is_anonymized".into(), json!(is_anonymized));
    ctx.insert("anon_anonymize".into(), json!(crate::settings::anon_anonymize(&cfg)));
    ctx.insert("can_chan".into(), json!(can_chan));
    ctx.insert("can_rate_item".into(), json!(can_rate_item));
    ctx.insert("is_mod_area".into(), json!(is_mod_area));
    ctx.insert("item_has_dimensions".into(), json!(item_has_dims));
    ctx.insert("user_alternative_infobox".into(), json!(use_alt_infobox));
    ctx.insert("user_alternative_steuerung".into(), json!(use_alt_steuerung));
    let banner = cfg.enabled("websrv.user_banner_enabled");
    ctx.insert("user_banner_enabled".into(), json!(banner));
    ctx.insert("comment_banner_enabled".into(), json!(banner && cfg.enabled("websrv.comment_banner_enabled")));
    ctx.insert(
        "comment_banner_max_height".into(),
        cfg.get("websrv.comment_banner_max_height").as_f64().map(Value::from).unwrap_or(json!(300)),
    );
    ctx.insert("comment_display_mode".into(), comment_display_mode);
    let brand = st.settings.brand_image_url();
    if !brand.is_empty() {
        ctx.insert("custom_brand_image".into(), json!(brand));
    }

    let onara_cfg = {
        let c = if cfg.get("onara").is_null() { cfg.get("websrv.onara") } else { cfg.get("onara") };
        c == &Value::Bool(true)
    };
    match req {
        Some(r) => {
            ctx.insert("mode".into(), json!(r.mode));
            let theme = if !r.theme.is_empty() {
                r.theme.clone()
            } else {
                r.cookie("theme").map(str::to_string).unwrap_or_else(|| cfg.str_or("websrv.theme", "f0ck").to_string())
            };
            ctx.insert("theme".into(), json!(theme));
            if !ctx.get("url").map(crate::config::truthy).unwrap_or(false) {
                ctx.insert(
                    "url".into(),
                    json!({
                        "pathname": r.path,
                        "split": r.path.split('/').skip(1).collect::<Vec<_>>(),
                        "qs": r.query_object(),
                        "search": if r.raw_query.is_empty() { String::new() } else { format!("?{}", r.raw_query) },
                    }),
                );
            }
            ctx.insert("user_strict_bool".into(), json!(session.and_then(|s| s.strict_mode).unwrap_or(false)));
            ctx.insert("user_logged_in_bool".into(), json!(session.is_some()));
            let csrf = session
                .and_then(|s| s.csrf_token.clone())
                .or_else(|| ctx.get("csrf_token").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            ctx.insert("csrf_token".into(), json!(csrf));
            let mult = if session.map(|s| s.admin).unwrap_or(false) { cfg.f64("main.adminmultiplier").unwrap_or(1.0) } else { 1.0 };
            let max = cfg.f64("main.maxfilesize").unwrap_or(0.0) * mult;
            ctx.insert("max_file_size".into(), json!(crate::util::format_size_f(max)));
            ctx.insert("max_file_size_bytes".into(), json!(max.floor() as i64));
            if ctx.get("web_url_upload").map(Value::is_null).unwrap_or(true) {
                ctx.insert("web_url_upload".into(), json!(cfg.truthy("websrv.web_url_upload")));
            }
            ctx.insert("onara".into(), json!(crate::settings::is_onara_enabled_for(&cfg, &r.cookies, session, false)));
            ctx.insert("onara_cfg".into(), Value::Null);
        }
        None => {
            let theme = ctx.get("theme").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or_else(|| cfg.str_or("websrv.theme", "f0ck").to_string());
            ctx.insert("theme".into(), json!(theme));
            ctx.insert("user_strict_bool".into(), json!(false));
            ctx.insert("user_logged_in_bool".into(), json!(false));
            ctx.insert("onara".into(), json!(onara_cfg));
            ctx.insert("onara_cfg".into(), Value::Null);
        }
    }
    if !ctx.contains_key("onara_item_html") {
        ctx.insert("onara_item_html".into(), json!(""));
    }

    st.tpl.render(view, &Value::Object(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_format_like_js() {
        assert_eq!(locale_number(1234567.0), "1,234,567");
        assert_eq!(locale_number(1234.5), "1,234.5");
        assert_eq!(js_to_string(&JValue::from(3.0)), "3");
        assert_eq!(js_to_string(&JValue::from(vec![1, 2])), "1,2");
    }

    #[test]
    fn blank_lines_collapse() {
        assert_eq!(collapse_blank_lines("a\n\n  \nb"), "a\nb");
    }
}
