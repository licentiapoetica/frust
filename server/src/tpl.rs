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
    views: PathBuf,
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
/// Any serializable value as a template value (minijinja 3 dropped `Value::from_serialize`).
pub fn to_jvalue<T: serde::Serialize>(v: T) -> JValue {
    JValue::from(minijinja::value::Serde(v))
}

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

fn js_formatter(out: &mut Output, state: &mut State, value: &JValue) -> Result<(), Error> {
    if value.is_undefined() || value.is_none() {
        return Ok(());
    }
    let s = js_to_string(value);
    let r = if value.is_safe() || *state.auto_escape() == AutoEscape::None { out.write_str(&s) } else { write_escaped(out, &s) };
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

/// The process locale (`LC_ALL` / `LC_MESSAGES` / `LANG`): what `toLocaleString()` without a
/// locale argument used in Node.
pub fn process_lang() -> Option<String> {
    static LANG: once_cell::sync::Lazy<Option<String>> = once_cell::sync::Lazy::new(|| {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.is_empty())
            .map(|v| v.split(['_', '.', '-', '@']).next().unwrap_or("").to_string())
            .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
    });
    LANG.clone()
}

fn locale_of(lang: Option<&str>) -> &'static str {
    let lang = lang.map(str::to_string).or_else(process_lang);
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
/// "UTC" or "UTC±hh:mm" for an offset in seconds.
fn tz_label(secs: i32) -> String {
    if secs == 0 {
        return "UTC".into();
    }
    let (sign, a) = if secs < 0 { ('-', -secs) } else { ('+', secs) };
    format!("UTC{sign}{:02}:{:02}", a / 3600, a % 3600 / 60)
}

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

/// Python-style methods (pycompat), except that `str.replace` takes any value for the
/// search and replacement and stringifies it like JS `String.replace` does, so
/// `.replace("{id}", n.item_slug or n.item_id)` works when the value is a number.
fn unknown_method(state: &mut State, value: &JValue, method: &str, args: &[JValue]) -> Result<JValue, Error> {
    if method == "replace" && value.as_str().is_some() && (2..=3).contains(&args.len()) {
        let args: Vec<JValue> = args
            .iter()
            .enumerate()
            .map(|(i, a)| if i < 2 && a.as_str().is_none() { JValue::from(js_to_string(a)) } else { a.clone() })
            .collect();
        return minijinja_contrib::pycompat::unknown_method_callback(state, value, method, &args);
    }
    minijinja_contrib::pycompat::unknown_method_callback(state, value, method, args)
}

fn opt_lang(state: &State, lang: Option<String>) -> Option<String> {
    lang.or_else(|| state.lookup("__date_lang").and_then(|v| v.as_str().map(str::to_string)))
}

