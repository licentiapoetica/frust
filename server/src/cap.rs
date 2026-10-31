//! Cap proof-of-work CAPTCHA (https://trycap.dev), server side. Port of @cap.js/server 4.0.5.
//!
//! The widget (public/s/cap/cap.min.js) POSTs /api/cap/challenge, solves `c` SHA-256 puzzles derived
//! from the challenge token, POSTs the nonces to /api/cap/redeem and puts the token it gets back in
//! the form as `cap-token`. A form handler checks that token once with [`verify`].
//!
//! Challenges and redeemed tokens live in memory (single server process): both are single-use and
//! expire (challenges after 10 minutes, tokens after 20).
//!
//! Config (`cap:`): `enabled` (falls back to the old `recaptcha.enabled`), `challenge_count` (50),
//! `difficulty` (4 hex digits = 16 bits per puzzle), `skip_onion` (false), and for uploads and
//! comments `uploads` / `comments` ("off", "anon", "all") with `pass_minutes` (10; 0 = every time)
//! and `pass_minutes_anon` (anonymous accounts; defaults to `pass_minutes`).

use crate::config::Config;
use crate::http::{Req, Res, Router, json_resp};
use crate::state::AppState;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicI64, Ordering};

const CHALLENGE_TTL_MS: i64 = 10 * 60 * 1000;
const TOKEN_TTL_MS: i64 = 20 * 60 * 1000;
/// Hex chars of salt per puzzle. The widget's wasm solver needs salt + nonce digits < 56 bytes.
const SALT_LEN: usize = 32;
/// Upper bound on open challenges/tokens, so a flood can't grow memory without limit.
const MAX_PENDING: usize = 50_000;
/// Challenges one IP may request per minute (the widget also solves speculatively in the background).
const CHALLENGES_PER_IP_MINUTE: u32 = 20;

struct Pending {
    expires: i64,
    count: usize,
    difficulty: usize,
}

static CHALLENGES: Lazy<DashMap<String, Pending>> = Lazy::new(DashMap::new);
/// `"<id>:<sha256(vertoken)>"` -> expiry (the plain token is only ever sent to the client).
static TOKENS: Lazy<DashMap<String, i64>> = Lazy::new(DashMap::new);
static IP_HITS: Lazy<DashMap<String, (i64, u32)>> = Lazy::new(DashMap::new);
static LAST_SWEEP: AtomicI64 = AtomicI64::new(0);

/// Cap is switched on (`cap.enabled`, or the old `recaptcha.enabled` when `cap` isn't configured).
pub fn enabled(cfg: &Config) -> bool {
    match cfg.get("cap.enabled") {
        Value::Null => cfg.truthy("recaptcha.enabled"),
        v => crate::config::truthy(v),
    }
}

/// Cap must be solved for this request. Not skipped for localhost: the Host header is
/// client-controlled. `.onion` requests are only skipped when `cap.skip_onion` is set.
pub fn required(cfg: &Config, req: &Req) -> bool {
    enabled(cfg) && !(req.is_onion() && cfg.truthy("cap.skip_onion"))
}

pub enum Captcha {
    Ok,
    Missing,
    Failed,
}

/// Check a `cap-token` from a form. Each token works once.
pub fn verify(token: Option<&str>) -> Captcha {
    let Some(token) = token.map(str::trim).filter(|t| !t.is_empty()) else { return Captcha::Missing };
    let mut parts = token.split(':');
    let (Some(id), Some(vertoken), None) = (parts.next(), parts.next(), parts.next()) else { return Captcha::Failed };
    if id.is_empty() || vertoken.is_empty() {
        return Captcha::Failed;
    }
    let key = format!("{id}:{}", crate::util::sha256(vertoken));
    match TOKENS.remove(&key) {
        Some((_, expires)) if expires > crate::util::now_ms() => Captcha::Ok,
        _ => Captcha::Failed,
    }
}

pub fn register(r: &mut Router) {
    r.post(r"^\/api\/cap\/challenge\/?$", challenge);
    r.post(r"^\/api\/cap\/redeem\/?$", redeem);
    r.get(r"^\/api\/cap\/pass\/?$", pass_status);
    r.post(r"^\/api\/cap\/pass\/?$", pass_grant);
}

