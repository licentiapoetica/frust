//! Scraper guard: notices clients that fetch far more content than a person would, keeps a log of
//! what clients do, and (in block mode) gives such clients a proof-of-work page that never
//! finishes (views/scraper-trap.html) until their block runs out. Admin page: /admin/scrapers
//! (routes/admin/scrapers.rs).
//!
//! Counted per client: the distinct items it opens (every route with an `itemid`, and
//! `/api/v2/item/<id>`) and the distinct original files it downloads (`/b/`), in a sliding window.
//! Over the limit (guests / signed-in users have their own) the client is blocked for a while; in
//! log-only mode that is only recorded ("would have been blocked"). Clients:
//!   * signed-in users by account (`u:<id>`),
//!   * .onion visitors by Tor circuit (`tor:<id>`, crate::tor_listener),
//!   * other guests by IP, the /64 for IPv6, as a keyed hash (`ip:<16 hex>`): no raw IP is kept,
//!     not even in memory.
//! Admins and moderators are never counted.
//!
//! Every client's requests are also summed per hour (items, files, thumbnails, assets, pages, API
//! calls, referers, peak count) and written to `scraper_activity`, so the limits can be set from
//! what visitors really do. Blocks are in `scraper_blocks` and survive restarts.
//!
//! Settings: admin page (site setting `scraper_guard`); until saved there, config.yaml's
//! `scraper_guard` section.

use crate::db::RowExt;
use crate::http::{Req, json_raw, raw};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Site setting key of the admin's settings.
pub const KEY: &str = "scraper_guard";

/// Leading zero hex digits the trap page's proof of work looks for: 16^20 hashes, never found.
pub const TRAP_DIFFICULTY: u32 = 20;

/// Distinct items/files remembered per client and hour (beyond that the counts stop growing).
const MAX_SEEN: usize = 100_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Nothing is counted or logged
    Off,
    /// Counted and logged; over the limit is recorded, nobody is blocked
    #[default]
    Log,
    /// Over the limit: the trap page until the block runs out
    Block,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "off" => Some(Mode::Off),
            "log" => Some(Mode::Log),
            "block" => Some(Mode::Block),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Log => "log",
            Mode::Block => "block",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuardConfig {
    pub mode: Mode,
    pub window_minutes: u32,
    pub guest_limit: u32,
    pub member_limit: u32,
    pub block_hours: u32,
    /// HTTP status of the trap page
    pub status: u16,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig { mode: Mode::Log, window_minutes: 60, guest_limit: 1500, member_limit: 3000, block_hours: 6, status: 200 }
    }
}

impl GuardConfig {
    pub fn clean(mut self) -> Self {
        self.window_minutes = self.window_minutes.clamp(1, 1440);
        self.guest_limit = self.guest_limit.clamp(1, 1_000_000);
        self.member_limit = self.member_limit.clamp(1, 1_000_000);
        self.block_hours = self.block_hours.clamp(1, 8760);
        if !(200..600).contains(&self.status) {
            self.status = 200;
        }
        self
    }
}

/// config.yaml's `scraper_guard` section (defaults until the admin saves the settings).
pub fn from_cfg(cfg: &crate::config::Config) -> GuardConfig {
    let d = GuardConfig::default();
    let num = |k: &str, def: u32| cfg.int(&format!("scraper_guard.{k}")).map(|v| v.clamp(0, u32::MAX as i64) as u32).unwrap_or(def);
    let mode = match cfg.str("scraper_guard.mode").and_then(Mode::parse) {
        Some(m) => m,
        None if cfg.is_false("scraper_guard.enabled") => Mode::Off,
        None => d.mode,
    };
    GuardConfig {
        mode,
        window_minutes: num("window_minutes", d.window_minutes),
        guest_limit: num("guest_limit", d.guest_limit),
        member_limit: num("member_limit", d.member_limit),
        block_hours: num("block_hours", d.block_hours),
        status: num("status", d.status as u32).min(u16::MAX as u32) as u16,
    }
    .clean()
}

/// The settings in effect: the admin's, or config.yaml's.
pub fn settings(st: &AppState) -> GuardConfig {
    st.settings.scraper_guard().unwrap_or_else(|| from_cfg(&st.cfg()))
}

