//! A remote f0ck (f0ck-rs, API v3) as a data source: f0ckm shows another site's content, seen
//! through your own account there, to test how f0ckm looks and behaves with it.
//!
//! * `f0ckm remote-sync` (and `remote.interval` while the server runs) logs in to the remote with
//!   `remote.username` / `remote.password` and copies items, tags, ratings, uploaders and favs
//!   into the database. `remote.watch` (or `remote-sync --watch`) asks the remote every few
//!   seconds for items newer than the newest one seen and mirrors those right away ([`Watcher`]). Uploaders, taggers and fav users become local accounts that can't log in
//!   (password `!`); a remote login that exists here too maps to that local account.
//! * Media files, thumbnails and cover art are not copied up front. By default every request for
//!   one (static_files.rs) is streamed through from the remote, Range requests included, and
//!   nothing is written to disk. With `remote.store_media` the first request fetches the file and
//!   keeps it in b/, t/, ca/ like an upload.
//! * `remote_items` maps each mirrored item to its remote id. Item ids stay the remote's when they
//!   are free here (`remote.keep_ids`), so /123 is the same item on both sites.
//! * Their avatars come along as `avatar_file` `remote_<id>.webp`, served from the remote by /a/.
//! * Nothing that identifies chats or chat accounts is kept: channel and network become
//!   `redacted`, Telegram sources (`tg://…`) are dropped, and uploads whose uploader is only a
//!   numeric Telegram id go to one `redacted` account ([`redact_existing`] cleans older rows).
//! * The remote wins on every sync: an item's tags are replaced by the remote's.
//! * `f0ckm remote-sync --purge` deletes every mirrored item and its files again.

use crate::config::Config;
use crate::db::RowExt;
use crate::http::Req;
use crate::routes::uploads::{assign_tag, ensure_tag};
use crate::sql;
use crate::state::AppState;
use anyhow::{Context, Result, bail};
use axum::body::Body;
use axum::response::Response;
use reqwest::header;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Feed page size (the remote clamps `eps` to 10..=300).
const FEED_EPS: usize = 300;
/// Ids per `items/bulk` call (the remote takes at most 120).
const BULK_MAX: usize = 120;
/// A media file that failed to download isn't asked for again for this long.
const FAILED_TTL: Duration = Duration::from_secs(600);
/// Moves the item id sequence past every id in use (never back): explicit ids (`keep_ids`)
/// don't advance it, and a local upload or a fallback insert must not run into one.
const SEQ_PAST_MAX: &str = "SELECT setval('public.items_id_seq', GREATEST((SELECT MAX(id) FROM items), (SELECT last_value FROM public.items_id_seq), 1))";
/// Stands in for the remote's channel / network / chat-only uploaders.
const REDACTED: &str = "redacted";
/// New items announced live per sync (more is a backfill, not news).
const ANNOUNCE_MAX: usize = 30;
/// Feed page size of a watch check (`newer=`: oldest first).
const WATCH_EPS: usize = 100;
/// Fewest seconds between two watch checks.
pub const WATCH_MIN_SECS: u64 = 10;

/// `remote:` in config.yaml.
#[derive(Debug, Clone)]
pub struct RemoteCfg {
    /// Base URL without the trailing slash, e.g. `https://f0ck.example`.
    pub base: String,
    pub username: String,
    pub password: String,
    /// An access token from logging in elsewhere (the remote site in a browser): used instead of
    /// logging in, so the password never has to be on this server.
    pub token: Option<String>,
    /// Remote content filter: 0 sfw, 1 nsfw, 2 untagged, 3 all.
    pub mode: i64,
    pub keep_ids: bool,
    /// Minutes between syncs while the server runs (0: only `f0ckm remote-sync`).
    pub interval: u64,
    /// Seconds between checks for new remote items while the server runs (0: off; else at least
    /// [`WATCH_MIN_SECS`]).
    pub watch: u64,
    /// Largest media file fetched, in bytes.
    pub max_bytes: u64,
    /// Keep fetched media in b/, t/, ca/ (false: stream it through, nothing is written).
    pub store_media: bool,
    /// Only these kinds (`video`, `image`, `audio`), each synced on its own; empty: everything.
    pub types: Vec<String>,
    /// Blank every mirrored item's source URL (`redact_src: true`; `admins` only hides it, see
    /// [`src_visible`]).
    pub redact_src: bool,
}

/// The remote source, when `remote.enabled` is on and a URL and login are set.
pub fn settings(cfg: &Config) -> Option<RemoteCfg> {
    if !cfg.truthy("remote.enabled") {
        return None;
    }
    let base = cfg.str("remote.url")?.trim().trim_end_matches('/').to_string();
    if !(base.starts_with("https://") || base.starts_with("http://")) {
        return None;
    }
    let username = cfg.str("remote.username").unwrap_or("").trim().to_string();
    let password = std::env::var("F0CKM_REMOTE_PASSWORD").ok().filter(|p| !p.is_empty()).or_else(|| cfg.str("remote.password").map(str::to_string)).unwrap_or_default();
    let token = std::env::var("F0CKM_REMOTE_TOKEN")
        .ok()
        .or_else(|| cfg.str("remote.token").map(str::to_string))
        // Pasted from the browser: maybe quoted, maybe with "Bearer " in front
        .map(|t| t.trim().trim_matches('"').trim_start_matches("Bearer ").trim().to_string())
        .filter(|t| !t.is_empty());
    if token.is_none() && (username.is_empty() || password.is_empty()) {
        return None;
    }
    Some(RemoteCfg {
        base,
        username,
        password,
        token,
        mode: cfg.int("remote.mode").filter(|m| (0..=3).contains(m)).unwrap_or(3),
        keep_ids: cfg.enabled("remote.keep_ids"),
        interval: cfg.int("remote.interval").unwrap_or(0).max(0) as u64,
        watch: match cfg.int("remote.watch").unwrap_or(0).max(0) as u64 {
            0 => 0,
            s => s.max(WATCH_MIN_SECS),
        },
        store_media: cfg.truthy("remote.store_media"),
        types: media_types(cfg.get("remote.types")),
        redact_src: src_policy(cfg) == SrcPolicy::Drop,
        max_bytes: cfg.int("remote.max_media_mb").filter(|m| *m > 0).unwrap_or(1024) as u64 * 1024 * 1024,
    })
}

/// `remote.redact_src`: what happens to mirrored items' source URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcPolicy {
    /// Kept and shown like any other item's (`false`).
    Keep,
    /// Kept, but only admins see them (`admins`).
    Admins,
    /// Not kept (`true`).
    Drop,
}

/// Read straight from the config (not [`settings`]): hiding sources doesn't need the login.
pub fn src_policy(cfg: &Config) -> SrcPolicy {
    match cfg.get("remote.redact_src") {
        Value::String(s) if matches!(s.trim().to_lowercase().as_str(), "admins" | "admin") => SrcPolicy::Admins,
        v if crate::config::truthy(v) => SrcPolicy::Drop,
        _ => SrcPolicy::Keep,
    }
}

/// SQL for `items.src` as this viewer may see it: blank for mirrored items unless admin, with
/// `redact_src: admins` (search, feeds).
pub fn src_sql(cfg: &Config, admin: bool) -> &'static str {
    if !admin && src_policy(cfg) == SrcPolicy::Admins {
        "(CASE WHEN items.id IN (SELECT item_id FROM remote_items) THEN '' ELSE items.src END)"
    } else {
        "items.src"
    }
}

/// Whether this viewer may see item `id`'s source (the item page).
pub async fn src_visible(st: &AppState, id: i64, admin: bool) -> bool {
    if admin || src_policy(&st.cfg()) != SrcPolicy::Admins {
        return true;
    }
    !matches!(sql!("SELECT 1 AS x FROM remote_items WHERE item_id = ", id as i32).fetch_optional(&st.db).await, Ok(Some(_)))
}

/// The remote's media kinds (its feed filters by these): `video`, `image`, `audio`.
const MEDIA_TYPES: [&str; 3] = ["video", "image", "audio"];