// ── Captcha for uploads and comments ────────────────────────────────────────
//
// `cap.uploads` / `cap.comments`: "off" (default), "anon" (anonymous accounts only) or "all".
// Admins, moderators and API-key uploads never need it. Solving one captcha gives the account a
// pass for that action for `cap.pass_minutes` (default 10), so a multi-file upload or a few
// comments in a row don't each need a new one. `pass_minutes: 0` makes every upload and every
// comment need its own captcha: the pass is used up by the request that performs the action
// ([`take`]) and expires after [`SINGLE_PASS_MS`] when unused.

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Upload,
    Comment,
}

impl Scope {
    pub const ALL: [Scope; 2] = [Scope::Upload, Scope::Comment];

    pub fn name(self) -> &'static str {
        match self {
            Scope::Upload => "upload",
            Scope::Comment => "comment",
        }
    }

    fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|sc| sc.name() == s)
    }

    fn config_key(self) -> &'static str {
        match self {
            Scope::Upload => "cap.uploads",
            Scope::Comment => "cap.comments",
        }
    }
}

/// user id + scope -> pass expiry
static PASSES: Lazy<DashMap<(i64, Scope), i64>> = Lazy::new(DashMap::new);

/// This account has to solve a captcha for `scope` (ignoring any pass it already has).
pub fn needed(cfg: &Config, session: &crate::session::Session, scope: Scope) -> bool {
    if !enabled(cfg) || session.admin || session.is_moderator || session.api_key_auth {
        return false;
    }
    // Also accepts true/false (YAML 1.1 parsers read a bare `off` as false)
    match cfg.get(scope.config_key()) {
        Value::Bool(true) => true,
        Value::String(s) if s == "all" => true,
        Value::String(s) if s == "anon" => crate::settings::is_anon_session(Some(session)),
        _ => false,
    }
}

/// Scopes the current visitor needs a captcha for (for the page, to load the captcha popup).
pub fn scopes_needed(cfg: &Config, req: &Req) -> Vec<&'static str> {
    let Some(session) = req.session() else { return Vec::new() };
    if !required(cfg, req) {
        return Vec::new();
    }
    Scope::ALL.into_iter().filter(|sc| needed(cfg, session, *sc)).map(Scope::name).collect()
}

fn has_pass(session: &crate::session::Session, scope: Scope) -> bool {
    PASSES.get(&(session.id, scope)).map(|e| *e > crate::util::now_ms()).unwrap_or(false)
}

/// The request may go ahead: no captcha needed, or the account holds a valid pass. Doesn't use the
/// pass up, so it's for steps that come before the action itself (comment attachments).
pub fn allowed(cfg: &Config, req: &Req, session: &crate::session::Session, scope: Scope) -> bool {
    !required(cfg, req) || !needed(cfg, session, scope) || has_pass(session, scope)
}

/// Like [`allowed`], for the request that performs the action (an upload, posting a comment). With
/// `pass_minutes: 0` this uses up the pass, so the next action needs a new captcha.
pub fn take(cfg: &Config, req: &Req, session: &crate::session::Session, scope: Scope) -> bool {
    if !required(cfg, req) || !needed(cfg, session, scope) {
        return true;
    }
    let minutes = pass_minutes(cfg, session);
    if minutes > 0 {
        return has_pass(session, scope);
    }
    let now = crate::util::now_ms();
    // Atomic, so two parallel requests can't share one pass
    PASSES.remove_if(&(session.id, scope), |_, exp| *exp > now).is_some()
}

/// How long a pass from `pass_minutes: 0` stays usable when nothing uses it.
const SINGLE_PASS_MS: i64 = 5 * 60 * 1000;

/// Pass length for this account: `cap.pass_minutes_anon` for anonymous accounts (falls back to
/// `cap.pass_minutes`), `cap.pass_minutes` for everyone else. 0 = one captcha per upload / comment.
fn pass_minutes(cfg: &Config, session: &crate::session::Session) -> i64 {
    let all = cfg.int_or("cap.pass_minutes", 10);
    let minutes = if crate::settings::is_anon_session(Some(session)) { cfg.int_or("cap.pass_minutes_anon", all) } else { all };
    minutes.clamp(0, 24 * 60)
}

