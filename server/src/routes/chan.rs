//! 4chan thread viewer & catalogue (`src/inc/routes/chan.mjs`), restricted to the `4chan` group
//! and admins (`Guard::ChanAuth`). Also holds the 4chan JSON helpers `external.rs` shares.
//!
//! Differences to the old code:
//! * 4chan comment HTML is rendered through an allow-list sanitizer ([`sanitize_comment_html`])
//!   instead of verbatim; names, subjects and file names are entity-decoded and autoescaped.
//! * Upstream errors shown to the client don't include curl's command line.

use crate::chan_http::{self, ChanUrl};
use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, html, html_code, json_resp, redirect, with_header};
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::response::Response;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/4\/?$", Guard::ChanAuth, chan_root);
    r.get_g(r"^\/4\/(?<board>[a-z0-9]+)\/(?:catalogue|catalog)\/?$", Guard::ChanAuth, catalogue);
    r.get_g(r"^\/4\/(?<board>[a-z0-9]+)\/(?<thread>\d+)\/?$", Guard::ChanAuth, thread);
    r.get_g(r"^\/4\/(?<board>[a-z0-9]+)\/(?<thread>\d+)\/(?<post>\d+)\/?$", Guard::ChanAuth, thread_item);
    r.get_g(r"^\/api\/v2\/chan\/(?<board>[a-z0-9]+)\/(?<thread>\d+)\/media\/?$", Guard::ChanAuth, thread_media);
}

const COMMON_BOARDS: [(&str, &str, &str); 12] = [
    ("wsg", "Worksafe GIF", "fa-film"),
    ("gif", "Adult GIF", "fa-video"),
    ("b", "Random", "fa-dice"),
    ("v", "Video Games", "fa-gamepad"),
    ("vg", "Video Game Generals", "fa-gamepad"),
    ("a", "Anime & Manga", "fa-tv"),
    ("g", "Technology", "fa-microchip"),
    ("pol", "Politically Incorrect", "fa-globe"),
    ("tv", "Television & Film", "fa-tv"),
    ("mu", "Music", "fa-music"),
    ("fit", "Fitness", "fa-dumbbell"),
    ("ck", "Food & Cooking", "fa-utensils"),
];

// ── 4chan JSON helpers ──────────────────────────────────────────────────────

/// `String(v)` for JSON values (numbers print like JavaScript).
pub(crate) fn js_str(v: &Value) -> String {
    crate::tpl::js_to_string(&crate::tpl::to_jvalue(v))
}