/// `remote.types`: a list or a comma-separated string, plurals allowed; unknown names are left out.
fn media_types(v: &Value) -> Vec<String> {
    let raw: Vec<String> = match v {
        Value::Array(a) => a.iter().filter_map(|t| t.as_str().map(str::to_string)).collect(),
        Value::String(s) => s.split(',').map(str::to_string).collect(),
        _ => vec![],
    };
    let mut out: Vec<String> = Vec::new();
    for t in raw {
        let t = t.trim().to_lowercase();
        let t = t.strip_suffix('s').filter(|s| MEDIA_TYPES.contains(s)).map(str::to_string).unwrap_or(t);
        if MEDIA_TYPES.contains(&t.as_str()) && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

// ── HTTP ────────────────────────────────────────────────────────────────────

fn client() -> &'static reqwest::Client {
    static C: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .user_agent(concat!("f0ckm-remote-mirror/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .read_timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default()
    });
    &C
}

/// The access token of the current login: (base + username, token, expiry in unix seconds).
static TOKEN: tokio::sync::Mutex<Option<(String, String, i64)>> = tokio::sync::Mutex::const_new(None);

async fn login(rc: &RemoteCfg) -> Result<(String, i64)> {
    let resp = client()
        .post(format!("{}/api/v3/auth/login", rc.base))
        .timeout(Duration::from_secs(60))
        .json(&json!({ "username": rc.username, "password": rc.password, "kmsi": false }))
        .send()
        .await
        .with_context(|| format!("cannot reach {}", rc.base))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if status == reqwest::StatusCode::UNAUTHORIZED {
        bail!("login to {} as {} refused (wrong username or password)", rc.base, rc.username);
    }
    let token = body["access_token"].as_str().filter(|_| status.is_success()).ok_or_else(|| anyhow::anyhow!("login to {} failed: HTTP {status} {}", rc.base, api_msg(&body)))?;
    let exp = body["expires_at"].as_i64().unwrap_or_else(|| crate::util::now_secs() + 3600);
    Ok((token.to_string(), exp))
}

/// A valid access token, logging in again when the last one expired (or `fresh`).
async fn token(rc: &RemoteCfg, fresh: bool) -> Result<String> {
    if let Some(t) = &rc.token {
        if fresh {
            bail!("{} doesn't accept remote.token (expired or revoked?): log in there again and copy a new one", rc.base);
        }
        return Ok(t.clone());
    }
    let key = format!("{}\n{}", rc.base, rc.username);
    let mut slot = TOKEN.lock().await;
    if !fresh {
        if let Some((k, t, exp)) = slot.as_ref() {
            if *k == key && *exp > crate::util::now_secs() + 60 {
                return Ok(t.clone());
            }
        }
    }
    let (t, exp) = login(rc).await?;
    *slot = Some((key, t.clone(), exp));
    Ok(t)
}

fn api_msg(body: &Value) -> String {
    body["msg"].as_str().or_else(|| body["error"].as_str()).unwrap_or("").to_string()
}

/// One API call with the login token (logging in again once when the remote says 401).
async fn call(rc: &RemoteCfg, path: &str, body: Option<&Value>) -> Result<Value> {
    for attempt in 0..2 {
        let token = token(rc, attempt > 0).await?;
        let url = format!("{}{path}", rc.base);
        let req = match body {
            Some(b) => client().post(&url).json(b),
            None => client().get(&url),
        };
        let resp = req.bearer_auth(token).timeout(Duration::from_secs(120)).send().await.with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
            continue;
        }
        let v: Value = resp.json().await.with_context(|| format!("{url}: no JSON (HTTP {status})"))?;
        if !status.is_success() || v["success"] == Value::Bool(false) {
            bail!("{url}: HTTP {status} {}", api_msg(&v));
        }
        return Ok(v);
    }
    bail!("{}{path}: login not accepted", rc.base)
}

// ── Sync ────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
pub struct SyncOpts {
    /// Walk the whole remote feed and refresh items already mirrored.
    pub full: bool,
    /// Stop after this many new items.
    pub limit: Option<usize>,
    /// With `store_media`: fetch the thumbnails of new items right away (instead of on first view).
    pub thumbs: bool,
    /// Announce new items on the live socket (the running server's periodic sync).
    pub announce: bool,
    /// Progress lines on stdout (the CLI).
    pub verbose: bool,
}

#[derive(Debug, Default)]
pub struct SyncReport {
    pub added: usize,
    pub updated: usize,
    pub failed: usize,
    pub remote_total: i64,
}

/// Who is logged in on the remote and how many items that account sees there.
pub async fn whoami(rc: &RemoteCfg) -> Result<(String, i64)> {
    let me = call(rc, "/api/v3/auth/me", None).await?;
    let meta = call(rc, &format!("/api/v3/feed/meta?mode={}&eps={FEED_EPS}", rc.mode), None).await?;
    Ok((me["user"].as_str().or_else(|| me["login"].as_str()).unwrap_or("?").to_string(), meta["total_items"].as_i64().unwrap_or(0)))
}

/// Local accounts by remote login / display name, created on first use.
#[derive(Default)]
struct Users {
    /// The remote's base URL, recorded with the accounts made for it (`remote_users`).
    source: String,
    by_key: HashMap<String, (i64, String)>,
    /// Local accounts whose avatar was looked at in this sync.
    avatar_done: HashSet<i64>,
}

impl Users {
    fn new(rc: &RemoteCfg) -> Users {
        Users { source: rc.base.clone(), ..Default::default() }
    }