fn pass_ms(cfg: &Config, session: &crate::session::Session) -> i64 {
    match pass_minutes(cfg, session) {
        0 => SINGLE_PASS_MS,
        m => m * 60_000,
    }
}

/// GET /api/cap/pass?scope=upload → { required }: whether the client must show the captcha first.
async fn pass_status(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let Some(scope) = req.query("scope").and_then(Scope::parse) else { return reply(json!({ "success": false, "error": "Unknown scope" })) };
    let required = match req.session() {
        Some(s) => !allowed(&cfg, &req, s, scope),
        None => false,
    };
    reply(json!({ "success": true, "required": required }))
}

/// POST /api/cap/pass { "cap-token", scope } → grants the pass for this account.
async fn pass_grant(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let body = req.post_value().await;
    let Some(session) = req.session().cloned() else { return reply(json!({ "success": false, "error": "Not logged in" })) };
    let Some(scope) = body.get("scope").and_then(Value::as_str).and_then(Scope::parse) else {
        return reply(json!({ "success": false, "error": "Unknown scope" }));
    };
    match verify(body.get("cap-token").and_then(Value::as_str)) {
        Captcha::Ok => {}
        Captcha::Missing => return reply(json!({ "success": false, "error": "Please complete the captcha." })),
        Captcha::Failed => return reply(json!({ "success": false, "error": "Captcha expired or invalid. Please try again." })),
    }
    let now = crate::util::now_ms();
    sweep(now);
    let expires = now + pass_ms(&cfg, &session);
    PASSES.insert((session.id, scope), expires);
    reply(json!({ "success": true, "expires": expires }))
}

fn reply(v: Value) -> Res {
    let mut r = json_resp(&v, 200);
    r.headers_mut().insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    Ok(r)
}

fn settings(cfg: &Config) -> (usize, usize) {
    let count = cfg.int_or("cap.challenge_count", 50).clamp(1, 500) as usize;
    let difficulty = cfg.int_or("cap.difficulty", 4).clamp(1, 8) as usize;
    (count, difficulty)
}

/// Drop expired entries, at most once a minute.
fn sweep(now: i64) {
    let last = LAST_SWEEP.load(Ordering::Relaxed);
    if now - last < 60_000 || LAST_SWEEP.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    CHALLENGES.retain(|_, p| p.expires > now);
    TOKENS.retain(|_, exp| *exp > now);
    PASSES.retain(|_, exp| *exp > now);
    IP_HITS.retain(|_, (start, _)| now - *start < 60_000);
}

fn rate_limited(ip: &str, now: i64) -> bool {
    let mut e = IP_HITS.entry(ip.to_string()).or_insert((now, 0));
    if now - e.0 >= 60_000 {
        *e = (now, 0);
    }
    e.1 += 1;
    e.1 > CHALLENGES_PER_IP_MINUTE
}

async fn challenge(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    if !enabled(&cfg) {
        return reply(json!({ "error": "Captcha is disabled" }));
    }
    let now = crate::util::now_ms();
    sweep(now);
    if rate_limited(&req.real_ip(), now) {
        return reply(json!({ "error": "Too many requests, try again in a minute" }));
    }
    if CHALLENGES.len() >= MAX_PENDING {
        tracing::warn!("[CAP] Too many open challenges, refusing new ones");
        return reply(json!({ "error": "Server busy, try again later" }));
    }
    let (count, difficulty) = settings(&cfg);
    let token = crate::util::random_hex(25);
    let expires = now + CHALLENGE_TTL_MS;
    CHALLENGES.insert(token.clone(), Pending { expires, count, difficulty });
    reply(json!({ "challenge": { "c": count, "s": SALT_LEN, "d": difficulty }, "token": token, "expires": expires }))
}

