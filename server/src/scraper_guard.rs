//! Scraper guard: a client that fetches far more content than a person would gets a proof-of-work
//! page that never finishes (views/scraper-trap.html) until its block runs out.
//!
//! Counted per client: the distinct items it opens (every route with an `itemid`, and
//! `/api/v2/item/<id>`) and the distinct original files it downloads (`/b/`), in a sliding window
//! (`scraper_guard.window_minutes`). Over the limit (`guest_limit`, `member_limit` for signed-in
//! users) the client is blocked for `block_hours`. Guests (anonymous-access users included) are
//! counted per IP (the /64 for IPv6, security::rate_key), signed-in users per account. Admins and
//! moderators are never counted or blocked. `.onion` requests are only counted per account: they
//! all come from the Tor service's address.
//!
//! State is in memory: a restart lifts every block.

use crate::db::RowExt;
use crate::http::{Req, json_raw, raw};
use crate::state::AppState;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

/// Leading zero hex digits the trap page's proof of work looks for: 16^20 hashes, never found.
pub const TRAP_DIFFICULTY: u32 = 20;

#[derive(Clone, Copy)]
struct Limits {
    window: u64,
    guest: u64,
    member: u64,
    block: u64,
}

fn limits(cfg: &crate::config::Config) -> Limits {
    Limits {
        window: cfg.int_or("scraper_guard.window_minutes", 60).max(1) as u64 * 60,
        guest: cfg.int_or("scraper_guard.guest_limit", 1500).max(1) as u64,
        member: cfg.int_or("scraper_guard.member_limit", 3000).max(1) as u64,
        block: cfg.int_or("scraper_guard.block_hours", 6).max(0) as u64 * 3600,
    }
}

/// Distinct things one client fetched: the current window's set and the previous window's count.
/// The estimate weighs the previous window by how much of it still lies in the sliding window.
struct Counter {
    bucket: u64,
    cur: HashSet<u64>,
    prev: u64,
}

#[derive(Default)]
struct Guard {
    counters: DashMap<String, Counter>,
    /// key → blocked until (unix seconds)
    blocked: DashMap<String, u64>,
    hits: AtomicU64,
}

static GUARD: Lazy<Guard> = Lazy::new(Guard::default);

impl Guard {
    fn is_blocked(&self, key: &str, now: u64) -> bool {
        self.blocked.get(key).is_some_and(|until| *until > now)
    }

    /// Counts `thing` for `key`; returns the estimated number of distinct things in the window
    /// when it is over `limit`.
    fn hit(&self, key: &str, thing: u64, now: u64, window: u64, limit: u64) -> Option<u64> {
        if self.hits.fetch_add(1, Ordering::Relaxed) % 4096 == 4095 {
            self.sweep(now, window);
        }
        let bucket = now - now % window;
        let mut c = self.counters.entry(key.to_string()).or_insert_with(|| Counter { bucket, cur: HashSet::new(), prev: 0 });
        if c.bucket != bucket {
            c.prev = if bucket.checked_sub(c.bucket) == Some(window) { c.cur.len() as u64 } else { 0 };
            c.cur = HashSet::new();
            c.bucket = bucket;
        }
        // Bounded: once over the limit the client is blocked anyway
        if (c.cur.len() as u64) <= limit {
            c.cur.insert(thing);
        }
        let left = (window - (now - bucket)) as f64 / window as f64;
        let estimate = (c.cur.len() as f64 + c.prev as f64 * left) as u64;
        (estimate > limit).then_some(estimate)
    }

    fn block(&self, key: &str, until: u64) {
        self.counters.remove(key);
        self.blocked.insert(key.to_string(), until);
    }

    /// Drops counters of clients idle for a whole window and blocks that ran out.
    fn sweep(&self, now: u64, window: u64) {
        let oldest = (now - now % window).saturating_sub(window);
        self.counters.retain(|_, c| c.bucket >= oldest);
        self.blocked.retain(|_, until| *until > now);
    }
}

#[derive(PartialEq, Debug)]
enum Who {
    Staff,
    Member(i64, String),
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
        return Who::Member(s.id, s.user.clone());
    }
    // Media files skip the full session (app::session_middleware): look the cookie up the cheap way
    if req.path.starts_with("/b/") {
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
            let name = u.string("user");
            if let Some(id) = u.i64("id").filter(|_| u.str("anon_fingerprint").is_none() && !name.is_empty()) {
                return Who::Member(id, name);
            }
        }
    }
    Who::Guest
}

/// What a request fetches, for counting: an item (any route with an item id) or an original file.
fn thing(req: &Req, params: &HashMap<String, String>) -> Option<u64> {
    if req.method != Method::GET && req.method != Method::HEAD {
        return None;
    }
    let key = if req.path.starts_with("/b/") {
        format!("b:{}", req.path)
    } else if let Some(id) = params.get("itemid").or_else(|| params.get("id").filter(|_| req.path.starts_with("/api/v2/item/"))) {
        format!("i:{id}")
    } else {
        return None;
    };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    Some(h.finish())
}