/// Parses the stored site setting.
pub fn parse(v: &str) -> Option<GuardConfig> {
    serde_json::from_str::<GuardConfig>(v).ok().map(GuardConfig::clean)
}

// ── In-memory state ─────────────────────────────────────────────────────────

/// What kind of request it was, for the activity log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    File,
    Item,
    Thumb,
    Asset,
    Page,
    Api,
}

/// Distinct things one client fetched for the sliding window: the current window's set and the
/// previous window's count, weighed by how much of it still lies in the window.
#[derive(Default)]
struct Counter {
    bucket: u64,
    cur: HashSet<u64>,
    prev: u64,
}

/// One client's requests in the current hour.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Activity {
    pub hour: u64,
    pub user_id: Option<i64>,
    pub onion: bool,
    pub requests: u32,
    pub items: u32,
    pub files: u32,
    pub thumbs: u32,
    pub assets: u32,
    pub pages: u32,
    pub api: u32,
    pub referer: u32,
    pub peak: u32,
    pub user_agent: String,
}

#[derive(Default)]
struct ClientState {
    counter: Counter,
    act: Activity,
    seen: HashSet<u64>,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct Block {
    until: u64,
    enforced: bool,
}

#[derive(Default)]
struct Guard {
    clients: DashMap<String, ClientState>,
    blocked: DashMap<String, Block>,
    /// Finished hours waiting to be written
    done: Mutex<Vec<(String, Activity)>>,
    hits: AtomicU64,
}

static GUARD: Lazy<Guard> = Lazy::new(Guard::default);

fn thing_hash(key: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish()
}

impl Guard {
    fn block_of(&self, key: &str, now: u64) -> Option<Block> {
        self.blocked.get(key).map(|b| *b).filter(|b| b.until > now)
    }

    /// Records one request; returns the client's estimated distinct things in the window when the
    /// request fetched one.
    #[allow(clippy::too_many_arguments)]
    fn record(&self, key: &str, kind: Kind, thing: Option<u64>, user_id: Option<i64>, onion: bool, referer: bool, ua: &str, now: u64, window: u64) -> Option<u64> {
        if self.hits.fetch_add(1, Ordering::Relaxed) % 4096 == 4095 {
            self.sweep(now, window);
        }
        let hour = now - now % 3600;
        let mut c = self.clients.entry(key.to_string()).or_default();
        let c = &mut *c;
        // A new hour: the finished one goes to the writer. Only forwards: a request that read the
        // clock just before the hour turned counts into the new one.
        if hour > c.act.hour {
            if c.dirty {
                self.done.lock().push((key.to_string(), c.act.clone()));
            }
            c.act = Activity { hour, ..Default::default() };
            c.seen = HashSet::new();
        }
        let a = &mut c.act;
        c.dirty = true;
        a.user_id = user_id;
        a.onion = onion;
        a.requests += 1;
        if referer {
            a.referer += 1;
        }
        if a.user_agent.is_empty() || kind == Kind::Page {
            a.user_agent = ua.chars().take(255).collect();
        }
        match kind {
            Kind::Thumb => a.thumbs += 1,
            Kind::Asset => a.assets += 1,
            Kind::Page => a.pages += 1,
            Kind::Api => a.api += 1,
            Kind::File | Kind::Item => {}
        }
        let thing = thing?;
        if c.seen.len() < MAX_SEEN && c.seen.insert(thing) {
            if kind == Kind::File { a.files += 1 } else { a.items += 1 }
        }
        let estimate = c.counter.hit(thing, now, window);
        a.peak = a.peak.max(estimate.min(u32::MAX as u64) as u32);
        Some(estimate)
    }

    fn block(&self, key: &str, until: u64, enforced: bool) {
        if enforced {
            if let Some(mut c) = self.clients.get_mut(key) {
                c.counter = Counter::default();
            }
        }
        self.blocked.insert(key.to_string(), Block { until, enforced });
    }