    /// The local account for a remote user: same login here, else a new one that can't log in.
    /// `login` is unknown for fav users (the remote only sends their display name).
    async fn get(&mut self, st: &AppState, login: Option<&str>, name: &str) -> Result<(i64, String)> {
        let name = name.trim();
        let login = login.map(str::trim).filter(|l| !l.is_empty()).unwrap_or(name).to_lowercase();
        let login = if login.is_empty() { "remote".to_string() } else { login };
        if let Some(u) = self.by_key.get(&login) {
            return Ok(u.clone());
        }
        let find = || sql!(r#"SELECT id, "user" FROM "user" WHERE LOWER(login) = "#, login.clone(), r#" OR LOWER("user") = "#, login.clone(), " ORDER BY (LOWER(login) = ", login.clone(), ") DESC, id LIMIT 1");
        let u = match find().fetch_optional(&st.db).await? {
            Some(r) => (r.i64("id").unwrap_or(0), r.string("user")),
            None => create_user(st, &self.source, &login, if name.is_empty() { &login } else { name }).await?,
        };
        self.by_key.insert(login, u.clone());
        Ok(u)
    }

    /// The remote avatar (an item id there) as `avatar_file` `remote_<id>.webp`, which /a/ streams
    /// from the remote's `/t/<id>.webp` (see [`serve_avatar`]). Only for accounts without an avatar
    /// of their own here: a local account the remote login maps to keeps its avatar.
    async fn set_avatar(&mut self, st: &AppState, uid: i64, remote_avatar: Option<i64>) -> Result<()> {
        if !self.avatar_done.insert(uid) {
            return Ok(());
        }
        let Some(a) = remote_avatar.filter(|a| *a > 0) else { return Ok(()) };
        sql!(
            "UPDATE user_options SET avatar_file = ", avatar_file_name(a), " WHERE user_id = ", uid as i32,
            r" AND avatar IS NULL AND (avatar_file IS NULL OR avatar_file IN ('', 'default.png') OR avatar_file LIKE 'remote\_%')"
        )
        .execute(&st.db)
        .await?;
        Ok(())
    }

    /// [`Users::set_avatar`] with the avatar from the user's remote profile (uploaders and taggers;
    /// fav users come with theirs). One request per user and sync.
    async fn avatar_from_profile(&mut self, st: &AppState, rc: &RemoteCfg, uid: i64, remote_name: &str) -> Result<()> {
        if self.avatar_done.contains(&uid) {
            return Ok(());
        }
        let avatar = match call(rc, &format!("/api/v3/users/{}/profile", crate::util::encode_uri_component(remote_name)), None).await {
            Ok(p) => p["avatar"].as_i64(),
            Err(e) => {
                tracing::debug!("[REMOTE] profile of {remote_name}: {e:#}");
                None
            }
        };
        self.set_avatar(st, uid, avatar).await
    }
}

fn avatar_file_name(remote_item: i64) -> String {
    format!("remote_{remote_item}.webp")
}

/// `items.username` holds 40 characters.
fn clip(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// A local account for a remote user, recorded in `remote_users` so purge and cleanup only ever
/// delete accounts the mirror made (password `!` alone also marks passkey-only accounts).
async fn create_user(st: &AppState, source: &str, login: &str, display: &str) -> Result<(i64, String)> {
    // "user" is unique and is what items.username refers to
    let base = clip(display, 36);
    let mut name = base.clone();
    let mut n = 1;
    while sql!(r#"SELECT 1 AS x FROM "user" WHERE LOWER("user") = "#, name.to_lowercase(), " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
        n += 1;
        name = format!("{base}_{n}");
    }
    let mut tx = st.db.begin().await?;
    let id = sql!(
        r#"INSERT INTO "user" (login, "user", password, admin, is_moderator, activated) VALUES ("#,
        clip(login, 255), ", ", name.clone(), ", '!', false, false, true) RETURNING id"
    )
    .fetch_scalar::<i32, _>(&mut *tx)
    .await?
    .ok_or_else(|| anyhow::anyhow!("user insert returned no id"))?;
    sql!(
        "INSERT INTO user_options (user_id, mode, theme, fullscreen, avatar, display_name) VALUES (",
        id, ", 0, 'amoled', 0, null, ", clip(display, 60), ") ON CONFLICT (user_id) DO NOTHING"
    )
    .execute(&mut *tx)
    .await?;
    sql!("INSERT INTO remote_users (user_id, source) VALUES (", id, ", ", clip(source, 255), ") ON CONFLICT DO NOTHING").execute(&mut *tx).await?;
    tx.commit().await?;
    Ok((id as i64, name))
}

/// State shared by the passes of one sync.
struct Run {
    users: Users,
    report: SyncReport,
    announced: usize,
}

/// Sync the remote into the database: one pass per `remote.types` entry (each with the remote's
/// feed for that kind), or one over everything. Without `full`, only items not mirrored yet are
/// fetched (plus the newest page again, for tags added since); a pass stops at the mirrored range
/// once an earlier run reached the remote's oldest item of its kind.
pub async fn sync(st: &AppState, rc: &RemoteCfg, opts: &SyncOpts) -> Result<SyncReport> {
    redact_existing(st, rc).await?;
    let mut run = Run { users: Users::new(rc), report: SyncReport::default(), announced: 0 };
    let lanes: Vec<Option<&str>> = if rc.types.is_empty() { vec![None] } else { rc.types.iter().map(|t| Some(t.as_str())).collect() };
    for lane in lanes {
        if sync_lane(st, rc, opts, lane, &mut run).await? {
            break; // --limit reached
        }
    }
    st.caches.feed_count.clear();
    Ok(run.report)
}

/// One pass over the remote feed, of one kind (`video`, ...) or of everything. True when
/// `opts.limit` was reached.
async fn sync_lane(st: &AppState, rc: &RemoteCfg, opts: &SyncOpts, lane: Option<&str>, run: &mut Run) -> Result<bool> {
    let mime_q = lane.map(|t| format!("&mime={t}")).unwrap_or_default();
    let meta = call(rc, &format!("/api/v3/feed/meta?mode={}&eps={FEED_EPS}{mime_q}", rc.mode), None).await?;
    let remote_oldest = meta["oldest_id"].as_i64();
    run.report.remote_total += meta["total_items"].as_i64().unwrap_or(0);
    if opts.verbose {
        println!("[REMOTE] {}: {} item(s) there", lane.unwrap_or("all types"), meta["total_items"].as_i64().unwrap_or(0));
    }

    let mut q = sql!("SELECT r.remote_id::bigint AS remote_id FROM remote_items r JOIN items i ON i.id = r.item_id WHERE r.source = ", rc.base.clone());
    if let Some(t) = lane {
        q.push(" AND split_part(i.mime, '/', 1) = ").bind(t.to_string());
    }
    let mut known: HashSet<i64> = q.fetch_scalar_list::<i64, _>(&st.db).await?.into_iter().collect();
    let newest_known = known.iter().max().copied();
    let oldest_known = known.iter().min().copied();
    let backfilled = matches!((oldest_known, remote_oldest), (Some(k), Some(o)) if k <= o);

    let mut older: Option<i64> = None;
    let mut first = true;
    loop {
        let mut path = format!("/api/v3/feed?mode={}&eps={FEED_EPS}{mime_q}", rc.mode);
        if let Some(o) = older {
            path.push_str(&format!("&older={o}"));
        }
        let page = call(rc, &path, None).await?;
        let ids: Vec<i64> = page["items"].as_array().map(|a| a.iter().filter_map(|i| i["id"].as_i64()).collect()).unwrap_or_default();
        let Some(&page_min) = ids.iter().min() else { break };
        let all_known = ids.iter().all(|i| known.contains(i));
        let todo: Vec<i64> = if opts.full || first { ids.clone() } else { ids.iter().copied().filter(|i| !known.contains(i)).collect() };

        for chunk in todo.chunks(BULK_MAX) {
            let bulk = call(rc, "/api/v3/items/bulk", Some(&json!({ "ids": chunk, "include_tags": true, "include_favorites": true, "mode": rc.mode }))).await?;
            for entry in bulk["items"].as_array().cloned().unwrap_or_default() {
                let rid = entry["item"]["id"].as_i64().unwrap_or(0);
                match mirror_item(st, rc, &entry, &mut run.users).await {
                    Ok((item_id, true)) => {
                        run.report.added += 1;
                        known.insert(rid);
                        if opts.verbose {
                            println!("  + #{rid} -> item {item_id} ({})", entry["item"]["mime"].as_str().unwrap_or("?"));
                        }
                        if opts.thumbs && rc.store_media {
                            fetch_thumb(st, &format!("{item_id}.webp")).await;
                        }
                        // Only items newer than everything mirrored before are news
                        if opts.announce && run.announced < ANNOUNCE_MAX && newest_known.is_some_and(|n| rid > n) {
                            run.announced += 1;
                            announce(st, item_id).await;
                        }
                        if opts.limit.is_some_and(|l| run.report.added >= l) {
                            return Ok(true);
                        }
                    }
                    Ok((_, false)) => run.report.updated += 1,
                    Err(e) => {
                        run.report.failed += 1;
                        eprintln!("[REMOTE] #{rid}: {e:#}");
                    }
                }
            }
        }
        if opts.verbose {
            println!("[REMOTE] page down to #{page_min}: {} added, {} updated so far", run.report.added, run.report.updated);
        }

        if page["atStart"] == Value::Bool(true) || remote_oldest.is_some_and(|o| page_min <= o) || page_min <= 1 {
            break;
        }
        older = Some(page_min - 1);
        if !opts.full && !first && all_known {
            if backfilled {
                break;
            }
            // Skip the range an earlier (interrupted) run already mirrored
            if let Some(k) = oldest_known.filter(|k| *k < page_min) {
                older = Some(k - 1);
            }
        }
        first = false;
    }
    Ok(false)
}

/// Mirror one `items/bulk` entry; (local item id, newly added).
async fn mirror_item(st: &AppState, rc: &RemoteCfg, entry: &Value, users: &mut Users) -> Result<(i64, bool)> {
    let it = &entry["item"];
    let rid = it["id"].as_i64().filter(|i| *i > 0).ok_or_else(|| anyhow::anyhow!("item without id"))?;
    let remote_dest = it["dest"].as_str().unwrap_or("").to_string();
    if !crate::media::is_plain_name(&remote_dest) {
        bail!("unusable file name {remote_dest:?}");
    }
    let mime = it["mime"].as_str().unwrap_or("application/octet-stream").to_string();
    let login = it["login"].as_str();
    let display = it["username"].as_str().or(login).unwrap_or("remote");
    let (uploader_id, uploader) = if is_chat_id_uploader(it["usernetwork"].as_str().unwrap_or(""), display) || login.is_some_and(|l| is_chat_id_uploader(it["usernetwork"].as_str().unwrap_or(""), l)) {
        users.get(st, Some(REDACTED), REDACTED).await?
    } else {
        let u = users.get(st, login, display).await?;
        users.avatar_from_profile(st, rc, u.0, login.unwrap_or(display)).await?;
        u
    };

    let existing = sql!("SELECT item_id FROM remote_items WHERE source = ", rc.base.clone(), " AND remote_id = ", rid as i32).fetch_scalar::<i32, _>(&st.db).await?;
    let (item_id, added) = match existing {
        Some(id) => (id as i64, false),
        None => (insert_item(st, rc, it, rid, &remote_dest, &mime, &uploader).await?, true),
    };
    if added {
        st.invalidate_upload_count_cache(&uploader);
    }

    // Tags (the remote's set replaces ours). The rating comes twice: as a pseudo tag (id -1 sfw,
    // -2 nsfw, no tagger) and, when someone rated it, as the real sfw/nsfw tag with its tagger.
    let mut wanted: Vec<(i64, i64)> = Vec::new();
    for t in entry["tags"].as_array().cloned().unwrap_or_default() {
        let name = t["tag"].as_str().unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let tag_id = match t["normalized"].as_str().unwrap_or(&name) {
            "sfw" => Some(1),
            "nsfw" => Some(2),
            _ if t["id"].as_i64().unwrap_or(0) < 0 => None,
            _ => ensure_tag(st, &clip(&name, 255)).await?,
        };
        let Some(tag_id) = tag_id else { continue };
        let tagger = t["user"].as_str().filter(|u| !u.is_empty());
        let by = match tagger {
            Some(u) => {
                let (uid, _) = users.get(st, None, u).await?;
                users.avatar_from_profile(st, rc, uid, u).await?;
                uid
            }
            None => uploader_id,
        };
        match wanted.iter_mut().find(|(id, _)| *id == tag_id) {
            Some(w) if tagger.is_some() => w.1 = by,
            Some(_) => {}
            None => wanted.push((tag_id, by)),
        }
    }
    let keep: Vec<i32> = wanted.iter().map(|(t, _)| *t as i32).collect();
    sql!("DELETE FROM tags_assign WHERE item_id = ", item_id as i32, " AND NOT (tag_id = ANY(", keep, "))").execute(&st.db).await?;
    for (tag_id, by) in wanted {
        assign_tag(st, item_id, tag_id, by).await?;
    }

    for f in entry["favorites"].as_array().cloned().unwrap_or_default() {
        if let Some(name) = f["user"].as_str().filter(|u| !u.is_empty()) {
            let (uid, _) = users.get(st, None, name).await?;
            users.set_avatar(st, uid, f["avatar"].as_i64()).await?;
            sql!("INSERT INTO favorites (user_id, item_id) VALUES (", uid as i32, ", ", item_id as i32, ") ON CONFLICT DO NOTHING").execute(&st.db).await?;
        }
    }
    sql!("UPDATE remote_items SET synced_at = ", crate::util::now_secs(), " WHERE item_id = ", item_id as i32).execute(&st.db).await?;
    Ok((item_id, added))
}

/// A source that only names a chat-app file (`tg://photo/<id>`) is dropped; URLs stay.
fn redact_src(src: &str) -> String {
    if src.to_lowercase().starts_with("tg://") { String::new() } else { src.to_string() }
}

/// An uploader that is only a numeric chat-app id (Telegram users without @name or linked account).
fn is_chat_id_uploader(network: &str, name: &str) -> bool {
    let digits = name.trim().trim_start_matches('-');
    network.eq_ignore_ascii_case("telegram") && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Redact mirrored rows from before redaction (or from another source): Telegram-id uploaders go
/// to the `redacted` account, channel / network / `tg://` sources are blanked, and the accounts
/// the mirror created for Telegram ids (password `!`, numeric login, no items left) are deleted.
async fn redact_existing(st: &AppState, rc: &RemoteCfg) -> Result<()> {
    let leftover = sql!(
        "SELECT COUNT(*)::bigint FROM items WHERE id IN (SELECT item_id FROM remote_items) AND usernetwork ILIKE 'telegram' AND username ~ '^-?[0-9]+$'"
    )
    .fetch_scalar::<i64, _>(&st.db)
    .await?
    .unwrap_or(0);
    if leftover > 0 {
        let (_, name) = Users::new(rc).get(st, Some(REDACTED), REDACTED).await?;
        sql!(
            "UPDATE items SET username = ", name,
            " WHERE id IN (SELECT item_id FROM remote_items) AND usernetwork ILIKE 'telegram' AND username ~ '^-?[0-9]+$'"
        )
        .execute(&st.db)
        .await?;
    }
    let mut n = sql!(
        "UPDATE items SET userchannel = ", REDACTED, ", usernetwork = ", REDACTED,
        ", src = CASE WHEN src ILIKE 'tg://%' THEN '' ELSE src END",
        " WHERE id IN (SELECT item_id FROM remote_items) AND (userchannel <> ", REDACTED, " OR usernetwork <> ", REDACTED, " OR src ILIKE 'tg://%')"
    )
    .execute(&st.db)
    .await?;
    if rc.redact_src {
        n += sql!("UPDATE items SET src = '' WHERE id IN (SELECT item_id FROM remote_items WHERE source = ", rc.base.clone(), ") AND src <> ''").execute(&st.db).await?;
    }
    let gone = sql!(
        r#"DELETE FROM "user" u WHERE u.id IN (SELECT user_id FROM remote_users) AND u.password = '!' AND u.login ~ '^-?[0-9]+$'
           AND NOT EXISTS (SELECT 1 FROM items i WHERE i.username = u."user")
           AND NOT EXISTS (SELECT 1 FROM comments c WHERE c.user_id = u.id)
           AND NOT EXISTS (SELECT 1 FROM tags_assign t WHERE t.user_id = u.id)"#
    )
    .execute(&st.db)
    .await?;
    if n + gone > 0 {
        tracing::info!("[REMOTE] {}: redacted {n} item(s), removed {gone} chat-id account(s)", rc.base);
    }
    Ok(())
}

/// The file name in b/: the remote's, unless an item or a file here already has it.
async fn local_dest(st: &AppState, remote_dest: &str, rid: i64) -> Result<String> {
    let taken = remote_dest.chars().count() > 60
        || st.cfg().paths.b.join(remote_dest).exists()
        || sql!("SELECT 1 AS x FROM items WHERE dest = ", remote_dest.to_string(), " LIMIT 1").fetch_optional(&st.db).await?.is_some();
    if !taken {
        return Ok(remote_dest.to_string());
    }
    let stem = crate::media::mime::file_stem(remote_dest);
    let ext: String = remote_dest[stem.len()..].trim_start_matches('.').chars().filter(|c| c.is_ascii_alphanumeric() || *c == '.').take(10).collect();
    Ok(if ext.is_empty() { format!("remote_{rid}") } else { format!("remote_{rid}.{ext}") })
}

async fn insert_item(st: &AppState, rc: &RemoteCfg, it: &Value, rid: i64, remote_dest: &str, mime: &str, uploader: &str) -> Result<i64> {
    let dest = local_dest(st, remote_dest, rid).await?;
    let host = rc.base.split("://").nth(1).unwrap_or(&rc.base).to_string();
    let checksum = clip(&format!("remote:{host}:{rid}"), 255);
    let src = if rc.redact_src { String::new() } else { clip(&redact_src(it["src"].as_str().unwrap_or("")), 255) };
    let size = it["size"].as_i64().unwrap_or(0).clamp(0, i32::MAX as i64) as i32;
    let stamp = it["stamp"].as_i64().unwrap_or_else(crate::util::now_secs) as i32;
    let (channel, network) = (REDACTED.to_string(), REDACTED.to_string());

    let mut tx = st.db.begin().await?;
    let columns = "src, dest, mime, size, checksum, username, userchannel, usernetwork, stamp, active, has_coverart, visibility, slug";
    let values = sql!(
        src, ", ", dest.clone(), ", ", clip(mime, 100), ", ", size, ", ", checksum, ", ", uploader.to_string(), ", ", channel, ", ", network, ", ",
        stamp, ", true, false, 0, ", crate::util::generate_slug(11)
    );
    let mut id = None;
    if rc.keep_ids {
        let mut q = sql!("INSERT INTO items (id, ");
        q.push(columns).push(") VALUES (").bind(rid as i32).push(", ").append(&values).push(") ON CONFLICT (id) DO NOTHING RETURNING id");
        id = q.fetch_scalar::<i32, _>(&mut *tx).await?;
    }
    // Explicit ids don't advance the sequence (setval isn't transactional: it takes effect now)
    crate::db::Sql::new(SEQ_PAST_MAX).execute(&mut *tx).await?;
    if id.is_none() {
        let mut q = sql!("INSERT INTO items (");
        q.push(columns).push(") VALUES (").append(&values).push(") RETURNING id");
        id = q.fetch_scalar::<i32, _>(&mut *tx).await?;
    }
    let id = id.ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
    sql!(
        "INSERT INTO remote_items (item_id, source, remote_id, remote_dest, synced_at) VALUES (",
        id, ", ", rc.base.clone(), ", ", rid as i32, ", ", clip(remote_dest, 255), ", ", crate::util::now_secs(), ")"
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(id as i64)
}

/// `new_item` on the live socket, like an upload.
async fn announce(st: &AppState, item_id: i64) {
    let row = sql!(
        r#"SELECT i.dest, i.mime, i.username, i.slug, o.display_name,
                  (SELECT ta.tag_id FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id IN (1, 2) LIMIT 1) AS tag_id
           FROM items i LEFT JOIN "user" u ON u."user" = i.username LEFT JOIN user_options o ON o.user_id = u.id
           WHERE i.id = "#,
        item_id as i32
    )
    .fetch_optional(&st.db)
    .await;
    let Ok(Some(r)) = row else { return };
    let payload = json!({
        "id": item_id,
        "dest": r.string("dest"),
        "mime": r.string("mime"),
        "username": r.string("username"),
        "display_name": r.str("display_name"),
        "tag_id": r.i64("tag_id").unwrap_or(0),
        "is_oc": false,
        "slug": r.str("slug"),
        "visibility": 0,
        "is_album": false,
        "album_count": 0,
    });
    let _ = sql!("SELECT pg_notify('new_item', ", payload.to_string(), ")").execute(&st.db).await;
}

/// The accounts a purge of `base` deletes (id, name): the ones the mirror made for it
/// (`remote_users`), and accounts from before that table that can only be the mirror's (password
/// `!` without passkey or anonymous identity, not the ghost), as long as nothing outside the
/// mirrored items belongs to them (items, comments, tags; favs and avatars go with the account).
pub async fn purge_accounts(st: &AppState, base: &str) -> Result<Vec<(i64, String)>> {
    let mirrored = || sql!("SELECT item_id FROM remote_items WHERE source = ", base.to_string());
    let mut q = sql!(r#"SELECT u.id, u."user" FROM "user" u WHERE (u.id IN (SELECT user_id FROM remote_users WHERE source = "#, base.to_string());
    q.push(
        r#") OR (u.id NOT IN (SELECT user_id FROM remote_users) AND u.password = '!' AND u.login <> 'deleted_user'
            AND NOT EXISTS (SELECT 1 FROM anon_identities a WHERE a.user_id = u.id)
            AND NOT EXISTS (SELECT 1 FROM webauthn_credentials w WHERE w.user_id = u.id)))
          AND NOT EXISTS (SELECT 1 FROM comments c WHERE c.user_id = u.id)
          AND NOT EXISTS (SELECT 1 FROM items i WHERE i.username = u."user" AND i.id NOT IN ("#,
    );
    q.append(&mirrored()).push(")) AND NOT EXISTS (SELECT 1 FROM tags_assign t WHERE t.user_id = u.id AND t.item_id NOT IN (");
    q.append(&mirrored()).push(r#")) ORDER BY u."user""#);
    Ok(q.fetch_all(&st.db).await?.iter().map(|r| (r.i64("id").unwrap_or(0), r.string("user"))).collect())
}

/// The number of items mirrored from `base`.
pub async fn mirrored_count(st: &AppState, base: &str) -> Result<i64> {
    Ok(sql!("SELECT COUNT(*)::bigint FROM remote_items WHERE source = ", base.to_string()).fetch_scalar::<i64, _>(&st.db).await?.unwrap_or(0))
}

/// Start over: delete every mirrored item of `base` (rows cascade) and its files, then the
/// accounts from [`purge_accounts`] with their avatar files. (items, accounts) deleted.
pub async fn purge(st: &AppState, base: &str) -> Result<(usize, usize)> {
    let accounts = purge_accounts(st, base).await?;
    let rows = sql!("SELECT i.id, i.dest FROM items i JOIN remote_items r ON r.item_id = i.id WHERE r.source = ", base.to_string()).fetch_all(&st.db).await?;
    delete_items(st, &rows).await?;
    delete_accounts(st, &accounts.iter().map(|(id, _)| *id as i32).collect::<Vec<_>>()).await?;
    st.caches.feed_count.clear();
    Ok((rows.len(), accounts.len()))
}

/// Mirrored items (rows of id, dest) and their stored files.
async fn delete_items(st: &AppState, rows: &[crate::db::Row]) -> Result<()> {
    let cfg = st.cfg();
    for r in rows {
        let id = r.i64("id").unwrap_or(0);
        let dest = r.string("dest");
        if crate::media::is_plain_name(&dest) {
            let _ = tokio::fs::remove_file(cfg.paths.b.join(&dest)).await;
        }
        for (dir, name) in [(&cfg.paths.t, format!("{id}.webp")), (&cfg.paths.t, format!("{id}_blur.webp")), (&cfg.paths.ca, format!("{id}.webp"))] {
            let _ = tokio::fs::remove_file(dir.join(name)).await;
        }
    }
    let ids: Vec<i32> = rows.iter().filter_map(|r| r.i64("id")).map(|i| i as i32).collect();
    sql!("DELETE FROM items WHERE id = ANY(", ids, ")").execute(&st.db).await?;
    Ok(())
}

/// The mirror's own items whose kind isn't in `remote.types` (any more): (id, dest) rows.
async fn off_type_items(st: &AppState, rc: &RemoteCfg) -> Result<Vec<crate::db::Row>> {
    if rc.types.is_empty() {
        return Ok(vec![]);
    }
    sql!(
        "SELECT i.id, i.dest FROM items i JOIN remote_items r ON r.item_id = i.id WHERE r.source = ", rc.base.clone(),
        " AND NOT (split_part(i.mime, '/', 1) = ANY(", rc.types.clone(), "))"
    )
    .fetch_all(&st.db)
    .await
}

/// How many mirrored items `prune` would delete.
pub async fn prune_count(st: &AppState, rc: &RemoteCfg) -> Result<usize> {
    Ok(off_type_items(st, rc).await?.len())
}

/// Delete the mirrored items of kinds no longer in `remote.types` (and their files), then the
/// accounts the mirror made that have nothing left. (items, accounts) deleted.
pub async fn prune(st: &AppState, rc: &RemoteCfg) -> Result<(usize, usize)> {
    let rows = off_type_items(st, rc).await?;
    if rows.is_empty() {
        return Ok((0, 0));
    }
    delete_items(st, &rows).await?;
    let orphans = sql!(
        r#"SELECT u.id FROM "user" u WHERE u.id IN (SELECT user_id FROM remote_users WHERE source = "#, rc.base.clone(),
        r#") AND NOT EXISTS (SELECT 1 FROM items i WHERE i.username = u."user")
            AND NOT EXISTS (SELECT 1 FROM favorites f WHERE f.user_id = u.id)
            AND NOT EXISTS (SELECT 1 FROM tags_assign t WHERE t.user_id = u.id)
            AND NOT EXISTS (SELECT 1 FROM comments c WHERE c.user_id = u.id)"#
    )
    .fetch_scalar_list::<i32, _>(&st.db)
    .await?;
    delete_accounts(st, &orphans).await?;
    st.caches.feed_count.clear();
    Ok((rows.len(), orphans.len()))
}

/// Delete accounts the mirror made, with their remote avatar files when nobody else uses them.
async fn delete_accounts(st: &AppState, ids: &[i32]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    let cfg = st.cfg();
    let ids = ids.to_vec();
    for f in sql!("SELECT avatar_file FROM user_options WHERE user_id = ANY(", ids.clone(), r") AND avatar_file LIKE 'remote\_%'").fetch_scalar_list::<String, _>(&st.db).await? {
        // Shared by every account with that remote avatar; kept while another one still uses it
        if crate::media::is_plain_name(&f) && sql!("SELECT 1 AS x FROM user_options WHERE avatar_file = ", f.clone(), " AND NOT (user_id = ANY(", ids.clone(), ")) LIMIT 1").fetch_optional(&st.db).await?.is_none() {
            let _ = tokio::fs::remove_file(cfg.paths.a.join(&f)).await;
        }
    }
    sql!(r#"DELETE FROM "user" WHERE id = ANY("#, ids, ")").execute(&st.db).await?;
    Ok(())
}

// ── Watch ───────────────────────────────────────────────────────────────────

/// Watches the remote for new items: each [`Watcher::check`] asks the remote's feed for items
/// newer than the newest one seen (`newer=`, oldest first) and mirrors them, announced live like
/// uploads. One small request when nothing is new. An item that fails to mirror is skipped (the
/// next sync picks it up).
#[derive(Default)]
pub struct Watcher {
    /// The remote this mark belongs to.
    source: String,
    /// The newest remote id seen; None: read again from `remote_items`.
    seen: Option<i64>,
}

impl Watcher {
    /// Start again from what is mirrored (after a sync, or when the remote changed).
    pub fn reset(&mut self) {
        self.seen = None;
    }

    /// One check: the number of items mirrored.
    pub async fn check(&mut self, st: &AppState, rc: &RemoteCfg, verbose: bool) -> Result<usize> {
        if self.source != rc.base {
            *self = Watcher { source: rc.base.clone(), seen: None };
        }
        let mut seen = match self.seen {
            Some(s) => s,
            None => {
                let max = sql!("SELECT MAX(remote_id)::bigint FROM remote_items WHERE source = ", rc.base.clone()).fetch_scalar::<i64, _>(&st.db).await?;
                match max {
                    Some(m) => m,
                    // Nothing mirrored yet (that's `remote-sync`'s job): only what comes from now on
                    None => {
                        let page = call(rc, &format!("/api/v3/feed?mode={}&eps=10", rc.mode), None).await?;
                        page["items"].as_array().and_then(|a| a.iter().filter_map(|i| i["id"].as_i64()).max()).unwrap_or(0)
                    }
                }
            }
        };
        self.seen = Some(seen);

        let mut users = Users::new(rc);
        let mut added = 0;
        loop {
            let page = call(rc, &format!("/api/v3/feed?mode={}&eps={WATCH_EPS}&newer={}", rc.mode, seen + 1), None).await?;
            let rows = page["items"].as_array().cloned().unwrap_or_default();
            let ids: Vec<i64> = rows
                .iter()
                .filter(|r| rc.types.is_empty() || r["mime"].as_str().is_some_and(|m| rc.types.iter().any(|t| m.split('/').next() == Some(t.as_str()))))
                .filter_map(|r| r["id"].as_i64())
                .filter(|i| *i > seen)
                .collect();
            for chunk in ids.chunks(BULK_MAX) {
                let bulk = call(rc, "/api/v3/items/bulk", Some(&json!({ "ids": chunk, "include_tags": true, "include_favorites": true, "mode": rc.mode }))).await?;
                for entry in bulk["items"].as_array().cloned().unwrap_or_default() {
                    let rid = entry["item"]["id"].as_i64().unwrap_or(0);
                    match mirror_item(st, rc, &entry, &mut users).await {
                        Ok((item_id, true)) => {
                            added += 1;
                            if verbose {
                                println!("  + #{rid} -> item {item_id} ({})", entry["item"]["mime"].as_str().unwrap_or("?"));
                            }
                            if added <= ANNOUNCE_MAX {
                                announce(st, item_id).await;
                            }
                        }
                        Ok((_, false)) => {}
                        Err(e) => eprintln!("[REMOTE] #{rid}: {e:#}"),
                    }
                }
            }
            let Some(max) = rows.iter().filter_map(|r| r["id"].as_i64()).max().filter(|m| *m > seen) else { break };
            seen = max;
            self.seen = Some(seen);
            if rows.len() < WATCH_EPS {
                break;
            }
        }
        if added > 0 {
            st.caches.feed_count.clear();
        }
        Ok(added)
    }
}

/// The remote work while the server runs: the periodic sync (`remote.interval` minutes) and the
/// watch for new items (`remote.watch` seconds), one after the other in one task. Settings are
/// re-read every round, so a config reload starts or stops either.
pub fn spawn_sync(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(60)).await;
        let mut last_sync: Option<Instant> = None;
        let mut last_watch: Option<Instant> = None;
        let mut watcher = Watcher::default();
        loop {
            let mut tick = 60;
            if let Some(rc) = settings(&st.cfg()) {
                if rc.interval > 0 && last_sync.is_none_or(|l| l.elapsed() >= Duration::from_secs(rc.interval * 60)) {
                    last_sync = Some(Instant::now());
                    match sync(&st, &rc, &SyncOpts { announce: true, ..Default::default() }).await {
                        Ok(r) if r.added + r.failed > 0 => tracing::info!("[REMOTE] {}: {} new, {} updated, {} failed", rc.base, r.added, r.updated, r.failed),
                        Ok(_) => {}
                        Err(e) => tracing::warn!("[REMOTE] sync with {} failed: {e:#}", rc.base),
                    }
                    watcher.reset();
                } else if rc.watch > 0 && last_watch.is_none_or(|l| l.elapsed() >= Duration::from_secs(rc.watch)) {
                    last_watch = Some(Instant::now());
                    match watcher.check(&st, &rc, false).await {
                        Ok(0) => {}
                        Ok(n) => tracing::info!("[REMOTE] {}: {n} new", rc.base),
                        Err(e) => tracing::warn!("[REMOTE] watching {} failed: {e:#}", rc.base),
                    }
                }
                if rc.watch > 0 {
                    tick = rc.watch.min(60);
                }
            }
            tokio::time::sleep(Duration::from_secs(tick)).await;
        }
    });
}

// ── Media on demand ─────────────────────────────────────────────────────────

/// Downloads in progress (one per target file) and recent failures.
static INFLIGHT: LazyLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);
static FAILED: LazyLock<Mutex<HashMap<PathBuf, Instant>>> = LazyLock::new(Default::default);

fn failed_recently(p: &Path) -> bool {
    let mut f = FAILED.lock().unwrap_or_else(|e| e.into_inner());
    f.retain(|_, t| t.elapsed() < FAILED_TTL);
    f.contains_key(p)
}

/// Download `url` to `target` (via tmp/), once even when several requests ask at the same time.
async fn download(st: &AppState, url: &str, target: &Path, max_bytes: u64, image_only: bool) -> bool {
    if failed_recently(target) {
        return false;
    }
    let lock = INFLIGHT.lock().unwrap_or_else(|e| e.into_inner()).entry(target.to_path_buf()).or_default().clone();
    let _guard = lock.lock().await;
    let ok = target.is_file() || match fetch_to(st, url, target, max_bytes, image_only).await {
        Ok(()) => true,
        Err(e) => {
            tracing::debug!("[REMOTE] {url}: {e:#}");
            FAILED.lock().unwrap_or_else(|e| e.into_inner()).insert(target.to_path_buf(), Instant::now());
            false
        }
    };
    INFLIGHT.lock().unwrap_or_else(|e| e.into_inner()).remove(target);
    ok
}

async fn fetch_to(st: &AppState, url: &str, target: &Path, max_bytes: u64, image_only: bool) -> Result<()> {
    let mut resp = client().get(url).send().await?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }
    let ct = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if ct.starts_with("text/html") || (image_only && !ct.starts_with("image/")) {
        bail!("unexpected content type {ct:?}");
    }
    if resp.content_length().is_some_and(|l| l > max_bytes) {
        bail!("larger than remote.max_media_mb");
    }
    let tmp = crate::media::TempPath::new(st.cfg().paths.tmp.join(format!("{}.remote", crate::media::queue::genuuid())));
    if let Some(dir) = tmp.path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let mut file = tokio::fs::File::create(&tmp.path).await?;
    let mut total = 0u64;
    use tokio::io::AsyncWriteExt;
    while let Some(chunk) = resp.chunk().await? {
        total += chunk.len() as u64;
        if total > max_bytes {
            bail!("larger than remote.max_media_mb");
        }
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    if total == 0 {
        bail!("empty file");
    }
    if let Some(dir) = target.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    crate::media::move_file(&tmp.path, target).await?;
    crate::media::make_public(target).await;
    Ok(())
}

/// The remote's id and base URL for a mirrored item, while the remote is enabled.
async fn remote_of(st: &AppState, item_id: i64) -> Option<(RemoteCfg, i64, String)> {
    let rc = settings(&st.cfg())?;
    let r = sql!("SELECT remote_id, remote_dest, source FROM remote_items WHERE item_id = ", item_id as i32).fetch_optional(&st.db).await.ok()??;
    // Files of another (earlier) source come from there
    let rc = RemoteCfg { base: r.string("source"), ..rc };
    Some((rc, r.i64("remote_id")?, r.string("remote_dest")))
}

/// `b/<name>` of a mirrored item: fetched from the remote when missing. True when the file is there.
pub async fn fetch_file(st: &AppState, name: &str) -> bool {
    let path = st.cfg().paths.b.join(name);
    if path.is_file() {
        return true;
    }
    if settings(&st.cfg()).is_none() {
        return false;
    }
    let Ok(Some(id)) = sql!("SELECT r.item_id FROM remote_items r JOIN items i ON i.id = r.item_id WHERE i.dest = ", name.to_string(), " LIMIT 1").fetch_scalar::<i32, _>(&st.db).await else {
        return false;
    };
    let Some((rc, _, remote_dest)) = remote_of(st, id as i64).await else { return false };
    let url = format!("{}/b/{}", rc.base, crate::util::encode_uri_component(&remote_dest));
    download(st, &url, &path, rc.max_bytes, false).await
}

/// `t/<id>.webp` / `t/<id>_blur.webp` of a mirrored item. True when the file is there.
pub async fn fetch_thumb(st: &AppState, file: &str) -> bool {
    let Some((id, blur)) = thumb_name(file) else { return false };
    let cfg = st.cfg();
    let thumb = cfg.paths.t.join(format!("{id}.webp"));
    if !thumb.is_file() {
        let Some((rc, rid, _)) = remote_of(st, id).await else { return false };
        if !download(st, &format!("{}/t/{rid}.webp", rc.base), &thumb, 20 * 1024 * 1024, true).await && !thumb_from_file(st, id).await {
            return false;
        }
    }
    if blur && !cfg.paths.t.join(format!("{id}_blur.webp")).is_file() {
        return crate::media::queue::gen_blurred_thumbnail(&cfg, id, false).await;
    }
    true
}

/// The thumbnail of an item for building a picture from (tag and list mosaics): `t/<id>.webp`, or
/// for a mirrored item without it the remote's. With `store_media` that is stored in t/ like on a
/// view; else it is fetched into tmp/ and the returned [`crate::media::TempPath`] removes it again
/// when dropped (keep it until the picture is built). None: no thumbnail to be had.
pub async fn mosaic_thumb(st: &AppState, id: i64) -> Option<(PathBuf, Option<crate::media::TempPath>)> {
    let cfg = st.cfg();
    let local = cfg.paths.t.join(format!("{id}.webp"));
    if local.is_file() {
        return Some((local, None));
    }
    let rc = settings(&cfg)?;
    if rc.store_media {
        return fetch_thumb(st, &format!("{id}.webp")).await.then_some((local, None));
    }
    let (rc, rid, _) = remote_of(st, id).await?;
    let tmp = crate::media::TempPath::new(cfg.paths.tmp.join(format!("{}.mosaic.webp", crate::media::queue::genuuid())));
    if !download(st, &format!("{}/t/{rid}.webp", rc.base), &tmp.path, 20 * 1024 * 1024, true).await {
        return None;
    }
    Some((tmp.path.clone(), Some(tmp)))
}

/// No thumbnail on the remote: fetch the file and make one here, like for an upload.
async fn thumb_from_file(st: &AppState, id: i64) -> bool {
    let Ok(Some(r)) = sql!("SELECT dest, mime FROM items WHERE id = ", id as i32).fetch_optional(&st.db).await else { return false };
    let (dest, mime) = (r.string("dest"), r.string("mime"));
    if !crate::media::is_plain_name(&dest) || !fetch_file(st, &dest).await {
        return false;
    }
    let cfg = st.cfg();
    let made = crate::media::queue::gen_thumbnail(&cfg, &dest, &mime, id, "", false, crate::media::queue::thumb_size(0)).await;
    if made.cover_extracted {
        let _ = sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", id as i32).execute(&st.db).await;
    }
    cfg.paths.t.join(format!("{id}.webp")).is_file()
}

/// `ca/<id>.webp` (cover art of a song) of a mirrored item. True when the file is there.
pub async fn fetch_cover(st: &AppState, item_id: i64) -> bool {
    let path = st.cfg().paths.ca.join(format!("{item_id}.webp"));
    if path.is_file() {
        return true;
    }
    let Some((rc, rid, _)) = remote_of(st, item_id).await else { return false };
    let ok = download(st, &format!("{}/ca/{rid}.webp", rc.base), &path, 20 * 1024 * 1024, true).await;
    if ok {
        let _ = sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", item_id as i32).execute(&st.db).await;
    }
    ok
}

// ── Streaming (remote.store_media off) ──────────────────────────────────────

/// Response headers passed on from the remote.
const PASS_HEADERS: [reqwest::header::HeaderName; 6] = [
    reqwest::header::CONTENT_TYPE,
    reqwest::header::CONTENT_LENGTH,
    reqwest::header::CONTENT_RANGE,
    reqwest::header::ACCEPT_RANGES,
    reqwest::header::LAST_MODIFIED,
    reqwest::header::ETAG,
];

/// The remote's answer for `url`, streamed through (Range and conditional requests forwarded).
/// None when the remote has no such file (remembered for a while).
async fn proxy(req: &Req, url: &str, image_only: bool) -> Option<Response> {
    let key = PathBuf::from(url);
    if failed_recently(&key) {
        return None;
    }
    let head = req.method == axum::http::Method::HEAD;
    let mut r = if head { client().head(url) } else { client().get(url) };
    for h in [header::RANGE, header::IF_RANGE, header::IF_NONE_MATCH, header::IF_MODIFIED_SINCE] {
        if let Some(v) = req.headers.get(&h) {
            r = r.header(h, v.clone());
        }
    }
    let resp = match r.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("[REMOTE] {url}: {e}");
            return None;
        }
    };
    let status = resp.status().as_u16();
    let ct = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    let bad_type = ct.starts_with("text/html") || (image_only && !ct.starts_with("image/"));
    if !matches!(status, 200 | 206 | 304 | 416) || (status != 304 && bad_type) {
        FAILED.lock().unwrap_or_else(|e| e.into_inner()).insert(key, Instant::now());
        return None;
    }
    let mut b = Response::builder().status(status);
    for h in PASS_HEADERS {
        if let Some(v) = resp.headers().get(&h) {
            b = b.header(h, v.clone());
        }
    }
    let body = if head { Body::empty() } else { Body::from_stream(resp.bytes_stream()) };
    b.body(body).ok()
}

/// A small remote image in memory (for making a blurred thumbnail).
async fn fetch_bytes(url: &str) -> Option<Vec<u8>> {
    let resp = client().get(url).timeout(Duration::from_secs(60)).send().await.ok()?;
    let ct = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
    if !resp.status().is_success() || !ct.starts_with("image/") || resp.content_length().is_some_and(|l| l > 20 * 1024 * 1024) {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    (!bytes.is_empty() && bytes.len() <= 20 * 1024 * 1024).then(|| bytes.to_vec())
}

fn webp(bytes: Vec<u8>) -> Response {
    Response::builder().status(200).header(header::CONTENT_TYPE, "image/webp").body(Body::from(bytes)).unwrap_or_default()
}

/// Run `magick` on `input` with `args` between the in and out file (both in tmp/, removed after).
async fn magick_bytes(st: &AppState, input: Vec<u8>, args: &[&str]) -> Option<Vec<u8>> {
    let tmp_dir = st.cfg().paths.tmp.clone();
    tokio::fs::create_dir_all(&tmp_dir).await.ok()?;
    let id = crate::media::queue::genuuid();
    let src = crate::media::TempPath::new(tmp_dir.join(format!("{id}.remote-in")));
    let dst = crate::media::TempPath::new(tmp_dir.join(format!("{id}.remote-out.webp")));
    tokio::fs::write(&src.path, input).await.ok()?;
    let (sp, dp) = (src.path.to_string_lossy().into_owned(), dst.path.to_string_lossy().into_owned());
    let mut all = vec![sp.as_str()];
    all.extend_from_slice(args);
    all.push(dp.as_str());
    crate::proc::spawn_with("magick", &all, crate::proc::Opts { quiet: true, timeout: Duration::from_secs(60), ..Default::default() }).await.ok()?;
    tokio::fs::read(&dst.path).await.ok().filter(|b| !b.is_empty())
}

/// `/b/<name>`: None when it's served from disk (a local file, not mirrored, or just stored).
pub async fn serve_file(st: &AppState, req: &Req, name: &str) -> Option<Response> {
    let cfg = st.cfg();
    if cfg.paths.b.join(name).is_file() {
        return None;
    }
    let rc = settings(&cfg)?;
    if rc.store_media {
        fetch_file(st, name).await;
        return None;
    }
    let id = sql!("SELECT r.item_id FROM remote_items r JOIN items i ON i.id = r.item_id WHERE i.dest = ", name.to_string(), " LIMIT 1").fetch_scalar::<i32, _>(&st.db).await.ok()??;
    let (rc, _, remote_dest) = remote_of(st, id as i64).await?;
    proxy(req, &format!("{}/b/{}", rc.base, crate::util::encode_uri_component(&remote_dest)), false).await
}

/// `/t/<id>.webp` and `/t/<id>_blur.webp`: None when it's served from disk (or doesn't exist).
pub async fn serve_thumb(st: &AppState, req: &Req, file: &str) -> Option<Response> {
    let cfg = st.cfg();
    if cfg.paths.t.join(file).is_file() {
        return None;
    }
    let rc = settings(&cfg)?;
    if rc.store_media {
        fetch_thumb(st, file).await;
        return None;
    }
    let (id, blur) = thumb_name(file)?;
    let (rc, rid, _) = remote_of(st, id).await?;
    let url = format!("{}/t/{rid}.webp", rc.base);
    if !blur {
        if let Some(r) = proxy(req, &url, true).await {
            return Some(r);
        }
        return audio_placeholder(st, id).await.map(webp);
    }
    // The remote has no blurred thumbnails: blur its thumbnail here, in tmp/ only
    let thumb = match fetch_bytes(&url).await {
        Some(b) => b,
        None => audio_placeholder(st, id).await?,
    };
    magick_bytes(st, thumb, &["-blur", "0x48"]).await.map(webp)
}

/// A song without a thumbnail on the remote gets the placeholder uploads get.
async fn audio_placeholder(st: &AppState, id: i64) -> Option<Vec<u8>> {
    let mime = sql!("SELECT mime FROM items WHERE id = ", id as i32).fetch_scalar::<String, _>(&st.db).await.ok()??;
    if !mime.starts_with("audio/") {
        return None;
    }
    let cfg = st.cfg();
    let tmp = crate::media::TempPath::new(cfg.paths.tmp.join(format!("{}.remote-ph.webp", crate::media::queue::genuuid())));
    tokio::fs::create_dir_all(&cfg.paths.tmp).await.ok()?;
    if !crate::media::queue::gen_audio_placeholder(&cfg, &tmp.path, crate::media::queue::thumb_size(0)).await {
        return None;
    }
    tokio::fs::read(&tmp.path).await.ok()
}

/// `/ca/<id>.webp`: None when it's served from disk (or doesn't exist).
pub async fn serve_cover(st: &AppState, req: &Req, file: &str) -> Option<Response> {
    let cfg = st.cfg();
    if cfg.paths.ca.join(file).is_file() {
        return None;
    }
    let rc = settings(&cfg)?;
    let id = file.strip_suffix(".webp")?.parse::<i64>().ok()?;
    if rc.store_media {
        fetch_cover(st, id).await;
        return None;
    }
    let (rc, rid, _) = remote_of(st, id).await?;
    proxy(req, &format!("{}/ca/{rid}.webp", rc.base), true).await
}

/// `/a/remote_<id>.webp`: a remote user's avatar (the remote's thumbnail of item `<id>`),
/// streamed, or stored in a/ with `remote.store_media`. None when it's served from disk.
pub async fn serve_avatar(st: &AppState, req: &Req, file: &str) -> Option<Response> {
    let rid: i64 = file.strip_prefix("remote_")?.strip_suffix(".webp")?.parse().ok()?;
    if avatar_file_name(rid) != file {
        return None;
    }
    let cfg = st.cfg();
    let path = cfg.paths.a.join(file);
    if path.is_file() {
        return None;
    }
    let rc = settings(&cfg)?;
    let url = format!("{}/t/{rid}.webp", rc.base);
    if rc.store_media {
        download(st, &url, &path, 20 * 1024 * 1024, true).await;
        return None;
    }
    proxy(req, &url, true).await
}

/// Whether a mirrored song has cover art on the remote (the item page asks): fetched when media
/// is stored, else one HEAD request, remembered.
pub async fn has_cover(st: &AppState, item_id: i64) -> bool {
    let Some(rc) = settings(&st.cfg()) else { return false };
    if rc.store_media {
        return fetch_cover(st, item_id).await;
    }
    static SEEN: LazyLock<Mutex<HashMap<i64, (bool, Instant)>>> = LazyLock::new(Default::default);
    if let Some((has, at)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).get(&item_id) {
        if *has || at.elapsed() < FAILED_TTL {
            return *has;
        }
    }
    let Some((rc, rid, _)) = remote_of(st, item_id).await else { return false };
    let has = match client().head(format!("{}/ca/{rid}.webp", rc.base)).timeout(Duration::from_secs(20)).send().await {
        Ok(r) => r.status().is_success() && r.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|c| c.starts_with("image/")),
        Err(_) => false,
    };
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).insert(item_id, (has, Instant::now()));
    if has {
        let _ = sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", item_id as i32).execute(&st.db).await;
    }
    has
}