/// `Number(p[k])` for post numbers.
pub(crate) fn num(p: &Value, k: &str) -> Option<i64> {
    match &p[k] {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(crate) fn truthy(v: &Value) -> bool {
    crate::config::truthy(v)
}

/// `p.tim && p.ext`
pub(crate) fn has_media(p: &Value) -> bool {
    truthy(&p["tim"]) && truthy(&p["ext"])
}

/// `p.tim` as a string key (only when truthy).
pub(crate) fn tim_key(p: &Value) -> Option<String> {
    truthy(&p["tim"]).then(|| js_str(&p["tim"]))
}

pub(crate) fn ext_raw(p: &Value) -> String {
    p["ext"].as_str().unwrap_or("").to_string()
}

pub(crate) fn ext_lower(p: &Value) -> String {
    ext_raw(p).to_lowercase()
}

/// `data.posts || []`
pub(crate) fn posts_of(data: &Value) -> Vec<Value> {
    data["posts"].as_array().cloned().unwrap_or_default()
}

/// `pages.flatMap(page => page.threads || [])` (non-array JSON throws in the old code).
pub(crate) fn catalog_threads(pages: &Value) -> anyhow::Result<Vec<Value>> {
    let pages = pages.as_array().ok_or_else(|| anyhow::anyhow!("pages is not iterable"))?;
    Ok(pages.iter().flat_map(|p| p["threads"].as_array().cloned().unwrap_or_default()).collect())
}

/// The `items.src` values a rehost of this post may have been stored under.
pub(crate) fn src_candidates(board: &str, posts: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for p in posts.iter().filter(|p| has_media(p)) {
        let tim = js_str(&p["tim"]);
        let (ext, lower) = (ext_raw(p), ext_lower(p));
        out.push(format!("https://i.4cdn.org/{board}/{tim}{ext}"));
        out.push(format!("https://i.4cdn.org/{board}/{tim}{lower}"));
        out.push(format!("http://i.4cdn.org/{board}/{tim}{ext}"));
        out.push(format!("http://i.4cdn.org/{board}/{tim}{lower}"));
        out.push(format!("/api/v2/scroller/external/4chan/{board}/media/{tim}{lower}"));
    }
    out
}

/// One rehosted item.
#[derive(Debug, Clone)]
pub(crate) struct RehostInfo {
    pub id: i64,
    pub stamp: Value,
    /// Link path: slug when slugs are enabled, else the id.
    pub path: Value,
}

/// `rehosts` / `rehostMap` of the old routes: keyed by `src` and by the 13–20 digit `tim` in it.
#[derive(Debug, Default)]
pub(crate) struct Rehosts {
    pub by_key: HashMap<String, RehostInfo>,
    /// Keys in insertion order (the JSON `rehosts` object keeps it).
    pub order: Vec<String>,
}

static TIM_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(\d{13,20})").unwrap());

impl Rehosts {
    fn insert(&mut self, key: String, info: RehostInfo) {
        if !self.by_key.contains_key(&key) {
            self.order.push(key.clone());
        }
        self.by_key.insert(key, info);
    }

    pub fn from_rows(rows: &[crate::db::Row], slugs: bool) -> Rehosts {
        let mut r = Rehosts::default();
        for row in rows {
            let Some(id) = row.i64("id") else { continue };
            let slug = row.str("slug").filter(|s| !s.is_empty());
            let path = match slug {
                Some(sl) if slugs => json!(sl),
                _ => json!(id),
            };
            let info = RehostInfo { id, stamp: row.val("stamp").clone(), path };
            let src = row.string("src");
            r.insert(src.clone(), info.clone());
            if let Some(m) = TIM_RE.captures(&src) {
                r.insert(m[1].to_string(), info);
            }
        }
        r
    }

    pub fn get(&self, k: &str) -> Option<&RehostInfo> {
        self.by_key.get(k)
    }

    /// `(p.tim && rehosts[p.tim]) || (url && rehosts[url])`
    pub fn for_post(&self, tim: Option<&str>, url: Option<&str>) -> Option<&RehostInfo> {
        tim.and_then(|t| self.get(t)).or_else(|| url.and_then(|u| self.get(u)))
    }

    /// The `rehosts` JSON object (`key → id`).
    pub fn ids_json(&self) -> Value {
        Value::Object(self.order.iter().map(|k| (k.clone(), json!(self.by_key[k].id))).collect())
    }

    /// The `rehost_paths` JSON object (`id → path`), integer keys ascending like a JS object.
    pub fn paths_json(&self) -> Value {
        let mut ids: Vec<&RehostInfo> = self.by_key.values().collect();
        ids.sort_by_key(|i| i.id);
        ids.dedup_by_key(|i| i.id);
        Value::Object(ids.into_iter().map(|i| (i.id.to_string(), i.path.clone())).collect())
    }

    /// Distinct rehosted item ids.
    pub fn ids(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.by_key.values().map(|i| i.id).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// `SELECT id, src, stamp, slug FROM items WHERE src = ANY(...)` for the posts' media.
pub(crate) async fn lookup_rehosts(st: &AppState, board: &str, posts: &[Value]) -> anyhow::Result<Rehosts> {
    let urls = src_candidates(board, posts);
    if urls.is_empty() {
        return Ok(Rehosts::default());
    }
    let rows = sql!("SELECT id, src, stamp, slug FROM items WHERE src = ANY(", urls, ")").fetch_all(&st.db).await?;
    Ok(Rehosts::from_rows(&rows, s::enable_item_slugs(&st.cfg())))
}

/// Item ids among `ids` the user has favorited.
async fn user_favorites(st: &AppState, req: &Req, ids: &[i64]) -> HashSet<i64> {
    let Some(uid) = req.session().map(|s| s.id).filter(|&id| id != 0) else { return HashSet::new() };
    if ids.is_empty() {
        return HashSet::new();
    }
    let ids32: Vec<i32> = ids.iter().map(|&i| i as i32).collect();
    match sql!("SELECT item_id FROM favorites WHERE user_id = ", uid as i32, " AND item_id = ANY(", ids32, ")").fetch_all(&st.db).await {
        Ok(rows) => rows.iter().filter_map(|r| r.i64("item_id")).collect(),
        Err(_) => HashSet::new(),
    }
}

// ── Text & HTML ─────────────────────────────────────────────────────────────

/// Decode the HTML entities 4chan uses in names, subjects and file names (the old templates
/// printed them raw; now they are decoded here and escaped again by the template).
pub(crate) fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..].find(';').map(|e| e + 1).filter(|&e| e <= 12);
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ => {
                    let n = if let Some(h) = name.strip_prefix("#x").or_else(|| name.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = name.strip_prefix('#') {
                        d.parse().ok()
                    } else {
                        None
                    };
                    n.and_then(char::from_u32).filter(|&c| c != '\0')
                }
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

static TAG_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<[^>]+>").unwrap());
static BR_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)<br\s*/?>").unwrap());

/// `s.replace(/<[^>]+>/g, '')`
pub(crate) fn strip_tags(s: &str) -> String {
    TAG_RE.replace_all(s, "").into_owned()
}

/// `s.replace(/<br\s*\/?>/gi, with)`
fn replace_br(s: &str, with: &str) -> String {
    BR_RE.replace_all(s, with).into_owned()
}

/// `str.slice(0, n)` (by characters).
pub(crate) fn slice_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn fre(p: &str) -> fancy_regex::Regex {
    fancy_regex::Regex::new(p).expect("valid regex")
}

static FC_LOCAL_LINK: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r##"(?i)<a\s+[^>]*href="#p(\d+)"[^>]*>.*?</a>"##));
static FC_CROSS_LINK: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r#"(?i)<a\s+[^>]*href="(/[^"]+)"[^>]*>(.*?)</a>"#));
static FC_ESC_QUOTE: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r"(?<!&gt;)&gt;&gt;(\d+)\b"));
static FC_PLAIN_QUOTE: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r"(?<![>\w])>>(\d+)\b"));
static FC_PLACEHOLDER: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r"___CHANREF_(\d+)___"));
static FC_GREENTEXT: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r"(^|<br\s*/?>)&gt;(?!&gt;)([^\n<]+)"));
static HREF_P_RE: once_cell::sync::Lazy<fancy_regex::Regex> = once_cell::sync::Lazy::new(|| fre(r##"href="#p(\d+)""##));

fn fre_replace(re: &fancy_regex::Regex, s: &str, rep: &str) -> String {
    re.replace_all(s, rep).into_owned()
}

/// `formatComment`: quote links and greentext, then the allow-list sanitizer.
pub(crate) fn format_comment(com: &str) -> String {
    if com.is_empty() {
        return String::new();
    }
    let mut f = fre_replace(&FC_LOCAL_LINK, com, "___CHANREF_${1}___");
    f = fre_replace(&FC_CROSS_LINK, &f, r#"<a href="${1}" class="chan-ref chan-cross-ref" target="_blank">${2}</a>"#);
    f = fre_replace(&FC_ESC_QUOTE, &f, "___CHANREF_${1}___");
    f = fre_replace(&FC_PLAIN_QUOTE, &f, "___CHANREF_${1}___");
    f = fre_replace(&FC_PLACEHOLDER, &f, r##"<a href="#p${1}" class="chan-ref quotelink" data-post="${1}">&gt;&gt;${1}</a>"##);
    f = fre_replace(&FC_GREENTEXT, &f, r#"${1}<span class="chan-quote">&gt;${2}</span>"#);
    sanitize_comment_html(&f)
}

/// `extractQuotes`: post numbers a comment quotes (insertion order, unique).
pub(crate) fn extract_quotes(com: &str) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for re in [&*HREF_P_RE, &*FC_ESC_QUOTE, &*FC_PLAIN_QUOTE] {
        for c in re.captures_iter(com).flatten() {
            if let Some(n) = c.get(1).and_then(|m| m.as_str().parse::<i64>().ok()) {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

const VOID_TAGS: [&str; 2] = ["br", "wbr"];
const ALLOWED_TAGS: [&str; 12] = ["a", "br", "wbr", "span", "s", "b", "i", "u", "em", "strong", "pre", "code"];

static ATTR_RE: once_cell::sync::Lazy<regex::Regex> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r#"([A-Za-z][A-Za-z0-9-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+))"#).unwrap());
static ENTITY_RE: once_cell::sync::Lazy<regex::Regex> =
    once_cell::sync::Lazy::new(|| regex::Regex::new(r"^&(?:#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[A-Za-z][A-Za-z0-9]{1,31});").unwrap());
static SAFE_HREF_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| {
    regex::Regex::new(r"^(?:#p[0-9]+|/[A-Za-z0-9_.#/-]*|https://boards\.4chan(?:nel)?\.org/[A-Za-z0-9_.#/-]*)$").unwrap()
});

/// Text outside tags: keep valid entities, escape everything else that is special in HTML.
fn escape_text(out: &mut String, text: &str) {
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or('\0');
        match c {
            '&' => match ENTITY_RE.find(rest) {
                Some(m) => {
                    out.push_str(m.as_str());
                    i += m.end();
                    continue;
                }
                None => out.push_str("&amp;"),
            },
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
        i += c.len_utf8();
    }
}

/// Rebuild one allowed opening tag with only safe attributes.
fn safe_open_tag(name: &str, attrs: &str) -> String {
    let mut out = format!("<{name}");
    let mut has_target = false;
    for c in ATTR_RE.captures_iter(attrs) {
        let key = c[1].to_ascii_lowercase();
        let raw = c.get(2).or_else(|| c.get(3)).or_else(|| c.get(4)).map(|m| m.as_str()).unwrap_or("");
        let val = decode_entities(raw);
        let keep = match (name, key.as_str()) {
            (_, "class") => {
                let v: String = val.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '_' | '-')).collect();
                Some(v)
            }
            ("a", "href") => (SAFE_HREF_RE.is_match(&val) && !val.starts_with("//")).then_some(val),
            ("a", "target") if val == "_blank" => {
                has_target = true;
                Some(val)
            }
            ("a", "data-post") => (!val.is_empty() && val.bytes().all(|b| b.is_ascii_digit())).then_some(val),
            _ => None,
        };
        if let Some(v) = keep {
            out.push_str(&format!(" {key}=\"{}\"", crate::util::escape_html(&v)));
        }
    }
    if has_target {
        out.push_str(" rel=\"noopener noreferrer\"");
    }
    out.push('>');
    out
}

/// Allow-list sanitizer for 4chan comment HTML: known formatting tags with checked attributes,
/// balanced nesting, everything else escaped or dropped. The old viewer printed `com` verbatim.
pub(crate) fn sanitize_comment_html(input: &str) -> String {
    static TOKEN_RE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<[^<>]*>").unwrap());
    static TAG_PARTS: once_cell::sync::Lazy<regex::Regex> =
        once_cell::sync::Lazy::new(|| regex::Regex::new(r"^<\s*(/?)\s*([A-Za-z][A-Za-z0-9]*)\b([^>]*)>$").unwrap());
    let mut out = String::with_capacity(input.len() + 16);
    let mut stack: Vec<String> = Vec::new();
    let mut last = 0;
    for m in TOKEN_RE.find_iter(input) {
        escape_text(&mut out, &input[last..m.start()]);
        last = m.end();
        let Some(c) = TAG_PARTS.captures(m.as_str()) else {
            // Not a tag (`1 < 2 > 0`): plain text
            escape_text(&mut out, m.as_str());
            continue;
        };
        let name = c[2].to_ascii_lowercase();
        if !ALLOWED_TAGS.contains(&name.as_str()) {
            continue;
        }
        let closing = !c[1].is_empty();
        if VOID_TAGS.contains(&name.as_str()) {
            if !closing {
                out.push_str(&format!("<{name}>"));
            }
            continue;
        }
        if closing {
            if let Some(pos) = stack.iter().rposition(|t| *t == name) {
                while stack.len() > pos {
                    let t = stack.pop().unwrap_or_default();
                    out.push_str(&format!("</{t}>"));
                }
            }
        } else {
            out.push_str(&safe_open_tag(&name, &c[3]));
            stack.push(name);
        }
    }
    escape_text(&mut out, &input[last..]);
    while let Some(t) = stack.pop() {
        out.push_str(&format!("</{t}>"));
    }
    out
}

// ── Formatting ──────────────────────────────────────────────────────────────

fn iso(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).unwrap_or_default().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn locale_string(secs: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(secs, 0).single().map(|d| crate::tpl::locale_string_str(&d, None)).unwrap_or_default()
}

fn time_ago(st: &AppState, lang: &str, secs: i64) -> String {
    let ago = (crate::util::now_ms() - secs * 1000).div_euclid(1000);
    crate::util::time_ago(&st.i18n.translator(lang), ago)
}

fn fsize(p: &Value) -> Value {
    if truthy(&p["fsize"]) { json!(crate::util::format_size(&p["fsize"])) } else { Value::Null }
}

fn media_path(board: &str, tim: &str, ext: &str) -> String {
    format!("/api/v2/scroller/external/4chan/{board}/media/{tim}{ext}")
}

/// `formatPosts`
fn format_posts(st: &AppState, posts: &[Value], board: &str, rehosts: &Rehosts, favs: &HashSet<i64>, target: Option<i64>, lang: &str) -> Vec<Value> {
    let nos: HashSet<i64> = posts.iter().filter_map(|p| num(p, "no")).collect();
    let mut reply_map: HashMap<i64, Vec<Value>> = HashMap::new();
    for p in posts {
        let Some(com) = p["com"].as_str().filter(|c| !c.is_empty()) else { continue };
        let no = num(p, "no");
        for q in extract_quotes(com) {
            if nos.contains(&q) && Some(q) != no {
                reply_map.entry(q).or_default().push(p["no"].clone());
            }
        }
    }
    posts
        .iter()
        .map(|p| {
            let ext = ext_lower(p);
            let tim = tim_key(p);
            let is_video = matches!(ext.as_str(), ".webm" | ".mp4");
            let is_image = matches!(ext.as_str(), ".jpg" | ".jpeg" | ".png" | ".gif" | ".webp");
            let external = tim.as_ref().map(|t| format!("https://i.4cdn.org/{board}/{t}{ext}"));
            let info = rehosts.for_post(tim.as_deref(), external.as_deref());
            let local_stamp = info.and_then(|i| crate::config::as_int(&i.stamp)).filter(|&s| s != 0);
            let time = local_stamp.unwrap_or_else(|| num(p, "time").unwrap_or(0));
            let date_str = match (local_stamp, p["now"].as_str().filter(|n| !n.is_empty())) {
                (None, Some(now)) => decode_entities(now),
                _ => locale_string(time),
            };
            let replies = num(p, "no").and_then(|n| reply_map.get(&n)).cloned().unwrap_or_default();
            let com = p["com"].as_str().unwrap_or("");
            let filename = p["filename"].as_str().filter(|f| !f.is_empty()).map(|f| format!("{}{ext}", decode_entities(f)));
            json!({
                "no": p["no"],
                "sub": decode_entities(p["sub"].as_str().unwrap_or("")),
                "com_raw": com,
                "com_formatted": format_comment(com),
                "name": p["name"].as_str().filter(|n| !n.is_empty()).map(decode_entities).unwrap_or_else(|| "Anonymous".into()),
                "trip": decode_entities(p["trip"].as_str().unwrap_or("")),
                "time": time,
                "timeago": time_ago(st, lang, time),
                "date_str": date_str,
                "iso_str": iso(time),
                "has_media": has_media(p),
                "tim": p["tim"],
                "ext": ext,
                "filename": filename,
                "w": p["w"],
                "h": p["h"],
                "fsize": fsize(p),
                "dest": tim.as_ref().map(|t| media_path(board, t, &ext)),
                "thumb": tim.as_ref().map(|t| media_path(board, t, "s.jpg")),
                "external_media_url": external,
                "is_video": is_video,
                "is_image": is_image,
                "local_id": info.map(|i| i.id),
                "local_path": info.map(|i| i.path.clone()),
                "rehosted": info.is_some(),
                "user_has_favorited": info.map(|i| favs.contains(&i.id)).unwrap_or(false),
                "is_onara_active": target.map(|t| num(p, "no") == Some(t)).unwrap_or(false),
                "replies_count": replies.len(),
                "replies": replies,
            })
        })
        .collect()
}

// ── Responses ───────────────────────────────────────────────────────────────

fn domain(st: &AppState) -> String {
    st.cfg().str_or("main.url.domain", "").to_string()
}

fn error_page(st: &AppState, req: &Req, code: u16, message: String) -> Response {
    let data = json!({ "message": message, "domain": domain(st), "tmp": null, "session": req.session_value() });
    html_code(crate::tpl::render(st, Some(req), "error", data), code)
}

fn wants_ajax(req: &Req) -> bool {
    req.is_xhr() || req.query("ajax") == Some("1") || req.accepts_json()
}

/// Board name from the route (`[a-z0-9]+`), lower-cased like the old code.
fn board_param(req: &Req) -> String {
    req.param("board").unwrap_or("").to_lowercase()
}

/// Short, client-safe reason for a failed upstream request (details go to the log).
pub(crate) fn short_reason(e: &anyhow::Error) -> String {
    let s = e.to_string();
    if s.contains("code Some(22)") {
        "4chan returned an HTTP error".into()
    } else if s.contains("Expected JSON") {
        "Expected JSON but got non-JSON response".into()
    } else if s.contains("Empty thread data") {
        "Empty thread data".into()
    } else {
        "4chan request failed".into()
    }
}

// ── Routes ──────────────────────────────────────────────────────────────────

async fn chan_root(_st: AppState, _req: Req) -> Res {
    Ok(redirect("/4/wsg/catalogue"))
}

async fn catalogue(st: AppState, req: Req) -> Res {
    let board = board_param(&req);
    let cfg = st.cfg();
    let pages = match ChanUrl::api_catalog(&board) {
        Some(u) => chan_http::fetch_json(&cfg, &u).await.and_then(|p| catalog_threads(&p)),
        None => Err(anyhow::anyhow!("invalid board")),
    };
    let raw_threads = match pages {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("[CHAN] Failed to fetch catalogue for /{board}/: {e:#}");
            return Ok(error_page(
                &st,
                &req,
                500,
                format!("Could not load catalogue for /{board}/. The board may not exist or 4chan is currently unreachable."),
            ));
        }
    };
    let rehosts = lookup_rehosts(&st, &board, &raw_threads).await.unwrap_or_default();
    let threads: Vec<Value> = raw_threads
        .iter()
        .map(|t| {
            let tim = tim_key(t);
            let com = t["com"].as_str().filter(|c| !c.is_empty()).map(|c| slice_chars(&decode_entities(&strip_tags(&replace_br(c, " "))), 180)).unwrap_or_default();
            let url = (tim.is_some() && truthy(&t["ext"])).then(|| format!("https://i.4cdn.org/{board}/{}{}", tim.as_deref().unwrap_or(""), ext_raw(t)));
            let info = rehosts.for_post(tim.as_deref(), url.as_deref());
            json!({
                "no": t["no"],
                "sub": decode_entities(t["sub"].as_str().unwrap_or("")),
                "com": com,
                "replies": if truthy(&t["replies"]) { t["replies"].clone() } else { json!(0) },
                "images": if truthy(&t["images"]) { t["images"].clone() } else { json!(0) },
                "tim": t["tim"],
                "ext": t["ext"],
                "sticky": truthy(&t["sticky"]),
                "closed": truthy(&t["closed"]),
                "time": t["time"],
                "thumb": tim.as_ref().map(|tm| media_path(&board, tm, "s.jpg")),
                "local_id": info.map(|i| i.id),
                "local_path": info.map(|i| i.path.clone()),
            })
        })
        .collect();
    let common: Vec<Value> = COMMON_BOARDS.iter().map(|(id, name, icon)| json!({ "id": id, "name": name, "icon": icon })).collect();
    let d = domain(&st);
    let data = json!({
        "board": board,
        "threads": threads,
        "common_boards": common,
        "session": req.session_value(),
        "domain": d,
        "tmp": null,
        "page_meta": {
            "title": format!("/{board}/ - Catalogue"),
            "description": format!("4chan /{board}/ thread catalogue"),
            "url": format!("https://{d}/4/{board}/catalogue"),
        }
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "chan/catalogue", data)))
}

/// Thread JSON from the API; must contain posts.
async fn load_thread(st: &AppState, board: &str, tid: Option<u64>) -> anyhow::Result<Vec<Value>> {
    let url = tid.and_then(|t| ChanUrl::api_thread(board, t)).ok_or_else(|| anyhow::anyhow!("invalid thread"))?;
    let data = chan_http::fetch_json(&st.cfg(), &url).await?;
    let posts = posts_of(&data);
    if posts.is_empty() {
        anyhow::bail!("Empty thread data");
    }
    Ok(posts)
}

/// Template copy of the OP with its subject decoded.
fn op_value(op: &Value) -> Value {
    let mut o = op.clone();
    if let Some(sub) = op["sub"].as_str() {
        o["sub"] = json!(decode_entities(sub));
    }
    o
}

/// `op.sub || op.com stripped (160) || fallback`
fn op_description(op: &Value, fallback: String) -> String {
    if let Some(sub) = op["sub"].as_str().filter(|s| !s.is_empty()) {
        return decode_entities(sub);
    }
    match op["com"].as_str().filter(|c| !c.is_empty()) {
        Some(c) => slice_chars(&decode_entities(&strip_tags(c)), 160),
        None => fallback,
    }
}

#[allow(clippy::too_many_arguments)]
fn thread_view_data(st: &AppState, req: &Req, board: &str, tid: i64, posts: &[Value], rehosts: &Rehosts, favs: &HashSet<i64>, target: Option<i64>, lang: &str) -> Value {
    let op = &posts[0];
    let formatted = format_posts(st, posts, board, rehosts, favs, target, lang);
    let first_media = formatted.iter().find(|p| p["has_media"] == json!(true)).map(|p| p["no"].clone()).unwrap_or(Value::Null);
    let media_count = posts.iter().filter(|p| has_media(p)).count();
    let d = domain(st);
    let sub = op["sub"].as_str().filter(|s| !s.is_empty()).map(decode_entities);
    json!({
        "board": board,
        "tid": tid,
        "op": op_value(op),
        "posts": formatted,
        "media_count": media_count,
        "first_media_no": first_media,
        "rehosts_count": rehosts.order.len(),
        "session": req.session_value(),
        "csrf_token": req.session().and_then(|s| s.csrf_token.clone()).unwrap_or_default(),
        "domain": d,
        "tmp": null,
        "page_meta": {
            "title": format!("/{board}/{tid} - {}", sub.unwrap_or_else(|| "Thread Overview".into())),
            "description": op_description(op, format!("4chan /{board}/ thread {tid}")),
            "url": format!("https://{d}/4/{board}/{tid}"),
        }
    })
}

async fn thread(st: AppState, req: Req) -> Res {
    let board = board_param(&req);
    let tid_raw = req.param("thread").unwrap_or("").to_string();
    let tid = tid_raw.parse::<u64>().ok();
    let posts = match load_thread(&st, &board, tid).await {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("[CHAN] Failed to load thread /{board}/{tid_raw}: {e:#}");
            return Ok(error_page(&st, &req, 404, format!("Could not load thread /{board}/{tid_raw}. It may be archived or 4chan is currently unreachable.")));
        }
    };
    let rehosts = lookup_rehosts(&st, &board, &posts).await.unwrap_or_else(|e| {
        tracing::error!("[CHAN] Rehost check error: {e}");
        Rehosts::default()
    });
    let favs = user_favorites(&st, &req, &rehosts.ids()).await;
    let lang = super::index::req_lang(&st, &req);
    let data = thread_view_data(&st, &req, &board, tid.unwrap_or(0) as i64, &posts, &rehosts, &favs, None, &lang);
    Ok(html(crate::tpl::render(&st, Some(&req), "chan/thread", data)))
}

fn mime_for_ext(ext: &str) -> &'static str {
    match ext {
        ".mp4" => "video/mp4",
        ".webm" => "video/webm",
        ".gif" => "image/gif",
        ".png" => "image/png",
        _ => "image/jpeg",
    }
}

async fn thread_item(st: AppState, req: Req) -> Res {
    let board = board_param(&req);
    let tid_raw = req.param("thread").unwrap_or("").to_string();
    let post_raw = req.param("post").unwrap_or("").to_string();
    match thread_item_inner(&st, &req, &board, &tid_raw, &post_raw).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[CHAN] Failed to load item /4/{board}/{tid_raw}/{post_raw}: {e:#}");
            if wants_ajax(&req) {
                return Ok(json_resp(
                    &json!({ "success": false, "message": format!("Could not load media post #{post_raw} in /{board}/{tid_raw}: {}", short_reason(&e)) }),
                    404,
                ));
            }
            Ok(error_page(&st, &req, 404, format!("Could not load media post #{post_raw} in /{board}/{tid_raw}.")))
        }
    }
}