    /// Drops idle clients (nothing for two hours and a window, all written) and blocks that ran out.
    fn sweep(&self, now: u64, window: u64) {
        let idle = (now - now % 3600).saturating_sub(7200).min(now.saturating_sub(window * 2));
        self.clients.retain(|_, c| c.dirty || c.act.hour >= idle);
        self.blocked.retain(|_, b| b.until > now);
    }
}

impl Counter {
    /// Counts `thing`, returns the estimated distinct things in the sliding window.
    fn hit(&mut self, thing: u64, now: u64, window: u64) -> u64 {
        let bucket = now - now % window;
        if self.bucket != bucket {
            self.prev = if bucket.checked_sub(self.bucket) == Some(window) { self.cur.len() as u64 } else { 0 };
            self.cur = HashSet::new();
            self.bucket = bucket;
        }
        if self.cur.len() < MAX_SEEN {
            self.cur.insert(thing);
        }
        let left = (window - (now - bucket)) as f64 / window as f64;
        (self.cur.len() as f64 + self.prev as f64 * left) as u64
    }

    fn estimate(&self, now: u64, window: u64) -> u64 {
        let bucket = now - now % window;
        if self.bucket == bucket {
            let left = (window - (now - bucket)) as f64 / window as f64;
            (self.cur.len() as f64 + self.prev as f64 * left) as u64
        } else if bucket.checked_sub(self.bucket) == Some(window) {
            let left = (window - (now - bucket)) as f64 / window as f64;
            (self.cur.len() as f64 * left) as u64
        } else {
            0
        }
    }
}

// ── Who is asking ───────────────────────────────────────────────────────────

#[derive(PartialEq, Debug)]
enum Who {
    Staff,
    Member(i64),
    Guest,
}

async fn who(st: &AppState, req: &Req) -> Who {
    if let Some(s) = &req.session {
        if s.admin || s.is_moderator || s.is_impersonating || s.preview_role.is_some() {
            return Who::Staff;
        }
        if s.is_anon || s.user.is_empty() {
            return Who::Guest;
        }
        return Who::Member(s.id);
    }
    // Files and static assets skip the full session (app::session_middleware): the cheap lookup
    if let Some(u) = match req.cookie("session") {
        Some(c) => crate::app::media_user(st, c).await,
        None => None,
    } {
        if u.bool("banned") {
            return Who::Guest;
        }
        if u.bool("admin") || u.bool("is_moderator") {
            return Who::Staff;
        }
        if let Some(id) = u.i64("id").filter(|_| u.str("anon_fingerprint").is_none() && !u.string("user").is_empty()) {
            return Who::Member(id);
        }
    }
    Who::Guest
}

/// The key a client is counted under; `None` when it can't be told apart from others (.onion
/// without circuit ids, no address).
fn client_key(cfg: &crate::config::Config, req: &Req, who: &Who) -> Option<String> {
    match who {
        Who::Member(id) => Some(format!("u:{id}")),
        Who::Staff => None,
        Who::Guest => {
            if let Some(c) = req.tor_circuit {
                return Some(format!("tor:{c}"));
            }
            if req.is_onion() {
                return None;
            }
            let ip = req.real_ip();
            if ip == "unknown" || ip.is_empty() {
                return None;
            }
            let h = crate::security::hash_ip(cfg, &crate::security::rate_key(&ip));
            Some(format!("ip:{}", &h[..h.len().min(16)]))
        }
    }
}

/// What a request fetches: its kind, and the thing counted for items and original files.
fn kind(req: &Req, params: &HashMap<String, String>) -> (Kind, Option<u64>) {
    let p = req.path.as_str();
    let get = req.method == Method::GET || req.method == Method::HEAD;
    if p.starts_with("/b/") {
        return (Kind::File, get.then(|| thing_hash(&format!("b:{p}"))));
    }
    if p.starts_with("/t/") || p.starts_with("/ca/") {
        return (Kind::Thumb, None);
    }
    if p.starts_with("/s/") || p.starts_with("/a/") || p.starts_with("/c/") || p.starts_with("/memes/") {
        return (Kind::Asset, None);
    }
    let id = params.get("itemid").or_else(|| params.get("id").filter(|_| p.starts_with("/api/v2/item/")));
    if let (Some(id), true) = (id, get) {
        return (Kind::Item, Some(thing_hash(&format!("i:{id}"))));
    }
    if !get || p.starts_with("/api/") || p.starts_with("/ajax/") || req.is_xhr() {
        return (Kind::Api, None);
    }
    (Kind::Page, None)
}

/// Paths that stay reachable while blocked: static files (the trap page's own CSS/JS) and what
/// signing in needs (maintenance::is_open_path).
fn is_open_path(path: &str) -> bool {
    path.starts_with("/s/") || crate::maintenance::is_open_path(path)
}

// ── The check ───────────────────────────────────────────────────────────────

/// Runs for every routed request (app::dispatch) with the route's params: the trap for a blocked
/// client, `None` to go on.
pub async fn check(st: &AppState, req: &Req, params: &HashMap<String, String>) -> Option<Response> {
    let g = settings(st);
    if g.mode == Mode::Off {
        return None;
    }
    let who = who(st, req).await;
    let cfg = st.cfg();
    let key = client_key(&cfg, req, &who)?;
    let now = crate::util::now_secs() as u64;
    let open = is_open_path(&req.path);
    let enforcing = g.mode == Mode::Block;
    let exempt = is_own_fetcher(req);
    let blocked = !open && !exempt && enforcing && GUARD.block_of(&key, now).is_some_and(|b| b.enforced);
    let (kind, thing) = kind(req, params);
    let user_id = match who {
        Who::Member(id) => Some(id),
        _ => None,
    };
    let window = g.window_minutes as u64 * 60;
    let estimate = GUARD.record(
        &key,
        kind,
        thing.filter(|_| !open && !blocked),
        user_id,
        req.is_onion(),
        req.header("referer").is_some_and(|r| !r.is_empty()),
        req.user_agent(),
        now,
        window,
    );
    if blocked {
        return Some(trap(st, req, &g));
    }
    if exempt {
        return None;
    }
    let limit = if user_id.is_some() { g.member_limit } else { g.guest_limit } as u64;
    let estimate = estimate.filter(|e| *e > limit)?;
    // Over the limit: once per block (log mode records it once too)
    if GUARD.block_of(&key, now).is_some_and(|b| b.enforced || !enforcing) {
        return None;
    }
    let block_secs = g.block_hours as u64 * 3600;
    // log mode: remembered for the block time too, so it's recorded once, not on every request
    GUARD.block(&key, now + block_secs, enforcing);
    let ua: String = req.user_agent().chars().take(255).collect();
    tracing::warn!(
        "[SCRAPER] {} {key} for {} h: ~{estimate} items/files in {} min (limit {limit}), ua={ua:?}",
        if enforcing { "Blocked" } else { "Would block (log only)" },
        g.block_hours,
        g.window_minutes,
    );
    let (st2, key2, onion) = (st.clone(), key.clone(), req.is_onion());
    tokio::spawn(async move {
        let r = sql!(
            "INSERT INTO scraper_blocks (client, user_id, onion, user_agent, estimate, limit_at, enforced, expires_at) VALUES (",
            key2, ", ", user_id, ", ", onion, ", ", ua, ", ", estimate.min(i32::MAX as u64) as i32, ", ", limit as i32, ", ", enforcing,
            ", now() + make_interval(secs => ", block_secs as f64, ")) ON CONFLICT DO NOTHING"
        )
        .execute(&st2.db)
        .await;
        if let Err(e) = r {
            tracing::error!("[SCRAPER] storing block failed: {e}");
        }
    });
    enforcing.then(|| trap(st, req, &g))
}

/// Fetchers of the site's own setup, counted (they show on the admin page) but never blocked:
/// Anubis' OpenGraph fetcher reads a page's preview tags for everyone it challenges (link preview
/// bots, crawlers), all from Anubis' address, and would cache the check page as the preview. Only
/// believed straight from the private network without forwarding headers: through nginx every
/// request has X-Real-IP, so a client sending that user agent from outside is still counted.
fn is_own_fetcher(req: &Req) -> bool {
    req.user_agent().starts_with("Anubis-OGTag-Fetcher/")
        && req.tor_circuit.is_none()
        && req.remote.is_some_and(|r| crate::http::is_proxy_addr(r.ip()))
        && ["x-real-ip", "x-forwarded-for", "forwarded", "cf-connecting-ip", "true-client-ip"].iter().all(|h| req.header(h).is_none())
}

/// The answer for a blocked client: API and script calls get a 429, everything else (pages and
/// files) the never-ending check page, with the configured status (default 200: scrapers tend to
/// stop retrying on an error, and keep a page they got as the file they asked for).
fn trap(st: &AppState, req: &Req, g: &GuardConfig) -> Response {
    let mut r = if req.wants_json() || (req.method != Method::GET && req.method != Method::HEAD) {
        let mut r = json_raw(&json!({ "success": false, "msg": "Too many requests" }), 429);
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("3600"));
        r
    } else {
        let challenge = crate::util::random_hex(16);
        let body = crate::tpl::render(st, Some(req), "scraper-trap", json!({ "trap": { "challenge": challenge, "difficulty": TRAP_DIFFICULTY } }));
        let mut r = raw(g.status, body);
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        r
    };
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    r
}