/// Paths that stay reachable while blocked: static files (the trap page's own CSS/JS) and what
/// signing in needs (maintenance::is_open_path).
fn is_open_path(path: &str) -> bool {
    path.starts_with("/s/") || crate::maintenance::is_open_path(path)
}

/// Runs for every routed request (app::dispatch) with the route's params: the trap for a blocked
/// client, `None` to go on.
pub async fn check(st: &AppState, req: &Req, params: &HashMap<String, String>) -> Option<Response> {
    let cfg = st.cfg();
    if !cfg.enabled("scraper_guard.enabled") || is_open_path(&req.path) {
        return None;
    }
    let thing = thing(req, params);
    let ip = req.real_ip();
    // Cheap way out for the common case: nothing to count and nobody blocked
    if thing.is_none() && GUARD.blocked.is_empty() {
        return None;
    }
    let who = who(st, req).await;
    if who == Who::Staff {
        return None;
    }
    let ip_key = (!req.is_onion() && ip != "unknown").then(|| format!("ip:{}", crate::security::rate_key(&ip)));
    let user_key = match &who {
        Who::Member(id, _) => Some(format!("u:{id}")),
        _ => None,
    };
    let now = crate::util::now_secs() as u64;
    if [&ip_key, &user_key].into_iter().flatten().any(|k| GUARD.is_blocked(k, now)) {
        return Some(trap(st, req));
    }
    let l = limits(&cfg);
    let (key, limit) = match (&user_key, &ip_key) {
        (Some(k), _) => (k, l.member),
        (None, Some(k)) => (k, l.guest),
        (None, None) => return None,
    };
    let estimate = GUARD.hit(key, thing?, now, l.window, limit)?;
    GUARD.block(key, now + l.block);
    let client = match &who {
        Who::Member(_, name) => format!("user={name}"),
        _ => format!("ip_hash={}", crate::security::hash_ip(&cfg, &ip)),
    };
    tracing::warn!(
        "[SCRAPER] Blocked {client} for {} h: ~{estimate} items/files in {} min (limit {limit}), ua={:?}",
        l.block / 3600,
        l.window / 60,
        req.user_agent().chars().take(200).collect::<String>()
    );
    Some(trap(st, req))
}

/// The answer for a blocked client: API and script calls get a 429, everything else (pages and
/// files) the never-ending check page, with `scraper_guard.status` (default 200: scrapers tend to
/// stop retrying on an error, and keep a page they got as the file they asked for).
fn trap(st: &AppState, req: &Req) -> Response {
    let mut r = if req.wants_json() || (req.method != Method::GET && req.method != Method::HEAD) {
        let mut r = json_raw(&json!({ "success": false, "msg": "Too many requests" }), 429);
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("3600"));
        r
    } else {
        let status = st.cfg().int_or("scraper_guard.status", 200);
        let status = if (200..600).contains(&status) { status as u16 } else { 200 };
        let challenge = crate::util::random_hex(16);
        let body = crate::tpl::render(st, Some(req), "scraper-trap", json!({ "trap": { "challenge": challenge, "difficulty": TRAP_DIFFICULTY } }));
        let mut r = raw(status, body);
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
        r
    };
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, no-store"));
    h.insert("x-robots-tag", HeaderValue::from_static("noindex"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_distinct_things_and_blocks_over_the_limit() {
        let g = Guard::default();
        let (now, window) = (7200, 3600);
        for t in 0..10 {
            assert_eq!(g.hit("k", t, now, window, 10), None);
            // the same thing again (a video's range requests) doesn't count twice
            assert_eq!(g.hit("k", t, now + 1, window, 10), None);
        }
        assert_eq!(g.hit("k", 10, now + 2, window, 10), Some(11));
        g.block("k", now + 60);
        assert!(g.is_blocked("k", now + 59));
        assert!(!g.is_blocked("k", now + 60));
        assert!(!g.is_blocked("other", now));
    }

    #[test]
    fn previous_window_fades_out() {
        let g = Guard::default();
        let window = 100;
        for t in 0..10 {
            g.hit("k", t, 100, window, 1000);
        }
        // half the next window gone: half of the previous window's 10 still counts (+1 new)
        assert_eq!(g.hit("k", 99, 250, window, 5), Some(6));
        // a window later than that: nothing left from before
        assert_eq!(g.hit("k", 98, 450, window, 5), None);
    }

    #[test]
    fn sweep_drops_idle_counters_and_old_blocks() {
        let g = Guard::default();
        g.hit("idle", 1, 100, 100, 10);
        g.hit("busy", 1, 390, 100, 10);
        g.block("gone", 300);
        g.block("still", 500);
        g.sweep(400, 100);
        assert!(!g.counters.contains_key("idle") && g.counters.contains_key("busy"));
        assert!(!g.blocked.contains_key("gone") && g.blocked.contains_key("still"));
    }
}