async fn thread_item_inner(st: &AppState, req: &Req, board: &str, tid_raw: &str, post_raw: &str) -> anyhow::Result<Response> {
    let cfg = st.cfg();
    let tid_u = tid_raw.parse::<u64>().ok();
    let tid = tid_u.unwrap_or(0) as i64;
    let post_no = post_raw.parse::<i64>().unwrap_or(-1);
    let posts = load_thread(st, board, tid_u).await?;
    let op = &posts[0];
    let media_posts: Vec<Value> = posts.iter().filter(|p| has_media(p)).cloned().collect();
    if media_posts.is_empty() {
        return Ok(redirect(&format!("/4/{board}/{tid}")));
    }
    let target = media_posts.iter().find(|p| num(p, "no") == Some(post_no)).unwrap_or(&media_posts[0]).clone();
    let target_no = num(&target, "no");

    let rehosts = lookup_rehosts(st, board, &media_posts).await.unwrap_or_else(|e| {
        tracing::error!("[CHAN] Rehost check error: {e}");
        Rehosts::default()
    });

    let media_index = media_posts.iter().position(|p| num(p, "no") == target_no).unwrap_or(0);
    let prev_media = media_index.checked_sub(1).map(|i| &media_posts[i]);
    let next_media = media_posts.get(media_index + 1);

    let ext = ext_lower(&target);
    let is_video = matches!(ext.as_str(), ".webm" | ".mp4");
    let tim = tim_key(&target);
    let external = tim.as_ref().map(|t| format!("https://i.4cdn.org/{board}/{t}{ext}"));
    let local = rehosts.for_post(tim.as_deref(), external.as_deref()).cloned();
    let local_id = local.as_ref().map(|l| l.id);
    let session = req.session();

    let mut item_rating: Option<String> = None;
    let mut item_tags: Vec<Value> = vec![];
    let mut can_manage = false;
    let (mut local_dest, mut local_thumb, mut local_title, mut local_stamp) = (None::<String>, None::<String>, None::<String>, None::<i64>);
    let mut favorites: Vec<Value> = vec![];
    let mut user_has_favorited = false;
    if let Some(lid) = local_id {
        let r: anyhow::Result<()> = async {
            let row = sql!("SELECT id, username, dest, title, mime, width, height, stamp FROM items WHERE id = ", lid as i32, " LIMIT 1").fetch_optional(&st.db).await?;
            if let Some(li) = row {
                local_stamp = crate::config::as_int(li.val("stamp"));
                local_title = li.str("title").filter(|t| !t.is_empty()).map(str::to_string);
                if let Some(dest) = li.str("dest").filter(|d| !d.is_empty()) {
                    local_dest = Some(format!("/b/{dest}"));
                    local_thumb = Some(format!("/t/{}.webp", li.i64("id").unwrap_or(lid)));
                }
                if let Some(sess) = session {
                    let uname = li.string("username").to_lowercase();
                    let owner = s::session_owner_name(Some(sess)).map(|o| o.to_lowercase());
                    can_manage = (!uname.is_empty() && owner.as_deref() == Some(uname.as_str())) || sess.admin || sess.is_moderator;
                }
            }
            item_tags = crate::util::get_tags(st, lid, session, None).await?;
            item_rating = item_tags.iter().filter_map(|t| t["normalized"].as_str()).find(|n| matches!(*n, "sfw" | "nsfw" | "nsfl")).map(str::to_string);
            let rows = sql!(
                r#"select "favorites".user_id, "user".user, "user".login, "user_options".avatar, "user_options".avatar_file, "user_options".display_name, "user_options".username_color, "user_options".hide_fav_badge, "anon_identities".fingerprint as anon_fingerprint
            from "favorites"
            left join "user" on "user".id = "favorites".user_id
            left join "user_options" on "user_options".user_id = "favorites".user_id
            left join "anon_identities" on "anon_identities".user_id = "favorites".user_id
            where "favorites".item_id = "#,
                lid as i32
            )
            .fetch_all(&st.db)
            .await?;
            favorites = rows.into_iter().map(Value::Object).collect();
            user_has_favorited = crate::util::user_has_favorited(session, &favorites);
            Ok(())
        }
        .await;
        if let Err(e) = r {
            tracing::error!("[CHAN] Failed to fetch tags for rehosted item: {e}");
        }
    }

    let lang = super::index::req_lang(st, req);
    let target_time = num(&target, "time").unwrap_or(0);
    let effective_stamp = match (local_id, local_stamp) {
        (Some(_), Some(s)) if s != 0 => s,
        _ => target_time,
    };
    let tim_s = tim.clone().unwrap_or_else(|| js_str(&target["tim"]));
    let title = local_title
        .or_else(|| target["sub"].as_str().filter(|s| !s.is_empty()).map(decode_entities))
        .or_else(|| op["sub"].as_str().filter(|s| !s.is_empty()).map(decode_entities))
        .unwrap_or_else(|| format!("/{board}/{tid} #{}", js_str(&target["no"])));
    let comment = target["com"].as_str().filter(|c| !c.is_empty()).map(|c| decode_entities(&strip_tags(&replace_br(c, "\n")))).unwrap_or_default();
    let mime = if is_video { mime_for_ext(&ext) } else { mime_for_ext(if matches!(ext.as_str(), ".gif" | ".png") { &ext } else { ".jpg" }) };
    let item_path = local.as_ref().map(|l| l.path.clone());
    let item = json!({
        "id": target["no"],
        "slug": null,
        "title": title,
        "dest": local_dest.unwrap_or_else(|| media_path(board, &tim_s, &ext)),
        "thumbnail": local_thumb.unwrap_or_else(|| media_path(board, &tim_s, "s.jpg")),
        "mime": mime,
        "width": if truthy(&target["w"]) { target["w"].clone() } else { Value::Null },
        "height": if truthy(&target["h"]) { target["h"].clone() } else { Value::Null },
        "size": if truthy(&target["fsize"]) { json!(crate::util::format_size(&target["fsize"])) } else { json!("0 B") },
        "username": target["name"].as_str().filter(|n| !n.is_empty()).map(decode_entities).unwrap_or_else(|| "Anonymous".into()),
        "stamp": effective_stamp,
        "timestamp": { "timeago": time_ago(st, &lang, effective_stamp), "timefull": iso(effective_stamp) },
        "comment": comment,
        "is_chan": true,
        "external_board": board,
        "external_tid": tid,
        "external_id": target["no"],
        "external_thread_url": format!("https://boards.4chan.org/{board}/thread/{tid}#p{}", js_str(&target["no"])),
        "external_media_url": external,
        "original_filename": target["filename"].as_str().filter(|f| !f.is_empty()).map(|f| format!("{}{ext}", decode_entities(f))),
        "local_id": local_id,
        "local_path": item_path,
        "rehosted": local_id.is_some(),
        "is_sfw": item_rating.as_deref() == Some("sfw"),
        "is_nsfw": item_rating.as_deref() == Some("nsfw"),
        "is_nsfl": item_rating.as_deref() == Some("nsfl"),
        "is_untagged": item_rating.is_none(),
        "favorites": favorites,
        "user_has_favorited": user_has_favorited,
        "halls": [],
        "user_halls": [],
        "tags": item_tags,
    });

    // f0ckm convention: next post (right / D) is pagination.prev, previous (left / A) is .next
    let pagination = json!({
        "prev": next_media.map(|p| p["no"].clone()),
        "next": prev_media.map(|p| p["no"].clone()),
    });
    let link = json!({ "main": format!("/4/{board}/{tid}/"), "mainDisplay": format!("/4/{board}/{tid}/"), "suffix": "" });

    let favs = user_favorites(st, req, &rehosts.ids()).await;
    let media_list: Vec<Value> = media_posts
        .iter()
        .enumerate()
        .map(|(idx, p)| {
            let pext = ext_lower(p);
            let pvid = matches!(pext.as_str(), ".webm" | ".mp4");
            let ptim = js_str(&p["tim"]);
            let purl = format!("https://i.4cdn.org/{board}/{ptim}{pext}");
            let info = rehosts.for_post(tim_key(p).as_deref(), Some(&purl));
            json!({
                "no": p["no"],
                "tim": p["tim"],
                "ext": pext,
                "dest": media_path(board, &ptim, &pext),
                "thumb": media_path(board, &ptim, "s.jpg"),
                "is_video": pvid,
                "is_image": !pvid,
                "is_active": num(p, "no") == target_no,
                "index": idx + 1,
                "local_id": info.map(|i| i.id),
                "local_path": info.map(|i| i.path.clone()),
                "rehosted": info.is_some(),
                "user_has_favorited": info.map(|i| favs.contains(&i.id)).unwrap_or(false),
                "width": if truthy(&p["w"]) { p["w"].clone() } else { Value::Null },
                "height": if truthy(&p["h"]) { p["h"].clone() } else { Value::Null },
            })
        })
        .collect();
    let op_sub = op["sub"].as_str().filter(|s| !s.is_empty()).map(decode_entities);
    let chan_thread = json!({
        "board": board,
        "tid": tid,
        "subject": op_sub.clone().unwrap_or_else(|| format!("Thread #{tid}")),
        "active_post": target["no"],
        "active_index": media_index + 1,
        "media_count": media_posts.len(),
        "media_posts": media_list,
    });

    let mimes = cfg.mimes();
    let mime_ext = mimes.get(mime).and_then(Value::as_str);
    let can_extract = can_manage
        && local_id.is_some()
        && !mime.contains("flash")
        && !(mime.starts_with("application/") && mime_ext.map(|e| !e.is_empty() && e != "swf" && e != "pdf").unwrap_or(false));
    let (rating_class, rating_label) = match item_rating.as_deref() {
        Some("nsfl") => ("is-nsfl", "NSFL"),
        Some("nsfw") => ("is-nsfw", "NSFW"),
        Some("sfw") => ("is-sfw", "SFW"),
        _ => ("is-untagged", "?"),
    };
    let d = domain(st);
    let title_s = item["title"].as_str().unwrap_or("").to_string();
    let page_meta = json!({
        "title": format!("/{board}/{tid}/{} - {title_s}", js_str(&target["no"])),
        "description": if comment_or_empty(&item).is_empty() { format!("4chan media post #{} in /{board}/{tid}", js_str(&target["no"])) } else { comment_or_empty(&item) },
        "url": format!("https://{d}/4/{board}/{tid}/{}", js_str(&target["no"])),
    });
    let data = json!({
        "item": item,
        "pagination": pagination,
        "link": link,
        "chan_thread": chan_thread,
        "session": req.session_value(),
        "csrf_token": session.and_then(|s| s.csrf_token.clone()).unwrap_or_default(),
        "domain": d,
        "tmp": null,
        "hidePagination": true,
        "enable_item_title": true,
        "enable_item_slugs": false,
        "can_manage_item": can_manage,
        "can_rate_item": can_manage,
        "can_extract_meta": can_extract,
        "user_has_favorited": user_has_favorited,
        "isSubscribed": false,
        "item_username_lower": item["username"].as_str().unwrap_or("").to_lowercase(),
        "item_rating_class": rating_class,
        "item_rating_label": rating_label,
        "is_flash_item": false,
        "is_archive_item": false,
        "item_has_dimensions": truthy(&item["width"]) && truthy(&item["height"]),
        "is_mod_or_admin": session.map(|s| s.admin || s.is_moderator).unwrap_or(false),
        "halls_enabled": false,
        "halls_slugs": "",
        "user_halls_slugs": "",
        "show_repost_row": false,
        "page_meta": page_meta,
    });

    let partial = if session.and_then(|s| s.use_new_layout).unwrap_or(false) { "item-partial-modern" } else { "item-partial-legacy" };
    if wants_ajax(req) {
        let html_s = crate::tpl::render(st, Some(req), partial, data.clone());
        let body = json!({
            "success": true,
            "html": html_s,
            "item": data["item"],
            "pagination": data["pagination"],
            "chan_thread": data["chan_thread"],
        });
        return Ok(json_resp(&body, 200));
    }

    if s::is_onara_enabled_for(&cfg, &req.cookies, session, false) {
        let item_html = crate::tpl::render(st, Some(req), partial, data.clone());
        let mut tv = thread_view_data(st, req, board, tid, &posts, &rehosts, &favs, target_no, &lang);
        if let Some(m) = tv.as_object_mut() {
            m.insert("page_meta".into(), data["page_meta"].clone());
            m.insert("is_onara_item".into(), json!(true));
            m.insert("onara".into(), json!(true));
            m.insert("onara_item_html".into(), json!(item_html));
            m.insert("item".into(), data["item"].clone());
        }
        return Ok(html(crate::tpl::render(st, Some(req), "chan/thread", tv)));
    }
    Ok(html(crate::tpl::render(st, Some(req), "item", data)))
}

