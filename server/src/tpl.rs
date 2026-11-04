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

pub(crate) fn build_env(views: PathBuf, i18n: Arc<I18n>) -> Environment<'static> {
    let mut env = Environment::new();
    env.set_loader(minijinja::path_loader(views));
    env.set_undefined_behavior(UndefinedBehavior::Chainable);
    env.set_auto_escape_callback(|name| if name.ends_with(".html") { AutoEscape::Html } else { AutoEscape::None });
    env.set_formatter(js_formatter);
    env.set_syntax(minijinja::syntax::SyntaxConfig::builder().keep_trailing_newline(true).build().expect("default syntax is valid"));
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
            other => Ok(to_jvalue(&other)),
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

/// Every selectable frontend: the default one first, then the installed ones.
pub fn frontend_names(st: &AppState) -> Vec<String> {
    let mut v = vec![DEFAULT_FRONTEND.to_string()];
    v.extend(st.tpl.frontends().into_iter().filter(|n| n != DEFAULT_FRONTEND));
    v
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
    if !frontend_switch_enabled(cfg) || !frontend_names(st).iter().any(|n| n == name) {
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
    let names = frontend_names(st);
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
        for v in &shared {
            if dir.join(v).is_file() {
                problems.push(format!("{}: overrides the shared page views/{v}; delete it (it renders in the shell)", at(v)));
            }
        }
        // Its own pages load the shared scripts; nothing copies the page data
        let mut own_includes_scripts = false;
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
                if text.contains("id=\"footer-data\"") {
                    problems.push(format!("{}: copies #footer-data; include snippets/frontend-scripts.html instead", at(&rel)));
                }
            }
        }
        if !own_includes_scripts {
            problems.push(format!("views/{FRONTENDS_DIR}/{fe}: no page layout includes snippets/frontend-scripts.html"));
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
    put("captcha_enabled", json!(crate::cap::enabled(cfg)));
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
    put("albums_enabled", json!(cfg.enabled("websrv.albums_enabled")));
    put("enable_youtube_upload", json!(cfg.enabled("websrv.enable_youtube_upload")));
    put("upload_limit", if cfg.get("main.upload_limit").is_null() { json!(69) } else { cfg.get("main.upload_limit").clone() });
    put("meme_creator", json!(cfg.truthy("websrv.meme_creator")));
    put("custom_favicon", json!(s.favicon_url));
    put("custom_brand_image", json!(brand));
    put("custom_navbar_brand_text", json!(s.brand_text));
    put("brand_glitch_text", json!(s.brand_glitch_text));
    put("brand_glitch_image", json!(s.brand_glitch_image));
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
    ctx.insert("captcha_enabled".into(), json!(captcha));
    // Image captcha in the login / register modals for visitors without JavaScript (cap_image.rs)
    if captcha {
        ctx.insert("nojs_captcha_id".into(), json!(crate::cap_image::new_id()));
    }
    // Actions this visitor must solve a captcha for (cap.uploads / cap.comments): loads the popup
    ctx.insert("captcha_scopes".into(), json!(req.map(|r| crate::cap::scopes_needed(&cfg, r)).unwrap_or_default()));
    ctx.insert("csp_nonce".into(), json!(req.map(|r| r.csp_nonce.as_str()).unwrap_or("")));
    // @font-face rules (one per weight) for the visitor's font, else the site's default font
    let chosen_font = session.and_then(|s| s.font.clone()).filter(|f| !f.is_empty()).or_else(|| cfg.str("websrv.default_font").map(str::to_string));
    ctx.insert("font_faces".into(), crate::fonts::faces_json(&cfg, chosen_font.as_deref()));
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
    ctx.insert("frontends".into(), json!(frontend_names(st)));
    ctx.insert("frontend_switch".into(), json!(frontend_switch_enabled(&cfg)));
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
    fn frontends_follow_contract() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let problems = check_frontends(&root.join("views"), &root.join("public"));
        assert!(problems.is_empty(), "frontend contract (views/frontends/README.md):\n{}", problems.join("\n"));
    }

    #[test]
    fn numbers_format_like_js() {
        assert_eq!(locale_number(1234567.0), "1,234,567");
        assert_eq!(locale_number(1234.5), "1,234.5");
        assert_eq!(js_to_string(&JValue::from(3.0)), "3");
        assert_eq!(js_to_string(&JValue::from(vec![1, 2])), "1,2");
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
        for id in ["language_select", "show_motd_toggle", "disable_autoplay_toggle", "favorites_private_toggle", "hide_fav_badge_toggle", "default_upload_visibility_select", "chk-do-not-disturb"] {
            assert!(all.contains(&format!("id=\"{id}\"")), "{id}");
            assert!(w0bm.contains(&format!("id=\"{id}\"")), "w0bm {id}");
        }
        // f0ckm's own preferences go
        for id in ["website_theme_select", "website_font_select", "show_background_toggle", "use_new_layout_toggle", "hide_infobox_toggle", "wheel_nav_toggle", "comment_display_mode_select", "blur_nsfw_toggle", "hide_item_ratings_toggle", "ruffle_background_toggle"] {
            assert!(all.contains(&format!("id=\"{id}\"")), "{id}");
            assert!(!w0bm.contains(&format!("id=\"{id}\"")), "w0bm {id}");
        }
    }

    #[test]
    fn blank_lines_collapse() {
        assert_eq!(collapse_blank_lines("a\n\n  \nb"), "a\nb");
    }
}