async fn redeem(_st: AppState, mut req: Req) -> Res {
    let body = req.post_value().await;
    let fail = |msg: &str| reply(json!({ "success": false, "error": msg }));
    let Some(token) = body.get("token").and_then(Value::as_str) else { return fail("Invalid body") };
    let Some(solutions) = body.get("solutions").and_then(Value::as_array) else { return fail("Invalid body") };
    let Some(solutions) = solutions.iter().map(Value::as_u64).collect::<Option<Vec<u64>>>() else { return fail("Invalid body") };

    let now = crate::util::now_ms();
    // Single use: removed before checking, so a wrong answer burns the challenge too
    let Some((_, pending)) = CHALLENGES.remove(token) else { return fail("Challenge invalid or expired") };
    if pending.expires < now {
        return fail("Challenge invalid or expired");
    }
    if !check_solutions(token, pending.count, SALT_LEN, pending.difficulty, &solutions) {
        return fail("Invalid solution");
    }
    if TOKENS.len() >= MAX_PENDING {
        return fail("Server busy, try again later");
    }
    let id = crate::util::random_hex(8);
    let vertoken = crate::util::random_hex(15);
    let expires = now + TOKEN_TTL_MS;
    TOKENS.insert(format!("{id}:{}", crate::util::sha256(&vertoken)), expires);
    reply(json!({ "success": true, "token": format!("{id}:{vertoken}"), "expires": expires }))
}

/// Every puzzle `i` (1-based) has salt `prng(token + i, s)` and target `prng(token + i + "d", d)`;
/// nonce `n` solves it when hex(sha256(salt + n)) starts with the target.
fn check_solutions(token: &str, count: usize, salt_len: usize, difficulty: usize, solutions: &[u64]) -> bool {
    if solutions.len() < count {
        return false;
    }
    (1..=count).all(|i| {
        let salt = prng(&format!("{token}{i}"), salt_len);
        let target = prng(&format!("{token}{i}d"), difficulty);
        crate::util::sha256(&format!("{salt}{}", solutions[i - 1])).starts_with(&target)
    })
}

/// Cap's PRNG: FNV-1a seed, xorshift32 stream, rendered as hex.
fn prng(seed: &str, length: usize) -> String {
    let mut state = seed.encode_utf16().fold(0x811c9dc5u32, |h, c| (h ^ c as u32).wrapping_mul(16777619));
    let mut out = String::with_capacity(length + 8);
    while out.len() < length {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        out.push_str(&format!("{state:08x}"));
    }
    out.truncate(length);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors computed with the original JavaScript implementation
    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef01";

    #[test]
    fn prng_matches_js() {
        assert_eq!(prng(&format!("{TOKEN}1"), 32), "2b5c4ca2fbc3e5066b0fb0573da6c475");
        assert_eq!(prng(&format!("{TOKEN}1d"), 4), "b948");
        assert_eq!(prng(&format!("{TOKEN}2"), 32), "a252484e802441830a9094a918b2adcb");
        assert_eq!(prng(&format!("{TOKEN}2d"), 4), "105e");
        assert_eq!(prng(&format!("{TOKEN}3"), 32), "7d5187daf8f6c0ef3d8d23daa733cb01");
        assert_eq!(prng(&format!("{TOKEN}3d"), 4), "6399");
    }

    #[test]
    fn known_solutions_pass() {
        assert!(check_solutions(TOKEN, 3, 32, 4, &[20759, 12490, 71352]));
        assert!(!check_solutions(TOKEN, 3, 32, 4, &[20759, 12490, 71353]));
        assert!(!check_solutions(TOKEN, 3, 32, 4, &[20759, 12490]));
    }

    #[test]
    fn redeemed_token_works_once() {
        let key = format!("abcd:{}", crate::util::sha256("secret"));
        TOKENS.insert(key, crate::util::now_ms() + 60_000);
        assert!(matches!(verify(Some("abcd:secret")), Captcha::Ok));
        assert!(matches!(verify(Some("abcd:secret")), Captcha::Failed));
        assert!(matches!(verify(Some("")), Captcha::Missing));
        assert!(matches!(verify(Some("a:b:c")), Captcha::Failed));
    }
}