// ── Admin ───────────────────────────────────────────────────────────────────

/// A client in memory right now (admin page, "Right now").
pub struct Live {
    pub key: String,
    pub estimate: u64,
    pub act: Activity,
    pub blocked: Option<(u64, bool)>,
}

/// The busiest clients of the current hour, by their sliding-window count.
pub fn live(g: &GuardConfig, n: usize) -> Vec<Live> {
    let now = crate::util::now_secs() as u64;
    let hour = now - now % 3600;
    let window = g.window_minutes as u64 * 60;
    let mut out: Vec<Live> = GUARD
        .clients
        .iter()
        .filter(|c| c.act.hour == hour)
        .map(|c| Live {
            key: c.key().clone(),
            estimate: c.counter.estimate(now, window),
            act: c.act.clone(),
            blocked: GUARD.block_of(c.key(), now).map(|b| (b.until, b.enforced)),
        })
        .collect();
    out.sort_by(|a, b| (b.estimate, b.act.items + b.act.files, b.act.requests).cmp(&(a.estimate, a.act.items + a.act.files, a.act.requests)));
    out.truncate(n);
    out
}

/// Clients counted in the current hour.
pub fn live_count() -> usize {
    let now = crate::util::now_secs() as u64;
    let hour = now - now % 3600;
    GUARD.clients.iter().filter(|c| c.act.hour == hour).count()
}