/// `123.webp` → (123, false), `123_blur.webp` → (123, true).
fn thumb_name(file: &str) -> Option<(i64, bool)> {
    let stem = file.strip_suffix(".webp")?;
    let (id, blur) = match stem.strip_suffix("_blur") {
        Some(s) => (s, true),
        None => (stem, false),
    };
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| id.parse().ok().map(|i| (i, blur))).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_names() {
        assert_eq!(thumb_name("123.webp"), Some((123, false)));
        assert_eq!(thumb_name("123_blur.webp"), Some((123, true)));
        assert_eq!(thumb_name("abc.webp"), None);
        assert_eq!(thumb_name("_blur.webp"), None);
        assert_eq!(thumb_name("12.png"), None);
        assert_eq!(avatar_file_name(56660), "remote_56660.webp");
    }

    #[test]
    fn src_policies() {
        let p = |v: Value| src_policy(&crate::bot::tests::test_cfg(json!({ "remote": { "redact_src": v } })));
        assert_eq!(p(json!(false)), SrcPolicy::Keep);
        assert_eq!(p(Value::Null), SrcPolicy::Keep);
        assert_eq!(p(json!(true)), SrcPolicy::Drop);
        assert_eq!(p(json!("Admins")), SrcPolicy::Admins);
        let cfg = crate::bot::tests::test_cfg(json!({ "remote": { "redact_src": "admins" } }));
        assert_eq!(src_sql(&cfg, true), "items.src");
        assert!(src_sql(&cfg, false).contains("remote_items"));
    }

    #[test]
    fn types() {
        assert_eq!(media_types(&json!(["video", "Images", "foo", "video"])), vec!["video", "image"]);
        assert_eq!(media_types(&json!("audio, videos")), vec!["audio", "video"]);
        assert!(media_types(&Value::Null).is_empty());
    }

    #[test]
    fn redaction() {
        assert!(is_chat_id_uploader("telegram", "123456789"));
        assert!(is_chat_id_uploader("Telegram", "-100123"));
        assert!(!is_chat_id_uploader("telegram", "flummi"));
        assert!(!is_chat_id_uploader("libera", "1337"));
        assert_eq!(redact_src("tg://photo/AQADxyz"), "");
        assert_eq!(redact_src("https://example.org/a.png"), "https://example.org/a.png");
    }

    /// Against a running f0ck-rs (a 4th field tests a token instead of logging in):
    /// `F0CKM_REMOTE_TEST=http://host:port,login,password cargo test
    /// remote::tests::live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live() {
        let spec = std::env::var("F0CKM_REMOTE_TEST").expect("F0CKM_REMOTE_TEST=url,login,password[,token]");
        let mut p = spec.splitn(4, ',');
        let cfg = crate::bot::tests::test_cfg(json!({ "remote": { "enabled": true, "url": p.next(), "username": p.next(), "password": p.next(), "token": p.next() } }));
        let rc = settings(&cfg).expect("remote settings");
        let (user, total) = whoami(&rc).await.expect("whoami");
        println!("logged in as {user}, {total} items");
        assert!(token(&rc, false).await.is_ok(), "cached token");
        let page = call(&rc, &format!("/api/v3/feed?mode={}&eps={FEED_EPS}", rc.mode), None).await.expect("feed");
        let ids: Vec<i64> = page["items"].as_array().unwrap().iter().filter_map(|i| i["id"].as_i64()).collect();
        assert_eq!(ids.len() as i64, total.min(FEED_EPS as i64));
        let bulk = call(&rc, "/api/v3/items/bulk", Some(&json!({ "ids": ids, "include_tags": true, "include_favorites": true, "mode": rc.mode }))).await.expect("bulk");
        for e in bulk["items"].as_array().unwrap() {
            println!("#{} {} by {} tags {:?}", e["item"]["id"], e["item"]["dest"], e["item"]["username"], e["tags"].as_array().unwrap().iter().map(|t| t["tag"].as_str().unwrap_or("")).collect::<Vec<_>>());
            assert!(e["item"]["login"].is_string(), "uploader login needs the login token");
        }
        // Streaming: whole file, a Range request, a missing thumbnail
        let get = |range: Option<&str>| {
            let mut h = axum::http::HeaderMap::new();
            if let Some(r) = range {
                h.insert(header::RANGE, r.parse().unwrap());
            }
            Req::new(axum::http::Method::GET, &"/b/x".parse().unwrap(), h, Body::empty(), None)
        };
        let dest = bulk["items"][0]["item"]["dest"].as_str().unwrap().to_string();
        let url = format!("{}/b/{dest}", rc.base);
        let full = proxy(&get(None), &url, false).await.expect("streamed file");
        assert_eq!(full.status(), 200);
        let full = axum::body::to_bytes(full.into_body(), usize::MAX).await.unwrap();
        let part = proxy(&get(Some("bytes=10-19")), &url, false).await.expect("range");
        assert_eq!(part.status(), 206);
        assert!(part.headers().get(header::CONTENT_RANGE).is_some());
        assert_eq!(axum::body::to_bytes(part.into_body(), usize::MAX).await.unwrap(), full.slice(10..20));
        println!("streamed {dest}: {} bytes, range ok", full.len());
        assert!(proxy(&get(None), &format!("{}/t/999999.webp", rc.base), true).await.is_none());

        if rc.token.is_none() {
            let bad = RemoteCfg { password: "wrong".into(), ..rc.clone() };
            assert!(login(&bad).await.unwrap_err().to_string().contains("refused"));
        }
    }

    #[test]
    fn settings_need_url_and_login() {
        let cfg = |remote: Value| crate::bot::tests::test_cfg(json!({ "remote": remote }));
        assert!(settings(&cfg(json!({ "enabled": false, "url": "https://x", "username": "a", "password": "b" }))).is_none());
        assert!(settings(&cfg(json!({ "enabled": true, "url": "ftp://x", "username": "a", "password": "b" }))).is_none());
        assert!(settings(&cfg(json!({ "enabled": true, "url": "https://x", "username": "", "password": "b" }))).is_none());
        // A token alone is enough; pasted with quotes or "Bearer " it still works
        let rc = settings(&cfg(json!({ "enabled": true, "url": "https://x", "token": " \"Bearer abc.def\" " }))).unwrap();
        assert_eq!(rc.token.as_deref(), Some("abc.def"));
        assert!(settings(&cfg(json!({ "enabled": true, "url": "https://x", "token": "  " }))).is_none());
        let rc = settings(&cfg(json!({ "enabled": true, "url": "https://x.y/", "username": "a", "password": "b" }))).unwrap();
        assert!(rc.token.is_none());
        assert_eq!(rc.base, "https://x.y");
        assert_eq!(rc.mode, 3);
        assert!(rc.keep_ids);
    }
}