pub(crate) fn build_env(views: PathBuf, i18n: Arc<I18n>) -> Environment<'static> {
    let mut env = Environment::new();
    // The project directory (views/ sits in it): mtime() paths ("/public/...") are relative to it
    let base = views.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
    env.set_loader(minijinja::path_loader(views));
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(|name| if name.ends_with(".html") { AutoEscape::Html } else { AutoEscape::None });
    env.set_formatter(js_formatter);
    env.set_syntax(minijinja::syntax::SyntaxConfig::builder().keep_trailing_newline(true).build().expect("default syntax is valid"));
    minijinja_contrib::add_to_environment(&mut env);
    env.set_unknown_method_callback(unknown_method);

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
            other => Ok(to_jvalue(&other)),
        }
    });

    // mtime("/public/s/....js"): the file's modification time, the ?v= of asset URLs so a deploy
    // changes them. Resolved against the project directory, not the working directory (a server
    // started elsewhere got 0 for every file, so browsers and caches kept old assets).
    env.add_function("mtime", move |path: String| -> i64 {
        let rel = path.trim_start_matches('/');
        let p = Some(base.join(rel)).filter(|p| p.exists()).unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join(rel));
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
    // The server's time zone for a date, "UTC" or "UTC+01:00" (what locale_string's text is in)
    env.add_filter("tz_label", |v: JValue| to_datetime(&v).map(|d| tz_label(d.offset().local_minus_utc())).unwrap_or_default());
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
    // True when a name has no capitals, digits or tall letters (e.g. "x", "anonymous"), so CSS can
    // trim it to the x-height instead of the cap height and its top lines up with the avatar.
    // Keep in sync with isShortName() in public/s/js/sidebar-activity.js.
    env.add_filter("short_letters", |v: JValue| {
        let s = js_to_string(&v);
        s.chars().any(|c| c.is_ascii_lowercase()) && s.chars().all(|c| "acegmnopqrsuvwxyz _.,-".contains(c))
    });
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
                // Loading templates raises access events: only changes count
                if res.is_ok_and(|ev| !ev.kind.is_access()) {
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
        Templates { env: RwLock::new(build_env(views.clone(), i18n)), views, dirty, _watcher: watcher }
    }

    /// Whether `views/<name>.html` exists.
    pub fn has(&self, name: &str) -> bool {
        let file = if name.ends_with(".html") { name.to_string() } else { format!("{name}.html") };
        self.views.join(file).is_file()
    }

    /// Installed alternative frontends: the directories in `views/frontends/` (sorted).
    pub fn frontends(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(self.views.join(FRONTENDS_DIR))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .filter(|n| valid_frontend_name(n))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// Render `views/<name>.html`. Errors are logged and rendered as a short message.
    pub fn render(&self, name: &str, ctx: &Value) -> String {
        if self.dirty.swap(false, Ordering::Relaxed) {
            self.env.write().clear_templates();
        }
        let env = self.env.read();
        let file = if name.ends_with(".html") { name.to_string() } else { format!("{name}.html") };
        let result = env.get_template(&file).and_then(|t| t.render(to_jvalue(ctx)));
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

// ── Frontends ───────────────────────────────────────────────────────────────
//
// The default frontend is the templates in views/. An alternative frontend is a directory
// views/frontends/<name>/ with its own templates (same view names: index.html, item.html, …)
// and its assets in public/s/fe/<name>/. A page the frontend has no template for renders with
// the default one.

/// Name of the built-in frontend (the templates directly in views/).
pub const DEFAULT_FRONTEND: &str = "f0ckm";
const FRONTENDS_DIR: &str = "frontends";

fn valid_frontend_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 32 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Every selectable frontend: the default one first, then the installed ones, narrowed down to
/// `websrv.frontends` when that lists any (see `pick_frontends`).
pub fn frontend_names(st: &AppState, cfg: &Config) -> Vec<String> {
    let mut installed = vec![DEFAULT_FRONTEND.to_string()];
    installed.extend(st.tpl.frontends().into_iter().filter(|n| n != DEFAULT_FRONTEND));
    pick_frontends(installed, cfg.str_list("websrv.frontends"), cfg.str("websrv.frontend"))
}

/// The installed frontends the admin picked in `websrv.frontends` (missing or empty: all of them).
/// The site's own frontend (`websrv.frontend`, else f0ckm) always stays available, listed or not;
/// names that aren't installed are ignored.
fn pick_frontends(installed: Vec<String>, picked: Option<Vec<String>>, site: Option<&str>) -> Vec<String> {
    let Some(picked) = picked.filter(|p| !p.is_empty()) else { return installed };
    let site = site.filter(|s| installed.iter().any(|n| n == s)).unwrap_or(DEFAULT_FRONTEND);
    installed.into_iter().filter(|n| n == site || picked.iter().any(|p| p == n)).collect()
}

/// Whether visitors may pick their own frontend (`websrv.frontend_switch`, default on).
pub fn frontend_switch_enabled(cfg: &Config) -> bool {
    !cfg.is_false("websrv.frontend_switch")
}

/// The `Set-Cookie` value that makes `name` the visitor's frontend, or None when `name` isn't a
/// frontend or switching is off. The site's own default clears the cookie, so a later change of
/// `websrv.frontend` applies again.
pub fn frontend_cookie(st: &AppState, cfg: &Config, name: &str) -> Option<String> {
    use crate::http::{CookieExpiry, EXPIRED, cookie_options};
    if !frontend_switch_enabled(cfg) || !frontend_names(st, cfg).iter().any(|n| n == name) {
        return None;
    }
    Some(if name == frontend_for(st, cfg, None) {
        format!("frontend=; {}", cookie_options(cfg, CookieExpiry::Expires(EXPIRED), false))
    } else {
        format!("frontend={name}; {}", cookie_options(cfg, CookieExpiry::MaxAge(365 * 24 * 3600), false))
    })
}

/// `?frontend=<name>` on a page: remember it like `/frontend/<name>` does.
pub fn remember_frontend_query(st: &AppState, req: &Req) {
    let Some(name) = req.query("frontend") else { return };
    if req.cookie("frontend") == Some(name) {
        return;
    }
    if let Some(c) = frontend_cookie(st, &st.cfg(), name) {
        req.set_cookie(c);
    }
}

/// The frontend of this request: `?frontend=<name>` or the visitor's `frontend` cookie when
/// switching is allowed, else `websrv.frontend`, else the default one.
pub fn frontend_for(st: &AppState, cfg: &Config, req: Option<&Req>) -> String {
    let names = frontend_names(st, cfg);
    let valid = |n: &str| names.iter().any(|x| x == n);
    if frontend_switch_enabled(cfg) {
        if let Some(r) = req {
            if let Some(f) = r.query("frontend").or_else(|| r.cookie("frontend")).filter(|f| valid(f)) {
                return f.to_string();
            }
        }
    }
    cfg.str("websrv.frontend").filter(|f| valid(f)).map(str::to_string).unwrap_or_else(|| DEFAULT_FRONTEND.to_string())
}

/// Whether this request's frontend can show the Onara viewer (an item over its grid page). The default
/// frontend can; an alternative one when it has its own `ajax-item.html`, the item box that
/// routes/index.rs renders into its index page (views/frontends/README.md). Without it, items open
/// on their own page.
pub fn frontend_has_onara(st: &AppState, req: Option<&Req>) -> bool {
    frontend_name_has_onara(st, &frontend_for(st, &st.cfg(), req))
}

fn frontend_name_has_onara(st: &AppState, fe: &str) -> bool {
    fe == DEFAULT_FRONTEND || st.tpl.has(&format!("{FRONTENDS_DIR}/{fe}/ajax-item"))
}

/// Whether the frontend of this request lists recommendations on its item page (it has
/// `snippets/recommendations.html`): the item routes then render them into the page
/// (`recommendations`, the items of /api/v2/recommendations), so the list works without JavaScript.
/// f0ckm loads its recommendations tab with JavaScript only.
pub fn frontend_has_recommendations(st: &AppState, req: Option<&Req>) -> bool {
    let fe = frontend_for(st, &st.cfg(), req);
    fe != DEFAULT_FRONTEND && st.tpl.has(&format!("{FRONTENDS_DIR}/{fe}/snippets/recommendations"))
}

/// Whether the frontend of this request has its own `views/frontends/<fe>/<view>.html`. For pages
/// only one frontend has (f0ckgle's `howsearch`): the route answers 404 in the others, so such a
/// page isn't one of `FRONTEND_OWN_PAGES` every frontend must provide.
pub fn frontend_has_view(st: &AppState, req: Option<&Req>, view: &str) -> bool {
    let fe = frontend_for(st, &st.cfg(), req);
    fe != DEFAULT_FRONTEND && st.tpl.has(&format!("{FRONTENDS_DIR}/{fe}/{view}"))
}

/// Shared pages: default views that render inside the frontend's shell (they include
/// `shell_header`). Paths relative to views/, e.g. "admin/users.html".
pub fn shared_views(views: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![views.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let rel = p.strip_prefix(views).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                if rel != FRONTENDS_DIR {
                    stack.push(p);
                }
            } else if rel.ends_with(".html") && std::fs::read_to_string(&p).map(|t| t.contains("shell_header")).unwrap_or(false) {
                out.push(rel);
            }
        }
    }
    out.sort();
    out
}

/// The own pages every alternative frontend has to provide itself (views/frontends/<fe>/<page>.html):
/// the visitor-facing full pages the routes render that aren't shared pages. A frontend without one
/// would show f0ckm's layout there. Standalone pages with their own look (scroller, maintenance,
/// forgot-password, the 4chan proxy, the meme creator) aren't in it. views/frontends/README.md
pub const FRONTEND_OWN_PAGES: [&str; 19] = [
    "about", "comments_user", "error", "halls", "index", "item", "login", "notifications", "ranking", "register", "rules", "search",
    "subscriptions", "tags", "tags_user", "terms", "upload", "user", "user-halls",
];

/// Checks every alternative frontend against the contract in views/frontends/README.md; returns
/// the problems found (empty = fine). `public` is the public/ directory.
pub fn check_frontends(views: &std::path::Path, public: &std::path::Path) -> Vec<String> {
    let read = |p: &std::path::Path| std::fs::read_to_string(p).unwrap_or_default();
    let mut problems = Vec::new();
    let shared = shared_views(views);
    for v in &shared {
        if !read(&views.join(v)).contains("shell_footer") {
            problems.push(format!("views/{v}: includes shell_header but not shell_footer"));
        }
    }
    let mut fronts: Vec<String> = std::fs::read_dir(views.join(FRONTENDS_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    fronts.sort();
    for fe in fronts {
        let dir = views.join(FRONTENDS_DIR).join(&fe);
        let at = |rel: &str| format!("views/{FRONTENDS_DIR}/{fe}/{rel}");
        let header = read(&dir.join("shell/header.html"));
        let footer = read(&dir.join("shell/footer.html"));
        if header.is_empty() {
            problems.push(format!("{}: missing", at("shell/header.html")));
        } else {
            for need in ["snippets/shell-head.html", "f0ck-shell-page"] {
                if !header.contains(need) {
                    problems.push(format!("{}: must contain {need}", at("shell/header.html")));
                }
            }
        }
        if footer.is_empty() {
            problems.push(format!("{}: missing", at("shell/footer.html")));
        } else {
            for need in ["snippets/frontend-scripts.html", "snippets/shell-foot.html"] {
                if !footer.contains(need) {
                    problems.push(format!("{}: must contain {need}", at("shell/footer.html")));
                }
            }
        }
        for page in FRONTEND_OWN_PAGES {
            if !dir.join(format!("{page}.html")).is_file() {
                problems.push(format!("{}: missing; every page needs the frontend's own template (it would show f0ckm's layout)", at(&format!("{page}.html"))));
            }
        }
        for v in &shared {
            if dir.join(v).is_file() {
                problems.push(format!("{}: overrides the shared page views/{v}; delete it (it renders in the shell)", at(v)));
            }
        }
        // Its own pages load the shared scripts; nothing copies the page data
        let mut own_includes_scripts = false;
        let mut includes_chat_head = false;
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                let text = read(&p);
                if !rel.starts_with("shell/") && text.contains("snippets/frontend-scripts.html") {
                    own_includes_scripts = true;
                }
                if text.contains("snippets/globalchat-head.html") {
                    includes_chat_head = true;
                }
                if text.contains("id=\"footer-data\"") {
                    problems.push(format!("{}: copies #footer-data; include snippets/frontend-scripts.html instead", at(&rel)));
                }
            }
        }
        if !own_includes_scripts {
            problems.push(format!("views/{FRONTENDS_DIR}/{fe}: no page layout includes snippets/frontend-scripts.html"));
        }
        // The site chat (frontend-scripts.html loads it): its structure in <head>, a skin in the
        // frontend's look (README "Site chat")
        if !includes_chat_head {
            problems.push(format!("views/{FRONTENDS_DIR}/{fe}: no <head> includes snippets/globalchat-head.html (the site chat's CSS)"));
        }
        let skin = format!("html.gchat-skin-{fe}");
        let skinned = std::fs::read_dir(public.join("s/fe").join(&fe).join("css"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| read(&e.path()).contains(&skin));
        if !skinned {
            problems.push(format!("public/s/fe/{fe}/css: no {skin} rules; skin the site chat in the frontend's look (README \"Site chat\")"));
        }
        // One live socket for everyone: F0ckLive (f0ck-core.js)
        for e in std::fs::read_dir(public.join("s/fe").join(&fe).join("js")).into_iter().flatten().flatten() {
            let text = read(&e.path());
            if text.contains("new WebSocket") || text.contains("/api/notifications/ws") {
                problems.push(format!("public/s/fe/{fe}/js/{}: opens its own live socket; use F0ckLive / f0ckFrontend hooks", e.file_name().to_string_lossy()));
            }
        }
    }
    problems
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

/// The f0ckyou frontend's own brand (admin panel → f0ckYou brand; snippets/logo.html): `site`
/// (show the site's brand instead), `image`, `mark` (the finger mark, without an image), `text` (as
/// saved), `parts` (the text to show: [{text, em}], a part in *asterisks* is `em`; no text shows
/// "f0ck*You*", or nothing next to an image), `font` / `weight` (one the family still has) /
/// `font_css`, `size` (percent, none = 100).
fn fy_brand(cfg: &Config, s: &crate::settings::SiteSettings) -> Value {
    let font = Some(s.fy_brand_font.as_str()).filter(|f| !f.is_empty() && crate::fonts::exists(cfg, f));
    let weight = font.and_then(|f| crate::fonts::weight_for(cfg, f, Some(s.fy_brand_weight)));
    let image = s.fy_brand_image_url.trim();
    let text = match s.fy_brand_text.trim() {
        "" if image.is_empty() => "f0ck*You*",
        t => t,
    };
    json!({
        "site": s.fy_brand_site,
        "image": image,
        "mark": s.fy_brand_mark,
        "text": s.fy_brand_text,
        "parts": brand_parts(text),
        "font": font.unwrap_or(""),
        "weight": weight,
        "font_css": font.map(|f| crate::fonts::fy_brand_css_url(f, weight)).unwrap_or_default(),
        "size": Some(s.fy_brand_size).filter(|n| *n > 0),
    })
}

/// "f0ck*You*" → [{text: "f0ck", em: false}, {text: "You", em: true}]: every other part between
/// asterisks is emphasized; empty parts are left out.
fn brand_parts(text: &str) -> Value {
    Value::Array(
        text.split('*')
            .enumerate()
            .filter(|(_, t)| !t.is_empty())
            .map(|(i, t)| json!({ "text": t, "em": i % 2 == 1 }))
            .collect(),
    )
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
    // Built-in and uploaded fonts, weights of one family grouped (crate::fonts)
    let fonts = crate::fonts::families_json(cfg);
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
    put("theme_color", json!(crate::pwa::theme_color(cfg)));
    put("hide_comments_from_public", cfg.get("main.hide_comments_from_public").clone());
    put("guest_anonymize", json!(cfg.truthy("main.guest_anonymize")));
    put("anon_anonymize", json!(false));
    put("is_anonymized", json!(false));
    put("git_hash", json!(st.git_hash));
    put("motd", json!(s.motd));
    put("maintenance_on", json!(s.maintenance.enabled));
    put("manual_approval", json!(s.manual_approval));
    put("min_tags", json!(s.min_tags));
    put("registration_open", json!(st.settings.registration_open(cfg)));
    put("registration_apply", json!(st.settings.registration_applications(cfg)));
    put("registration_apply_email", json!(s.applications_require_email));
    // The applicant gets a mail when the application is decided (routes/admin/applications.rs)
    put("application_mail", json!(cfg.str("smtp.host").is_some_and(|h| !h.is_empty())));
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
    put("captcha_enabled", json!(crate::cap::enabled(cfg)));
    put("show_background_cfg", json!(cfg.enabled("websrv.background")));
    put("allowed_mimes", json!(allowed_mimes));
    // the drop zones list these: [{kind: "video", exts: ["webm", "mp4", …]}, …]
    let upload_types: Vec<Value> = crate::media::mime::upload_types(cfg, s.enable_pdf)
        .into_iter()
        .map(|(kind, exts)| json!({ "kind": kind, "exts": exts }))
        .collect();
    put("upload_types", json!(upload_types));
    // audio uploads off (no audio/* left in `allowedMimes`): the sidebar tuner drops its Audio tab
    put("audio_uploads_enabled", json!(crate::media::mime::allowed_mimes(cfg).iter().any(|m| m.starts_with("audio/"))));
    put("require_video_sound", json!(cfg.truthy("websrv.require_video_sound")));
    put("mimes_json", json!(json_script(&Value::Object(mimes))));
    put("development", json!(cfg.development()));
    put("show_mime_picker", json!(cfg.enabled("websrv.show_mime_picker")));
    put("private_society", if cfg.truthy("websrv.private_society") { w("private_society") } else { json!(false) });
    put("private_society_gate", json!(cfg.str_or("websrv.private_society_gate", "")));
    put("private_society_gate_location", json!(cfg.str_or("websrv.private_society_gate_location", "Frankfurt")));
    put("show_content_warning", json!(cfg.enabled("websrv.show_content_warning")));
    put("web_url_upload", json!(cfg.truthy("websrv.web_url_upload")));
    put("albums_enabled", json!(cfg.enabled("websrv.albums_enabled")));
    put("enable_youtube_upload", json!(cfg.enabled("websrv.enable_youtube_upload")));
    put("upload_limit", if cfg.get("main.upload_limit").is_null() { json!(69) } else { cfg.get("main.upload_limit").clone() });
    put("meme_creator", json!(cfg.truthy("websrv.meme_creator")));
    put("custom_favicon", json!(s.favicon_url));
    put("custom_brand_image", json!(brand));
    put("custom_navbar_brand_text", json!(s.brand_text));
    put("brand_glitch_text", json!(s.brand_glitch_text));
    put("brand_glitch_image", json!(s.brand_glitch_image));
    // Brand text case and font (admin panel → branding); a font that is gone counts as none
    put("brand_case", json!(s.brand_case));
    put("brand_size", json!(Some(s.brand_size).filter(|n| *n > 0)));
    put("brand_sizes", json!(crate::settings::BRAND_SIZES));
    let brand_font = Some(s.brand_font.as_str())
        .filter(|f| !f.is_empty() && fonts.as_array().is_some_and(|a| a.iter().any(|fam| fam["files"].as_array().is_some_and(|fs| fs.iter().any(|x| x.as_str() == Some(f))))));
    // Brand text weight: only one the brand font's family still has (a .brand-weight-<n> class)
    let brand_weight = brand_font.and_then(|f| {
        let fam = fonts.as_array()?.iter().find(|fam| fam["files"].as_array().is_some_and(|fs| fs.iter().any(|x| x.as_str() == Some(f))))?;
        let has = fam["weights"].as_array()?.iter().any(|w| w["weight"].as_i64() == Some(s.brand_weight));
        u16::try_from(s.brand_weight).ok().filter(|_| has)
    });
    put("brand_font", json!(brand_font.unwrap_or("")));
    put("brand_weight", json!(brand_weight));
    put("brand_font_css", json!(brand_font.map(|f| crate::fonts::brand_css_url(f, brand_weight)).unwrap_or_default()));
    put("fy_brand", fy_brand(cfg, &s));
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
    // The yes/no actions follow the anonymous accounts' permission defaults (crate::perms)
    let mut anon_perms = crate::settings::anon_permissions(cfg);
    let pc = st.settings.permissions();
    for (p, key) in crate::perms::ANON_CONFIG_KEYS {
        anon_perms.insert(key.to_string(), json!(pc.role_default(cfg, crate::perms::Role::Anon, crate::perms::info(*p))));
    }
    let perms = Value::Object(anon_perms);
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
    put("irc_enabled", json!(cfg.client_enabled("irc")));
    // settings → linked accounts: who to /msg and on which network
    put("irc_bot_nick", cfg.client("irc").map(|c| c["nickname"].clone()).unwrap_or(Value::Null));
    put("irc_network", cfg.client("irc").map(|c| c["network"].clone()).unwrap_or(Value::Null));
    put("ts", json!(crate::util::now_ms()));
    put("default_layout", json!(s.default_layout));
    put("show_koepfe", json!(cfg.truthy("websrv.show_koepfe")));
    put("hide_sidebar_default", json!(cfg.truthy("websrv.hide_sidebar_default")));
    put("allow_language_change", json!(cfg.enabled("websrv.allow_language_change")));
    put("languages", json!(crate::i18n::selectable_langs(cfg)));

    put("enable_dynamic_thumbs", json!(cfg.truthy("websrv.enable_dynamic_thumbs")));
    put("default_thumb_size", json!(if ["s", "m", "l", "xl"].contains(&thumb.as_str()) { thumb.as_str() } else { "m" }));
    put("comment_max_length", cfg.get("main.comment_max_length").clone());
    put("enable_swf", json!(cfg.truthy("websrv.enable_swf")));
    put("enable_archive", json!(cfg.truthy("websrv.enable_archive")));
    put("enable_danmaku", json!(cfg.enabled("websrv.enable_danmaku")));
    // the Konami code's player (snippets/f0ck-core-script.html → f0ck-core.js); null when switched off
    put("bad_apple", bad_apple(cfg));
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
    put("fonts", fonts);
    g
}

fn bool_or(v: Option<&Value>) -> Option<bool> {
    v.and_then(Value::as_bool)
}

/// Render a page: globals, then `data`, then per-request values (the old patched `tpl.render`).
/// `websrv.bad_apple` for the Konami code: `{ theme, sound }`, or null when `enabled: false`.
/// On by default; an unknown theme falls back to classic (Windows 98, square corners).
fn bad_apple(cfg: &Config) -> Value {
    if !cfg.enabled("websrv.bad_apple.enabled") {
        return Value::Null;
    }
    let theme = cfg.str_or("websrv.bad_apple.theme", "classic");
    let theme = if ["classic", "win10", "xp", "breeze"].contains(&theme) { theme } else { "classic" };
    json!({ "theme": theme, "sound": cfg.enabled("websrv.bad_apple.sound") })
}

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
            if crate::i18n::is_selectable(&cfg, &raw) || raw == cfg.str_or("websrv.language", "en") { raw } else { cfg.str_or("websrv.language", "en").to_string() }
        }
        None => data.get("lang").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| cfg.str_or("websrv.language", "en").to_string()),
    };

    let captcha = match req {
        Some(r) => crate::cap::required(&cfg, r),
        None => bool_or(data.get("captcha_enabled")).unwrap_or_else(|| crate::cap::enabled(&cfg)),
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
    let can_chan = crate::settings::can_use_chan(st, session);

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
    ctx.insert("captcha_enabled".into(), json!(captcha));
    // Image captcha in the login / register modals for visitors without JavaScript (cap_image.rs)
    if captcha {
        ctx.insert("nojs_captcha_id".into(), json!(crate::cap_image::new_id()));
    }
    // Actions this visitor must solve a captcha for (cap.uploads / cap.comments): loads the popup
    ctx.insert("captcha_scopes".into(), json!(req.map(|r| crate::cap::scopes_needed(&cfg, r)).unwrap_or_default()));
    ctx.insert("csp_nonce".into(), json!(req.map(|r| r.csp_nonce.as_str()).unwrap_or("")));
    // The visitor's own font: a stylesheet (#user-font-css), so settings.js can swap it live and
    // no older @font-face rules stay behind. Without one (or when it is gone), the site's default
    // font: its @font-face rules (one per weight) go into snippets/header.html's <style>.
    let user_font = session.and_then(|s| s.font.as_deref()).filter(|f| crate::fonts::exists(&cfg, f));
    let user_weight = user_font.and_then(|f| crate::fonts::weight_for(&cfg, f, session.and_then(|s| s.font_weight)));
    ctx.insert("user_font_css".into(), json!(user_font.map(|f| crate::fonts::user_css_url(f, user_weight)).unwrap_or_default()));
    // The weight select's value (settings): the chosen weight, else the font's own
    let shown_weight = user_font.and_then(|f| crate::fonts::effective_weight(&cfg, f, session.and_then(|s| s.font_weight)));
    ctx.insert("user_font_weight".into(), json!(shown_weight));
    // Text size: a class on <html> (user-fs-<percent>, rules in f0ckm.css / the frontends' CSS),
    // chosen per frontend
    let user_size = session.and_then(|s| s.font_size_for(&frontend_for(st, &cfg, req)));
    ctx.insert("user_font_size".into(), json!(user_size));
    ctx.insert("font_sizes".into(), json!(crate::fonts::SIZES));
    let site_font = if user_font.is_some() { None } else { cfg.str("websrv.default_font").filter(|f| crate::fonts::exists(&cfg, f)) };
    ctx.insert("site_font".into(), json!(site_font.unwrap_or("")));
    ctx.insert("font_faces".into(), crate::fonts::faces_json(&cfg, site_font, None));
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
            // Messages of the login / register modals after a form post without JavaScript (routes/register.rs)
            for key in crate::routes::register::auth_query_keys() {
                let msg = r.query(key).and_then(|code| crate::routes::register::auth_message(key, code));
                ctx.insert(format!("{key}_msg"), json!(msg));
            }
            ctx.insert("login_captcha".into(), json!(r.query("login_captcha") == Some("1")));
            // No-JavaScript forms (crate::nojs): the message of a failed post, and the preferences
            // that live in a cookie instead of localStorage
            if let Some(flash) = crate::nojs::take_flash(&cfg, r) {
                ctx.insert("nojs_flash".into(), flash);
            }
            let prefs = crate::nojs::prefs(r);
            ctx.insert("nojs_pref_classes".into(), json!(crate::nojs::pref_classes(&prefs)));
            ctx.insert("nojs_prefs".into(), json!(prefs));
            ctx.insert("language_cookie".into(), json!(r.cookie("language").filter(|l| !l.is_empty())));
            ctx.insert("user_strict_bool".into(), json!(session.and_then(|s| s.strict_mode).unwrap_or(false)));
            ctx.insert("user_logged_in_bool".into(), json!(session.is_some()));
            // the media types this visitor may filter by (the type filters of the frontends)
            ctx.insert("filter_mimes".into(), json!(crate::settings::filter_mimes(&cfg, session)));
            let csrf = session
                .and_then(|s| s.csrf_token.clone())
                .or_else(|| ctx.get("csrf_token").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            ctx.insert("logout_url".into(), json!(crate::auth::logout_url(&csrf)));
            ctx.insert("csrf_token".into(), json!(csrf));
            let max = st.settings.upload_limit(&cfg, crate::perms::can(st, session, crate::perms::P::UploadLarge));
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

    // Alternative frontend: its own template for this view, if it has one
    let frontend = frontend_for(st, &cfg, req);
    let fe_view = format!("{FRONTENDS_DIR}/{frontend}/{}", view.trim_end_matches(".html"));
    let view = if frontend != DEFAULT_FRONTEND && st.tpl.has(&fe_view) { fe_view.as_str() } else { view };
    // Shared pages (settings, messages, admin, mod, ...) exist once, as default views. They include
    // `shell_header` / `shell_footer`: the frontend's own chrome (views/frontends/<fe>/shell/) when it
    // has one, else the default header/footer. See views/frontends/README.md.
    for part in ["header", "footer"] {
        let fe_part = format!("{FRONTENDS_DIR}/{frontend}/shell/{part}");
        let shell = if frontend != DEFAULT_FRONTEND && st.tpl.has(&fe_part) { format!("{fe_part}.html") } else { format!("snippets/{part}.html") };
        ctx.insert(format!("shell_{part}"), json!(shell));
    }
    // the settings page offers the Onara viewer only where the frontend has it
    ctx.insert("frontend_onara".into(), json!(frontend_name_has_onara(st, &frontend)));
    // ... and only the preferences the frontend uses, when it lists them (views/frontends/README.md)
    let prefs_tpl = format!("{FRONTENDS_DIR}/{frontend}/settings-prefs");
    let prefs_tpl = (frontend != DEFAULT_FRONTEND && st.tpl.has(&prefs_tpl)).then(|| format!("{prefs_tpl}.html"));
    ctx.insert("frontend_settings_prefs".into(), json!(prefs_tpl));
    ctx.insert("frontend".into(), json!(frontend));
    ctx.insert("frontends".into(), json!(frontend_names(st, &cfg)));
    ctx.insert("frontend_switch".into(), json!(frontend_switch_enabled(&cfg)));
    // the colour scheme picked for frontends with a light and a dark look ("" = the frontend's
    // own): the `scheme` cookie, /scheme/<x> (routes/theme.rs), views/frontends/README.md
    let scheme = req.and_then(|r| r.cookie("scheme")).filter(|s| matches!(*s, "light" | "dark" | "auto")).unwrap_or("");
    ctx.insert("color_scheme".into(), json!(scheme));
    // the media filter (mime picker): the "mime" cookie, e.g. "video,audio" (empty: everything)
    if !ctx.contains_key("mime_filter") {
        let mime = req.and_then(|r| r.cookie("mime")).map(crate::util::decode_lossy).unwrap_or_default();
        ctx.insert("mime_filter".into(), json!(mime));
    }

    st.tpl.render(view, &Value::Object(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_labels() {
        assert_eq!(super::tz_label(0), "UTC");
        assert_eq!(super::tz_label(3600), "UTC+01:00");
        assert_eq!(super::tz_label(-(5 * 3600 + 30 * 60)), "UTC-05:30");
    }

    #[test]
    fn frontends_follow_contract() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let problems = check_frontends(&root.join("views"), &root.join("public"));
        assert!(problems.is_empty(), "frontend contract (views/frontends/README.md):\n{}", problems.join("\n"));
    }

    #[test]
    fn frontends_can_be_handpicked() {
        let all = || ["f0ckm", "f0ckgle", "f0ckv2", "w0bm"].map(String::from).to_vec();
        let pick = |p: &[&str]| Some(p.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        // nothing picked: every installed one
        assert_eq!(pick_frontends(all(), None, None), all());
        assert_eq!(pick_frontends(all(), pick(&[]), None), all());
        // picked ones, in the usual order; the site's own frontend always stays; unknown names ignored
        assert_eq!(pick_frontends(all(), pick(&["w0bm", "nope"]), None), ["f0ckm", "w0bm"]);
        assert_eq!(pick_frontends(all(), pick(&["w0bm"]), Some("f0ckgle")), ["f0ckgle", "w0bm"]);
        assert_eq!(pick_frontends(all(), pick(&["w0bm"]), Some("nope")), ["f0ckm", "w0bm"]);
    }

    #[test]
    fn numbers_format_like_js() {
        assert_eq!(locale_number(1234567.0), "1,234,567");
        assert_eq!(locale_number(1234.5), "1,234.5");
        assert_eq!(js_to_string(&JValue::from(3.0)), "3");
        assert_eq!(js_to_string(&JValue::from(vec![1, 2])), "1,2");
    }

    /// `.replace` takes a number like JS (item id when there is no slug, upload limits).
    #[test]
    fn replace_accepts_numbers() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = serde_json::json!({ "n": { "item_slug": null, "item_id": 42 } });
        let out = env.render_str(r#"{{ "on #{id}".replace("{id}", (n.item_slug or n.item_id), 1) }}|{{ "a1b".replace(1, "-") }}"#, to_jvalue(&ctx)).unwrap();
        assert_eq!(out, "on #42|a-b");
    }

    /// views/maintenance.html renders the admin's text, the video and the glitch attributes; the
    /// admin form parses.
    #[test]
    fn maintenance_page_renders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let m = crate::maintenance::parse(
            r#"{"enabled":true,"since":1794700000,"title":"brb","text":"line <one>\n\nline three","eta":"~16:00","video_url":"/s/maintenance/video-ab12.webm",
                "glitch":{"headline":{"on":true,"strength":"high","mode":"bursts"},"video":{"on":true,"strength":"low","mode":"hover"}}}"#,
        );
        let mut ctx = crate::maintenance::page_ctx(&m, false);
        ctx["domain"] = serde_json::json!("example.org");
        let out = env.get_template("maintenance.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(out.contains(r#"<h1 class="mt-headline mt-fx mt-fx-high mt-fx-bursts" data-glitch="0.95" data-glitch-idle>brb</h1>"#), "{out}");
        assert!(out.contains(r#"<div class="mt-tv-frame mt-fx mt-fx-low mt-fx-hover" data-glitch="0.3" data-glitch-hover>"#), "{out}");
        assert!(out.contains(r#"<section class="mt-term" aria-label="Status">"#), "{out}");
        assert!(out.contains(r#"<video class="mt-video" src="/s/maintenance/video-ab12.webm" autoplay muted loop playsinline controls></video>"#), "{out}");
        assert!(out.contains(r#"<p class="mt-line">line &lt;one&gt;</p>"#), "{out}");
        assert_eq!(out.matches(r#"<p class="mt-line">"#).count(), 3);
        assert!(out.contains("expected back: ~16:00") && out.contains("since 2026-11-14 23:46 UTC") && out.contains(r#"href="/login?form""#), "{out}");
        assert!(out.contains(r#"http-equiv="refresh""#));
        // no video: the test card; preview: no refresh, link back to the settings
        let mut ctx = crate::maintenance::page_ctx(&crate::maintenance::Maintenance::default(), true);
        ctx["domain"] = serde_json::json!("example.org");
        let out = env.get_template("maintenance.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(out.contains(r#"class="mt-bars""#) && out.contains(">MAINTENANCE</h1>") && !out.contains("<video"), "{out}");
        assert!(!out.contains(r#"http-equiv="refresh""#) && out.contains(r#"href="/admin/maintenance""#), "{out}");
        // the other layouts: same text, video and glitch attributes in their own markup
        for (layout, marks) in [("signal", vec![r#"<div class="mt-bg mt-fx mt-fx-low mt-fx-hover" data-glitch="0.3" data-glitch-hover>"#, r#"class="mt-center""#]), ("panel", vec![r#"<div class="mt-box-video mt-fx"#, r#"class="mt-box""#])] {
            let mut m = m.clone();
            m.layout = layout.into();
            let mut ctx = crate::maintenance::page_ctx(&m, false);
            ctx["domain"] = serde_json::json!("example.org");
            let out = env.get_template("maintenance.html").unwrap().render(to_jvalue(&ctx)).unwrap();
            assert!(out.contains(&format!(r#"<body class="mt-body mt-layout-{layout}">"#)), "{out}");
            for mark in marks {
                assert!(out.contains(mark), "{layout}: {mark}\n{out}");
            }
            assert!(out.contains(r#"<p class="mt-p">line &lt;one&gt;</p>"#) && out.contains("<video ") && out.contains(r#"data-glitch="0.95" data-glitch-idle>brb</h1>"#), "{out}");
            assert!(!out.contains("mt-term"), "{out}");
        }
        env.get_template("admin/maintenance.html").unwrap();
    }

    /// The abyss (scroller.html): the comment and tag sheets, and a page data block that parses.
    #[test]
    fn scroller_sheets_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let base = serde_json::json!({
            "domain": "example.org", "lang": "en", "themes_json": "[]", "anon_permissions_json": "{}",
            "allowed_comment_images_json": "[]", "scroller_mime_cats": ["video", "image"], "captcha_scopes": [],
            "comment_max_length": 2000, "_scrollerShowSfw": true,
        });
        let render = |ctx: &serde_json::Value| env.get_template("scroller.html").unwrap().render(to_jvalue(ctx)).unwrap();
        let data_of = |out: &str| -> serde_json::Value {
            let start = out.find(r#"id="scroller-data">"#).expect("data block") + r#"id="scroller-data">"#.len();
            let end = start + out[start..].find("</script>").unwrap();
            serde_json::from_str(&out[start..end]).unwrap_or_else(|e| panic!("{e}: {}", &out[start..end]))
        };
        let sheets = |out: &str| -> String {
            let a = out.find("<!-- COMMENTS SHEET").expect("comments sheet");
            let b = out.find("<!-- SHARE PANEL").expect("share panel");
            out[a..b].to_string()
        };

        let mut ctx = base.clone();
        ctx["session"] = serde_json::json!({ "user": "bob", "display_name": "Bob", "avatar_file": "bob.webp", "csrf_token": "tok" });
        let out = render(&ctx);
        // F0CKM_DUMP_SCROLLER=<dir> writes the page out, to look at it in a browser
        if let Ok(dir) = std::env::var("F0CKM_DUMP_SCROLLER") {
            std::fs::write(PathBuf::from(dir).join("scroller.html"), &out).unwrap();
        }
        let part = sheets(&out);
        for id in ["comments-close-btn", "comments-open-link", "comment-send-btn", "tag-bar", "tag-backdrop", "tag-bar-current", "scroll-tag-suggestions", "scroll-tag-send-btn"] {
            assert!(part.contains(&format!(r#"id="{id}""#)), "{id}\n{part}");
        }
        assert!(part.contains(r#"<img class="ig-self-avatar" src="/a/bob.webp""#), "{part}");
        assert!(part.contains(">Post</button>") && part.contains(r#"maxlength="2000""#), "{part}");
        assert!(!part.contains("style=") && !part.contains("<script") && !part.contains("border-radius"), "{part}");
        let data = data_of(&out);
        assert_eq!(data["i18n"]["view_replies"], "View {n} more replies");
        assert_eq!(data["i18n"]["tag_posts"], "{n} posts");
        assert_eq!(data["session"]["avatar"], "/a/bob.webp");

        // logged out: a login note instead of the input, no tag sheet
        let out = render(&base);
        let part = sheets(&out);
        assert!(part.contains(r#"class="comments-login-note""#) && !part.contains("comment-input") && !part.contains(r#"id="tag-bar""#), "{part}");
        assert!(data_of(&out)["session"].is_null());
    }

    /// snippets/comments-static.html (no-JavaScript item comments) renders the thread markup.
    #[test]
    fn static_comments_snippet_renders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let reply = serde_json::json!({ "id": 3, "is_anon": false, "username": "Bob", "name": "bob", "username_color": "#f00", "avatar_src": "/a/default.png",
            "banner": null, "content_html": "<span class=\"greentext\">&gt;hi</span>", "attachments_html": "", "timeago": "1 day ago", "iso": "x", "backlinks": [], "replies": [] });
        let root_c = serde_json::json!({ "id": 2, "is_pinned": true, "is_anon": false, "username": "Al", "name": "Al", "username_color": null, "avatar_src": "/t/5.webp",
            "banner": { "file": "b.png", "position": "center top", "size": "100% auto" }, "content_html": "x &lt;b&gt;", "attachments_html": "", "timeago": "2 days ago", "iso": "y",
            "backlinks": [3], "replies": [reply] });
        let ctx = serde_json::json!({ "static_comments": [root_c], "comment_banner_enabled": true });
        let html = env.get_template("snippets/comments-static.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("<div class=\"comments-list comments-static\">"));
        assert!(html.contains("<div class=\"comment pinned\" id=\"c2\" style=\"--author-banner: url(&#x27;/a/b.png&#x27;)") || html.contains("id=\"c2\" style=\"--author-banner"));
        assert!(html.contains("<div class=\"comment-replies\"><div class=\"comment reply\" id=\"c3\""));
        assert!(html.contains("class=\"comment-author user-color-text\" style=\"--user-color: #f00\">bob</a>"));
        assert!(html.contains("<a href=\"#c3\" class=\"comment-context-link\">&gt;&gt;3</a>"));
        assert!(html.contains("<div class=\"comment-content\">x &lt;b&gt;</div>"));
        assert!(html.contains("<a href=\"/user/al\""));
        let empty = env.get_template("snippets/comments-static.html").unwrap().render(to_jvalue(&serde_json::json!({ "static_comments": [] }))).unwrap();
        assert!(!empty.contains("comments-static"));
    }

    /// A sample poll as social/polls.rs sends it (`can_vote` / `can_delete` / vote for the viewer).
    pub(crate) fn sample_poll(can_vote: bool, closed: bool) -> serde_json::Value {
        serde_json::json!({ "id": 7, "comment_id": 2, "item_id": 5, "question": "Best <cat>?", "is_anonymous": false, "expires_at": null, "is_closed": closed,
            "options": [
                { "id": 11, "text": "Tom", "vote_count": 1, "pct": 33, "voters": [{ "username": "Al", "avatar": null, "avatar_file": "x.webp" }] },
                { "id": 12, "text": "Garfield", "vote_count": 2, "pct": 67, "voters": [] }
            ],
            "total_votes": 3, "user_vote_option_id": 12, "can_vote": can_vote && !closed, "can_unvote": can_vote && !closed, "can_delete": can_vote })
    }

    /// snippets/comment-poll.html: plain vote forms (option_id, remove=1 on the own vote), results
    /// only when the viewer can't vote, no inline styles (the bar is a <progress>).
    #[test]
    fn comment_poll_snippet_renders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let tpl = r#"{% from "snippets/comment-poll.html" import poll %}{{ poll(p, "/5#c2", "tok") }}"#;
        let render = |p: serde_json::Value| env.render_named_str("poll-test.html", tpl, to_jvalue(&serde_json::json!({ "p": p }))).unwrap();

        let html = render(sample_poll(true, false));
        assert!(html.contains(r#"<div class="comment-poll has-voted" id="poll-7" data-poll-id="7" data-comment-id="2">"#), "{html}");
        assert!(html.contains("Best &lt;cat&gt;?"), "{html}");
        assert!(html.contains(r#"<form class="poll-form" method="post" action="/api/polls/7/vote"><input type="hidden" name="csrf_token" value="tok"><input type="hidden" name="back" value="/5#c2">"#), "{html}");
        assert!(html.contains(r#"name="option_id" value="11""#), "{html}");
        assert!(html.contains(r#"<li class="poll-option poll-option-voted"><button type="submit" class="poll-option-btn" name="remove" value="1""#), "{html}");
        assert!(html.contains(r#"<progress class="poll-option-bar" max="100" value="67">"#), "{html}");
        assert!(html.contains(r#"<a href="/user/al" title="Al"><img class="poll-voter-avatar" src="/a/x.webp""#), "{html}");
        assert!(html.contains(r#"<form class="poll-delete-form" method="post" action="/api/polls/7/delete">"#), "{html}");
        assert!(html.contains("3 votes") && html.contains("poll-public-badge"), "{html}");
        assert!(!html.contains("style="), "{html}");

        // votes can't be taken back (websrv.comment_polls_unvote: false): the own option is no button
        let mut fixed = sample_poll(true, false);
        fixed["can_unvote"] = false.into();
        let fixed = render(fixed);
        assert!(fixed.contains(r#"<li class="poll-option poll-option-voted"><span class="poll-option-btn">"#), "{fixed}");
        assert!(fixed.contains(r#"name="option_id" value="11""#) && !fixed.contains(r#"name="remove""#), "{fixed}");

        let closed = render(sample_poll(false, true));
        if let Ok(f) = std::env::var("F0CKM_DUMP_POLL") {
            std::fs::write(f, format!("{html}\n{closed}\n")).unwrap();
        }
        assert!(closed.contains(r#"class="comment-poll is-closed has-voted""#), "{closed}");
        assert!(!closed.contains("<form") && !closed.contains("<button"), "{closed}");
        assert!(closed.contains(r#"<div class="poll-form"><ul class="poll-options">"#) && closed.contains("poll-expired-badge"), "{closed}");

        // a busy public option: the newest voters, "+N", and (server-rendered) the dialog with everyone
        let mut busy = sample_poll(true, false);
        let voter = |i: usize| serde_json::json!({ "username": format!("U{i}"), "display_name": if i == 0 { serde_json::json!("Zero") } else { serde_json::Value::Null }, "avatar": null, "avatar_file": null });
        busy["options"][1]["vote_count"] = 7.into();
        busy["options"][1]["voters"] = (0..5).map(voter).collect::<Vec<_>>().into();
        let all = render(busy.clone());
        assert!(all.contains(r##"<a class="poll-voters-more" href="#poll-voters-12" data-option-id="12" title="All voters">+2</a>"##), "{all}");
        assert!(!all.contains("poll-voters-target"), "{all}");
        busy["options"][1]["voters_all"] = (0..7).map(voter).collect::<Vec<_>>().into();
        let page = render(busy);
        assert!(page.contains(r#"<div class="poll-voters-modal poll-voters-target" id="poll-voters-12" role="dialog" aria-label="Garfield">"#), "{page}");
        assert!(page.contains(r##"<a class="poll-voters-close" href="#c2">Close</a>"##) && page.contains("7 votes"), "{page}");
        assert_eq!(page.matches(r#"class="poll-voters-user""#).count(), 7, "{page}");
        assert!(page.contains(r#"<span class="poll-voters-name">Zero</span>"#) && page.contains(r#"<span class="poll-voters-name">U6</span>"#), "{page}");

        assert_eq!(render(serde_json::Value::Null).trim(), "");
    }

    /// Soft Deleted (mod/trash): restore on the left, purge on the right, both usable without
    /// JavaScript (plain form / link to the confirmation form, which offers the delete reason).
    /// F0CKM_DUMP_MOD_TRASH=<dir> writes the pages out.
    #[test]
    fn mod_trash_renders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let card = |id: i64, mime: &str, kind: &str, reason: &str| serde_json::json!({ "id": id, "username": "al", "mime": mime, "kind": kind, "dest": format!("{id}.bin"),
            "title": "A title", "rating": "sfw", "tags": [{ "tag": "sfw", "normalized": "sfw", "badge": "badge-success" }, { "tag": "cat", "normalized": "cat", "badge": "badge-light" }],
            "time_ago": "3 days ago", "time_full": "2026-01-01T00:00:00.000Z", "size_fmt": "1.2 MB", "dims": "1280×720", "original_filename": "cat.mp4",
            "delete_reason": reason, "deleted_by": "mod1", "deleted_ago": "2 hours ago", "deleted_full": "2026-01-03T00:00:00.000Z" });
        let render = |admin: bool, page: i64, purge: serde_json::Value| {
            let ctx = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "modes": ["sfw", "nsfw", "untagged", "all", "nsfl"], "mode": 1,
                "url": { "pathname": "/mod/trash", "search": "", "qs": {} }, "tmp": null, "anon_permissions_json": "{}", "captcha_scopes": [],
                "session": { "user": "bob", "id": 1, "admin": admin, "csrf_token": "tok" },
                "trash": [card(7, "video/mp4", "video", "spam <b>"), card(8, "image/png", "image", "")], "total": 2, "page": page, "pages": 3, "purge": purge });
            env.get_template("mod/trash.html").unwrap().render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{e:#}"))
        };
        let dump = |name: &str, html: &str| {
            if let Ok(dir) = std::env::var("F0CKM_DUMP_MOD_TRASH") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), html).unwrap();
            }
        };

        let admin = render(true, 2, serde_json::Value::Null);
        dump("admin", &admin);
        let restore = admin.find("<form method=\"post\" action=\"/mod/trash/restore\"").expect("restore form");
        let purge = admin.find("href=\"/mod/trash?page=2&amp;purge=7#mq-purge\"").expect("purge link");
        assert!(restore < purge, "restore comes first, purge sits on the right");
        assert!(admin.contains("<input type=\"hidden\" name=\"back\" value=\"/mod/trash?page=2\">"));
        assert!(admin.contains("data-reason=\"spam &lt;b&gt;\""));
        assert!(admin.contains("No reason recorded."));
        assert!(admin.contains("id=\"mq-modal-suggest\""));
        assert!(!admin.contains("id=\"mq-purge\""));

        // moderators restore only
        let moderator = render(false, 1, serde_json::json!({ "id": 7, "username": "al", "delete_reason": "spam" }));
        assert!(moderator.contains("action=\"/mod/trash/restore\""));
        assert!(!moderator.contains("btn-purge\"") && !moderator.contains("id=\"mq-purge\"") && !moderator.contains("btn-purge-trash"));

        // the confirmation form: one item (with the delete reason on offer), then everything
        let one = render(true, 1, serde_json::json!({ "id": 7, "username": "al", "delete_reason": "spam" }));
        dump("purge-one", &one);
        assert!(one.contains("<form method=\"post\" action=\"/mod/trash/purge\" class=\"mq-modal-box\""));
        assert!(one.contains("<input type=\"hidden\" name=\"id\" value=\"7\">"));
        assert!(one.contains("name=\"use_delete_reason\" value=\"1\""));
        assert!(one.contains("<textarea id=\"mq-purge-reason\" name=\"reason\" rows=\"3\" placeholder=\"Enter a reason…\"></textarea>"));
        let no_reason = render(true, 1, serde_json::json!({ "id": 8, "username": "al", "delete_reason": null }));
        assert!(!no_reason.contains("use_delete_reason") && no_reason.contains("placeholder=\"Enter a reason…\" required>"));
        let all = render(true, 1, serde_json::json!({ "all": true }));
        dump("purge-all", &all);
        assert!(all.contains("<form method=\"post\" action=\"/mod/trash/purge-all\" class=\"mq-modal-box\""));
        assert!(!all.contains("mq-purge-reason"));
    }

    /// f0ckgle (the search engine parody frontend): its own pages render for guests and members,
    /// with the forms that make them work without JavaScript (and live with it, f0ckgle.js). F0CKM_DUMP_F0CKGLE=<dir> writes them out.
    #[test]
    fn f0ckgle_pages_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let url = |path: &str, qs: serde_json::Value| serde_json::json!({ "pathname": path, "search": "", "qs": qs });
        let session = serde_json::json!({ "user": "bob", "id": 1, "admin": true, "avatar": 5, "show_motd": true });
        let item = |id: i64| serde_json::json!({ "id": id, "username": "al", "mime": "video/mp4", "tag_id": 1, "thumb": null, "title": null });
        let base = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "modes": ["sfw", "nsfw", "untagged", "all", "nsfl"], "mode": 1,
            "frontend": "f0ckgle", "frontends": ["f0ckm", "f0ckgle"], "frontend_switch": true, "enable_comments": true, "private_messages": true,
            "motd": "hello", "nsfl_tag_id": 3, "anon_permissions_json": "{}", "captcha_scopes": [] });
        let render = |view: &str, data: serde_json::Value| {
            let mut ctx = base.clone();
            ctx.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
            let html = env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            assert!(!html.contains("border-radius"), "{view}");
            html
        };
        let dump = |name: &str, html: &str| {
            if let Ok(dir) = std::env::var("F0CKM_DUMP_F0CKGLE") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), html).unwrap();
            }
        };
        let fe = |v: &str| format!("frontends/f0ckgle/{v}.html");
        let pag = serde_json::json!({ "start": 1, "end": 9, "page": 2, "prev": 1, "next": 3, "cheat": [1, 2, 3, 4] });

        // homepage: logo, search box, both buttons; members get the modes
        let home = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "session": session }));
        dump("home", &home);
        assert!(home.contains("<form class=\"fg-search\" id=\"fg-search\" action=\"/search\""));
        assert!(home.contains("<button type=\"submit\" form=\"fg-search\" class=\"fg-btn\">f0ckgle Search</button>"));
        assert!(home.contains("<a class=\"fg-btn\" href=\"/random\">I'm feeling f0cky</a>"));
        assert!(home.contains("<a href=\"/mode/1\" class=\"active\">NSFW</a>"));
        assert!(home.contains("snippets/frontend-scripts") || home.contains("id=\"footer-data\""));
        let guest = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {} }));
        assert!(guest.contains("<a class=\"fg-signin\" href=\"/?login=1\">Sign in</a>"));

        // image results with the stretched-logo page links
        let grid = render(&fe("index"), serde_json::json!({ "url": url("/tag/cat/p/2", serde_json::json!({})), "tmp": { "tag": "cat" }, "session": session,
            "items": [item(7), item(8)], "total": 1234, "pagination": pag, "link": { "main": "/tag/cat/", "path": "p/", "suffix": "" } }));
        dump("grid", &grid);
        assert!(grid.contains("<a class=\"fg-card\" href=\"/tag/cat/7\""));
        // the type tabs set the media filter cookie (/mime) and come back; the filtered one is active
        assert!(grid.contains("<a href=\"/mime?video=1&amp;back=%2Ftag%2Fcat%2F\">Videos</a>"));
        assert!(grid.contains("<a href=\"/mime?back=%2Ftag%2Fcat%2F\" class=\"active\" aria-current=\"page\">All</a>"));
        let videos = render(&fe("index"), serde_json::json!({ "url": url("/p/1", serde_json::json!({})), "tmp": { "mime": "video" }, "items": [item(7)], "total": 1 }));
        assert!(videos.contains("<a href=\"/mime?video=1&amp;back=%2Fp%2F1\" class=\"active\" aria-current=\"page\">Videos</a>"));
        assert!(videos.contains("<a href=\"/mime?back=%2Fp%2F1\">All</a>"));
        // only the types the site has (allowedMimes → scroller_mime_cats, Flash with enable_swf)
        let only = |cats: serde_json::Value, swf: bool| render(&fe("index"), serde_json::json!({ "url": url("/p/1", serde_json::json!({})), "tmp": {}, "items": [item(7)], "total": 1,
            "scroller_mime_cats": cats, "enable_swf": swf }));
        let two = only(serde_json::json!(["video", "image"]), false);
        assert!(two.contains(">Images</a>") && two.contains(">Videos</a>") && !two.contains(">Audio</a>") && !two.contains(">Flash</a>"));
        assert!(only(serde_json::json!(["video", "image", "audio"]), true).contains("<a href=\"/mime?flash=1&amp;back=%2Fp%2F1\">Flash</a>"));
        let one = only(serde_json::json!(["video"]), false);
        assert!(!one.contains("/mime?video=1"), "one type: no type tabs");
        assert!(one.contains("<a class=\"fg-user-link\" href=\"/mime?back=%2Fp%2F1\""));
        // a type filter doesn't hide the homepage
        assert!(render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": { "mime": "video" } })).contains("fg-home-logo"));
        assert!(grid.contains("<a class=\"fg-pg fg-pg-end\" href=\"/tag/cat/p/1\" rel=\"prev\">"));
        assert!(grid.contains("<span class=\"fg-pg fg-pg-cur\" aria-current=\"page\"><span class=\"fg-pg-l fg-c2\">0</span><span class=\"fg-pg-n\">2</span></span>"));
        let empty = render(&fe("index"), serde_json::json!({ "url": url("/tag/zzz", serde_json::json!({})), "tmp": { "tag": "zzz" }, "items": [], "total": 0 }));
        assert!(empty.contains("did not match any f0cks"));

        // web results
        let search = render(&fe("search"), serde_json::json!({ "url": url("/search", serde_json::json!({ "tag": "cat" })), "searchstring": "cat", "count": 2, "search_mode": "strict",
            "result": [{ "id": 7, "username": "al", "mime": "image/png", "tag": "cat", "score": 0.93, "href": "/search/7?q=cat&match=all" }], "pagination": pag, "link": { "main": "/search/?tag=cat", "path": "&page=" },
            "search_explain": ["tagged \u{201c}cat\u{201d}", "sorted by views"], "search_notes": ["unknown sort \u{201c}x\u{201d}"] }));
        dump("search", &search);
        assert!(search.contains("<p class=\"search-understood fg-search-understood\">Showing f0cks tagged \u{201c}cat\u{201d} · sorted by views</p>"));
        assert!(search.contains("<p class=\"search-notes fg-search-notes\">Left out: unknown sort"));
        assert!(search.contains("<h3 class=\"fg-result-title\"><a href=\"/search/7?q=cat&amp;match=all\">cat - f0ck #7</a></h3>"));
        assert!(search.contains("<input type=\"hidden\" name=\"mode\" value=\"strict\">"));
        assert!(search.contains("Match: 93%"));
        assert!(render(&fe("search"), serde_json::json!({ "url": url("/search", serde_json::json!({})), "searchstring": [] })).contains("fg-home-logo"));

        // item: the actions are plain forms
        let tags = serde_json::json!([{ "id": 1, "tag": "sfw", "normalized": "sfw", "can_exclude": false }, { "id": 9, "tag": "Cat", "normalized": "cat", "can_exclude": true, "is_excluded": true }]);
        let it = serde_json::json!({ "id": 7, "username": "al", "mime": "video/mp4", "dest": "/b/7.mp4", "size": "1 MB", "tags": tags, "timestamp": { "timeago": "1 day ago", "timefull": "x" },
            "favorites": [{ "user": "bob", "avatar": 5 }], "src": { "short": "f0ck.test", "long": "f0ck.test" } });
        // a comment with a poll, and a reply with a closed one (results only)
        let mut closed_poll = sample_poll(false, true);
        closed_poll["id"] = serde_json::json!(8);
        let reply = serde_json::json!({ "id": 4, "username": "bob", "name": "bob", "avatar_src": "/t/5.webp", "content": "yes", "content_html": "yes", "attachments_html": "", "timeago": "now", "iso": "x",
            "poll": closed_poll });
        let comment = serde_json::json!({ "id": 3, "username": "al", "name": "al", "avatar_src": "/t/5.webp", "content": "hi <b>", "content_html": "hi", "attachments_html": "", "timeago": "now", "iso": "x",
            "poll": sample_poll(true, false), "replies": [reply] });
        // in a search (/search/<f0ck>?q=…, routes/search.rs nav): prev/next walk its results, the box
        // keeps the search (and its strict mode), a chip leads back to the results
        let searched = render(&fe("item"), serde_json::json!({ "url": url("/search/7", serde_json::json!({ "q": "cat sort:views", "match": "all" })), "tmp": {}, "session": session,
            "item": it.clone(), "pagination": { "prev": 8, "next": 6 }, "link": { "main": "/search/", "path": "", "suffix": "?q=cat%20sort%3Aviews&match=all" },
            "search_ctx": { "q": "cat sort:views", "strict": true, "position": 23, "total": 40, "page": 2, "results": "/search/?tag=cat%20sort%3Aviews&mode=strict&page=2" } }));
        dump("item-search", &searched);
        assert!(searched.contains("<a class=\"fg-chip\" id=\"fg-prev\" href=\"/search/8?q=cat%20sort%3Aviews&amp;match=all\""));
        assert!(searched.contains("<a class=\"fg-chip\" id=\"fg-next\" href=\"/search/6?q=cat%20sort%3Aviews&amp;match=all\""));
        assert!(searched.contains("<a class=\"fg-chip\" href=\"/search/?tag=cat%20sort%3Aviews&amp;mode=strict&amp;page=2\" title=\"back to the results\">Result 23 of 40</a>"));
        assert!(searched.contains("value=\"cat sort:views\"") && searched.contains("<input type=\"hidden\" name=\"mode\" value=\"strict\">"));
        assert!(searched.contains("&rsaquo; search &rsaquo; cat sort:views"));
        let page = render(&fe("item"), serde_json::json!({ "url": url("/tag/cat/7", serde_json::json!({})), "tmp": { "tag": "cat" }, "session": session, "item": it,
            "pagination": { "prev": 8, "next": 6 }, "link": { "main": "/tag/cat/", "suffix": "" }, "is_mod_or_admin": true, "can_manage_item": true, "can_extract_meta": true,
            "can_rate_item": true, "user_has_favorited": true, "isSubscribed": false, "static_comments": [comment], "phrase": "nice", "enable_comment_polls": true }));
        dump("item", &page);
        // comment polls: the shared markup after the comment's text (plain vote forms back to the
        // comment), results only on the closed one, the poll fields in the comment form, the CSS
        assert!(page.contains(r#"<div class="fg-comment-poll"><div class="comment-poll has-voted" id="poll-7" data-poll-id="7" data-comment-id="2">"#), "{page}");
        assert!(page.contains(r#"<form class="poll-form" method="post" action="/api/polls/7/vote"><input type="hidden" name="csrf_token" value="tok"><input type="hidden" name="back" value="/tag/cat/7#c3">"#), "{page}");
        assert!(page.contains(r#"<form class="poll-delete-form" method="post" action="/api/polls/7/delete">"#));
        assert!(page.contains(r#"<div class="comment-poll is-closed has-voted" id="poll-8""#) && !page.contains("/api/polls/8/vote"));
        assert!(page.contains(r#"<details class="poll-builder-nojs">"#) && page.contains(r#"name="poll_question""#) && page.contains(r#"name="poll_options""#));
        assert!(page.contains(r#"<textarea name="content" rows="3" placeholder="Join the discussion" maxlength"#) || page.contains(r#"<textarea name="content" rows="3" placeholder="Join the discussion">"#), "a comment may be just a poll");
        assert!(page.contains("/s/css/comment-poll.css?v="));
        assert!(!home.contains("/s/css/comment-poll.css"), "no comments, no poll CSS");
        assert!(page.contains("<a class=\"fg-chip\" id=\"fg-prev\" href=\"/tag/cat/8\""));
        // mods edit comments in place (as in f0ckm): Edit opens the form in the text's place (#edit-c3,
        // :target without JavaScript), the text escaped in the field, with the emoji picker
        assert!(page.contains("<form method=\"post\" action=\"/api/comments/3/edit\" class=\"fg-comment-edit fg-ajax\" id=\"edit-c3\" data-fg-refresh=\"comments\" data-fg-edit>"));
        assert!(page.contains("aria-label=\"Edit comment #3\">hi &lt;b&gt;</textarea>"));
        assert!(page.contains("<a href=\"#edit-c3\" class=\"fg-edit-link\" data-edit=\"3\">Edit</a>"));
        assert!(page.contains("fg-emoji-btn\" aria-expanded=\"false\" aria-controls=\"fg-emoji-box-e3\""));
        assert!(page.contains("fg-edit-cancel\" href=\"#c3\">Cancel</a>"));
        assert!(page.contains("<input type=\"hidden\" name=\"action\" value=\"remove\">"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/item/7/rating\" class=\"fg-inline fg-ajax\" data-fg-refresh=\"kp\">"));
        assert!(page.contains("class=\"f0ck-exclude-form is-excluded\" data-tag-id=\"9\""));
        assert!(page.contains("<form method=\"post\" action=\"/api/comments\" class=\"fg-comment-form fg-ajax\" id=\"comment-form\" data-fg-refresh=\"comments comments-head\" data-fg-comment>"));
        // with JavaScript the forms go through fetch and these regions are swapped in (f0ckgle.js)
        for live in ["actions", "kp", "comments", "comments-head"] {
            assert_eq!(page.matches(&format!("data-fg-live=\"{live}\"")).count(), 1, "{live}");
        }
        assert!(page.contains("data-reply=\"3\" data-name=\"al\""));
        // custom emojis: the picker (JavaScript) under the comment field
        assert!(page.contains("class=\"fg-chip fg-chip-small js-only fg-emoji-btn\""));
        assert!(page.contains("<div class=\"fg-emoji-box\" id=\"fg-emoji-box\" hidden></div>"));
        assert!(page.contains("<div id=\"report\" class=\"fg-modal\">"));
        assert!(page.contains("<div id=\"delete\" class=\"fg-modal\">"));
        assert!(page.contains("\"itemId\": 7"));
        // Edit: everything of f0ckm's info box (visibility, expiration, the file, tools) as plain forms
        assert!(page.contains("<a class=\"fg-chip\" href=\"#edit\">Edit</a>") && page.contains("<div id=\"edit\" class=\"fg-modal\" data-fg-live=\"edit\">"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/item/visibility\" class=\"fg-edit-vis fg-ajax\""));
        assert!(page.contains("name=\"visibility\" value=\"0\" checked>") && page.contains("name=\"visibility\" value=\"3\">"), "staff: 451 too");
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/items/7/expiry\" class=\"fg-edit-exp fg-ajax\"") && page.contains("<option value=\"custom\">"));
        assert!(page.contains("<p>Never: it stays.</p>"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/items/7/rethumb\" class=\"fg-edit-row fg-ajax\""));
        assert!(page.contains("<a class=\"fg-chip\" href=\"/tag/cat/7?meta=1#meta\" data-fg-meta>Tag suggestions</a>"));
        assert!(page.contains("<div id=\"meta\" class=\"fg-modal\" data-fg-live=\"meta\">") && page.contains("Reading the file's metadata"));
        for live in ["title", "edit", "edit-vis", "edit-exp", "meta"] {
            assert_eq!(page.matches(&format!("data-fg-live=\"{live}\"")).count(), 1, "{live}");
        }
        // the uploader: no 451; a private, expiring item; ?meta=1 brings the tag suggestions
        let mut own = it.clone();
        own["visibility"] = serde_json::json!(2);
        own["expires_at"] = serde_json::json!("1900000000");
        own["expires_countdown"] = serde_json::json!("1d 2h");
        let mine = render(&fe("item"), serde_json::json!({ "url": { "pathname": "/7", "search": "?meta=1", "qs": {} }, "tmp": {}, "session": session, "item": own,
            "pagination": {}, "link": { "main": "/", "suffix": "" }, "can_manage_item": true, "can_extract_meta": true, "meta_fields": ["cat", "holiday 2024"] }));
        dump("item-mine", &mine);
        assert!(mine.contains("name=\"visibility\" value=\"2\" checked>") && !mine.contains("name=\"visibility\" value=\"3\""));
        assert!(mine.contains("<dt>Visibility</dt><dd>private</dd>"));
        assert!(mine.contains("<b class=\"fg-countdown\" data-countdown=\"1900000000\">1d 2h</b>"));
        assert!(mine.contains("<span class=\"fg-chip fg-meta-tag is-added\">&#10003; cat</span>"), "already a tag (Cat)");
        assert!(mine.contains("<input type=\"hidden\" name=\"back\" value=\"/7?meta=1#meta\">") && mine.contains("<input type=\"hidden\" name=\"tagname\" value=\"holiday 2024\">"));
        assert!(mine.contains("href=\"/7?meta=1#meta\" data-fg-meta") && !mine.contains("Reading the file's metadata"));
        let failed = render(&fe("item"), serde_json::json!({ "url": url("/7", serde_json::json!({})), "tmp": {}, "session": session, "item": it.clone(),
            "pagination": {}, "link": { "main": "/", "suffix": "" }, "can_manage_item": true, "can_extract_meta": true, "meta_error": "Extraction failed" }));
        assert!(failed.contains("<p class=\"fg-dim\">Extraction failed</p>"));
        assert!(!failed.contains("href=\"#halls\""), "no hall lists: no halls dialog");

        // halls: the item's "Save to a hall" dialog (a form per hall, the opposite of has_item; the
        // site's halls for staff) and a user's halls page with its edit dialogs, all plain forms
        let halls = render(&fe("item"), serde_json::json!({ "url": url("/7", serde_json::json!({})), "tmp": {}, "session": session, "item": it.clone(),
            "pagination": {}, "link": { "main": "/", "suffix": "" }, "is_mod_or_admin": true,
            "my_hall_choices": [{ "slug": "cats", "name": "Cats", "is_private": false, "has_item": true, "total_items": "3" },
                                { "slug": "secret", "name": "Secret", "is_private": true, "has_item": false, "total_items": "0" }],
            "hall_choices": [{ "slug": "best", "name": "Best", "has_item": false }] }));
        dump("item-halls", &halls);
        assert!(halls.contains("<a class=\"fg-chip active\" href=\"#halls\" title=\"Add to one of your lists\">&#10003; In 1 list</a>"));
        assert!(halls.contains("<div id=\"halls\" class=\"fg-modal\">") && halls.contains("data-fg-live=\"halls-pick\""));
        assert!(halls.contains("<input type=\"hidden\" name=\"hall\" value=\"cats\">\n          <input type=\"hidden\" name=\"action\" value=\"remove\">"));
        assert!(halls.contains("<input type=\"hidden\" name=\"hall\" value=\"secret\">\n          <input type=\"hidden\" name=\"action\" value=\"add\">"));
        assert!(halls.contains("aria-pressed=\"true\" title=\"Remove from Cats\"") && halls.contains("<span class=\"fg-hall-private\">Private</span>"));
        assert!(halls.contains("<input type=\"text\" name=\"new_name\""));
        assert!(halls.contains("<form method=\"post\" action=\"/mod/lists/add\" class=\"fg-ajax\" data-fg-refresh=\"halls-pick kp\""));
        assert!(halls.contains("<input type=\"hidden\" name=\"back\" value=\"/7#halls\">"));
        assert!(halls.contains("<a href=\"/user/bob/lists\" class=\"fg-btn-text\">Manage lists</a>"));
        let al_halls = serde_json::json!({ "user": "al", "id": 2 });
        let uh = render(&fe("user-halls"), serde_json::json!({ "url": url("/user/al/lists", serde_json::json!({})), "session": session, "user": al_halls, "ownerUser": al_halls,
            "isOwner": false, "can_manage": true, "manage_qs": "?user_id=2", "enable_userhall_image_upload": true, "userhalls_enabled": true,
            "hallsList": [{ "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": "3" }] }));
        dump("user-halls-manage", &uh);
        assert!(uh.contains("<h1 class=\"fg-kp-title\">Lists by <a href=\"/user/al\">al</a></h1>"));
        assert!(uh.contains("<a class=\"fg-card\" href=\"/user/al/list/mine\">") && uh.contains("src=\"/user_list_image/2/mine?m=0&v=5\""));
        assert!(uh.contains("<form method=\"post\" action=\"/api/v2/me/lists?user_id=2\" class=\"fg-modal-box fg-ajax\""));
        assert!(uh.contains("<div id=\"hall-mine\" class=\"fg-modal\">") && uh.contains("<form method=\"post\" action=\"/api/v2/me/lists/mine?user_id=2\" class=\"fg-hall-form fg-ajax\""));
        assert!(uh.contains("name=\"is_private\" value=\"1\" checked>") && uh.contains("name=\"is_private\" value=\"0\">"));
        assert!(uh.contains("action=\"/api/v2/me/lists/mine/image?user_id=2\" enctype=\"multipart/form-data\""));
        assert!(uh.contains("action=\"/api/v2/me/lists/mine/image/delete?user_id=2\""));
        assert!(uh.contains("<div id=\"hall-mine-delete\" class=\"fg-modal\">") && uh.contains("action=\"/api/v2/me/lists/mine/delete?user_id=2\""));
        for live in ["halls", "hall-dialogs", "hall-mine"] {
            assert_eq!(uh.matches(&format!("data-fg-live=\"{live}\"")).count(), 1, "{live}");
        }
        let guest = render(&fe("user-halls"), serde_json::json!({ "url": url("/user/al/lists", serde_json::json!({})), "user": al_halls, "ownerUser": al_halls,
            "isOwner": false, "can_manage": false, "manage_qs": "", "hallsList": [] }));
        dump("user-halls-guest", &guest);
        assert!(guest.contains("al has no public lists.") && !guest.contains("#new-hall") && !guest.contains("/api/v2/me/lists"));

        // profile, error, sign-in
        let user = render(&fe("user"), serde_json::json!({ "url": url("/user/al", serde_json::json!({})), "session": session,
            "user": { "user": "al", "id": 2, "timestamp": { "timeago": "1 year ago", "timefull": "x" } }, "count": { "f0cks": 2, "favs": 0 },
            "f0cks": { "items": [item(7)], "link": { "main": "/user/al/uploads/" } } }));
        dump("user", &user);
        assert!(user.contains("<a class=\"fg-card\" href=\"/user/al/uploads/7\""));
        let error = render(&fe("error"), serde_json::json!({ "message": "gone", "url": url("/x", serde_json::json!({})) }));
        dump("error", &error);
        assert!(error.contains("That's an error."));
        let login = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({ "login": "1" })), "error": "wrong password" }));
        dump("login", &login);
        assert!(login.contains("<form class=\"fg-signin-card\" method=\"post\" action=\"/login\">"));
        assert!(login.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"tok\">"));
        assert!(login.contains("wrong password"));

        // the other own pages (tpl::FRONTEND_OWN_PAGES): none falls back to f0ckm's layout
        let tag = serde_json::json!({ "tag": "Cat", "safe_tag": "cat", "encoded_tag": "Cat", "total_items": 3 });
        let al = serde_json::json!({ "user": "al", "id": 2 });
        let pages = [
            ("about", serde_json::json!({ "about_text": "" })),
            ("rules", serde_json::json!({ "rules_text": "# Be nice", "rules_text_b64": "IyBCZSBuaWNl" })),
            ("terms", serde_json::json!({})),
            ("tags", serde_json::json!({ "toptags": [tag], "session": session })),
            ("tags_user", serde_json::json!({ "toptags": [tag], "tagger": "al", "user": al, "pagination": pag, "link": { "main": "/user/al/tags", "path": "?page=" } })),
            ("halls", serde_json::json!({ "hallsList": [{ "slug": "best", "name": "Best", "rating": "nsfw", "total_items": 4, "description": "the best" }] })),
            // a user's halls, managed by their owner (forms for each hall, the new-hall form)
            ("user-halls", serde_json::json!({ "session": session, "user": al, "ownerUser": al, "isOwner": true, "can_manage": true, "manage_qs": "",
                "enable_userhall_image_upload": true, "userhalls_enabled": true,
                "hallsList": [{ "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": 3 },
                              { "id": 2, "slug": "empty", "name": "Empty", "description": null, "is_private": false, "custom_image": false, "total_items": 0 }] })),
            ("subscriptions", serde_json::json!({ "session": session, "items": [{ "id": 7, "user": "al", "thumb": "/t/7.webp" }], "totalCount": 1,
                "pagination": { "start": 1, "end": 1, "page": 1 }, "link": { "main": "/subscriptions", "path": "?page=" } })),
            ("notifications", serde_json::json!({ "session": session, "activeTab": "user", "tabUnread": { "user": 1 },
                "notifications": [{ "type": "comment_reply", "from_user": "al", "item_id": 7, "reference_id": 3, "is_read": false, "created_at": "2026-11-15T10:00:00Z" }] })),
            ("ranking", serde_json::json!({ "list": [{ "user": "al", "count": 9 }], "hoster": [{ "part": "youtube.com", "c": 4 }], "favotop": [{ "item_id": 7, "favs": 2 }],
                "stats": { "total": 10 }, "totalComments": 1, "totalFavs": 2, "totalUsers": 3, "totalAnonUsers": 0, "diskSize": "1 GB" })),
            ("comments_user", serde_json::json!({ "user": al, "comments_page": 2, "has_more": true,
                "static_comments": [{ "id": 3, "item_id": 7, "item_key": "7", "content_html": "<b>hi</b>", "attachments_html": "", "timeago": "now", "iso": "x", "poll": sample_poll(true, false) }] })),
            ("upload", serde_json::json!({ "session": session, "mimes_json": "{}", "uploads_remaining": null, "upload_types": [], "min_tags": 1 })),
            // f0ckgle's own page (no other frontend has it: routes/index.rs how_search_works)
            ("howsearch", serde_json::json!({})),
        ];
        for (view, data) in pages {
            let mut data = data;
            data["url"] = url(&format!("/{view}"), serde_json::json!({}));
            let html = render(&fe(view), data);
            dump(view, &html);
            assert!(html.contains("/s/fe/f0ckgle/css/f0ckgle.css") && html.contains("class=\"fg-top f0ck-shell-chrome\""), "{view}: not in f0ckgle's layout");
            assert!(!html.contains("/s/css/f0ckm.css"), "{view}");
            if view == "comments_user" {
                assert!(html.contains(r#"<input type="hidden" name="back" value="/user/al/comments?page=2#c3">"#) && html.contains("/s/css/comment-poll.css?v="), "{html}");
            }
        }
        for page in FRONTEND_OWN_PAGES {
            assert!(root.join("views/frontends/f0ckgle").join(format!("{page}.html")).is_file(), "{page}");
        }

        // a shared page in the shell
        let shell = render("settings.html", serde_json::json!({ "url": url("/settings", serde_json::json!({})), "session": session,
            "shell_header": "frontends/f0ckgle/shell/header.html", "shell_footer": "frontends/f0ckgle/shell/footer.html" }));
        dump("settings", &shell);
        assert!(shell.contains("class=\"no-js f0ck-shell scheme-"));
        assert!(shell.contains("<div class=\"f0ck-shell-page\">"));
    }

    /// f0ckv2's media filter goes through /mime (the `mime` cookie wins over a /video path, so plain
    /// /video links showed mixed items while f0ckm's picker had set one): back to the listing, an
    /// item page back to itself; only the types the site offers.
    /// f0ckv2's halls: a user's halls page (cards; for the owner / an admin the forms to create, edit,
    /// picture and delete, as :target boxes) and the item's halls box (add / remove per hall, a new
    /// hall, the site halls for staff). F0CKM_DUMP_F0CKV2_HALLS=<dir> writes them out.
    #[test]
    fn f0ckv2_halls_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let al = serde_json::json!({ "user": "al", "id": 2 });
        let base = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "frontend": "f0ckv2", "enable_comments": true,
            "themes_json": "[]", "custom_brand_images_json": "[]", "allowed_comment_images_json": "[]", "nsfl_tag_id": 3, "anon_permissions_json": "{}", "captcha_scopes": [],
            "userhalls_enabled": true, "halls_enabled": true, "enable_userhall_image_upload": true, "modes": ["sfw", "nsfw", "untagged", "all"],
            "url": { "pathname": "/user/al/lists", "search": "", "qs": {} }, "user": al, "ownerUser": al,
            "hallsList": [{ "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": "3" },
                          { "id": 2, "slug": "empty", "name": "Empty", "description": null, "is_private": false, "custom_image": false, "total_items": 0 }] });
        let render = |view: &str, data: serde_json::Value| {
            let mut ctx = base.clone();
            ctx.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
            env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{view}: {e:#}"))
        };
        let dump = |name: &str, html: &str| {
            if let Ok(dir) = std::env::var("F0CKM_DUMP_F0CKV2_HALLS") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), html).unwrap();
            }
        };
        let page = "frontends/f0ckv2/user-halls.html";
        let admin = render(page, serde_json::json!({ "session": { "user": "bob", "id": 1, "admin": true, "mode": 1, "ruffle_volume": 0.5 }, "isOwner": false, "can_manage": true, "manage_qs": "?user_id=2" }));
        dump("manage", &admin);
        assert!(admin.contains("<form method=\"post\" action=\"/api/v2/me/lists?user_id=2\" class=\"v2-form v2-hall-form v2-uhall-create\""));
        assert!(admin.contains("<form method=\"post\" action=\"/api/v2/me/lists/mine?user_id=2\" class=\"v2-form v2-hall-form v2-uhall-edit\""));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine/image?user_id=2\" enctype=\"multipart/form-data\""));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine/image/delete?user_id=2\""), "remove only with a custom picture");
        assert!(!admin.contains("/api/v2/me/lists/empty/image/delete"));
        assert!(admin.contains("action=\"/api/v2/me/lists/empty/delete?user_id=2\""));
        assert!(admin.contains("<input type=\"radio\" name=\"is_private\" value=\"1\" checked>"));
        assert!(admin.contains("<input type=\"radio\" name=\"is_private\" value=\"0\" checked>"));
        assert!(admin.contains("<input type=\"hidden\" name=\"back\" value=\"/user/al/lists\">"));
        assert!(admin.contains("href=\"#hall-mine-delete\"") && admin.contains("<div id=\"hall-mine-delete\" class=\"v2-modal\">"));
        assert!(admin.contains("--thumb: url('/user_list_image/2/mine?m=1&v=5')"));
        assert!(admin.contains("<span class=\"badge badge-light v2-uhall-lock\">private</span>"));
        assert!(admin.contains("<span class=\"badge badge-dark\">3 f0cks</span>"));
        assert!(admin.contains("/s/fe/f0ckv2/css/f0ckv2.css") && !admin.contains("/s/css/f0ckm.css"));
        assert!(!admin.contains("border-radius") && !admin.contains("<script>"));
        let guest = render(page, serde_json::json!({ "isOwner": false, "can_manage": false, "manage_qs": "" }));
        dump("guest", &guest);
        assert!(guest.contains("<a class=\"v2-uhall-name\" href=\"/user/al/list/empty\">Empty</a>"));
        assert!(!guest.contains("v2-hall-form") && !guest.contains("v2-modal\""));

        // the item's halls box
        let item = serde_json::json!({ "id": 7, "username": "al", "mime": "image/png", "dest": "/b/7.png", "tags": [], "favorites": [],
            "timestamp": { "timeago": "now", "timefull": "x" }, "src": { "long": "", "short": null },
            "primaryHall": { "slug": "best", "name": "Best" }, "otherHalls": [] });
        let item_ctx = serde_json::json!({ "item": item, "session": { "user": "bob", "id": 1, "ruffle_volume": 0.5 }, "url": { "pathname": "/7", "search": "", "qs": {} },
            "pagination": {}, "link": { "main": "/", "suffix": "" }, "is_mod_or_admin": true,
            "my_hall_choices": [{ "slug": "mine", "image_v": "5", "name": "Mine", "is_private": true, "total_items": "3", "has_item": true },
                                { "slug": "other", "name": "Other", "is_private": false, "total_items": 0, "has_item": false }],
            "hall_choices": [{ "slug": "best", "name": "Best", "has_item": true }, { "slug": "meh", "name": "Meh", "has_item": false }] });
        let html = render("frontends/f0ckv2/item.html", item_ctx.clone());
        dump("item", &html);
        assert!(html.contains("<a href=\"#halls\" class=\"v2-iconlink\" id=\"a_halls\""));
        assert!(html.contains("<div id=\"halls\" class=\"v2-modal\">"));
        assert!(html.contains("<input type=\"hidden\" name=\"back\" value=\"/7#halls\">"));
        assert!(html.contains("<input type=\"hidden\" name=\"hall\" value=\"mine\">\n            <input type=\"hidden\" name=\"action\" value=\"remove\">"));
        assert!(html.contains("<input type=\"hidden\" name=\"hall\" value=\"other\">\n            <input type=\"hidden\" name=\"action\" value=\"add\">"));
        assert!(html.contains("name=\"new_name\""));
        assert!(html.contains("action=\"/mod/lists/remove\"") && html.contains("action=\"/mod/lists/add\""));
        assert!(html.contains("<a class=\"badge badge-light\" href=\"/list/best\">list: Best</a>"));
        assert!(html.contains("data-v2-live=\"extra\"") && html.contains("data-v2-live=\"hallbox\""));
        // the Onara viewer's item box has it too
        let onara = render("frontends/f0ckv2/ajax-item.html", item_ctx);
        assert!(onara.contains("<div id=\"halls\" class=\"v2-modal\">"));
        // a guest: no box
        let guest_item = render("frontends/f0ckv2/item.html", serde_json::json!({ "item": item, "url": { "pathname": "/7", "search": "", "qs": {} }, "pagination": {}, "link": { "main": "/", "suffix": "" } }));
        assert!(!guest_item.contains("id=\"halls\"") && !guest_item.contains("a_halls"));

        // comment polls: the shared poll markup in the server-rendered comments (back to the open
        // comments box), the poll fields in the comment form, the shared CSS
        let polled = serde_json::json!({ "id": 2, "username": "Al", "name": "Al", "avatar_src": "/t/5.webp", "content_html": "vote", "attachments_html": "",
            "timeago": "now", "iso": "x", "poll": sample_poll(true, false), "replies": [] });
        let html = render("frontends/f0ckv2/item.html", serde_json::json!({ "item": item, "session": { "user": "bob", "id": 1, "ruffle_volume": 0.5 },
            "url": { "pathname": "/7", "search": "", "qs": {} }, "pagination": {}, "link": { "main": "/", "suffix": "" },
            "enable_comment_polls": true, "static_comments": [polled] }));
        dump("item-poll", &html);
        assert!(html.contains("<div class=\"v2-comment-body v2-comment-html\">vote<div class=\"comment-poll has-voted\" id=\"poll-7\""), "{html}");
        assert!(html.contains("<form class=\"poll-form\" method=\"post\" action=\"/api/polls/7/vote\"><input type=\"hidden\" name=\"csrf_token\" value=\"tok\"><input type=\"hidden\" name=\"back\" value=\"/7?comments=1#c2\">"), "{html}");
        assert!(html.contains("<details class=\"poll-builder-nojs\">") && html.contains("name=\"poll_question\"") && html.contains("name=\"poll_options\""), "{html}");
        assert!(!html.contains("placeholder=\"say something f0cked...\" required"), "a poll may come without text");
        assert!(html.contains("/s/css/comment-poll.css?v="));
        let user_comments = render("frontends/f0ckv2/comments_user.html", serde_json::json!({ "url": { "pathname": "/user/al/comments", "search": "", "qs": {} },
            "comments": [{ "id": 2, "item_id": 5, "content": "vote", "created_at": "2024-01-01T00:00:00Z", "files": [], "poll": sample_poll(false, true) }] }));
        dump("comments-user-poll", &user_comments);
        assert!(user_comments.contains("class=\"comment-poll is-closed has-voted\" id=\"poll-7\""), "{user_comments}");
    }

    #[test]
    fn f0ckv2_filter_sets_the_mime_cookie() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let session = serde_json::json!({ "user": "bob", "id": 1 });
        let render = |data: serde_json::Value| {
            let mut ctx = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "session": session, "frontend": "f0ckv2",
                "scroller_mime_cats": ["video", "image", "audio"], "filter_mimes": ["image", "video", "audio", "flash"] });
            ctx.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
            env.get_template("frontends/f0ckv2/snippets/navbar.html").unwrap().render(to_jvalue(&ctx)).unwrap()
        };
        let tag = render(serde_json::json!({ "url": { "pathname": "/tag/cat/p/2", "search": "", "qs": {} }, "tmp": { "tag": "cat", "mime": "video" } }));
        assert!(tag.contains("<a href=\"/mime?back=%2Ftag%2Fcat%2F\">all</a>"), "{tag}");
        assert!(tag.contains("<a href=\"/mime?video=1&amp;back=%2Ftag%2Fcat%2F\" class=\"active\">video</a>"));
        assert!(tag.contains("<a href=\"/mime?audio=1&amp;back=%2Ftag%2Fcat%2F\">audio</a>"));
        assert!(!tag.contains("flash=1"), "no Flash without enable_swf");
        assert!(!tag.contains("href=\"/tag/cat/video\""));
        let item = render(serde_json::json!({ "url": { "pathname": "/tag/cat/7", "search": "", "qs": {} }, "tmp": { "tag": "cat" }, "item": { "id": 7 } }));
        assert!(item.contains("<a href=\"/mime?back=%2Ftag%2Fcat%2F7\" class=\"active\">all</a>"), "{item}");
        assert!(item.contains("<a href=\"/mime?image=1&amp;back=%2Ftag%2Fcat%2F7\">image</a>"));
        let one = render(serde_json::json!({ "url": { "pathname": "/", "search": "", "qs": {} }, "tmp": {}, "scroller_mime_cats": ["video"] }));
        assert!(!one.contains("/mime?"), "one type: nothing to filter");
    }

    /// f0ckYou (the video site parody): its own pages render for guests and members, with the forms
    /// that make them work without JavaScript (and live with it, f0ckyou.js). F0CKM_DUMP_F0CKYOU=<dir> writes them out.
    #[test]
    fn f0ckyou_pages_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let url = |path: &str, qs: serde_json::Value| serde_json::json!({ "pathname": path, "search": "", "qs": qs });
        let session = serde_json::json!({ "user": "bob", "id": 1, "admin": true, "avatar": 5, "show_motd": true });
        let item = |id: i64| {
            serde_json::json!({ "id": id, "username": "al", "display_name": "Al", "username_color": "#f00", "mime": "video/mp4", "tag_id": 1,
                "rating_class": "sfw", "thumb": null, "title": null, "stamp": 1_794_700_000, "my_views": 1, "views": 1290, "duration": 75.4, "dest": "abc.mp4" })
        };
        let base = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "modes": ["sfw", "nsfw", "untagged", "all", "nsfl"], "mode": 1,
            "frontend": "f0ckyou", "frontends": ["f0ckm", "f0ckyou"], "frontend_switch": true, "enable_comments": true, "private_messages": true,
            "motd": "hello", "nsfl_tag_id": 3, "anon_permissions_json": "{}", "captcha_scopes": [] });
        let render = |view: &str, data: serde_json::Value| {
            let mut ctx = base.clone();
            ctx.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
            let html = env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            assert!(!html.contains("border-radius"), "{view}");
            // no inline styles but the database values (user colour, banner) as custom properties
            // (the shared pages in the shell have their own)
            for style in html.split("style=\"").skip(1).filter(|_| view.starts_with("frontends/f0ckyou/")) {
                let value = style.split('"').next().unwrap();
                assert!(value.starts_with("--user-color: ") || value.starts_with("--fy-banner: url("), "{view}: inline style {value}");
            }
            html
        };
        let dump = |name: &str, html: &str| {
            if let Ok(dir) = std::env::var("F0CKM_DUMP_F0CKYOU") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), html).unwrap();
            }
        };
        let fe = |v: &str| format!("frontends/f0ckyou/{v}.html");
        let pag = serde_json::json!({ "start": 1, "end": 9, "page": 2, "prev": 1, "next": 3, "cheat": [1, 2, 3, 4] });

        // home: masthead, guide (opens without JavaScript: a checkbox and labels), chips, cards
        let home = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "session": session,
            "items": [item(7), item(8)], "total": 2, "pagination": pag, "link": { "main": "/", "path": "p/", "suffix": "" } }));
        dump("home", &home);
        assert!(home.contains("<body class=\"fy-page-home\">"));
        assert!(home.contains("<input type=\"checkbox\" id=\"fy-guide-toggle\""));
        assert!(home.contains("<label for=\"fy-guide-toggle\" class=\"fy-iconbtn fy-guide-btn\""));
        assert!(home.contains("<label for=\"fy-guide-toggle\" class=\"fy-scrim f0ck-shell-chrome\""));
        assert!(home.contains("<form class=\"fy-search\" id=\"fy-search\" action=\"/search\" method=\"get\" role=\"search\">"));
        assert!(home.contains("<li><a href=\"/\" class=\"active\" aria-current=\"page\">"), "Home is the active guide entry");
        assert!(home.contains("<a href=\"/mode/1\" class=\"active\" aria-current=\"true\">NSFW</a>"));
        assert!(home.contains("<sup class=\"fy-logo-cc\">NSFW</sup>"));
        // no f0ckYou brand set: the finger mark and "f0ck*You*", not marked for the site brand (f0ck-core.js)
        assert!(home.contains("<span class=\"fy-logo fy-logo-own\" data-fy-logo><span class=\"fy-logo-mark\" aria-hidden=\"true\"></span><span class=\"fy-logo-word\">f0ck<b>You</b></span><sup class=\"fy-logo-cc\">NSFW</sup></span>"));
        assert!(!home.contains("data-brand-text"));
        // f0ckYou's own brand (admin panel → f0ckYou brand): picture, emphasized text, font, weight, size
        let own = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "custom_navbar_brand_text": "SiteBrand",
            "fy_brand": { "site": false, "image": "/s/img/navbar/fybrand.1.webp", "mark": true, "text": "my*Tube*", "parts": brand_parts("my*Tube*"),
                "font": "VCR.ttf", "weight": 700, "size": 150, "font_css": "/fy-brand-font.css?v=1" } }));
        assert!(own.contains("<span class=\"fy-logo fy-logo-own fy-logo-custom fy-bsize-150\" data-fy-logo><img class=\"fy-logo-img\" src=\"/s/img/navbar/fybrand.1.webp\" alt=\"myTube\"><span class=\"fy-logo-word fy-bfont fy-bweight-700\">my<b>Tube</b></span></span>"), "{own}");
        assert!(own.contains("<link rel=\"stylesheet\" id=\"fy-brand-font-css\" href=\"/fy-brand-font.css?v=1\">"));
        assert!(!own.contains("SiteBrand") && !own.contains("fy-logo-mark"));
        // the Abyss in the guide, when it is on
        assert!(!own.contains("href=\"/abyss\""));
        let abyss = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "abyss_enabled": true }));
        assert!(abyss.contains("<a href=\"/abyss\"") && abyss.contains("icons.svg?v=") && abyss.contains("#abyss\""), "abyss link");
        // "use the site's brand": the site brand's picture and text in its font, no finger mark
        let branded = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "custom_brand_image": "/s/brand.png",
            "fy_brand": { "site": true, "font_css": "/fy-brand-font.css?v=1" },
            "custom_navbar_brand_text": "MyTube", "brand_font": "VCR", "brand_weight": 700, "brand_case": "upper", "brand_font_css": "/brand-font.css?f=VCR" }));
        assert!(branded.contains("<span class=\"fy-logo navbar-brand fy-logo-custom\" data-fy-logo>") && branded.contains("src=\"/s/brand.png\">"));
        assert!(!branded.contains("fy-brand-font-css"));
        assert!(branded.contains("class=\"fy-logo-word brand-case-upper brand-font brand-weight-700\" data-brand-text data-text=\"MyTube\" data-fallback=\"f0ckYou\">MyTube</span>"));
        assert!(branded.contains("<link rel=\"stylesheet\" id=\"brand-font-css\" href=\"/brand-font.css?f=VCR\">"));
        assert!(home.contains("<article class=\"fy-card\" data-id=\"7\">") && home.contains("<a class=\"fy-thumb\" href=\"/7\""));
        assert!(home.contains("<span class=\"fy-watched\" title=\"watched\"></span>"));
        // the play time on the thumbnail, and the video to preview on hover
        assert!(home.contains("<span class=\"fy-badges\"><span class=\"fy-tbadge fy-time\">1:15</span></span>"));
        assert!(home.contains("<a class=\"fy-thumb\" href=\"/7\" tabindex=\"-1\" aria-hidden=\"true\" data-preview=\"/b/abc.mp4\">"));
        assert!(home.contains("class=\"fy-ava fy-ava-letter fy-ava-color\" style=\"--user-color: #f00\""));
        assert!(home.contains("<div class=\"fy-grid\" data-fy-feed>"));
        assert!(home.contains("<a class=\"fy-pg fy-pg-end\" href=\"/p/3\" rel=\"next\">"));
        assert!(home.contains("<span class=\"fy-pg fy-pg-cur\" aria-current=\"page\">2</span>"));
        assert!(home.contains("/s/fe/f0ckyou/img/icons.svg?v="));
        assert!(home.contains("snippets/frontend-scripts") || home.contains("id=\"footer-data\""));
        let guest = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {} }));
        assert!(guest.contains("<a class=\"fy-btn-signin\" href=\"/?login=1\">"));
        assert!(!guest.contains("fy-account") && !guest.contains("fy-logo-cc"));

        // the media chips set the filter cookie (/mime) and come back; the filtered one is active
        let tag = render(&fe("index"), serde_json::json!({ "url": url("/tag/cat/p/2", serde_json::json!({})), "tmp": { "tag": "cat" }, "session": session,
            "items": [item(7)], "total": 1234, "pagination": pag, "link": { "main": "/tag/cat/", "path": "p/", "suffix": "" } }));
        dump("tag", &tag);
        assert!(tag.contains("<h1 class=\"fy-listhead-title\">#cat</h1>"));
        assert!(tag.contains("<a class=\"fy-thumb\" href=\"/tag/cat/7\""));
        assert!(tag.contains("<a class=\"fy-chip\" href=\"/mime?video=1&amp;back=%2Ftag%2Fcat%2F\">Videos</a>"));
        assert!(tag.contains("<a class=\"fy-chip active\" href=\"/mime?back=%2Ftag%2Fcat%2F\" aria-current=\"page\">All</a>"));
        assert!(!tag.contains("fy-page-home") && !tag.contains("class=\"fy-motd\""));
        let videos = render(&fe("index"), serde_json::json!({ "url": url("/p/1", serde_json::json!({})), "tmp": { "mime": "video" }, "items": [item(7)], "total": 1 }));
        assert!(videos.contains("<a class=\"fy-chip active\" href=\"/mime?video=1&amp;back=%2Fp%2F1\" aria-current=\"page\">Videos</a>"));
        let one = render(&fe("index"), serde_json::json!({ "url": url("/p/1", serde_json::json!({})), "tmp": {}, "items": [item(7)], "total": 1,
            "scroller_mime_cats": ["video"], "enable_swf": false }));
        assert!(!one.contains("/mime?video=1"), "one type: no type chips");
        // a user's uploads: their channel header with the tabs
        let uploads = render(&fe("index"), serde_json::json!({ "url": url("/user/al/uploads", serde_json::json!({})), "tmp": { "user": "al", "view_mode": "uploads" },
            "session": session, "items": [item(7)], "total": 1, "link": { "main": "/user/al/uploads/", "path": "p/", "suffix": "" } }));
        assert!(uploads.contains("<a href=\"/user/al/uploads\" class=\"active\" aria-current=\"page\">F0cks</a>"));
        let listed = render(&fe("index"), serde_json::json!({ "url": url("/user/al/uploads", serde_json::json!({})), "tmp": { "user": "al", "view_mode": "uploads" },
            "list_user": { "user": "Al", "display_name": "Al B", "avatar_file": "a.webp" }, "items": [item(7)], "total": 1, "link": { "main": "/user/al/uploads/", "path": "p/", "suffix": "" } }));
        assert!(listed.contains("<img class=\"fy-channel-ava\" src=\"/a/a.webp\" alt=\"\">") && listed.contains(">Al B</span>"));
        let empty = render(&fe("index"), serde_json::json!({ "url": url("/tag/zzz", serde_json::json!({})), "tmp": { "tag": "zzz" }, "items": [], "total": 0 }));
        assert!(empty.contains("No f0cks here"));

        // search results
        let search = render(&fe("search"), serde_json::json!({ "url": url("/search", serde_json::json!({ "tag": "cat" })), "searchstring": "cat", "count": 2, "search_mode": "strict",
            "result": [{ "id": 7, "username": "al", "display_name": "Al B", "avatar_file": "a.webp", "mime": "video/mp4", "dest": "s.mp4", "duration": 62.5, "views": 3, "tag": "cat", "score": 0.93, "href": "/search/7?q=cat&match=all" }],
            "pagination": pag, "link": { "main": "/search/?tag=cat", "path": "&page=" }, "search_explain": ["tagged \u{201c}cat\u{201d}"] }));
        dump("search", &search);
        assert!(search.contains("<p class=\"search-understood fy-search-understood\">Showing f0cks tagged \u{201c}cat\u{201d}</p>"));
        assert!(search.contains("data-preview=\"/b/s.mp4\"") && search.contains("<span class=\"fy-tbadge fy-time\">1:02</span>"));
        assert!(search.contains("<img class=\"fy-ava\" src=\"/a/a.webp\" alt=\"\" loading=\"lazy\"><span>Al B</span>") && search.contains("3 views"));
        assert!(search.contains("<h3 class=\"fy-row-title\"><a href=\"/search/7?q=cat&amp;match=all\">cat #7</a></h3>"));
        assert!(search.contains("<input type=\"hidden\" name=\"mode\" value=\"strict\">"));
        assert!(search.contains("93% match"));
        assert!(search.contains("<ol class=\"fy-list fy-results\" data-fy-feed>"));
        assert!(render(&fe("search"), serde_json::json!({ "url": url("/search", serde_json::json!({})), "searchstring": [] })).contains("Search f0ckYou"));

        // watch page: the actions are plain forms, the regions they change are swapped in by f0ckyou.js
        let tags = serde_json::json!([{ "id": 1, "tag": "sfw", "normalized": "sfw", "can_exclude": false }, { "id": 9, "tag": "Cat", "normalized": "cat", "can_exclude": true, "is_excluded": true }]);
        let it = serde_json::json!({ "id": 7, "username": "al", "author_color": "#0f0", "mime": "video/mp4", "dest": "/b/7.mp4", "size": "1 MB", "tags": tags,
            "timestamp": { "timeago": "1 day ago", "timefull": "x" }, "favorites": [{ "user": "bob", "avatar": 5 }], "src": { "short": "f0ck.test", "long": "f0ck.test" }, "views": "12345" });
        let rec = serde_json::json!({ "id": 9, "slug": null, "title": null, "tags": ["cat", "dog"], "username": "al", "display_name": "Al", "mime": "video/mp4",
            "stamp": 1_794_700_000, "views": 1, "personalized": true, "rating_class": "nsfw", "has_coverart": false, "duration": 3725, "dest": "r.webm" });
        let comment = serde_json::json!({ "id": 3, "username": "al", "name": "al", "avatar_src": "/t/5.webp", "content": "hi <b>", "content_html": "hi", "attachments_html": "", "timeago": "now", "iso": "x",
            "poll": sample_poll(true, false),
            "replies": [{ "id": 4, "username": "bob", "name": "bob", "avatar_src": "/t/5.webp", "content_html": "yo", "attachments_html": "", "timeago": "now", "iso": "x",
                "poll": sample_poll(false, true) }] });
        // in a search: prev/next walk its results, the search above them leads back to the results
        let searched = render(&fe("item"), serde_json::json!({ "url": url("/search/7", serde_json::json!({ "q": "cat" })), "tmp": {}, "session": session, "item": it.clone(),
            "pagination": { "prev": 6, "next": 8 }, "link": { "main": "/search/", "path": "", "suffix": "?q=cat" }, "recommendations": [rec.clone()],
            "search_ctx": { "q": "cat", "strict": false, "position": 3, "total": 9, "page": 1, "results": "/search/?tag=cat" } }));
        dump("item-search", &searched);
        assert!(searched.contains("id=\"fy-prev\" href=\"/search/6?q=cat\"") && searched.contains("id=\"fy-next\" href=\"/search/8?q=cat\""));
        assert!(searched.contains("<p class=\"fy-upnext-search\"><a href=\"/search/?tag=cat\" title=\"back to the results\">"));
        assert!(searched.contains("<span class=\"fy-dim\">3 of 9</span>") && searched.contains("<p class=\"fy-upnext-search fy-upnext-nav-narrow\">"));
        assert!(!searched.contains("<input type=\"hidden\" name=\"mode\" value=\"strict\">"));
        let page = render(&fe("item"), serde_json::json!({ "url": url("/tag/cat/7", serde_json::json!({})), "tmp": { "tag": "cat" }, "session": session, "item": it.clone(),
            "pagination": { "prev": 6, "next": 8, "cheat": [4, 5, 6, 7, 8, 9] }, "link": { "main": "/tag/cat/", "suffix": "" }, "is_mod_or_admin": true, "can_manage_item": true,
            "can_extract_meta": true, "can_rate_item": true, "user_has_favorited": true, "isSubscribed": false, "static_comments": [comment], "phrase": "nice",
            "enable_comment_polls": true,
            "recommendations": [rec.clone()],
            "my_hall_choices": [{ "slug": "mine", "image_v": "5", "name": "Mine", "is_private": true, "has_item": true, "total_items": 2 }, { "slug": "other", "name": "Other", "is_private": false, "has_item": false, "total_items": 0 }],
            "hall_choices": [{ "slug": "best", "name": "Best", "has_item": false }] }));
        dump("item", &page);
        // Save to a playlist (#save): a row per playlist with the explicit action, a new playlist,
        // the site's playlists for staff; the pill says it's saved somewhere
        assert!(page.contains("<a class=\"fy-pill fy-pill-on\" href=\"#save\" title=\"Save to a playlist\">") && page.contains("<span>Saved</span>"));
        assert!(page.contains("<div id=\"save\" class=\"fy-modal\">") && page.contains("<div class=\"fy-save\" data-fy-live=\"save\">"));
        assert!(page.contains("<input type=\"hidden\" name=\"hall\" value=\"mine\">\n            <input type=\"hidden\" name=\"action\" value=\"remove\">"));
        assert!(page.contains("<input type=\"hidden\" name=\"hall\" value=\"other\">\n            <input type=\"hidden\" name=\"action\" value=\"add\">"));
        assert!(page.contains("<button type=\"submit\" class=\"fy-save-row is-on\" role=\"checkbox\" aria-checked=\"true\">"));
        assert!(page.contains("name=\"new_name\"") && page.contains("<input type=\"hidden\" name=\"back\" value=\"/tag/cat/7#save\">"));
        assert!(page.contains("action=\"/mod/lists/add\"") && page.contains("<span>Site playlists</span>"));
        assert!(page.contains("<dt>Playlists</dt>") || !page.contains("<dt>Halls</dt>"));
        // mods edit comments in place (as in f0ckm): Edit opens the form in the text's place (#edit-c3,
        // :target without JavaScript), the text escaped in the field, with the emoji picker
        assert!(page.contains("<form method=\"post\" action=\"/api/comments/3/edit\" class=\"fy-comment-edit fy-ajax\" id=\"edit-c3\" data-fy-refresh=\"comments\" data-fy-edit>"));
        assert!(page.contains("aria-label=\"Edit comment #3\">hi &lt;b&gt;</textarea>"));
        assert!(page.contains("<a href=\"#edit-c3\" class=\"fy-edit-link\" data-edit=\"3\">Edit</a>"));
        assert!(page.contains("fy-emoji-btn\" aria-expanded=\"false\" aria-controls=\"fy-emoji-box-e3\""));
        assert!(page.contains("fy-edit-cancel\" href=\"#c3\">Cancel</a>"));
        // comment polls (the shared snippets/comment-poll.html, coloured by f0ckyou.css): plain vote forms
        // back to the comment, under its text; a closed one (the reply's) only shows the results
        assert!(page.contains("/s/css/comment-poll.css?v="));
        assert!(page.contains(r#"<form class="poll-form" method="post" action="/api/polls/7/vote"><input type="hidden" name="csrf_token" value="tok"><input type="hidden" name="back" value="/tag/cat/7#c3">"#), "{page}");
        assert!(page.contains(r#"name="option_id" value="11""#) && page.contains(r#"<form class="poll-delete-form" method="post" action="/api/polls/7/delete">"#));
        assert!(page.find("<div class=\"fy-comment-body\">hi").unwrap() < page.find("action=\"/api/polls/7/vote\"").unwrap());
        assert!(page.contains(r#"class="comment-poll is-closed has-voted""#));
        // a poll with a new comment: its fields in the comment form, opened by the chart button (no JavaScript needed)
        assert!(page.contains(r#"<input type="checkbox" class="fy-poll-toggle fy-sr" id="fy-poll-toggle" aria-controls="fy-poll-builder">"#));
        assert!(page.contains(r#"<label for="fy-poll-toggle" class="fy-iconbtn fy-poll-btn" title="Add a poll">"#));
        let form = &page[page.find("id=\"comment-form\"").unwrap()..];
        let form = &form[..form.find("</form>").unwrap()];
        assert!(form.contains("name=\"poll_question\"") && form.contains("name=\"poll_options\"") && form.contains("name=\"poll_anonymous\" value=\"1\" checked"));
        // a song: its square cover and the audio element (native controls without JavaScript,
        // f0ckyou.js puts v0ck on it)
        let mut song = it.clone();
        song["mime"] = serde_json::json!("audio/mpeg");
        song["dest"] = serde_json::json!("/b/7.mp3");
        song["coverart"] = serde_json::json!("/ca/7.webp");
        let audio = render(&fe("item"), serde_json::json!({ "url": url("/7", serde_json::json!({})), "tmp": {}, "session": session, "item": song,
            "pagination": {}, "link": { "main": "/", "suffix": "" } }));
        dump("item-audio", &audio);
        assert!(audio.contains("<div class=\"fy-player fy-player-audio\">"));
        assert!(audio.contains("<img class=\"fy-audio-cover\" src=\"/ca/7.webp\" alt=\"\">"));
        assert!(audio.contains("<audio id=\"my-video\" class=\"fy-media fy-media-audio\"") && audio.contains(" controls loop src=\"/b/7.mp3\""));
        assert!(page.contains("<body class=\"fy-page-watch\">"));
        assert!(page.contains("<video id=\"my-video\" class=\"fy-media\""));
        assert!(page.contains("<a class=\"fy-btn\" id=\"fy-prev\" href=\"/tag/cat/6\""));
        // one column: Previous / Random / Next again above the comments (without the ids)
        assert!(page.contains("<div class=\"fy-upnext-nav fy-upnext-nav-narrow\">\n      <a class=\"fy-btn\" href=\"/tag/cat/6\""));
        assert!(page.find("fy-upnext-nav-narrow") < page.find("id=\"comments\""));
        // the comment field's emoji picker: f0ckm's (shared emoji-picker.js)
        assert!(page.contains("<div class=\"f0ck-ep\" id=\"fy-emoji-box\" hidden></div>") && page.contains("/s/js/emoji-picker.js?v="));
        assert!(page.contains("<input type=\"hidden\" name=\"action\" value=\"remove\">"));
        assert!(page.contains("aria-pressed=\"true\"") && page.contains("#like-on"));
        assert!(page.contains("<form method=\"post\" action=\"/api/subscribe/7\" class=\"fy-inline fy-ajax\" data-fy-refresh=\"actions\">"));
        assert!(page.contains("aria-pressed=\"false\"><svg class=\"fy-ico\" aria-hidden=\"true\" focusable=\"false\"><use href=\"/s/fe/f0ckyou/img/icons.svg?v=") && page.contains("#bell\"></use>"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/item/7/rating\" class=\"fy-inline fy-ajax\" data-fy-refresh=\"desc\">"));
        assert!(page.contains("class=\"f0ck-exclude-form is-excluded\" data-tag-id=\"9\""));
        assert!(page.contains("<form method=\"post\" action=\"/api/comments\" class=\"fy-comment-form fy-ajax\" id=\"comment-form\" data-fy-refresh=\"comments comments-head\" data-fy-comment>"));
        // the description: collapsed, opened by a click (no JavaScript needed)
        assert!(page.contains("<details class=\"fy-desc\" data-fy-live=\"desc\">") && page.contains("<summary class=\"fy-desc-top\">"));
        assert!(!page.contains("<details class=\"fy-desc\" data-fy-live=\"desc\" open"));
        // no "View channel"; Edit (title, new thumbnail, 451 for staff) next to Delete
        assert!(!page.contains("View channel"));
        // Upload in the masthead opens f0ckm's upload dropdown (f0ck_upload_init.js), /upload without JavaScript
        assert!(page.contains("<a class=\"fy-create\" id=\"nav-upload-link\" href=\"/upload\" title=\"Upload (u)\" aria-haspopup=\"dialog\" aria-expanded=\"false\">"));
        assert!(page.contains("id=\"upload-drag-modal\"") && page.contains("/s/js/f0ck_upload_init.js?v="));
        // the bell: /notifications without JavaScript, the notifications popover with it (f0ckyou.js)
        assert!(page.contains("<a class=\"fy-iconbtn\" id=\"fy-notif-btn\" href=\"/notifications\" title=\"Notifications\" aria-haspopup=\"dialog\" aria-expanded=\"false\" aria-controls=\"fy-notif-pop\">"));
        assert!(page.contains("<div class=\"fy-menu-box fy-notif-pop\" id=\"fy-notif-pop\" role=\"dialog\" aria-label=\"Notifications\" hidden>"));
        assert!(page.contains("data-fy-notif-tabs") && page.contains("data-fy-notif-body") && page.contains("data-fy-notif-read"));
        assert!(page.contains("<a class=\"fy-pill\" href=\"#edit\">") && page.contains("<div id=\"edit\" class=\"fy-modal\" data-fy-live=\"edit\">"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/items/7/rethumb\" class=\"fy-edit-tool fy-ajax\""));
        // Edit: everything of f0ckm's info box (visibility, expiration, the file, tools) as plain forms
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/item/visibility\" class=\"fy-edit-vis fy-ajax\""));
        assert!(page.contains("name=\"visibility\" value=\"0\" checked>") && page.contains("name=\"visibility\" value=\"3\">"), "staff: 451 too");
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/items/7/expiry\" class=\"fy-edit-exp fy-ajax\"") && page.contains("<option value=\"custom\">"));
        assert!(page.contains("href=\"/tag/cat/7?meta=1#meta\" data-fy-meta>"));
        assert!(page.contains("<div id=\"meta\" class=\"fy-modal\" data-fy-live=\"meta\">") && page.contains("Reading the file's metadata"));
        for live in ["actions", "desc", "comments", "comments-head", "title", "edit", "edit-vis", "edit-exp", "meta"] {
            assert_eq!(page.matches(&format!("data-fy-live=\"{live}\"")).count(), 1, "{live}");
        }
        // the uploader: no 451; a private, expiring item; ?meta=1 brings the tag suggestions
        let mut own = it.clone();
        own["visibility"] = serde_json::json!(2);
        own["expires_at"] = serde_json::json!("1900000000");
        own["expires_countdown"] = serde_json::json!("1d 2h");
        let mine = render(&fe("item"), serde_json::json!({ "url": { "pathname": "/7", "search": "?meta=1", "qs": {} }, "tmp": {}, "session": session, "item": own,
            "pagination": {}, "link": { "main": "/", "suffix": "" }, "can_manage_item": true, "can_extract_meta": true, "meta_fields": ["cat", "holiday 2024"] }));
        dump("item-mine", &mine);
        assert!(mine.contains("name=\"visibility\" value=\"2\" checked>") && !mine.contains("name=\"visibility\" value=\"3\""));
        assert!(mine.contains("#lock\"></use></svg> private</dd>"));
        assert!(mine.contains("<b class=\"fy-countdown\" data-countdown=\"1900000000\">1d 2h</b>"));
        assert!(mine.contains("<span class=\"fy-meta-tag is-added\">"), "already a tag (Cat)");
        assert!(mine.contains("<input type=\"hidden\" name=\"back\" value=\"/7?meta=1#meta\">") && mine.contains("<input type=\"hidden\" name=\"tagname\" value=\"holiday 2024\">"));
        assert!(mine.contains("href=\"/7?meta=1#meta\" data-fy-meta>") && !mine.contains("Reading the file's metadata"));
        assert!(page.contains("data-reply=\"3\" data-name=\"al\""));
        assert!(page.contains("<summary>1 reply</summary>"));
        assert!(page.contains("2 Comments"));
        // up next: the recommendations (snippets/recommendations.html, the card f0ckyou.js builds too)
        assert!(page.contains("<ol class=\"fy-upnext-list\" id=\"fy-recs\" data-source=\"recs\" data-item-id=\"7\">"));
        assert!(!page.contains("fy-upnext-tabs"), "no tag list: one list, no tabs");
        assert!(page.contains("<a href=\"/9\" data-rec=\"9\" data-tags=\"cat,dog\" data-creator=\"al\">"));
        assert!(page.contains("<span class=\"fy-upnext-name\" title=\"#cat #dog\">#cat #dog</span>"));
        assert!(page.contains("<span class=\"fy-badges\"><span class=\"fy-tbadge fy-rate-nsfw\">NSFW</span><span class=\"fy-tbadge fy-time\">1:02:05</span></span>"));
        assert!(page.contains("<span class=\"fy-upnext-thumb\" data-preview=\"/b/r.webm\">"));
        assert!(page.contains("1 view &middot; <time") && page.contains("<span class=\"fy-foryou\">For you</span>"));
        // in a tag: the tag's list under its own tab first (its cards stay in the tag), the recommendations beside it
        let tagged = render(&fe("item"), serde_json::json!({ "url": url("/tag/cat/7", serde_json::json!({})), "tmp": { "tag": "cat" }, "session": session,
            "item": it.clone(), "pagination": { "prev": 6, "next": 8 }, "link": { "main": "/tag/cat/", "suffix": "" },
            "recommendations": [rec.clone()], "tag_recommendations": [rec.clone()], "upnext_tag": "Cat" }));
        assert!(tagged.contains("<input type=\"radio\" name=\"fy-upnext-tab\" id=\"fy-upnext-tag\" class=\"fy-upnext-radio\" value=\"tag\" checked>"));
        assert!(tagged.contains("<label for=\"fy-upnext-tag\" class=\"fy-chip\">#cat</label>") && tagged.contains("<label for=\"fy-upnext-recs\" class=\"fy-chip\">Recommended</label>"));
        assert!(tagged.contains("<ol class=\"fy-upnext-list\" id=\"fy-recs-tag\" data-source=\"tag\" data-tag=\"Cat\" data-item-id=\"7\" data-tag-done=\"1\">"));
        // the tag's tab shows the tag only: at its end a way to the recommendations
        assert!(tagged.contains("That's all in #cat. <label for=\"fy-upnext-recs\" class=\"fy-linkbtn\">Show recommended</label>"));
        assert!(tagged.contains("<a href=\"/tag/Cat/9\" data-rec=\"9\"") && tagged.contains("<a href=\"/9\" data-rec=\"9\""));
        dump("item-tag", &tagged);
        // views; no message of the day in f0ckyou
        // the view beacon (counts without JavaScript; f0ck-core.js starts from it) on item pages only
        assert!(page.contains("<img class=\"f0ck-view-beacon\" src=\"/api/v2/view/7/nojs.gif\" data-item-id=\"7\" alt=\"\" width=\"1\" height=\"1\" loading=\"lazy\" hidden>"));
        assert!(!home.contains("f0ck-view-beacon"));
        // f0ckm's player and blurred background on the watch page only
        assert!(page.contains("<canvas id=\"bg\" class=\"fy-bg\" data-tuner=\"null\" aria-hidden=\"true\"></canvas>"));
        assert!(page.contains("/s/js/v0ck.js?v=") && page.contains("/s/js/f0ck-bg.js?v=") && page.contains("/s/css/v0ck.css?v="));
        assert!(page.find("/s/js/f0ck-bg.js?v=").unwrap() < page.find("/s/fe/f0ckyou/js/f0ckyou.js?v=").unwrap());
        // the scripts on every page (pages change in place), the canvas only on the watch page
        assert!(home.contains("/s/js/v0ck.js?v=") && !home.contains("id=\"bg\""));
        // the MOTD at the top of the guide, hidden without one, not for who turned it off
        let motd = "<p class=\"fy-guide-motd\"><span class=\"fy-motd-tie\" aria-hidden=\"true\"></span><span class=\"fy-motd-text\">hello</span></p>";
        assert!(page.contains(motd) && home.contains(motd), "MOTD in the guide");
        let guide_at = |h: &str| h.find("<nav class=\"fy-guide f0ck-shell-chrome\"").unwrap();
        assert!(home.find(motd).unwrap() > guide_at(&home));
        let no_motd = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {}, "session": session, "motd": "" }));
        assert!(no_motd.contains("<p class=\"fy-guide-motd\" hidden>"));
        let off = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({})), "tmp": {},
            "session": { "user": "bob", "id": 1, "show_motd": false } }));
        assert!(!off.contains("fy-guide-motd"));
        assert!(page.contains("<b class=\"fy-views-count\" data-views=\"12345\">12,345 views</b>"), "a bigint view count comes as text");
        assert!(home.contains("<div class=\"fy-card-sub\">1.2K views &middot; <time"));
        assert!(page.contains("<button type=\"button\" class=\"fy-iconbtn js-only fy-emoji-btn\""));
        for modal in ["share", "report", "delete"] {
            assert!(page.contains(&format!("<div id=\"{modal}\" class=\"fy-modal\">")), "{modal}");
        }
        assert!(page.contains("\"itemId\": 7"));
        assert!(page.contains("class=\"fy-owner-name fy-namecolor\" href=\"/user/al\" style=\"--user-color: #0f0\""));

        // channel, error, sign-in
        let user = render(&fe("user"), serde_json::json!({ "url": url("/user/al", serde_json::json!({})), "session": session, "user_banner_enabled": true,
            "user": { "user": "al", "id": 2, "banner_file": "b.png", "timestamp": { "timeago": "1 year ago", "timefull": "x" } }, "count": { "f0cks": 2, "favs": 0 },
            "f0cks": { "items": [item(7)], "link": { "main": "/user/al/uploads/" } } }));
        dump("user", &user);
        assert!(user.contains("<div class=\"fy-banner\" style=\"--fy-banner: url('/a/b.png')\"></div>") || user.contains("--fy-banner: url(&#x27;/a/b.png&#x27;)"));
        assert!(user.contains("<a class=\"fy-thumb\" href=\"/user/al/uploads/7\""));
        assert!(user.contains("<a href=\"/user/al\" class=\"active\" aria-current=\"page\">Home</a>"));
        assert!(user.contains("href=\"/messages/al\""));
        let error = render(&fe("error"), serde_json::json!({ "message": "gone", "url": url("/x", serde_json::json!({})) }));
        dump("error", &error);
        assert!(error.contains("This page isn't available."));
        let login = render(&fe("index"), serde_json::json!({ "url": url("/", serde_json::json!({ "login": "1" })), "error": "wrong password" }));
        dump("login", &login);
        assert!(login.contains("<form class=\"fy-signin-card\" method=\"post\" action=\"/login\">"));
        assert!(login.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"tok\">"));
        assert!(login.contains("wrong password"));

        // the other own pages (tpl::FRONTEND_OWN_PAGES): none falls back to f0ckm's layout
        let tag = serde_json::json!({ "tag": "Cat", "safe_tag": "cat", "encoded_tag": "Cat", "total_items": 3 });
        let al = serde_json::json!({ "user": "al", "id": 2 });
        let pages = [
            ("about", serde_json::json!({ "about_text": "" })),
            ("rules", serde_json::json!({ "rules_text": "# Be nice", "rules_text_b64": "IyBCZSBuaWNl" })),
            ("terms", serde_json::json!({})),
            ("tags", serde_json::json!({ "toptags": [tag], "session": session })),
            ("tags_user", serde_json::json!({ "toptags": [tag], "tagger": "al", "user": al, "pagination": pag, "link": { "main": "/user/al/tags", "path": "?page=" } })),
            ("halls", serde_json::json!({ "hallsList": [{ "slug": "best", "name": "Best", "rating": "nsfw", "total_items": 4, "description": "the best" }] })),
            // a user's halls, managed by their owner (forms for each hall, the new-hall form)
            ("user-halls", serde_json::json!({ "session": session, "user": al, "ownerUser": al, "isOwner": true, "can_manage": true, "manage_qs": "",
                "enable_userhall_image_upload": true, "userhalls_enabled": true,
                "hallsList": [{ "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": 3 },
                              { "id": 2, "slug": "empty", "name": "Empty", "description": null, "is_private": false, "custom_image": false, "total_items": 0 }] })),
            ("subscriptions", serde_json::json!({ "session": session, "items": [{ "id": 7, "user": "al", "thumb": "/t/7.webp" }], "totalCount": 1,
                "pagination": { "start": 1, "end": 1, "page": 1 }, "link": { "main": "/subscriptions", "path": "?page=" } })),
            ("notifications", serde_json::json!({ "session": session, "activeTab": "user", "tabUnread": { "user": 1 },
                "notifications": [{ "type": "comment_reply", "from_user": "al", "id": 9, "item_id": 7, "reference_id": 3, "is_read": false, "created_at": "2026-11-15T10:00:00Z" }] })),
            ("ranking", serde_json::json!({ "list": [{ "user": "al", "count": 9 }], "hoster": [{ "part": "youtube.com", "c": 4 }], "favotop": [{ "item_id": 7, "favs": 2 }],
                "stats": { "total": 10 }, "totalComments": 1, "totalFavs": 2, "totalUsers": 3, "totalAnonUsers": 0, "diskSize": "1 GB" })),
            ("comments_user", serde_json::json!({ "user": al, "comments_page": 2, "has_more": true,
                "static_comments": [{ "id": 3, "item_id": 7, "item_key": "7", "content_html": "<b>hi</b>", "attachments_html": "", "timeago": "now", "iso": "x",
                    "poll": sample_poll(true, false) }] })),
            ("upload", serde_json::json!({ "session": session, "mimes_json": "{}", "uploads_remaining": null, "upload_types": [], "min_tags": 1 })),
        ];
        for (view, data) in pages {
            let mut data = data;
            data["url"] = url(&format!("/{view}"), serde_json::json!({}));
            let html = render(&fe(view), data);
            dump(view, &html);
            // the Playlists tab: playlist cards, and for the owner the forms (new, edit, cover, delete)
            if view == "user-halls" {
                assert!(html.contains("<nav class=\"fy-tabs\" aria-label=\"Channel\">") && html.contains("aria-current=\"page\">Playlists</a>"));
                assert!(html.contains("<div class=\"fy-playlists\" data-fy-live=\"playlists\">"));
                assert!(html.contains("src=\"/user_list_image/2/mine?m=0&v=5\"") && html.contains("href=\"/user/al/list/mine\""));
                assert!(html.contains("action=\"/api/v2/me/lists\" class=\"fy-modal-box fy-edit-form fy-ajax\""));
                assert!(html.contains("<form method=\"post\" action=\"/api/v2/me/lists/mine\" class=\"fy-edit-form fy-ajax\""));
                assert!(html.contains("<input type=\"radio\" name=\"is_private\" value=\"1\" checked>"));
                assert!(html.contains("action=\"/api/v2/me/lists/mine/image\" enctype=\"multipart/form-data\" class=\"fy-cover-form\""));
                assert!(html.contains("action=\"/api/v2/me/lists/mine/image/delete\"") && !html.contains("/api/v2/me/lists/empty/image/delete"));
                assert!(html.contains("<div id=\"pl-del-mine\" class=\"fy-modal\">") && html.contains("action=\"/api/v2/me/lists/mine/delete\""));
                // (the page data's i18n strings are f0ckm's: only the visible page counts)
                let main = &html[html.find("<main").unwrap()..html.find("</main>").unwrap()];
                assert!(!main.contains("Hall"), "f0ckyou calls them playlists");
            }
            // a comment's poll on the user's comments: votes come back to this page of them
            if view == "comments_user" {
                assert!(html.contains(r#"<input type="hidden" name="back" value="/user/al/comments?page=2#c3">"#), "{html}");
            }
            // the popover of the bell takes its rows from this page: each with its id (read on click)
            if view == "notifications" {
                assert!(html.contains("<li class=\"fy-notif fy-unread\" data-id=\"9\">") && html.contains("<ol class=\"fy-list fy-notifs\" data-fy-live=\"notifs\">"));
            }
            assert!(html.contains("/s/fe/f0ckyou/css/f0ckyou.css") && html.contains("<header class=\"fy-mast f0ck-shell-chrome\">"), "{view}: not in f0ckyou's layout");
            assert!(!html.contains("/s/css/f0ckm.css"), "{view}");
            assert!(html.trim_end().ends_with("</html>"), "{view}");
        }
        // a visitor's look at someone's Playlists tab without any: no forms, the guest's empty state
        let none = render(&fe("user-halls"), serde_json::json!({ "url": url("/user/al/lists", serde_json::json!({})), "user": al, "ownerUser": al,
            "isOwner": false, "can_manage": false, "manage_qs": "", "hallsList": [], "userhalls_enabled": true }));
        assert!(none.contains("This channel has no public playlists.") && !none.contains("/api/v2/me/lists"));
        for page in FRONTEND_OWN_PAGES {
            assert!(root.join("views/frontends/f0ckyou").join(format!("{page}.html")).is_file(), "{page}");
        }

        // a shared page in the shell
        let shell = render("settings.html", serde_json::json!({ "url": url("/settings", serde_json::json!({})), "session": session,
            "shell_header": "frontends/f0ckyou/shell/header.html", "shell_footer": "frontends/f0ckyou/shell/footer.html",
            "frontend_settings_prefs": "frontends/f0ckyou/settings-prefs.html", "csrf_token": "tok",
            "excluded_tags": [{ "id": 4, "tag": "Cats", "normalized": "cats" }] }));
        dump("settings", &shell);
        // the excluded tags: listed with a remove form each, and a field to add one (works without JavaScript)
        assert!(shell.contains("<h2 id=\"excluded-tags\">") && shell.contains("<a href=\"#excluded-tags\">"));
        assert!(shell.contains("<li class=\"settings-excluded-tag\" data-tag=\"Cats\">") && shell.contains("<a href=\"/tag/cats\">#Cats</a>"));
        assert!(shell.contains("<input type=\"hidden\" name=\"tag_id\" value=\"4\">") && shell.contains("<input type=\"hidden\" name=\"remove\" value=\"1\">"));
        assert!(shell.contains("<form method=\"post\" action=\"/api/v2/settings/excluded_tags\" class=\"settings-excluded-add\">"));
        assert!(shell.contains("<p class=\"settings-muted settings-excluded-empty\" hidden>"));
        assert!(shell.contains("/s/js/views/settings.excluded.js?v="));
        // and the guide links to it
        assert!(shell.contains("href=\"/settings#excluded-tags\""));
        assert!(shell.contains("class=\"no-js f0ck-shell scheme-"));
        assert!(shell.contains("<div class=\"f0ck-shell-page\">"));
        assert!(shell.contains("<a href=\"/settings\" class=\"active\" aria-current=\"page\">"), "Settings is the active guide entry");
    }

    /// comments_user-partial.html: a user's comments server-rendered for visitors without
    /// JavaScript, with links to the newer / older pages, under the profile sub-page head.
    #[test]
    fn user_comments_page_renders_without_js() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let user = serde_json::json!({ "user": "al", "username_color": "#f00", "display_name": "Al B", "banner_file": "b.png", "banner_position": "center", "banner_size": "cover" });
        let c = serde_json::json!({ "id": 7, "item_id": 5, "item_key": "5", "content_html": "x &lt;b&gt;", "attachments_html": "", "timeago": "2 days ago", "iso": "y" });
        let ctx = serde_json::json!({ "user": user, "static_comments": [c], "comments_page": 2, "has_more": true, "enable_comments": true, "user_banner_enabled": true });
        let html = env.get_template("comments_user-partial.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("class=\"profile_head profile-subhead\""), "{html}");
        assert!(html.contains("--author-banner: url(&#x27;/a/b.png&#x27;)") || html.contains("--author-banner: url('/a/b.png')"), "{html}");
        assert!(html.contains(">Al B</a> <span class=\"profile-subhead-bracket\">(al)</span>"), "{html}");
        assert!(html.contains("href=\"/user/al/comments\" class=\"is-active\""), "{html}");
        assert!(html.contains("<div class=\"comments-static user-comments-static\">"), "{html}");
        assert!(html.contains("<div class=\"comment\" id=\"c7\">"), "{html}");
        assert!(html.contains("<a href=\"/5\"><img src=\"/t/5.webp\""), "{html}");
        assert!(html.contains("<div class=\"comment-content\">x &lt;b&gt;</div>"), "{html}");
        assert!(html.contains("href=\"/5#c7\" class=\"comment-permalink\""), "{html}");
        assert!(html.contains("href=\"?page=1\"") && html.contains("href=\"?page=3\""), "{html}");
        let ctx = serde_json::json!({ "user": { "user": "al" }, "static_comments": [], "comments_page": 1, "has_more": false });
        let empty = env.get_template("comments_user-partial.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(empty.contains("user-comments-empty") && !empty.contains("user-comments-pager"), "{empty}");
    }

    /// f0ckm's halls: a user's halls page with the forms that manage them (works without JavaScript;
    /// user-halls-partial.js sends them with fetch), and the item's halls dialog.
    /// F0CKM_DUMP_F0CKM_HALLS=<dir> writes the pages out.
    #[test]
    fn f0ckm_halls_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let render = |view: &str, name: &str, ctx: serde_json::Value| {
            let html = env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            assert!(!html.contains("border-radius") && !html.contains(" style=\""), "{view}");
            if let Ok(dir) = std::env::var("F0CKM_DUMP_F0CKM_HALLS") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), &html).unwrap();
            }
            html
        };
        let al = serde_json::json!({ "user": "Al", "id": 2 });
        let halls = serde_json::json!([{ "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": 3 },
            { "id": 2, "slug": "empty", "name": "Empty", "description": null, "is_private": false, "custom_image": false, "total_items": 0 }]);
        let ctx = |manage: bool, qs: &str| {
            serde_json::json!({ "csrf_token": "tok", "user": al, "ownerUser": al, "isOwner": manage && qs.is_empty(), "can_manage": manage, "manage_qs": qs,
                "enable_userhall_image_upload": true, "hallsList": halls, "url": { "pathname": "/user/al/lists", "search": "" } })
        };
        // an admin managing someone else's halls: every action names the user
        let page = render("user-halls-partial.html", "user-halls-admin", ctx(true, "?user_id=2"));
        assert!(page.contains("<form method=\"post\" action=\"/api/v2/me/lists?user_id=2\" class=\"uh-create\" data-uh-form=\"create\">"), "{page}");
        assert!(page.contains("action=\"/api/v2/me/lists/mine?user_id=2\" class=\"uh-edit-form\""));
        assert!(page.contains("action=\"/api/v2/me/lists/mine/delete?user_id=2\""));
        assert!(page.contains("action=\"/api/v2/me/lists/mine/image?user_id=2\" enctype=\"multipart/form-data\""));
        assert!(page.contains("action=\"/api/v2/me/lists/mine/image/delete?user_id=2\""));
        assert!(!page.contains("/api/v2/me/lists/empty/image/delete"), "no custom picture, nothing to remove");
        assert!(page.contains("<input type=\"radio\" name=\"is_private\" value=\"1\" checked>"));
        assert!(page.contains("<input type=\"hidden\" name=\"back\" value=\"/user/al/lists\">"));
        assert!(page.contains("<div class=\"uh-dialog\" id=\"uh-edit-mine\"") && page.contains("id=\"uh-del-mine\""));
        assert!(page.contains("href=\"/user/al/list/mine\"") && page.contains("src=\"/user_list_image/2/mine?m=0&v=5\"") && !page.contains("tags-grid") && !page.contains("tag-card-image-title"));
        assert!(page.contains("/s/js/views/user-halls-partial.js"));
        // a guest: the cards only
        let guest = render("user-halls-partial.html", "user-halls-guest", ctx(false, ""));
        assert!(!guest.contains("<form") && !guest.contains("uh-dialog") && !guest.contains("user-halls-partial.js"), "{guest}");
        assert!(guest.contains("<span class=\"tag-count\">"));

        // the item's halls dialog: one form per hall, the opposite of has_item
        let dialog = render("snippets/halls-dialog.html", "halls-dialog", serde_json::json!({ "csrf_token": "tok", "item": { "id": 7 }, "session": { "user": "Al" },
            "url": { "pathname": "/7", "search": "" },
            "my_hall_choices": [{ "slug": "mine", "image_v": "5", "name": "Mine", "is_private": true, "has_item": true, "total_items": 3 }, { "slug": "other", "name": "Other", "is_private": false, "has_item": false, "total_items": 0 }],
            "hall_choices": [{ "slug": "best", "name": "Best", "has_item": false }] }));
        assert!(dialog.contains("<input type=\"hidden\" name=\"hall\" value=\"mine\">\n          <input type=\"hidden\" name=\"action\" value=\"remove\">"), "{dialog}");
        assert!(dialog.contains("<input type=\"hidden\" name=\"hall\" value=\"other\">\n          <input type=\"hidden\" name=\"action\" value=\"add\">"));
        assert!(dialog.contains("<input type=\"hidden\" name=\"back\" value=\"/7#halls-modal\">"));
        assert!(dialog.contains("name=\"new_name\""));
        assert!(dialog.contains("<form method=\"post\" action=\"/mod/lists/add\" class=\"hd-form\" data-hall-kind=\"site\">"));
        assert!(dialog.contains("href=\"/user/al/lists\""));
    }

    /// w0bm's notification rows name who commented (anonymous for anon accounts and anonymized viewers).
    #[test]
    fn w0bm_notifications_name_the_commenter() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let row = |user: &str, ty: &str| {
            serde_json::json!({ "id": 1, "type": ty, "item_id": 5, "item_slug": "abc", "reference_id": 9, "is_read": false, "created_at": "2026-01-01T00:00:00.000Z",
                "from_user": user, "from_display_name": "", "from_user_id": 7, "username_color": "#f00", "comment_content": "hi", "mime": "video/mp4" })
        };
        let tpl = env.get_template("frontends/w0bm/snippets/notifications-list.html").unwrap();
        let html = tpl.render(to_jvalue(&serde_json::json!({ "notifications": [row("Kibi", "upload_comment"), row("Bob", "comment_reply")] }))).unwrap();
        assert!(html.contains("<span class=\"w0bm-msg-from w0bm-usercolor\" style=\"--user-color: #f00\">Kibi</span> commented on your video #abc"));
        assert!(html.contains(">Bob</span> replied"));
        let anon = tpl.render(to_jvalue(&serde_json::json!({ "notifications": [row("anon_x1", "upload_comment")] }))).unwrap();
        assert!(anon.contains("<span class=\"w0bm-msg-from\">anonymous</span> commented on your video #abc") && !anon.contains("anon_x1"));
        let hidden = tpl.render(to_jvalue(&serde_json::json!({ "is_anonymized": true, "notifications": [row("Kibi", "upload_comment")] }))).unwrap();
        assert!(hidden.contains(">anonymous</span>") && !hidden.contains("Kibi"));
    }

    /// w0bm's server-rendered comment list already has w0bm.js's action icons and order (flat,
    /// pinned first, then oldest first), so nothing pops in or moves when w0bm.js replaces it.
    #[test]
    fn w0bm_static_comments_match_the_js_list() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let c = |id: i64, user: &str, pinned: bool, replies: Value| serde_json::json!({ "id": id, "username": user, "name": user, "is_pinned": pinned,
            "content_html": format!("text{id}"), "attachments_html": "", "iso": "", "timeago": "", "replies": replies });
        let mut comments = serde_json::json!([c(1, "bob", false, serde_json::json!([c(3, "Kibi", false, serde_json::json!([]))])), c(2, "bob", true, serde_json::json!([]))]);
        comments[1]["poll"] = sample_poll(true, false);
        let render_with = |session: Value, polls: bool| env.get_template("frontends/w0bm/partials/comments.html").unwrap()
            .render(to_jvalue(&serde_json::json!({ "item": { "id": 9 }, "static_comments": comments, "session": session, "allow_comment_deletion": true,
                "enable_comment_polls": polls, "csrf_token": "tok" }))).unwrap();
        let render = |session: Value| render_with(session, false);
        let actions = |html: &str, id: i64| {
            let panel = &html[html.find(&format!("id=\"c{id}\"")).unwrap()..];
            let panel = &panel[..panel.find("</span>\n</div>").or(panel.find("</div>\n</div>")).unwrap()];
            ["reply_comment", "pin_comment", "edit_comment", "report_comment", "delete_comment"].iter().filter(|a| panel.contains(*a)).copied().collect::<Vec<_>>()
        };
        let guest = render(Value::Bool(false));
        assert!(guest.find("id=\"c2\"").unwrap() < guest.find("id=\"c1\"").unwrap() && guest.find("id=\"c1\"").unwrap() < guest.find("id=\"c3\"").unwrap());
        assert!(actions(&guest, 1).is_empty());
        let user = render(serde_json::json!({ "user": "kibi" }));
        assert_eq!(actions(&user, 1), ["reply_comment", "report_comment"]);
        assert_eq!(actions(&user, 3), ["reply_comment", "report_comment", "delete_comment"]);
        let modr = render(serde_json::json!({ "user": "mod", "is_moderator": true }));
        assert_eq!(actions(&modr, 1), ["reply_comment", "pin_comment", "edit_comment", "report_comment", "delete_comment"]);
        assert!(modr.contains("pin_comment w0bm-cact w0bm-cact-on\" href=\"#\" title=\"Unpin\""));

        // the comment's poll in its body, after the text (plain forms back to the comment), only there
        let panel = |html: &str, id: i64| {
            let p = &html[html.find(&format!("id=\"c{id}\"")).unwrap()..];
            p[..p.find("<div class=\"panel-footer\">").unwrap()].to_string()
        };
        for html in [&guest, &user] {
            let c2 = panel(html, 2);
            assert!(c2.find("text2").unwrap() < c2.find("<div class=\"comment-poll has-voted\" id=\"poll-7\"").unwrap(), "{c2}");
            assert!(c2.contains(r#"action="/api/polls/7/vote"><input type="hidden" name="csrf_token" value="tok"><input type="hidden" name="back" value="/9#c2">"#), "{c2}");
            assert!(!panel(html, 1).contains("comment-poll") && !panel(html, 3).contains("comment-poll"));
        }
        // the poll fields in the comment form with polls on (the text then optional), none without
        assert!(user.contains("id=\"cinput\" class=\"form-control\" required") && !user.contains("poll_question"));
        let polls = render_with(serde_json::json!({ "user": "kibi" }), true);
        if let Ok(f) = std::env::var("F0CKM_DUMP_W0BM_COMMENTS") {
            std::fs::write(f, &polls).unwrap();
        }
        assert!(polls.contains("id=\"cinput\" class=\"form-control\">") || polls.contains("id=\"cinput\" class=\"form-control\" maxlength"), "{polls}");
        let form = &polls[polls.find("<form action=\"/api/comments\"").unwrap()..];
        let form = &form[..form.find("</form>").unwrap()];
        for field in ["<details class=\"poll-builder-nojs w0bm-pollbuilder\">", "name=\"poll_question\"", "name=\"poll_options\"", "name=\"poll_anonymous\" value=\"1\" checked"] {
            assert!(form.contains(field), "{field}");
        }
        assert!(!render_with(Value::Bool(false), true).contains("poll_question"));

        // a user's comments page: the poll beside the text that w0bm.js formats, back to the comment
        let url = serde_json::json!({ "pathname": "/user/bob/comments", "search": "?page=2", "qs": { "page": "2" } });
        let mut row = serde_json::json!({ "id": 2, "item_id": 5, "item_slug": null, "content": "vote pls", "created_at": "2026-11-19T10:00:00Z" });
        row["poll"] = sample_poll(true, false);
        let page = env.get_template("frontends/w0bm/comments_user.html").unwrap()
            .render(to_jvalue(&serde_json::json!({ "user": { "user": "bob" }, "comments": [row, { "id": 1, "item_id": 5, "content": "x", "created_at": "2026-11-19T09:00:00Z", "poll": null }],
                "url": url, "csrf_token": "tok", "session": { "user": "kibi" } }))).unwrap();
        if let Ok(f) = std::env::var("F0CKM_DUMP_W0BM_COMMENTS") {
            std::fs::write(format!("{f}.user.html"), &page).unwrap();
        }
        assert!(page.contains(r#"<div class="panel-body w0bm-cmd" data-id="2">vote pls</div><div class="panel-body w0bm-cmd-poll"><div class="comment-poll has-voted" id="poll-7""#), "{page}");
        assert!(page.contains(r#"<input type="hidden" name="back" value="/user/bob/comments?page=2#c2">"#), "{page}");
        assert!(page.contains("<div class=\"panel panel-default\" id=\"c2\">") && page.matches("class=\"comment-poll").count() == 1, "{page}");
    }

    /// A 4chan item's id and timestamp links never point at /<4chan post number> (a different local
    /// item): they go to the rehosted copy, or to the post's /4/ page while it isn't rehosted.
    #[test]
    fn chan_item_links_skip_the_4chan_post_number() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        for view in ["item-partial-modern.html", "item-partial-legacy.html"] {
            let item = |local: Value| serde_json::json!({ "id": 6250458, "is_chan": true, "external_board": "wsg", "external_tid": 6250450,
                "external_id": 6250458, "local_id": local, "local_path": if local.is_null() { Value::Null } else { Value::from("6BHrY1UUphB") },
                "timestamp": { "timefull": "x", "timeago": "1 day ago" }, "tags": {}, "mime": "video/mp4", "dest": "/b/x.mp4", "favorites": [] });
            let render = |local: Value| env.get_template(view).unwrap()
                .render(to_jvalue(&serde_json::json!({ "item": item(local), "enable_item_slugs": true }))).unwrap();
            let rehosted = render(Value::from(42));
            assert!(rehosted.contains("href=\"/6BHrY1UUphB\" class=\"timestamp-link\""), "{view}");
            assert!(!rehosted.contains("href=\"/6250458\""), "{view}");
            let fresh = render(Value::Null);
            assert!(fresh.contains("href=\"/4/wsg/6250450/6250458\" class=\"timestamp-link\""), "{view}");
            assert!(!fresh.contains("href=\"/6250458\""), "{view}");
            if view == "item-partial-legacy.html" {
                // no thread badge (the location line has it); rehost in the action row, like the alternative author box
                assert!(!fresh.contains("item-legacy-chan-badge") && !fresh.contains("item-chan-thread-link"));
                let actions = |h: &str| h[h.find("item-legacy-actions").unwrap()..].to_string();
                assert!(actions(&fresh).contains("class=\"chan-rehost-action-btn item-chan-rehost-btn\" id=\"chan-item-rehost-btn\""));
                assert!(actions(&rehosted).contains("item-chan-rehosted-link") && actions(&rehosted).contains("item-chan-edit-btn"));
                if let Ok(dir) = std::env::var("F0CKM_DUMP_CHAN_ITEM") {
                    std::fs::write(PathBuf::from(&dir).join("chan-item-fresh.html"), &fresh).unwrap();
                    std::fs::write(PathBuf::from(&dir).join("chan-item-rehosted.html"), &rehosted).unwrap();
                }
            }
            if view == "item-partial-modern.html" {
                assert!(rehosted.contains("class=\"id-link\" data-item-id=\"6250458\">6BHrY1UUphB</a>"));
                assert!(fresh.contains("href=\"/4/wsg/6250450/6250458\" class=\"id-link\" data-item-id=\"6250458\">6250458</a>"));
            }
        }
    }

    /// Every frontend's search page says what the search understood and what it left out
    /// (snippets/search-understood.html, routes/search.rs search_explain / search_notes), each in
    /// its own class; without them nothing shows.
    #[test]
    fn search_pages_say_what_they_understood() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = |explain: bool, guest: bool| {
            let session = if guest { serde_json::Value::Bool(false) } else { serde_json::json!({ "user": "bob", "id": 1 }) };
            let mut c = serde_json::json!({ "url": { "pathname": "/search", "search": "?tag=cat", "qs": { "tag": "cat" } }, "domain": "f0ck.test",
                "session": session, "searchstring": "cat", "count": 1, "result": [{ "id": 7, "username": "al", "mime": "image/png", "tag": "cat" }],
                "pagination": { "start": 1, "end": 1, "page": 1, "cheat": [1] }, "link": { "main": "/search/?tag=cat", "path": "&page=" } });
            if explain {
                c["search_explain"] = serde_json::json!(["tagged \u{201c}cat\u{201d}", "uploaded by al"]);
                c["search_notes"] = serde_json::json!(["unknown type \u{201c}x\u{201d}"]);
            }
            c
        };
        for (view, cls) in [
            ("search.html", "srch"),
            ("frontends/w0bm/search.html", "w0bm-search"),
            ("frontends/f0ckv2/search.html", "v2-search"),
            ("frontends/f0ckyou/search.html", "fy-search"),
            ("frontends/f0ckgle/search.html", "fg-search"),
        ] {
            let render = |explain: bool, guest: bool| {
                env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx(explain, guest))).unwrap_or_else(|e| panic!("{view}: {e:#}"))
            };
            // guests may search too
            let guest = render(true, true);
            assert!(guest.contains("Showing f0cks tagged \u{201c}cat\u{201d}") && guest.contains("/7"), "{view} for guests");
            let html = render(true, false);
            assert!(
                html.contains(&format!("<p class=\"search-understood {cls}-understood\">Showing f0cks tagged \u{201c}cat\u{201d} · uploaded by al</p>")),
                "{view}"
            );
            assert!(html.contains(&format!("<p class=\"search-notes {cls}-notes\">Left out: unknown type")), "{view}");
            let plain = render(false, false);
            assert!(!plain.contains("search-understood") && !plain.contains("search-notes"), "{view}");
        }
    }

    /// f0ckm's location bar in a search reads like the address (/search/<f0ck>?q=…), each part a
    /// link (the results, the f0ck), then where the f0ck is in the results.
    #[test]
    fn search_location_reads_like_the_address() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = serde_json::json!({ "enable_item_slugs": true, "session": { "user": "bob", "id": 1 }, "tmp": {},
            "item": { "id": 7, "slug": "eFqWVn9cYGi", "mime": "video/webm", "dest": "/b/x.webm", "tags": [], "favorites": [] },
            "pagination": { "prev": "a", "next": "b" }, "link": { "main": "/search/", "path": "", "suffix": "?q=type%3Avideo%20duration%3A%3C15s" },
            "search_ctx": { "q": "type:video duration:<15s", "strict": false, "position": 2, "total": 38751, "page": 1, "results": "/search?q=type%3Avideo%20duration%3A%3C15s" } });
        for view in ["item-partial-modern.html", "item-partial-legacy.html"] {
            let html = env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            let results = "/search?q=type%3Avideo%20duration%3A%3C15s";
            let want = format!(
                "<a href=\"{results}\" class=\"location-link\" title=\"Back to the search results\">/search</a>\
                 <a href=\"/search/eFqWVn9cYGi?q=type%3Avideo%20duration%3A%3C15s\" class=\"location-link\" title=\"Post #7\">/eFqWVn9cYGi</a>\
                 <a href=\"{results}\" class=\"location-link\" title=\"Back to the search results\">?q=type:video duration:&lt;15s</a>\
                 <span class=\"location-pos\" title=\"Result 2 of 38751\">2 of 38,751</span>"
            );
            assert!(html.contains(&want), "{view}: {}", html.split("class=\"location\"").nth(1).unwrap_or("").chars().take(700).collect::<String>());
        }
    }

    /// f0ckm, w0bm and f0ckv2 show a search's results as their feed grid unless the visitor picked
    /// the list (`search_view`); both views link every result into the search (`href`: prev/next
    /// walk the results) and offer the switch to the other view. The grids are the ones their
    /// endless scrolling knows (f0ckm: .index-container > .posts + #footbar, w0bm:
    /// body.w0bm-index-page .items, f0ckv2: .index-container > div.posts + #footbar).
    #[test]
    fn search_results_as_grid_or_list() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = |view: Option<&str>| serde_json::json!({ "url": { "pathname": "/search", "search": "?q=cat", "qs": { "q": "cat" } }, "domain": "f0ck.test",
            "session": { "user": "bob", "id": 1 }, "searchstring": "cat", "count": 21,
            "result": [{ "id": 7, "username": "al", "mime": "image/png", "tag": "cat", "tag_id": 2, "rating_class": "nsfw", "href": "/search/7?q=cat" }],
            "pagination": { "start": 1, "end": 2, "page": 1, "next": 2, "cheat": [1, 2] }, "link": { "main": "/search/", "path": "p/", "suffix": "?q=cat" },
            "search_view": view, "search_view_urls": { "grid": "/search?q=cat&view=grid", "list": "/search?q=cat&view=list" } });
        for (view, grid_marks, list_mark) in [
            ("search.html", &["<div class=\"index-container\">", "<div class=\"posts\" data-current-page=\"1\" data-has-more=\"true\">", "<div id=\"footbar\">"][..], "<ul class=\"srch-list\">"),
            ("frontends/w0bm/search.html", &["class=\"w0bm-index-page w0bm-search-grid", "<div class=\"items\">", "w0bm-mode-nsfw"][..], "<table class=\"table table-hover table-condensed\">"),
            ("frontends/f0ckv2/search.html", &["<div class=\"index-container v2-search-grid\">", "<div class=\"posts\">", "data-mode=\"nsfw\"", "<div id=\"footbar\">"][..], "<ul class=\"v2-search-list\">"),
        ] {
            let render = |v: Option<&str>| env.get_template(view).unwrap_or_else(|e| panic!("{view}: {e}")).render(to_jvalue(&ctx(v))).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            for grid in [render(None), render(Some("grid"))] {
                for m in grid_marks {
                    assert!(grid.contains(m), "{view}: {m}");
                }
                assert!(grid.contains("href=\"/search/7?q=cat\"") && !grid.contains(list_mark), "{view}");
                assert!(grid.contains("href=\"/search?q=cat&amp;view=list\""), "{view}");
                assert!(grid.contains("/search/p/2?q=cat"), "{view}: page links");
            }
            let list = render(Some("list"));
            assert!(list.contains(list_mark) && list.contains("href=\"/search/7?q=cat\"") && !list.contains(grid_marks[1]), "{view}");
            assert!(list.contains("href=\"/search?q=cat&amp;view=grid\""), "{view}");
            if view == "search.html" {
                // f0ckm: both buttons, the current view marked
                assert!(render(None).contains("<a href=\"/search?q=cat&amp;view=grid\" class=\"srch-view-btn active\""));
                assert!(list.contains("<a href=\"/search?q=cat&amp;view=list\" class=\"srch-view-btn active\""));
            } else {
                // w0bm, f0ckv2: the current view as text, the other one a link
                assert!(!render(None).contains("view=grid") && !list.contains("view=list"), "{view}");
            }
        }
    }

    /// The upload forms (w0bm's page, f0ckm's form snippet) list the accepted file types, "1 tag"
    /// in the singular, and the sound rule only while `require_video_sound` is on.
    #[test]
    fn upload_forms_list_types_and_rules() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = |min_tags: i64, sound: bool| serde_json::json!({ "session": { "user": "bob" }, "max_file_size": "350 MB", "min_tags": min_tags,
            "upload_types": [{ "kind": "video", "exts": ["webm", "mov"] }, { "kind": "pdf", "exts": ["pdf"] }], "require_video_sound": sound });
        let w0bm = |min_tags: i64, sound: bool| env.get_template("frontends/w0bm/upload.html").unwrap().render(to_jvalue(&ctx(min_tags, sound))).unwrap();
        let one = w0bm(1, true);
        assert!(one.contains("<span class=\"w0bm-ul-dndtype\"><b>video</b> webm mov</span><span class=\"w0bm-ul-dndtype\">pdf</span>"));
        assert!(one.contains("At least <b>1 tag</b>"));
        assert!(one.contains("Video must have <b>sound</b>"));
        let three = w0bm(3, false);
        assert!(three.contains("At least <b>3 tags</b>"));
        assert!(!three.contains("must have"));
        let form = |sound: bool| env.get_template("snippets/upload-form.html").unwrap().render(to_jvalue(&ctx(1, sound))).unwrap();
        let f = form(true);
        assert!(f.contains("<p class=\"upload-form-drop-types\"><span>.webm</span><span>.mov</span><span>.pdf</span></p>"));
        assert!(f.contains("upload-form-drop-sound"));
        assert!(!form(false).contains("upload-form-drop-sound"));
    }

    /// A passkey besides the password at registration (routes/register.rs,
    /// public/s/js/passkey-signup.js): every register form keeps its password fields and has the
    /// "Also create a passkey" box, unticked in the markup (the script ticks it; without JavaScript
    /// the form registers with the password), and loads the script.
    #[test]
    fn register_forms_offer_passkeys() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = serde_json::json!({ "csrf_token": "tok", "domain": "f0ck.test", "registration_open": true, "captcha_enabled": false,
            "url": { "pathname": "/", "search": "", "qs": {} } });
        // the pages whose own script sends the form call f0ckPasskeySignup; the others are sent by passkey-signup.js
        let forms = [("snippets/navbar.html", false), ("gate/login_injection.html", false), ("register.html", true), ("frontends/f0ckv2/register.html", true),
            ("frontends/f0ckgle/register.html", true), ("frontends/f0ckyou/register.html", true), ("frontends/w0bm/partials/registerform.html", true)];
        for (view, auto) in forms {
            let page = env.get_template(view).unwrap().render(to_jvalue(&ctx)).unwrap();
            // the register form (the navbar has the login form too)
            let at = page.find("action=\"/register\"").or_else(|| page.find("id=\"gate-register-form\"")).unwrap_or_else(|| panic!("{view}: no register form"));
            let html = &page[at..];
            for f in ["password", "password_confirm"] {
                let field = html.split(&format!("name=\"{f}\"")).nth(1).unwrap_or_else(|| panic!("{view}: no {f}"));
                assert!(field.split('>').next().unwrap().contains("required"), "{view}: {f} not required");
            }
            assert!(!html.contains("reg-if-password"), "{view}: the password is not optional");
            let tag = html.split("name=\"add_passkey\"").nth(1).unwrap_or_else(|| panic!("{view}: no passkey box"));
            assert!(!tag.split('>').next().unwrap().contains("checked"), "{view}: ticked without JavaScript");
            assert!(html.contains("reg-passkey-opt") && html.contains("reg-if-passkey"), "{view}");
            assert!(page.contains("src=\"/s/js/passkey-signup.js?v="), "{view}: script missing");
            assert_eq!(page.contains("data-passkey-signup=\"auto\""), auto, "{view}");
            if auto {
                // after a success only the message stays: the fields sit in .reg-fields, the message outside
                let msg = page.find("data-passkey-signup-msg").unwrap_or_else(|| panic!("{view}: nowhere to show the answer"));
                let fields = page.find("class=\"reg-fields\"").unwrap_or_else(|| panic!("{view}: fields not wrapped"));
                assert!(msg < fields && fields < at + html.find("name=\"password\"").unwrap(), "{view}: message inside the fields or password outside");
            }
        }
        // The register modal: after a success the message and a close button instead of the form
        let modal = |ctx: &serde_json::Value| env.get_template("snippets/navbar.html").unwrap().render(to_jvalue(ctx)).unwrap();
        let html = modal(&ctx);
        assert!(html.contains("<div class=\"lm-done\" id=\"register-done\" hidden"));
        assert!(html.contains("id=\"register-done-close\""));
        assert!(!html.contains("lm-form-done"));
        let mut done = ctx.clone();
        done["register_done_msg"] = serde_json::json!("Registration successful! You can now login.");
        let html = modal(&done);
        assert!(html.contains("<div class=\"lm-done\" id=\"register-done\" role=\"status\">"));
        assert!(html.contains("lm-form lm-form-done"));
        for css in ["public/s/css/f0ckm.css", "public/s/css/gate.css", "public/s/fe/f0ckgle/css/f0ckgle.css", "public/s/fe/f0ckyou/css/f0ckyou.css",
            "public/s/fe/f0ckv2/css/f0ckv2.css", "public/s/fe/w0bm/css/w0bm-fe.css"] {
            let text = std::fs::read_to_string(root.join(css)).unwrap();
            assert!(text.contains("form:not(.pk-signup-ready) :is(.reg-passkey-opt, .reg-if-passkey)"), "{css}");
            assert!(text.contains("form:not(:has(input[name=\"add_passkey\"]:checked)) .reg-if-passkey"), "{css}");
            if !css.ends_with("gate.css") {
                assert!(text.contains("form.pk-signup-done .reg-fields"), "{css}");
            }
        }
    }

    /// Registration by application (routes/register.rs): every register form gets the message
    /// field and an optional token, and the admin page lists them as plain forms.
    #[test]
    fn register_forms_take_applications() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let render = |view: &str, ctx: &serde_json::Value| env.get_template(view).unwrap().render(to_jvalue(ctx)).unwrap();
        let forms = ["snippets/navbar.html", "register.html", "frontends/f0ckv2/register.html", "frontends/f0ckgle/register.html", "frontends/f0ckyou/register.html", "frontends/w0bm/partials/registerform.html", "gate/login_injection.html"];
        let mut ctx = serde_json::json!({ "csrf_token": "tok", "domain": "f0ck.test", "registration_open": false, "captcha_enabled": false,
            "registration_apply": true, "registration_apply_email": false, "url": { "pathname": "/", "search": "", "qs": {} } });
        for view in forms {
            let html = render(view, &ctx);
            if let Ok(dir) = std::env::var("F0CKM_DUMP_REGISTER") {
                std::fs::write(PathBuf::from(dir).join(view.replace('/', "_")), &html).unwrap();
            }
            assert!(html.contains("name=\"application\""), "{view}: no message field");
            assert!(html.contains("name=\"email\""), "{view}: no email field");
            // the token is optional here
            let token = html.split("name=\"token\"").nth(1).unwrap_or_else(|| panic!("{view}: no token field"));
            assert!(!token.split('>').next().unwrap().contains("required"), "{view}: token still required");
            // "Do you have an invite token?" yes / no shows either the token or the application (CSS :has())
            assert_eq!(html.matches("name=\"has_invite\"").count(), 2, "{view}: no invite question");
            assert!(html.contains("value=\"yes\"") && html.contains("value=\"no\""), "{view}");
            let tag = |name: &str| {
                let start = html.find(&format!("name=\"{name}\"")).unwrap();
                // the field or a wrapper up to three tags before it (icon, inner div) carries the class
                let open = html[..start].rmatch_indices('<').nth(3).map_or(0, |(i, _)| i);
                html[open..start + html[start..].find('>').unwrap()].to_string()
            };
            assert!(tag("token").contains("reg-if-token"), "{view}: token field not tied to the answer");
            for f in ["application", "email"] {
                assert!(tag(f).contains("reg-if-apply"), "{view}: {f} not tied to the answer");
            }
            // nothing is preselected without an invite link
            assert!(!html.contains("value=\"yes\" required checked"), "{view}");
        }
        // an invite link answers "yes" and fills in the token
        ctx["url"]["qs"]["invite"] = serde_json::json!("abc123");
        for view in ["snippets/navbar.html", "frontends/w0bm/partials/registerform.html"] {
            let html = render(view, &ctx);
            assert!(html.contains("value=\"yes\" required checked"), "{view}");
            assert!(html.contains("value=\"abc123\""), "{view}");
        }
        ctx["url"]["qs"] = serde_json::json!({});
        for view in ["register.html", "frontends/f0ckv2/register.html", "frontends/f0ckgle/register.html", "frontends/f0ckyou/register.html"] {
            ctx["token"] = serde_json::json!("abc123");
            assert!(render(view, &ctx).contains("value=\"yes\" required checked"), "{view}");
        }
        ctx.as_object_mut().unwrap().remove("token");
        // the frontends' early.js turns invite links into /?register=1&token=…
        ctx["url"]["qs"] = serde_json::json!({ "register": "1", "token": "abc123" });
        for view in ["frontends/f0ckv2/register.html", "frontends/f0ckgle/register.html", "frontends/f0ckyou/register.html"] {
            let html = render(view, &ctx);
            assert!(html.contains("value=\"yes\" required checked") && html.contains("value=\"abc123\""), "{view}");
        }
        ctx["url"]["qs"] = serde_json::json!({});
        // the CSS that does the showing is in every stylesheet these forms use
        for css in ["public/s/css/f0ckm.css", "public/s/css/gate.css", "public/s/fe/f0ckgle/css/f0ckgle.css", "public/s/fe/f0ckyou/css/f0ckyou.css",
            "public/s/fe/f0ckv2/css/f0ckv2.css", "public/s/fe/w0bm/css/w0bm-fe.css"] {
            let text = std::fs::read_to_string(root.join(css)).unwrap();
            assert!(text.contains("[value=\"no\"]:checked)) .reg-if-apply"), "{css}");
        }
        ctx["registration_apply"] = serde_json::json!(false);
        for view in forms {
            let html = render(view, &ctx);
            assert!(!html.contains("name=\"application\""), "{view}");
            assert!(html.contains("name=\"token\""), "{view}");
        }

        // The dialog that has the rules and terms read comes with the forms (not under private_society)
        for view in ["snippets/navbar.html", "register.html", "frontends/f0ckv2/register.html", "frontends/f0ckgle/register.html", "frontends/f0ckyou/register.html", "frontends/w0bm/partials/navigation.html"] {
            assert_eq!(render(view, &ctx).matches("<div id=\"tos-consent-modal\" hidden>").count(), 1, "{view}");
        }
        ctx["private_society"] = serde_json::json!(true);
        assert!(!render("snippets/navbar.html", &ctx).contains("tos-consent"));

        let page = serde_json::json!({ "csrf_token": "tok", "pending_count": 1, "applications_on": true, "applications": [
            { "id": 7, "username": "Neu", "email": "n@example.org", "message": "<b>hi</b> please let me in", "status": "pending", "created_at": "2026-11-15T10:00:00Z" },
            { "id": 6, "username": "Alt", "message": "let me in, I upload a lot", "status": "denied", "created_at": "2026-11-14T10:00:00Z",
              "reviewed_at": "2026-11-14T12:00:00Z", "reviewed_by_name": "admin" },
        ] });
        let html = render("admin/applications.html", &page);
        assert!(html.contains("&lt;b&gt;hi&lt;/b&gt; please let me in"));
        assert_eq!(html.matches("action=\"/api/v2/admin/applications/approve\"").count(), 1);
        assert_eq!(html.matches("action=\"/api/v2/admin/applications/deny\"").count(), 1);
        assert_eq!(html.matches("<input type=\"hidden\" name=\"id\" value=\"7\">").count(), 2);
        assert!(html.contains("Declined by admin"));
    }

    #[test]
    fn login_modal_works_without_js() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = serde_json::json!({ "csrf_token": "tok", "domain": "f0ck.test", "registration_open": false, "captcha_enabled": false,
            "login_error_msg": "user doesn't exist or wrong password", "register_error_msg": "Passwords do not match.",
            "url": { "pathname": "/3", "search": "?login_error=wrong", "qs": { "login_error": "wrong", "invite": "abc\"x" } } });
        let html = env.get_template("snippets/navbar.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        if let Ok(out) = std::env::var("F0CKM_DUMP_NAVBAR") {
            std::fs::write(out, &html).unwrap();
        }
        assert!(html.contains("<a href=\"#login-modal\" id=\"nav-login-btn\">"));
        assert!(html.contains("<a href=\"#_\" class=\"lm-nojs-backdrop\""));
        assert!(html.contains("<input type=\"hidden\" name=\"csrf_token\" value=\"tok\" />"));
        assert!(html.contains("<input type=\"hidden\" name=\"return_to\" value=\"/3?login_error=wrong\" />"));
        assert!(html.contains("<div class=\"flash-error\">user doesn&#39;t exist or wrong password</div>"));
        // register modal
        assert!(html.contains("<a href=\"#register-modal\" id=\"nav-register-btn\">"));
        assert!(html.contains("<a href=\"#login-modal\" id=\"register-to-login\""));
        assert!(html.contains("<div class=\"flash-error\">Passwords do not match.</div>"));
        assert!(html.contains("value=\"abc&quot;x\" />"));
        assert!(!html.contains("lm-imgcap"));
        assert!(!html.contains("style=\"display: none !important;\""));
        // with Cap on: the image captcha in the register form, in the login form only when asked for
        let mut ctx = ctx;
        ctx["captcha_enabled"] = serde_json::json!(true);
        ctx["nojs_captcha_id"] = serde_json::json!("0123456789abcdef");
        let html = env.get_template("snippets/navbar.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert_eq!(html.matches("<iframe src=\"/api/cap/image/0123456789abcdef/frame\"").count(), 1);
        assert!(html.contains("<input type=\"hidden\" name=\"cap-image-id\" value=\"0123456789abcdef\" />"));
        ctx["login_captcha"] = serde_json::json!(true);
        let html = env.get_template("snippets/navbar.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        if let Ok(out) = std::env::var("F0CKM_DUMP_NAVBAR") {
            std::fs::write(out, &html).unwrap();
        }
        assert_eq!(html.matches("name=\"cap-image-answer\"").count(), 2);
    }

    #[test]
    fn captcha_frame_links_to_new_characters() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let ctx = serde_json::json!({ "id": "0123456789abcdef", "v": "beef" });
        let html = env.get_template("cap-image-frame.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        if let Ok(out) = std::env::var("F0CKM_DUMP_CAPFRAME") {
            std::fs::write(out, &html).unwrap();
        }
        assert!(html.contains("<a href=\"/api/cap/image/0123456789abcdef/frame?new=beef\" class=\"cap-image-new\""));
        assert!(html.contains("<img src=\"/api/cap/image/0123456789abcdef?v=beef\""));
        assert!(!html.contains("style="));
    }

    #[test]
    fn nojs_forms_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let url = serde_json::json!({ "pathname": "/7", "search": "", "qs": {} });
        // favourite: add when not favourited, remove when it is
        let ctx = serde_json::json!({ "item": { "id": 7 }, "csrf_token": "tok", "url": url, "fav_slot": "a", "user_has_favorited": false });
        let html = env.get_template("snippets/fav-nojs.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("<form method=\"post\" action=\"/api/v2/togglefav\" id=\"fav-nojs-form-a\""));
        assert!(html.contains("name=\"action\" value=\"add\""));
        assert!(html.contains("<button type=\"submit\" form=\"fav-nojs-form-a\" class=\"iconset fa-regular fa-heart fav-nojs-btn\""));
        let mut ctx = ctx;
        ctx["user_has_favorited"] = serde_json::json!(true);
        let html = env.get_template("snippets/fav-nojs.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("name=\"action\" value=\"remove\""));
        assert!(html.contains("class=\"iconset fa-solid fa-heart fav-nojs-btn\""));

        // comment form for a logged-in user
        let ctx = serde_json::json!({ "item": { "id": 7 }, "csrf_token": "tok", "url": url, "session": { "user": "bob" }, "captcha_scopes": [] });
        let html = env.get_template("snippets/comment-input.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("<form method=\"post\" action=\"/api/comments\" class=\"comment-input comment-form-nojs\">"));
        assert!(html.contains("name=\"back\" value=\"/7#comments-container\""));
        assert!(html.contains("<textarea name=\"content\""));
        assert!(!html.contains("cap-image-answer"));
        let mut ctx = ctx;
        ctx["captcha_scopes"] = serde_json::json!(["comment"]);
        ctx["nojs_captcha_id"] = serde_json::json!("0123456789abcdef");
        let html = env.get_template("snippets/comment-input.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("name=\"cap-image-answer\""));
    }

    /// The admin font manager: plain upload/delete forms (no JavaScript needed), deletes only for uploads.
    #[test]
    fn admin_fonts_page_renders() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let face = |file: &str, w: u16, uploaded: bool, builtin: bool| serde_json::json!({ "file": file, "weight": w, "weight_name": "Regular", "uploaded": uploaded, "builtin": builtin });
        let ctx = serde_json::json!({
            "shell_header": "snippets/admin-skin.html", "shell_footer": "snippets/admin-skin.html",
            "csrf_token": "tok", "session": { "user": "admin", "admin": true },
            "preview_css": "/admin/fonts/preview.css?v=1", "default_font": "Manrope-Regular.woff2", "brand_font": "Inter-Bold.woff2",
            "font_weights": [{ "weight": 400, "name": "Regular" }],
            "font_families": [
                { "name": "Inter", "key": "k1", "files": ["Inter-Bold.woff2"], "faces": [face("Inter-Bold.woff2", 700, true, false)] },
                { "name": "Manrope", "key": "k2", "files": ["Manrope-Regular.woff2"], "faces": [face("Manrope-Regular.woff2", 400, false, true)] },
            ],
        });
        let html = env.get_template("admin/fonts.html").unwrap().render(to_jvalue(&ctx)).unwrap();
        assert!(html.contains("action=\"/admin/fonts/upload\" enctype=\"multipart/form-data\""));
        assert!(html.contains("name=\"back\" value=\"/admin/fonts#font-upload\""));
        assert!(html.contains("<link rel=\"stylesheet\" href=\"/admin/fonts/preview.css?v=1\">"));
        assert!(html.contains("adm-font-sample-k1 adm-font-w700"));
        assert_eq!(html.matches("action=\"/admin/fonts/delete\"").count(), 1);
        assert!(html.contains("name=\"file\" value=\"Inter-Bold.woff2\""));
        assert!(html.contains(">Site default<") && html.contains(">Brand text<") && html.contains(">Built-in<"));
        assert!(!html.contains("style="));
    }

    /// The forced password change page (routes/password_change.rs) renders in f0ckm and in every
    /// frontend's shell, with a form that works without JavaScript. F0CKM_DUMP_PASSWORD_CHANGE=<dir>
    /// writes them out.
    #[test]
    fn password_change_page_renders_in_every_shell() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let mut shells = vec![(None, None)];
        for fe in ["f0ckgle", "f0ckyou", "w0bm", "f0ckv2"] {
            shells.push((Some(format!("frontends/{fe}/shell/header.html")), Some(format!("frontends/{fe}/shell/footer.html"))));
        }
        for (header, footer) in shells {
            let ctx = serde_json::json!({
                "shell_header": header, "shell_footer": footer, "domain": "f0ck.test", "csrf_token": "tok",
                "session": { "user": "bob", "login": "bob", "id": 1, "force_password_change": true },
                "modes": ["sfw", "nsfw", "untagged", "all", "nsfl"], "mode": 1, "anon_permissions_json": "{}", "captcha_scopes": [],
                "error": "New passwords do not match", "logout_url": "/logout?t=x",
            });
            let html = env.get_template("password-change.html").unwrap().render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{header:?}: {e:#}"));
            if let Ok(dir) = std::env::var("F0CKM_DUMP_PASSWORD_CHANGE") {
                let name = header.as_deref().and_then(|h| h.split('/').nth(1)).unwrap_or("f0ckm");
                std::fs::write(format!("{dir}/password-change-{name}.html"), &html).unwrap();
            }
            assert!(html.contains("<form id=\"force-password-form\" class=\"force-pw-form\" method=\"post\" action=\"/password-change\">"), "{header:?}");
            assert!(html.contains("name=\"csrf_token\" value=\"tok\""), "{header:?}");
            assert!(html.contains("name=\"new_password\"") && html.contains("name=\"new_password_confirm\""), "{header:?}");
            assert!(html.contains("force-pw-status-error\" role=\"alert\">New passwords do not match</div>"), "{header:?}");
            assert!(html.contains("href=\"/logout?t=x\""), "{header:?}");
            assert!(html.contains("/s/js/views/password-change.js"), "{header:?}");
            // the passkey offer (password-change.js) is there, hidden until the password is changed
            assert!(html.contains("<div id=\"force-pw-passkey\" class=\"force-pw-passkey force-pw-hidden\""), "{header:?}");
            assert!(html.contains("id=\"force-pw-passkey-add\"") && html.contains("data-msg-cancelled=\"Passkey creation was cancelled"), "{header:?}");
            let page = &html[html.find("force-pw-main").unwrap()..html.find("/s/js/views/password-change.js").unwrap()];
            assert!(!page.contains("style="), "{header:?}");
        }
    }

    #[test]
    fn settings_form_names_its_controls() {
        let s = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../views/settings.html")).unwrap();
        assert!(s.contains("<form method=\"post\" action=\"/settings\" class=\"settings-prefs-form\">"));
        for name in ["theme", "font", "language", "comment_display_mode", "default_upload_visibility"] {
            assert!(s.contains(&format!("name=\"{name}\"")), "{name}");
        }
        // every boolean the server saves has its hidden "0" and the box's "1"
        for col in ["show_motd", "use_new_layout", "disable_autoplay", "quote_emojis", "favorites_private", "do_not_disturb", "blur_nsfw", "hide_infobox"] {
            assert!(s.contains(&format!("<input type=\"hidden\" name=\"{col}\" value=\"0\">")), "{col}");
        }
    }

    #[test]
    fn settings_shows_only_the_frontends_preferences() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let empty = "frontends/w0bm/settings-prefs.html";
        let render = |prefs: Option<&str>| {
            let ctx = serde_json::json!({
                "shell_header": empty, "shell_footer": empty, "frontend_settings_prefs": prefs,
                "session": { "user": "bob", "id": 1 }, "enable_comments": true, "enable_swf": true,
                "allow_language_change": true, "languages": ["en", "de"], "show_koepfe": true,
                "csrf_token": "tok", "themes": ["f0ck"], "fonts": {},
            });
            env.get_template("settings.html").unwrap().render(to_jvalue(&ctx)).unwrap()
        };
        let all = render(None);
        let w0bm = render(Some("frontends/w0bm/settings-prefs.html"));
        // what w0bm uses stays
        for id in ["language_select", "website_font_select", "show_motd_toggle", "disable_autoplay_toggle", "favorites_private_toggle", "hide_fav_badge_toggle", "default_upload_visibility_select", "chk-do-not-disturb"] {
            assert!(all.contains(&format!("id=\"{id}\"")), "{id}");
            assert!(w0bm.contains(&format!("id=\"{id}\"")), "w0bm {id}");
        }
        // f0ckm's own preferences go
        for id in ["website_theme_select", "show_background_toggle", "use_new_layout_toggle", "hide_infobox_toggle", "wheel_nav_toggle", "comment_display_mode_select", "blur_nsfw_toggle", "hide_item_ratings_toggle", "ruffle_background_toggle"] {
            assert!(all.contains(&format!("id=\"{id}\"")), "{id}");
            assert!(!w0bm.contains(&format!("id=\"{id}\"")), "w0bm {id}");
        }
    }

    /// The font weight select lists the chosen family's weights, with the stored one selected; the
    /// text size select its sizes.
    #[test]
    fn settings_font_weight_and_size_selects() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let empty = "frontends/w0bm/settings-prefs.html";
        let fonts = serde_json::json!([
            { "name": "Hack", "file": "Hack.woff", "files": ["Hack.woff"], "weights": [{ "weight": 400, "name": "Regular" }] },
            { "name": "S", "file": "S-Regular.ttf", "files": ["S-Regular.ttf"], "variable": true,
              "weights": [{ "weight": 300, "name": "Light" }, { "weight": 400, "name": "Regular" }, { "weight": 700, "name": "Bold" }] },
        ]);
        let render = |font: &str, weight: u16, size: Option<u16>| {
            let ctx = serde_json::json!({ "shell_header": empty, "shell_footer": empty, "session": { "user": "bob", "id": 1, "font": font }, "csrf_token": "tok",
                "themes": ["f0ck"], "fonts": fonts, "user_font_weight": weight, "user_font_size": size, "font_sizes": crate::fonts::SIZES });
            env.get_template("settings.html").unwrap().render(to_jvalue(&ctx)).unwrap()
        };
        let html = render("S-Regular.ttf", 300, Some(120));
        assert!(html.contains("name=\"font_weight\""));
        assert!(html.contains("<option value=\"300\" selected>300 Light</option>"));
        assert!(html.contains("<option value=\"700\">700 Bold</option>"));
        assert!(html.contains("name=\"font_size\""));
        assert!(html.contains("<option value=\"120\" selected>120%</option>"));
        let one = render("Hack.woff", 400, None);
        let sel = one.split("id=\"website_font_weight_select\"").nth(1).unwrap().split('>').next().unwrap();
        assert!(sel.contains(" hidden"), "one weight: no weight select");
    }

    /// Admin → branding: the brand weight select lists the brand font family's weights after
    /// "Default weight", and is hidden for a family with one weight (or the frontend's own font).
    #[test]
    fn admin_brand_weight_select() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let empty = "frontends/w0bm/settings-prefs.html";
        let fonts = serde_json::json!([
            { "name": "Hack", "file": "Hack.woff", "files": ["Hack.woff"], "weights": [{ "weight": 400, "name": "Regular" }] },
            { "name": "M", "file": "M-Regular.woff2", "files": ["M-Light.woff2", "M-Regular.woff2", "M-Bold.woff2"],
              "weights": [{ "weight": 300, "name": "Light" }, { "weight": 400, "name": "Regular" }, { "weight": 700, "name": "Bold" }] },
        ]);
        let render = |font: &str, weight: Option<u16>| {
            let ctx = serde_json::json!({ "shell_header": empty, "shell_footer": empty, "session": { "user": "admin", "id": 1, "admin": true }, "csrf_token": "tok",
                "domain": "f0ck.me", "fonts": fonts, "brand_font": font, "brand_weight": weight, "brand_sizes": crate::settings::BRAND_SIZES });
            let html = env.get_template("admin.html").unwrap().render(to_jvalue(&ctx)).unwrap();
            html.split("id=\"brand-weight-select\"").nth(1).unwrap().split("</select>").next().unwrap().to_string()
        };
        let sel = render("M-Regular.woff2", Some(700));
        assert!(!sel.split('>').next().unwrap().contains(" hidden"));
        assert!(sel.contains("<option value=\"\">Default weight</option>"));
        assert!(sel.contains("<option value=\"700\" selected>700 Bold</option>"));
        assert!(sel.contains("<option value=\"300\">300 Light</option>"));
        assert!(render("Hack.woff", None).split('>').next().unwrap().contains(" hidden"));
        assert!(render("", None).split('>').next().unwrap().contains(" hidden"));
    }

    /// The admin panel's f0ckYou brand section: only with the f0ckyou frontend, filled from `fy_brand`.
    #[test]
    fn admin_fy_brand_section() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let empty = "frontends/w0bm/settings-prefs.html";
        let fonts = serde_json::json!([{ "name": "M", "file": "M-Regular.woff2", "files": ["M-Regular.woff2", "M-Bold.woff2"],
            "weights": [{ "weight": 400, "name": "Regular" }, { "weight": 700, "name": "Bold" }] }]);
        let render = |frontends: serde_json::Value| {
            let ctx = serde_json::json!({ "shell_header": empty, "shell_footer": empty, "session": { "user": "admin", "id": 1, "admin": true }, "csrf_token": "tok",
                "domain": "f0ck.me", "fonts": fonts, "brand_sizes": crate::settings::BRAND_SIZES, "frontends": frontends,
                "fy_brand": { "site": false, "image": "", "mark": false, "text": "f0ck*Tube*", "font": "M-Bold.woff2", "weight": 700, "size": 150 } });
            env.get_template("admin.html").unwrap().render(to_jvalue(&ctx)).unwrap()
        };
        assert!(!render(serde_json::json!(["f0ckm"])).contains("fy-brand-section"));
        let html = render(serde_json::json!(["f0ckm", "f0ckyou"]));
        assert!(html.contains("<form method=\"post\" action=\"/admin/fy_brand\" data-on=\"submit:prevent,saveFyBrand\">"));
        assert!(html.contains("name=\"text\" class=\"adm-text\" maxlength=\"68\" value=\"f0ck*Tube*\""));
        assert!(html.contains("<option value=\"M-Regular.woff2\" data-weights="));
        assert!(html.contains("<option value=\"700\" selected>700 Bold</option>"));
        assert!(html.contains("<option value=\"150\" selected>150%</option>"));
        assert!(html.contains("<input type=\"checkbox\" id=\"fy-brand-mark\" name=\"mark\" value=\"1\"> Finger mark"));
        assert!(html.contains("<button type=\"submit\" id=\"fy-brand-remove-btn\" class=\"adm-btn adm-btn-danger\" disabled>"));
    }

    /// mtime() finds assets through the project directory, whatever the working directory.
    #[test]
    fn mtime_resolves_from_the_project_dir() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let v: i64 = env.render_str(r#"{{ mtime("/public/s/js/f0ck-core.js") }}"#, ()).unwrap().parse().unwrap();
        assert!(v > 0);
    }

    /// websrv.bad_apple reaches f0ck-core.js' script tag: on by default (classic, with sound),
    /// gone with enabled: false, unknown themes fall back to classic.
    #[test]
    fn bad_apple_config_reaches_the_script_tag() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let tag = |yaml: &str| {
            let cfg = crate::media::test_cfg(yaml);
            let html = env.get_template("snippets/f0ck-core-script.html").unwrap().render(to_jvalue(&json!({ "bad_apple": bad_apple(&cfg) }))).unwrap();
            html.lines().find(|l| l.contains("f0ck-core.js")).unwrap().to_string()
        };
        let on = tag("websrv: {}");
        assert!(on.contains(r#"data-bad-apple="classic""#) && !on.contains("silent"), "{on}");
        assert!(on.contains(r#"data-bad-apple-v=""#), "the player is versioned: {on}");
        let xp = tag("websrv: { bad_apple: { theme: xp, sound: false } }");
        assert!(xp.contains(r#"data-bad-apple="xp""#) && xp.contains("data-bad-apple-silent"), "{xp}");
        assert!(tag("websrv: { bad_apple: { theme: '\"><script>' } }").contains(r#"data-bad-apple="classic""#));
        assert!(!tag("websrv: { bad_apple: { enabled: false } }").contains("data-bad-apple"));
    }

    /// Every navbar brand size has its rule in each frontend's CSS.
    #[test]
    fn brand_sizes_have_css() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        for f in ["public/s/css/f0ckm.css", "public/s/fe/w0bm/css/w0bm-fe.css", "public/s/fe/f0ckv2/css/f0ckv2.css", "public/s/fe/f0ckyou/css/f0ckyou.css"] {
            let css = std::fs::read_to_string(root.join(f)).unwrap();
            for n in crate::settings::BRAND_SIZES {
                assert!(css.contains(&format!("[data-brand-text].brand-size-{n} {{")), "{f} {n}");
            }
        }
    }

    /// f0ckYou's own brand (admin panel → f0ckYou brand): every size and weight has its rule.
    #[test]
    fn fy_brand_has_css() {
        let css = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../public/s/fe/f0ckyou/css/f0ckyou.css")).unwrap();
        for n in crate::settings::BRAND_SIZES {
            assert!(css.contains(&format!(".fy-logo-own.fy-bsize-{n} .fy-logo-word {{")), "{n}");
        }
        for w in crate::fonts::WEIGHTS {
            assert!(css.contains(&format!(".fy-logo-word.fy-bfont.fy-bweight-{w} {{\n  font-weight: {w};")), "{w}");
        }
    }

    #[test]
    fn brand_parts_emphasize_asterisks() {
        let p = brand_parts("f0ck*You*");
        assert_eq!(p, serde_json::json!([{ "text": "f0ck", "em": false }, { "text": "You", "em": true }]));
        assert_eq!(brand_parts("*a* b *c"), serde_json::json!([{ "text": "a", "em": true }, { "text": " b ", "em": false }, { "text": "c", "em": true }]));
        assert_eq!(brand_parts("plain"), serde_json::json!([{ "text": "plain", "em": false }]));
    }

    /// Every navbar brand weight has its rule in each frontend's CSS.
    #[test]
    fn brand_weights_have_css() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        for f in ["public/s/css/f0ckm.css", "public/s/fe/w0bm/css/w0bm-fe.css", "public/s/fe/f0ckv2/css/f0ckv2.css", "public/s/fe/f0ckyou/css/f0ckyou.css"] {
            let css = std::fs::read_to_string(root.join(f)).unwrap();
            for w in crate::fonts::WEIGHTS {
                assert!(css.contains(&format!("[data-brand-text].brand-font.brand-weight-{w} {{\n  font-weight: {w} !important;")), "{f} {w}");
            }
        }
    }

    /// The site chat comes with every frontend for logged-in visitors (with enable_global_chat): its
    /// CSS before the frontend's, the skin name, the frontend's own texts, the script; nothing for
    /// guests. F0CKM_DUMP_GCHAT=<dir> writes the pages out (<frontend>.html).
    #[test]
    fn globalchat_in_every_frontend() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let session = serde_json::json!({ "user": "bob", "id": 1, "admin": true, "avatar": 5, "csrf_token": "tok", "ruffle_volume": 0.5 });
        for fe in ["f0ckm", "f0ckgle", "f0ckyou", "f0ckv2", "w0bm"] {
            let view = if fe == "f0ckm" { "about.html".to_string() } else { format!("frontends/{fe}/about.html") };
            let ctx = |session: serde_json::Value| {
                to_jvalue(&serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "frontend": fe, "frontends": ["f0ckm", fe],
                    "enable_global_chat": true, "session": session, "mode": 1, "modes": ["sfw", "nsfw", "untagged", "all"],
                    "url": { "pathname": "/about", "search": "", "qs": {} }, "tmp": {}, "anon_permissions_json": "{}", "captcha_scopes": [],
                    // the page data blocks' raw values (so the dumped pages run)
                    "nsfl_tag_id": 3, "comment_display_mode": 0, "comment_max_length": 2000, "fileupload_comments_size": 0, "fileupload_comments_max": 0,
                    "themes_json": "[]", "allowed_comment_images_json": "[]", "custom_brand_images_json": "[]" }))
            };
            let tpl = env.get_template(&view).unwrap_or_else(|e| panic!("{view}: {e}"));
            let html = tpl.render(ctx(session.clone())).unwrap_or_else(|e| panic!("{view}: {e:#}"));
            if let Ok(dir) = std::env::var("F0CKM_DUMP_GCHAT") {
                std::fs::write(PathBuf::from(dir).join(format!("{fe}.html")), &html).unwrap();
            }
            let css = html.find("/s/css/globalchat.css").unwrap_or_else(|| panic!("{fe}: no globalchat.css"));
            let own_css = if fe == "f0ckm" { "/s/css/bundle.css".to_string() } else { format!("/s/fe/{fe}/css/") };
            assert!(css < html.find(&own_css).unwrap(), "{fe}: globalchat.css after the frontend's CSS");
            assert!(html.contains("<script type=\"application/json\" id=\"gchat-skin\">"), "{fe}");
            assert!(html.contains(&format!("\"skin\": \"{fe}\"")), "{fe}");
            assert!(html.contains("/s/js/globalchat.js?v="), "{fe}");
            assert_eq!(html.contains("id=\"gchat-skin-texts\""), fe != "f0ckm" && fe != "w0bm", "{fe}");
            let guest = tpl.render(ctx(serde_json::Value::Null)).unwrap();
            assert!(!guest.contains("globalchat"), "{fe}: chat for a guest");
            // f0ckm's layout under another frontend (its 4chan proxy items): f0ckm's skin
            if fe != "f0ckm" {
                let f0ckm_page = env.get_template("about.html").unwrap().render(ctx(session.clone())).unwrap();
                assert!(f0ckm_page.contains("\"skin\": \"f0ckm\"") && !f0ckm_page.contains("gchat-skin-texts"), "{fe}: f0ckm's layout");
            }
            // a shared page in the frontend's shell: once
            if fe != "f0ckm" {
                let mut sctx = serde_json::to_value(ctx(session.clone())).unwrap();
                sctx["shell_header"] = format!("frontends/{fe}/shell/header.html").into();
                sctx["shell_footer"] = format!("frontends/{fe}/shell/footer.html").into();
                let shell = env.get_template("privacy.html").unwrap().render(to_jvalue(&sctx)).unwrap_or_else(|e| panic!("{fe} shell: {e:#}"));
                assert!(shell.contains(&format!("\"skin\": \"{fe}\"")), "{fe} shell");
                assert_eq!(shell.matches("/s/css/globalchat.css").count(), 1, "{fe} shell");
                assert_eq!(shell.matches("/s/js/globalchat.js").count(), 1, "{fe} shell");
                if let Ok(dir) = std::env::var("F0CKM_DUMP_GCHAT") {
                    std::fs::write(PathBuf::from(dir).join(format!("{fe}-shell.html")), &shell).unwrap();
                }
            }
        }
    }

    /// Every text size has its rule in f0ckm.css, w0bm-fe.css, f0ckv2.css, f0ckgle.css and f0ckyou.css.
    #[test]
    fn text_sizes_have_css() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let f0ckm = std::fs::read_to_string(root.join("public/s/css/f0ckm.css")).unwrap();
        let w0bm = std::fs::read_to_string(root.join("public/s/fe/w0bm/css/w0bm-fe.css")).unwrap();
        let v2 = std::fs::read_to_string(root.join("public/s/fe/f0ckv2/css/f0ckv2.css")).unwrap();
        let gle = std::fs::read_to_string(root.join("public/s/fe/f0ckgle/css/f0ckgle.css")).unwrap();
        let tube = std::fs::read_to_string(root.join("public/s/fe/f0ckyou/css/f0ckyou.css")).unwrap();
        for n in crate::fonts::SIZES {
            assert!(gle.contains(&format!("html.user-fs-{n} {{\n  font-size: {n}%;")), "f0ckgle.css {n}");
            assert!(tube.contains(&format!("html.user-fs-{n} {{\n  font-size: {n}%;")), "f0ckyou.css {n}");
            assert!(v2.contains(&format!("html.user-fs-{n} {{\n  --fs-zoom: ")), "f0ckv2.css {n}");
            assert!(f0ckm.contains(&format!("html.user-fs-{n} {{")), "f0ckm.css {n}");
            assert!(w0bm.contains(&format!("html.user-fs-{n} {{\n    --fs-zoom: ")), "w0bm-fe.css {n}");
        }
    }

    /// w0bm loads the visitor's website font as a stylesheet (its pages have no f0ckm header <style>).
    #[test]
    fn w0bm_loads_the_users_font() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let head = |css: &str| env.get_template("frontends/w0bm/partials/head-assets.html").unwrap().render(to_jvalue(&serde_json::json!({ "user_font_css": css }))).unwrap();
        assert!(head("/user-font.css?f=Inter-Bold.woff2").contains("<link rel=\"stylesheet\" id=\"user-font-css\" href=\"/user-font.css?f=Inter-Bold.woff2\">"));
        assert!(!head("").contains("user-font-css"));
        assert_eq!(crate::fonts::user_css_url("My Font-Bold.woff2", None), "/user-font.css?f=My%20Font-Bold.woff2");
        assert_eq!(crate::fonts::user_css_url("X.ttf", Some(300)), "/user-font.css?f=X.ttf&w=300");
    }

    /// w0bm's user halls page: the tiles, and for whoever manages them the plain forms (new hall,
    /// edit, picture, delete behind a confirmation dialog) with `back` and the admin's `manage_qs`;
    /// guests get the tiles only. F0CKM_DUMP_W0BM_HALLS=<dir> writes the pages out.
    #[test]
    fn w0bm_user_halls_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let al = serde_json::json!({ "user": "al", "id": 2 });
        let halls = serde_json::json!([
            { "id": 1, "slug": "mine", "image_v": "5", "name": "Mine", "description": "my hall", "is_private": true, "custom_image": true, "total_items": 3 },
            { "id": 2, "slug": "empty", "name": "Empty", "description": null, "is_private": false, "custom_image": false, "total_items": 1 }]);
        let render = |data: serde_json::Value| {
            let mut ctx = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "frontend": "w0bm", "mode": 1, "userhalls_enabled": true,
                "enable_userhall_image_upload": true, "user": al, "ownerUser": al, "hallsList": halls,
                "url": { "pathname": "/user/al/lists", "search": "", "qs": {} } });
            ctx.as_object_mut().unwrap().extend(data.as_object().unwrap().clone());
            env.get_template("frontends/w0bm/user-halls.html").unwrap().render(to_jvalue(&ctx)).unwrap()
        };
        let dump = |name: &str, html: &str| {
            if let Ok(dir) = std::env::var("F0CKM_DUMP_W0BM_HALLS") {
                std::fs::write(PathBuf::from(dir).join(format!("{name}.html")), html).unwrap();
            }
        };
        let admin = render(serde_json::json!({ "session": { "user": "bob", "id": 1, "admin": true }, "isOwner": false, "can_manage": true, "manage_qs": "?user_id=2" }));
        dump("manage", &admin);
        assert!(admin.contains("<form method=\"post\" action=\"/api/v2/me/lists?user_id=2\" class=\"w0bm-uh-form w0bm-uh-new\" data-uh=\"create\">"));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine?user_id=2\" class=\"w0bm-uh-form\" data-uh=\"save\""));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine/delete?user_id=2\""));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine/image?user_id=2\" enctype=\"multipart/form-data\""));
        assert!(admin.contains("action=\"/api/v2/me/lists/mine/image/delete?user_id=2\""), "remove only with a custom picture");
        assert!(!admin.contains("/api/v2/me/lists/empty/image/delete"));
        assert!(admin.contains("<input type=\"radio\" name=\"is_private\" value=\"1\" checked>"));
        assert!(admin.contains("<input type=\"hidden\" name=\"back\" value=\"/user/al/lists\">"));
        assert!(admin.contains("href=\"#w0bm-uh-mine-delete\"") && admin.contains("id=\"w0bm-uh-mine-delete\""));
        assert!(admin.contains("src=\"/user_list_image/2/mine?m=1&v=5\""));
        assert!(admin.contains("<a href=\"/user/al/list/mine\" class=\"btn btn-primary\" role=\"button\">View</a>"));
        assert!(!admin.contains("border-radius") && !admin.contains("<script>"));
        let guest = render(serde_json::json!({ "isOwner": false, "can_manage": false, "manage_qs": "" }));
        dump("guest", &guest);
        assert!(guest.contains("<a href=\"/user/al/list/empty\" class=\"btn btn-primary\" role=\"button\">View</a>"));
        assert!(!guest.contains("w0bm-uh-form") && !guest.contains("-edit\""));
    }

    /// The admin user manager: every kind of row renders, each action is a plain form (works without
    /// JavaScript) that returns to its row, and the self/system/legacy rows offer only what applies.
    /// `F0CKM_DUMP_ADMIN_USERS=<dir>` writes the page out for a look in a browser.
    #[test]
    fn admin_users_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let user = |id: Option<i64>, login: &str, extra: serde_json::Value| {
            let mut u = serde_json::json!({ "id": id, "login": login, "user": login, "email": null, "created_at": "2026-11-18T10:00:00Z",
                "banned": false, "ban_reason": null, "ban_expires": null, "is_moderator": false, "admin": false, "groups": [], "activated": true,
                "avatar_file": null, "display_name": null, "force_comment_display_mode": 0, "comment_display_mode": 0, "reg_method": null,
                "age_days": 1.0, "upload_count": 2.0, "comment_count": 0.0, "failed_attempts": 0.0 });
            u.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            u
        };
        let users = serde_json::json!([
            user(Some(1), "bob", serde_json::json!({ "admin": true, "email": "bob@f0ck.test" })),
            user(Some(2), "yotap", serde_json::json!({ "display_name": "Yo Tap", "groups": ["4chan", "trusted"], "failed_attempts": 6.0, "force_comment_display_mode": 1, "comment_display_mode": 1,
                "permissions": { "comment": false }, "perms_effective": { "comment": false, "upload": true } })),
            user(Some(3), "newbie", serde_json::json!({ "activated": false, "upload_count": 0.0 })),
            user(Some(4), "troll", serde_json::json!({ "banned": true, "ban_reason": "spam", "ban_expires": "2026-12-01T00:00:00Z", "is_moderator": true })),
            user(Some(5), "deleted_user", serde_json::json!({ "banned": true, "activated": false })),
            user(None, "old guy", serde_json::json!({ "reg_method": "Legacy" })),
        ]);
        let ctx = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "session": { "user": "bob", "id": 1, "admin": true },
            "users": users, "back": "/admin/users?q=o", "prevUrl": null, "nextUrl": "/admin/users?q=o&page=2", "openRow": "2", "rawQ": "o", "q": "o",
            "page": 1, "total": 6, "hasMore": true, "onlyLegacy": false, "status": "", "role": "", "totalLabel": "registered members",
            "emptyMsg": "No users matched your search.", "totals": {}, "log_user_ips": true, "tmp": null,
            "perm_catalog": crate::perms::catalog_json(), "perm_groups": ["4chan", "vip"], "perm_off": { "chat": "websrv.enable_global_chat" } });
        let html = env.get_template("admin/users.html").unwrap().render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{e:#}"));
        if let Ok(dir) = std::env::var("F0CKM_DUMP_ADMIN_USERS") {
            std::fs::write(PathBuf::from(dir).join("users.html"), &html).unwrap();
        }
        let row = |id: &str| {
            let start = html.find(&format!("id=\"user-row-{id}\"")).unwrap_or_else(|| panic!("row {id}"));
            // the row's own </details> starts a line (the nested permission overrides are indented)
            let end = html[start..].find("\n</details>").unwrap() + start;
            html[start..end].to_string()
        };
        let page = &html[html.find("id=\"admin-users\"").unwrap()..html.find("id=\"admin-users-data\"").unwrap()];
        assert!(!page.contains("border-radius") && !page.contains(" style=\"") && !page.contains("<script>"));
        // the row a plain form came back to is open; the forms return there
        assert!(row("2").contains(" open>"));
        assert!(!row("3").contains(" open>"));
        assert!(row("2").contains("name=\"back\" value=\"/admin/users?q=o&amp;open=2#user-row-2\""));
        assert!(row("2").contains("Yo Tap") && row("2").contains("@yotap") && row("2").contains(">4chan<"));
        assert!(row("2").contains("Locked out") && row("2").contains("/api/v2/admin/users/reset-login-attempts"));
        assert!(row("2").contains("name=\"lock\" value=\"false\""));
        // pending: verify; banned: unban instead of ban
        assert!(row("3").contains("/api/v2/admin/users/activate") && row("3").contains(">Pending<"));
        assert!(row("4").contains("/api/v2/admin/unban") && !row("4").contains("/api/v2/admin/ban\""));
        assert!(row("4").contains("Banned until") && row("4").contains("spam"));
        // yourself: no ban, role change or delete
        let me = row("1");
        assert!(me.contains(">you<") && !me.contains("/api/v2/admin/ban\"") && !me.contains("set-role") && !me.contains("/api/v2/admin/users/delete\""));
        // the system ghost and legacy names: content tools only
        let sys = row("5");
        assert!(sys.contains("System account") && !sys.contains("users/rename") && !sys.contains("reassign-uploads") && sys.contains("bulk-delete-items"));
        let legacy = row("ghost-old guy");
        assert!(legacy.contains("name=\"source_username\" value=\"old guy\"") && legacy.contains("open=ghost-old%20guy") && !legacy.contains("set-password"));
        // destructive forms need a ticked box, also without JavaScript
        assert!(row("2").matches("name=\"confirm\" value=\"1\" required").count() == 4);
        // no-JS filters and paging
        assert!(html.contains("<form class=\"adm-users-filters\" id=\"user-filters\" method=\"get\" action=\"/admin/users\""));
        assert!(html.contains("href=\"/admin/users?q=o&amp;page=2\""));
        assert!(html.contains("<a href=\"#create-user-modal\""));
        // groups: every known one to tick, the user's own extra ones ticked; permission overrides
        let yo = row("2");
        assert!(yo.contains("name=\"groups\" value=\"vip\">") && yo.contains("name=\"groups\" value=\"4chan\" checked>") && yo.contains("name=\"groups\" value=\"trusted\" checked>"));
        assert!(yo.contains(">trusted<") && yo.contains("action=\"/api/v2/admin/users/set-permissions\""));
        assert!(yo.contains("name=\"perm_comment\" value=\"deny\" checked>") && yo.contains("name=\"perm_upload\" value=\"inherit\" checked>"));
        assert!(yo.contains("name=\"clear\" value=\"1\"") && yo.contains("id=\"user-perms-2\""));
        assert!(yo.contains("not allowed now") && !yo.contains("perm_mod_ban"), "mod powers only for moderators");
        assert!(row("4").contains("perm_mod_ban"));
        assert!(me.contains("Admins have every permission") && !me.contains("set-permissions"));
        // a feature that is off on the site: greyed out, no ✓/✗
        assert!(yo.contains("class=\"adm-user-perm is-off\" title=\"Off on this site (websrv.enable_global_chat). ") && !yo.contains("data-perm-now=\"chat\""));
        // warnings: a form for everyone but yourself
        assert!(yo.contains("action=\"/api/v2/mod/warnings/issue\"") && yo.contains("name=\"reason\" class=\"adm-text\" required") && !me.contains("warnings/issue"));
    }

    /// 4chan catalogue and thread: plain links and forms (no JavaScript needed), the filter state
    /// rendered on the server, the page scripts inside #main (they run on in-place navigation).
    /// `F0CKM_DUMP_CHAN=<dir>` writes both pages out.
    #[test]
    fn chan_pages_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let session = serde_json::json!({ "user": "bob", "id": 1, "admin": true, "csrf_token": "tok" });
        let cat = serde_json::json!({ "domain": "f0ck.test", "session": session, "tmp": null, "board": "wsg", "q": "cat", "sort": "replies", "shown": 1,
            "common_boards": [{ "id": "wsg", "name": "Worksafe GIF", "icon": "fa-film" }, { "id": "b", "name": "Random", "icon": "fa-dice" }],
            "threads": [
                { "no": 11, "sub": "cat thread", "com": "meow", "replies": 9, "images": 3, "time": 100, "sticky": false, "closed": false, "thumb": "/x/11s.jpg", "bump": 1, "hidden": false },
                { "no": 22, "sub": "dogs", "com": "woof", "replies": 2, "images": 1, "time": 200, "sticky": false, "closed": false, "thumb": null, "bump": 0, "hidden": true, "local_id": 5, "local_path": 5 },
            ] });
        let html = env.get_template("chan/catalogue.html").unwrap().render(to_jvalue(&cat)).unwrap_or_else(|e| panic!("{e:#}"));
        let main = &html[html.find("id=\"main\"").unwrap()..];
        assert!(main.contains("data-chan-path=\"/4/wsg/catalogue\""));
        assert!(main.contains("<form class=\"chan-board-custom-form\" method=\"get\" action=\"/4/catalogue\">") && main.contains("name=\"board\""));
        assert!(main.contains("action=\"/4/wsg/catalogue\" role=\"search\"") && main.contains("name=\"q\" value=\"cat\""));
        assert!(main.contains("<option value=\"replies\" selected>") && main.contains("href=\"/4/wsg/catalogue?sort=replies\" id=\"chan-search-clear\" class=\"is-shown\""));
        assert!(main.contains("class=\"chan-thread-card is-hidden\" data-no=\"22\" data-bump=\"0\"") && main.contains("class=\"chan-thread-card\" data-no=\"11\""));
        assert!(main.contains("id=\"chan-no-results\" class=\"chan-empty-state\""), "results shown: no empty note");
        let cat_scripts = &main[..main.find("<div class=\"chan-catalogue-grid\"").unwrap()];
        assert!(cat_scripts.contains("catalogue.actions.js"));
        let in_main = &main[..main.find("</div>\n</div>").unwrap()];
        assert!(in_main.contains("/s/js/views/chan/catalogue.js"), "the page script runs on in-place navigation");
        assert!(!in_main.contains(" style=\"") && !in_main.contains("data-on=\"submit"));

        let post = |no: i64, media: bool, local: Option<i64>, fav: bool| serde_json::json!({ "no": no, "name": "Anonymous", "iso_str": "2026-01-01T00:00:00Z", "date_str": "x", "timeago": "1d",
            "has_media": media, "ext": ".webm", "tim": 1700000000000i64 + no, "filename": "clip", "w": 640, "h": 360, "fsize": "1 MB", "thumb": "/t.jpg", "is_video": true,
            "external_media_url": format!("https://i.4cdn.org/wsg/{no}.webm"), "dest": format!("/media/{no}.webm"), "local_id": local, "local_path": local, "user_has_favorited": fav, "com_raw": "hi", "com_formatted": "hi", "replies": [] });
        let thr = serde_json::json!({ "domain": "f0ck.test", "session": session, "csrf_token": "tok", "tmp": null, "board": "wsg", "tid": 100, "show": "media",
            "op": { "sub": "op" }, "media_count": 2, "first_media_no": 100, "rehosts_count": 1,
            "posts": [post(100, true, None, false), post(101, true, Some(7), true), post(102, false, None, false)] });
        let html = env.get_template("chan/thread.html").unwrap().render(to_jvalue(&thr)).unwrap_or_else(|e| panic!("{e:#}"));
        if let Ok(dir) = std::env::var("F0CKM_DUMP_CHAN") {
            std::fs::write(PathBuf::from(&dir).join("thread.html"), &html).unwrap();
            let cat_html = env.get_template("chan/catalogue.html").unwrap().render(to_jvalue(&cat)).unwrap();
            std::fs::write(PathBuf::from(&dir).join("catalogue.html"), cat_html).unwrap();
        }
        let main = &html[html.find("id=\"main\"").unwrap()..];
        assert!(main.contains("data-chan-path=\"/4/wsg/100\""));
        // filters: links, the current one active, posts it leaves out hidden
        assert!(main.contains("href=\"/4/wsg/100?show=media\" class=\"chan-toggle-btn active\""));
        assert!(main.contains("href=\"/4/wsg/100\" class=\"chan-toggle-btn\" id=\"filter-all-btn\""));
        let card = |no: i64| { let at = main.find(&format!("id=\"p{no}\"")).unwrap(); &main[main[..at].rfind("<article").unwrap()..at] };
        assert!(card(102).contains("is-hidden") && !card(100).contains("is-hidden"));
        // not rehosted: a rehost form and a rehost-and-favourite form; rehosted: the favourite toggle
        assert!(main.contains("action=\"/api/v2/scroller/rehost\" class=\"chan-rehost-form\""));
        assert!(main.contains("<input type=\"hidden\" name=\"fav\" value=\"add\">"));
        assert!(main.contains("name=\"postid\" value=\"7\">") && main.contains("<input type=\"hidden\" name=\"action\" value=\"delete\">"));
        assert!(main.contains("name=\"back\" value=\"/4/wsg/100?show=media#p100\""));
        assert!(main.contains("<a href=\"/7\" class=\"chan-rehost-edit-btn\" data-item-id=\"7\""));
        assert!(main.contains("data-inline=\"/media/100.webm\" data-kind=\"video\""), "a click plays it in the post");
        let in_main = &main[..main.find("</div>\n</div>").unwrap()];
        assert_eq!(in_main.matches("name=\"csrf_token\" value=\"tok\"").count(), 3, "rehost + rehost-and-favourite, favourite toggle");
        assert!(in_main.contains("/s/js/views/chan/thread.js"), "the page script is inside #main");
        assert!(!in_main.contains(" style=\"") && !in_main.contains("<button type=\"button\" class=\"chan-rehost-btn"));
    }

    /// Admin panel → Permissions: the role table (moderator powers only for moderators), the groups,
    /// plain forms that come back to their place without JavaScript.
    #[test]
    fn admin_permissions_render() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = build_env(root.join("views"), Arc::new(I18n::new(root.join("locales"))));
        let cfg = crate::media::test_cfg("{}");
        let mut pc = crate::perms::PermConfig::default();
        pc.roles.insert("user".into(), std::collections::BTreeMap::from([("comment".to_string(), false)]));
        pc.groups.insert("trusted".into(), std::collections::BTreeMap::from([("upload_large".to_string(), true)]));
        let ctx = serde_json::json!({ "domain": "f0ck.test", "csrf_token": "tok", "session": { "user": "bob", "id": 1, "admin": true },
            "catalog": crate::perms::catalog_json(),
            "perm_off": { "chat": "websrv.enable_global_chat" },
            "roles": crate::perms::Role::ALL.iter().map(|r| serde_json::json!({ "key": r.key(), "label": r.label() })).collect::<Vec<_>>(),
            "role_values": crate::perms::roles_json(&cfg, &pc),
            "groups": [
                { "name": "4chan", "members": 2, "perms": { "chan": true }, "configured": true },
                { "name": "trusted", "members": 1, "perms": { "upload_large": true }, "configured": true },
                { "name": "vip", "members": 3, "perms": {}, "configured": false },
            ],
            "anon_access": true, "tmp": null });
        let html = env.get_template("admin/permissions.html").unwrap().render(to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{e:#}"));
        if let Ok(dir) = std::env::var("F0CKM_DUMP_ADMIN_PERMS") {
            std::fs::write(PathBuf::from(dir).join("permissions.html"), &html).unwrap();
        }
        let page = &html[html.find("id=\"perm-page\"").unwrap()..html.find("/s/js/views/admin/permissions.js").unwrap()];
        assert!(!page.contains("border-radius") && !page.contains(" style=\"") && !page.contains("<script>"));
        // the role table: the user default set to no, the rest default; mod powers only for mods
        let sel = |name: &str| {
            let at = page.find(&format!("name=\"{name}\"")).unwrap_or_else(|| panic!("{name}"));
            page[at..at + page[at..].find("</select>").unwrap()].to_string()
        };
        assert!(sel("user.comment").contains("<option value=\"deny\" selected>") && page.contains("name=\"user.comment\" class=\"adm-text adm-select adm-perm-select is-set\""));
        assert!(sel("anon.upload").contains("<option value=\"default\" selected>Default (no)</option>"));
        assert!(page.contains("<tr id=\"perm-chat\" class=\"is-off\">") && page.contains("off on this site</span>"));
        assert!(sel("mod.upload_large").contains("Default (yes)"));
        assert!(page.contains("name=\"mod.mod_ban\"") && !page.contains("name=\"user.mod_ban\"") && !page.contains("name=\"anon.mod_ban\""));
        // groups: their own rules, removal only where there are any, a form for a new one
        assert!(page.contains("<input type=\"hidden\" name=\"name\" value=\"trusted\">"));
        assert!(page.matches("action=\"/admin/permissions/group/delete\"").count() == 2);
        assert!(page.contains("no permissions yet") && page.contains("id=\"perm-new-group\""));
        assert!(page.contains("name=\"back\" value=\"/admin/permissions#perm-roles\""));
        assert!(page.matches("class=\"adm-form-actions adm-nojs-only\"").count() == 4, "a no-JS save button per form");
    }

    #[test]
    fn blank_lines_collapse() {
        assert_eq!(collapse_blank_lines("a\n\n  \nb"), "a\nb");
    }
}