/// Lifts a client's block (admin): in memory and in the table. Returns whether one was active.
pub async fn lift(st: &AppState, key: &str, by: i64) -> anyhow::Result<bool> {
    let in_memory = GUARD.blocked.remove(key).is_some();
    if let Some(mut c) = GUARD.clients.get_mut(key) {
        c.counter = Counter::default();
    }
    let n = sql!("UPDATE scraper_blocks SET lifted_at = now(), lifted_by = ", by as i32, " WHERE client = ", key, " AND lifted_at IS NULL AND expires_at > now()")
        .execute(&st.db)
        .await?;
    Ok(in_memory || n > 0)
}

// ── Writer and startup ──────────────────────────────────────────────────────

/// Rows worth keeping: a client that fetched an item or file, or made a handful of requests.
fn worth_keeping(a: &Activity) -> bool {
    a.items + a.files > 0 || a.requests >= 5
}

/// Writes finished hours and the current hour's counts so far.
pub async fn flush(st: &AppState) {
    let mut rows = std::mem::take(&mut *GUARD.done.lock());
    for mut c in GUARD.clients.iter_mut() {
        if c.dirty {
            c.dirty = false;
            rows.push((c.key().clone(), c.act.clone()));
        }
    }
    rows.retain(|(_, a)| worth_keeping(a));
    // One row per client and hour (the latest counts): an upsert can't touch a row twice
    let mut seen = HashSet::new();
    rows.reverse();
    rows.retain(|(k, a)| seen.insert((k.clone(), a.hour)));
    for chunk in rows.chunks(200) {
        let mut q = sql!(
            "INSERT INTO scraper_activity (hour, client, user_id, onion, requests, items, files, thumbs, assets, pages, api, referer, peak, ua_class, user_agent) VALUES "
        );
        for (i, (key, a)) in chunk.iter().enumerate() {
            if i > 0 {
                q.push(", ");
            }
            q.push("(to_timestamp(").bind(a.hour as f64).push("), ").bind(key.clone()).push(", ").bind(a.user_id.map(|v| v as i32)).push(", ").bind(a.onion);
            for v in [a.requests, a.items, a.files, a.thumbs, a.assets, a.pages, a.api, a.referer, a.peak] {
                q.push(", ").bind(v.min(i32::MAX as u32) as i32);
            }
            q.push(", ").bind(ua_class(&a.user_agent)).push(", ").bind(a.user_agent.clone()).push(")");
        }
        q.push(
            " ON CONFLICT (hour, client) DO UPDATE SET user_id = excluded.user_id, onion = excluded.onion, requests = excluded.requests, \
             items = excluded.items, files = excluded.files, thumbs = excluded.thumbs, assets = excluded.assets, pages = excluded.pages, \
             api = excluded.api, referer = excluded.referer, peak = excluded.peak, ua_class = excluded.ua_class, user_agent = excluded.user_agent",
        );
        if let Err(e) = q.execute(&st.db).await {
            tracing::error!("[SCRAPER] writing activity failed: {e}");
        }
    }
}