fn comment_or_empty(item: &Value) -> String {
    item["comment"].as_str().unwrap_or("").to_string()
}

async fn thread_media(st: AppState, req: Req) -> Res {
    let board = board_param(&req);
    let tid_raw = req.param("thread").unwrap_or("").to_string();
    let tid_u = tid_raw.parse::<u64>().ok();
    let result: anyhow::Result<Value> = async {
        let url = tid_u.and_then(|t| ChanUrl::api_thread(&board, t)).ok_or_else(|| anyhow::anyhow!("invalid thread"))?;
        let data = chan_http::fetch_json(&st.cfg(), &url).await?;
        let posts = posts_of(&data);
        let op = posts.first().cloned().unwrap_or(json!({}));
        let media_posts: Vec<Value> = posts.iter().filter(|p| has_media(p)).cloned().collect();
        let rehosts = lookup_rehosts(&st, &board, &media_posts).await.unwrap_or_default();
        let media: Vec<Value> = media_posts
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let ext = ext_lower(p);
                let is_video = matches!(ext.as_str(), ".webm" | ".mp4");
                let tim = js_str(&p["tim"]);
                let url = format!("https://i.4cdn.org/{board}/{tim}{ext}");
                let info = rehosts.for_post(tim_key(p).as_deref(), Some(&url));
                json!({
                    "no": p["no"],
                    "tim": p["tim"],
                    "ext": ext,
                    "filename": p["filename"].as_str().filter(|f| !f.is_empty()).map(|f| format!("{f}{ext}")),
                    "w": p["w"],
                    "h": p["h"],
                    "dest": media_path(&board, &tim, &ext),
                    "thumb": media_path(&board, &tim, "s.jpg"),
                    "is_video": is_video,
                    "is_image": !is_video,
                    "index": idx + 1,
                    "local_id": info.map(|i| i.id),
                    "local_path": info.map(|i| i.path.clone()),
                    "rehosted": info.is_some(),
                })
            })
            .collect();
        let subject = op["sub"].as_str().filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| format!("Thread #{}", tid_u.unwrap_or(0)));
        Ok(json!({
            "success": true,
            "board": board,
            "tid": tid_u.unwrap_or(0),
            "subject": subject,
            "total": media.len(),
            "items": media,
            "media": media,
        }))
    }
    .await;
    match result {
        Ok(v) => Ok(with_header(json_resp(&v, 200), "cache-control", "private, max-age=60")),
        Err(e) => {
            tracing::error!("[CHAN] Media API error for /{board}/{tid_raw}: {e:#}");
            Ok(json_resp(&json!({ "success": false, "msg": "Failed to fetch thread media" }), 500))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entities() {
        assert_eq!(decode_entities("Tom &amp; Jerry&#039;s &quot;x&quot; &lt;3 &#x41;"), "Tom & Jerry's \"x\" <3 A");
        assert_eq!(decode_entities("a & b &unknown; &#0; &"), "a & b &unknown; &#0; &");
        assert_eq!(decode_entities("plain"), "plain");
    }

    #[test]
    fn quotes() {
        let com = r##"<a href="#p123" class="quotelink">&gt;&gt;123</a><br>&gt;&gt;456 and >>789<br>&gt;&gt;&gt;/g/1"##;
        assert_eq!(extract_quotes(com), vec![123, 456, 789]);
        assert_eq!(extract_quotes(""), Vec::<i64>::new());
    }

    #[test]
    fn comment_formatting() {
        // 4chan API comment: quotelink, greentext span, plain text with entities
        let com = r##"<a href="#p111" class="quotelink">&gt;&gt;111</a><br><span class="quote">&gt;be me</span><br>Tom &amp; Jerry"##;
        let out = format_comment(com);
        assert_eq!(
            out,
            r##"<a href="#p111" class="chan-ref quotelink" data-post="111">&gt;&gt;111</a><br><span class="quote">&gt;be me</span><br>Tom &amp; Jerry"##
        );
        // greentext without span (start of comment and after <br>)
        assert_eq!(format_comment("&gt;implying<br>&gt;more"), r#"<span class="chan-quote">&gt;implying</span><br><span class="chan-quote">&gt;more</span>"#);
        // cross-board link
        let out = format_comment(r#"<a href="/g/thread/5#p6" class="quotelink">&gt;&gt;&gt;/g/6</a>"#);
        assert_eq!(out, r#"<a href="/g/thread/5#p6" class="chan-ref chan-cross-ref" target="_blank" rel="noopener noreferrer">&gt;&gt;&gt;/g/6</a>"#);
        // unlinked quote
        assert_eq!(format_comment("see &gt;&gt;42"), r##"see <a href="#p42" class="chan-ref quotelink" data-post="42">&gt;&gt;42</a>"##);
    }

    #[test]
    fn sanitizer_blocks_markup() {
        let evil = r#"<script>alert(1)</script><img src=x onerror=alert(1)><a href="javascript:alert(1)" onclick="x()">c</a><span class="quote" style="x" onmouseover="y">q</span><iframe src="//evil"></iframe>"#;
        let out = sanitize_comment_html(evil);
        assert!(!out.contains("<script"), "{out}");
        assert!(!out.contains("<img"), "{out}");
        assert!(!out.contains("<iframe"), "{out}");
        assert!(!out.contains("javascript:"), "{out}");
        assert!(!out.contains("onclick") && !out.contains("onmouseover") && !out.contains("style="), "{out}");
        assert!(out.contains(r#"<span class="quote">q</span>"#), "{out}");
        assert!(out.contains("<a>c</a>"), "{out}");
        // protocol-relative and absolute foreign links are dropped
        assert_eq!(sanitize_comment_html(r#"<a href="//evil.com/x">x</a>"#), "<a>x</a>");
        assert_eq!(sanitize_comment_html(r#"<a href="https://evil.com/">x</a>"#), "<a>x</a>");
        assert_eq!(sanitize_comment_html(r#"<a href="https://boards.4chan.org/g/thread/1">x</a>"#), r#"<a href="https://boards.4chan.org/g/thread/1">x</a>"#);
        // attribute breakouts are escaped
        assert_eq!(sanitize_comment_html(r#"<span class='a" onclick="x'>t</span>"#), r#"<span class="a onclickx">t</span>"#);
        // unbalanced tags are closed, stray closers dropped, text escaped
        assert_eq!(sanitize_comment_html("<b>bold<i>x</b> y</i> 1 < 2 & 3 > \"q\""), "<b>bold<i>x</i></b> y 1 &lt; 2 &amp; 3 &gt; &quot;q&quot;");
        assert_eq!(sanitize_comment_html("<s>spoiler"), "<s>spoiler</s>");
        assert_eq!(sanitize_comment_html("a<wbr>b<br/>c</br>"), "a<wbr>b<br>c");
        assert_eq!(sanitize_comment_html("&lt;script&gt; &#039; &amp;"), "&lt;script&gt; &#039; &amp;");
        assert_eq!(sanitize_comment_html("<pre class=\"prettyprint\">code</pre>"), "<pre class=\"prettyprint\">code</pre>");
    }

    fn thread_json() -> Value {
        // Shape of https://a.4cdn.org/<board>/thread/<no>.json
        serde_json::from_str(
            r##"{"posts":[
            {"no":1000,"now":"10/30/26(Fri)12:00:00","name":"Anonymous","sub":"YLYL &amp; stuff","com":"OP text","filename":"cat &amp; dog","ext":".WEBM","w":640,"h":360,"tn_w":250,"tn_h":140,"tim":1761825600123456,"time":1761825600,"md5":"x","fsize":2097152,"resto":0,"replies":2,"images":1},
            {"no":1001,"now":"10/30/26(Fri)12:01:00","name":"Anonymous","com":"<a href=\"#p1000\" class=\"quotelink\">&gt;&gt;1000</a><br>nice","time":1761825660,"resto":1000},
            {"no":1002,"now":"10/30/26(Fri)12:02:00","name":"Anonymous","trip":"!abc","com":"&gt;&gt;1000 &gt;&gt;1001","filename":"pic","ext":".jpg","w":10,"h":20,"tim":1761825720000001,"time":1761825720,"fsize":1000,"resto":1000}
            ]}"##,
        )
        .unwrap()
    }

    #[test]
    fn rehost_candidates_and_map() {
        let posts = posts_of(&thread_json());
        let c = src_candidates("wsg", &posts);
        assert_eq!(c.len(), 10);
        assert_eq!(c[0], "https://i.4cdn.org/wsg/1761825600123456.WEBM");
        assert_eq!(c[1], "https://i.4cdn.org/wsg/1761825600123456.webm");
        assert_eq!(c[4], "/api/v2/scroller/external/4chan/wsg/media/1761825600123456.webm");
        let rows: Vec<crate::db::Row> = vec![
            serde_json::from_value(json!({"id": 77, "src": "https://i.4cdn.org/wsg/1761825600123456.webm", "stamp": 1761900000, "slug": "abcdefghijk"})).unwrap(),
        ];
        let r = Rehosts::from_rows(&rows, true);
        assert_eq!(r.order, ["https://i.4cdn.org/wsg/1761825600123456.webm", "1761825600123456"]);
        assert_eq!(r.for_post(Some("1761825600123456"), None).unwrap().path, json!("abcdefghijk"));
        assert_eq!(r.ids_json(), json!({"https://i.4cdn.org/wsg/1761825600123456.webm": 77, "1761825600123456": 77}));
        assert_eq!(r.paths_json(), json!({"77": "abcdefghijk"}));
        let r = Rehosts::from_rows(&rows, false);
        assert_eq!(r.paths_json(), json!({"77": 77}));
        assert_eq!(r.ids(), vec![77]);
    }

    #[test]
    fn catalog_pages() {
        let pages: Value = serde_json::from_str(r#"[{"page":1,"threads":[{"no":1,"sub":"a"},{"no":2}]},{"page":2,"threads":[{"no":3}]},{"page":3}]"#).unwrap();
        let t = catalog_threads(&pages).unwrap();
        assert_eq!(t.iter().map(|t| num(t, "no").unwrap()).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(catalog_threads(&json!({"error": 1})).is_err());
    }

    #[test]
    fn misc_text() {
        assert_eq!(strip_tags(r#"<span class="quote">&gt;hi</span> there"#), "&gt;hi there");
        assert_eq!(replace_br("a<br>b<BR />c", " "), "a b c");
        assert_eq!(slice_chars("äöü", 2), "äö");
        assert_eq!(iso(1761825600), "2025-10-30T12:00:00.000Z");
        assert_eq!(tim_key(&json!({"tim": 1761825600123456_u64})).as_deref(), Some("1761825600123456"));
        assert_eq!(tim_key(&json!({"tim": 0})), None);
        assert!(has_media(&json!({"tim": 1, "ext": ".jpg"})));
        assert!(!has_media(&json!({"tim": 1})));
        assert_eq!(op_description(&json!({"com": "a<br>&amp;b"}), "x".into()), "a&b");
        assert_eq!(op_description(&json!({}), "x".into()), "x");
    }
}