/// Loads the active blocks and starts the writer (every minute).
pub fn start(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        match sql!("SELECT client, enforced, extract(epoch from expires_at)::bigint AS until FROM scraper_blocks WHERE lifted_at IS NULL AND expires_at > now()")
            .fetch_all(&st.db)
            .await
        {
            Ok(rows) => {
                for r in &rows {
                    if let Some(until) = r.i64("until") {
                        GUARD.blocked.insert(r.string("client"), Block { until: until.max(0) as u64, enforced: r.bool("enforced") });
                    }
                }
                if !rows.is_empty() {
                    tracing::info!("[SCRAPER] {} active block(s) loaded", rows.len());
                }
            }
            Err(e) => tracing::error!("[SCRAPER] loading blocks failed: {e}"),
        }
        let mut tick = tokio::time::interval(Duration::from_secs(60));
        tick.tick().await;
        loop {
            tick.tick().await;
            flush(&st).await;
        }
    });
}

/// Rough kind of client from its user agent.
pub fn ua_class(ua: &str) -> &'static str {
    let l = ua.to_ascii_lowercase();
    if ua.trim().is_empty() || ua == "-" {
        "empty"
    } else if ["headless", "phantom", "puppeteer", "playwright", "selenium"].iter().any(|w| l.contains(w)) {
        "headless"
    } else if [
        "python", "curl", "wget", "go-http", "java/", "okhttp", "axios", "node-fetch", "scrapy", "httpx", "aiohttp", "libwww", "perl", "ruby", "php", "reqwest",
        "gallery-dl", "yt-dlp", "jdownloader",
    ]
    .iter()
    .any(|w| l.contains(w))
    {
        "tool"
    } else if ["bot", "crawl", "spider", "slurp", "facebookexternalhit", "preview", "embed", "fetcher", "archiver"].iter().any(|w| l.contains(w)) {
        "bot"
    } else if l.contains("mozilla") || l.contains("opera") {
        "browser"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(peer: &str, headers: &[(&str, &str)]) -> Req {
        let mut h = axum::http::HeaderMap::new();
        for (k, v) in headers {
            h.insert(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        Req::new(Method::GET, &"/123".parse().unwrap(), h, axum::body::Body::empty(), Some(peer.parse().unwrap()))
    }

    #[test]
    fn anubis_og_fetcher_is_never_blocked_but_cant_be_faked() {
        let ua = ("user-agent", "Anubis-OGTag-Fetcher/1.0");
        assert!(is_own_fetcher(&req("172.18.0.4:4000", &[ua])));
        assert!(is_own_fetcher(&req("127.0.0.1:4000", &[ua])));
        // through nginx (a visitor sending that user agent): X-Real-IP is there
        assert!(!is_own_fetcher(&req("172.18.0.4:4000", &[ua, ("x-real-ip", "198.51.100.7")])));
        assert!(!is_own_fetcher(&req("172.18.0.4:4000", &[ua, ("x-forwarded-for", "198.51.100.7")])));
        // straight from the internet, or another user agent
        assert!(!is_own_fetcher(&req("203.0.113.9:4000", &[ua])));
        assert!(!is_own_fetcher(&req("172.18.0.4:4000", &[("user-agent", "curl/8.5.0")])));
        // the Tor port
        let mut r = req("172.18.0.4:4000", &[ua]);
        r.tor_circuit = Some(7);
        assert!(!is_own_fetcher(&r));
    }

    fn rec(g: &Guard, key: &str, thing: u64, now: u64, window: u64) -> Option<u64> {
        g.record(key, Kind::Item, Some(thing), None, false, false, "ua", now, window)
    }

    #[test]
    fn counts_distinct_things() {
        let g = Guard::default();
        let (now, window) = (7200, 3600);
        for t in 0..10 {
            assert_eq!(rec(&g, "k", t, now, window), Some(t + 1));
            // the same thing again (a video's range requests) doesn't count twice
            assert_eq!(rec(&g, "k", t, now + 1, window), Some(t + 1));
        }
        let c = g.clients.get("k").unwrap();
        assert_eq!((c.act.requests, c.act.items, c.act.peak), (20, 10, 10));
    }

    #[test]
    fn previous_window_fades_out() {
        let mut c = Counter::default();
        for t in 0..10 {
            c.hit(t, 100, 100);
        }
        // half the next window gone: half of the previous window's 10 still counts (+1 new)
        assert_eq!(c.hit(99, 250, 100), 6);
        assert_eq!(c.estimate(250, 100), 6);
        // a window later than that: nothing left from before
        assert_eq!(c.hit(98, 450, 100), 1);
    }

    #[test]
    fn hours_roll_over_into_the_writer() {
        let g = Guard::default();
        g.record("k", Kind::Page, None, Some(3), false, true, "Mozilla/5.0", 3600, 600);
        g.record("k", Kind::Thumb, None, Some(3), false, false, "", 3700, 600);
        g.record("k", Kind::File, Some(1), Some(3), false, false, "", 7300, 600);
        let done = g.done.lock();
        assert_eq!(done.len(), 1);
        let (key, a) = &done[0];
        assert_eq!((key.as_str(), a.hour, a.requests, a.pages, a.thumbs, a.referer, a.user_id), ("k", 3600, 2, 1, 1, 1, Some(3)));
        assert_eq!(a.user_agent, "Mozilla/5.0");
        let c = g.clients.get("k").unwrap();
        assert_eq!((c.act.hour, c.act.files, c.act.requests), (7200, 1, 1));
    }

    #[test]
    fn blocks_run_out_and_sweep() {
        let g = Guard::default();
        g.block("a", 20_000, true);
        g.block("b", 300, false);
        assert!(g.block_of("a", 19_999).is_some_and(|b| b.enforced));
        assert!(g.block_of("a", 20_000).is_none());
        assert!(g.block_of("b", 200).is_some_and(|b| !b.enforced));
        rec(&g, "idle", 1, 100, 600);
        g.clients.get_mut("idle").unwrap().dirty = false;
        g.sweep(4 * 3600, 600);
        assert!(!g.blocked.contains_key("b") && g.blocked.contains_key("a"));
        assert!(!g.clients.contains_key("idle"));
    }

    #[test]
    fn settings_from_config_and_cleaned() {
        let c = crate::media::test_cfg("scraper_guard: { mode: block, guest_limit: 20, window_minutes: 0, status: 999 }");
        let g = from_cfg(&c);
        assert_eq!((g.mode, g.guest_limit, g.member_limit, g.window_minutes, g.status), (Mode::Block, 20, 3000, 1, 200));
        assert_eq!(from_cfg(&crate::media::test_cfg("scraper_guard: { enabled: false }")).mode, Mode::Off);
        assert_eq!(from_cfg(&crate::media::test_cfg("x: 1")).mode, Mode::Log);
        assert_eq!(parse(r#"{"mode":"block","guest_limit":0}"#).map(|g| (g.mode, g.guest_limit)), Some((Mode::Block, 1)));
        assert!(parse("nope").is_none());
    }

    #[test]
    fn user_agent_classes() {
        assert_eq!(ua_class("Mozilla/5.0 (X11; Linux x86_64) Firefox/140.0"), "browser");
        assert_eq!(ua_class("Mozilla/5.0 HeadlessChrome/140"), "headless");
        assert_eq!(ua_class("python-requests/2.32"), "tool");
        assert_eq!(ua_class("Googlebot/2.1"), "bot");
        assert_eq!(ua_class(""), "empty");
    }
}
