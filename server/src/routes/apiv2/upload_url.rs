//! URL uploads (port of src/inc/routes/apiv2/upload.mjs): `POST /api/v2/upload-url` (YouTube
//! embeds right away, everything else downloaded in the background with live progress),
//! `GET /api/v2/upload-url/progress/:jobId` and `GET /api/v2/meta/extract-url`.
//! `calculateExpiresAt` lives in `crate::media::calculate_expires_at`.

use crate::config::Config;
use crate::db::RowExt;
use crate::http::{Group, Req, Res, Router, json_resp, with_header};
use crate::media::{self, mime, queue};
use crate::routes::uploads::{assign_tag, ensure_tag};
use crate::routes::uploads::upload::validate_remote_url;
use crate::session::Session;
use crate::settings as s;
use crate::social::helpers::{js_str, js_truthy};
use crate::sql;
use crate::state::AppState;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn register(r: &mut Router) {
    let mut g = Group::new(r, r"^\/api\/v2");
    g.get(r"\/upload-url\/progress\/(?<jobId>[a-zA-Z0-9_-]+)$", progress);
    g.get(r"\/meta\/extract-url$", extract_url);
    g.post(r"\/upload-url$", upload_url);
}

fn j(v: Value, code: u16) -> Res {
    Ok(json_resp(&v, code))
}

/// `uploadApiAuth`: a session; anonymous ones need the upload permission, others must be
/// registered users.
async fn upload_auth(st: &AppState, req: &mut Req) -> Option<Res> {
    let cfg = st.cfg();
    let Some(sess) = req.session() else {
        return Some(j(json!({ "success": false, "msg": "Unauthorized" }), 401));
    };
    if s::is_anon_session(Some(sess)) {
        if !s::can_anon_do(&cfg, "upload") {
            return Some(j(json!({ "success": false, "msg": "Action requires a registered account or anonymous upload permission" }), 403));
        }
        return None;
    }
    crate::http::check_guard(st, req, crate::http::Guard::RegisteredUser).await.map(Ok)
}

// ── job progress ────────────────────────────────────────────────────────────

struct Job {
    owner: i64,
    state: Map<String, Value>,
}

/// jobId → progress `{ stage, percent, speed, eta, done, error, itemId? }`; removed 10 min after
/// the job finished.
static JOBS: Lazy<parking_lot::Mutex<std::collections::HashMap<String, Job>>> = Lazy::new(Default::default);

fn set_progress(job_id: &str, patch: Value) {
    let mut jobs = JOBS.lock();
    if let (Some(job), Value::Object(p)) = (jobs.get_mut(job_id), patch) {
        for (k, v) in p {
            job.state.insert(k, v);
        }
    }
}

fn cleanup_job(job_id: &str) {
    let id = job_id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(600)).await;
        JOBS.lock().remove(&id);
    });
}

async fn progress(st: AppState, mut req: Req) -> Res {
    if let Some(r) = upload_auth(&st, &mut req).await {
        return r;
    }
    let job_id = req.param("jobId").unwrap_or("").to_string();
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    // Only the uploader sees a job (the old code showed any job to any user who knew its id)
    let state = JOBS.lock().get(&job_id).filter(|j| j.owner == uid).map(|j| j.state.clone());
    let res = match state {
        Some(mut m) => {
            let mut out = Map::new();
            out.insert("success".into(), json!(true));
            out.append(&mut m);
            json_resp(&Value::Object(out), 200)
        }
        None => json_resp(&json!({ "success": false, "msg": "Job not found" }), 404),
    };
    Ok(with_header(res, "cache-control", "no-store"))
}

// ── helpers ─────────────────────────────────────────────────────────────────

static YT_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?\?(?:\S*?&?v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").unwrap());

/// Automatic tags for a URL: `youtube` for YouTube links.
fn auto_tags_from_url(url: &str) -> Vec<String> {
    let Ok(u) = url::Url::parse(url) else { return vec![] };
    let host = u.host_str().unwrap_or("").to_lowercase();
    let parts: Vec<&str> = host.split('.').collect();
    let short = ["co", "com", "net", "org", "gov", "edu", "ac", "or", "ne"];
    let domain = if parts.len() >= 3 && short.contains(&parts[parts.len() - 2]) { parts[parts.len() - 3..].join(".") } else { parts[parts.len().saturating_sub(2)..].join(".") };
    let yt = |d: &str| d.ends_with("youtube.com") || d.ends_with("youtu.be");
    if yt(&domain) || yt(&host) { vec!["youtube".into()] } else { vec![] }
}

fn rating_tag(cfg: &Config, rating: &str) -> i64 {
    match rating {
        "sfw" => 1,
        "nsfw" => 2,
        _ => cfg.nsfl_tag_id(),
    }
}

/// `getTargetVisibility(req, post.visibility)`
fn target_visibility(cfg: &Config, req: &Req, session: &Session, post_vis: Option<String>) -> i64 {
    if cfg.is_false("enable_private_uploads") {
        return 0;
    }
    let num = |p: &str| cfg.get(p).as_f64().map(|f| f as i64);
    let sys_default = num("default_upload_visibility").or_else(|| num("websrv.default_upload_visibility")).unwrap_or(0);
    if cfg.is_false("allow_user_upload_visibility") || cfg.is_false("websrv.allow_user_upload_visibility") {
        return sys_default;
    }
    let raw = req.header("x-upload-visibility").filter(|h| !h.is_empty()).map(str::to_string).or(post_vis).unwrap_or_default();
    match raw.trim().to_lowercase().as_str() {
        "private" | "2" => 2,
        "unlisted" | "1" => 1,
        "public" | "0" => 0,
        _ => session.default_upload_visibility.unwrap_or(sys_default),
    }
}

/// Find-or-create a tag and assign it (errors are logged).
async fn add_tag(st: &AppState, item_id: i64, name: &str, user_id: i64) {
    let r = async {
        if let Some(tid) = ensure_tag(st, name).await? {
            assign_tag(st, item_id, tid, user_id).await?;
        }
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(e) = r {
        tracing::error!("[UPLOAD] Failed to assign tag \"{name}\": {e}");
    }
}

async fn save_comment(st: &AppState, item_id: i64, user_id: i64, content: &str) {
    if content.trim().is_empty() {
        return;
    }
    let filtered = crate::social::helpers::apply_word_filter(st, content).await;
    if let Err(e) = sql!(
        "INSERT INTO comments (item_id, user_id, parent_id, content) VALUES (",
        item_id as i32,
        ", ",
        user_id as i32,
        ", NULL, ",
        filtered.trim(),
        ")"
    )
    .execute(&st.db)
    .await
    {
        tracing::error!("[UPLOAD] Failed to save upload comment: {e}");
    }
}

async fn notify_user(st: &AppState, user_id: i64, kind: &str, item_id: Option<i64>, data: Value) {
    if let Err(e) = sql!(
        "INSERT INTO notifications (user_id, type, reference_id, item_id, data) VALUES (",
        user_id as i32,
        ", ",
        kind,
        ", 0, ",
        item_id.map(|i| i as i32),
        ", ",
        data,
        ")"
    )
    .execute(&st.db)
    .await
    {
        tracing::error!("[UPLOAD-URL] notification failed: {e}");
    }
}

// ── GET /meta/extract-url ───────────────────────────────────────────────────

static TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]*>").unwrap());

async fn extract_url(st: AppState, mut req: Req) -> Res {
    if let Some(r) = upload_auth(&st, &mut req).await {
        return r;
    }
    let Some(url) = req.query("url").filter(|u| !u.is_empty()).map(str::to_string) else {
        return j(json!({ "success": false, "msg": "URL required" }), 400);
    };
    let mut out: Vec<String> = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut add = |v: &str| {
        let c = TAG_RE.replace_all(v, "");
        let c: String = c.chars().filter(|c| !(*c as u32 <= 0x1f || *c as u32 == 0x7f)).collect();
        let c = c.trim();
        let n = c.encode_utf16().count();
        if n > 1 && n <= 255 && seen.insert(c.to_lowercase()) {
            out.push(c.to_string());
        }
    };
    for t in auto_tags_from_url(&url) {
        add(&t);
    }
    // Title/description via yt-dlp, only for public http(s) URLs (the old code passed anything on)
    if validate_remote_url(&url).is_some() {
        let proxy = media::proxy_url(&st.cfg());
        let mut a: Vec<&str> = vec![];
        if let Some(p) = &proxy {
            a.extend(["--proxy", p.as_str()]);
        }
        a.extend(["--get-title", "--get-description", "--no-playlist", "--skip-download", "--", &url]);
        let o = crate::proc::Opts { timeout: Duration::from_secs(5), quiet: true, ..Default::default() };
        if let Ok(r) = crate::proc::spawn_with("yt-dlp", &a, o).await {
            let text = r.stdout_str();
            let lines: Vec<&str> = text.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect();
            if let Some(t) = lines.first() {
                add(t);
            }
            if let Some(d) = lines.get(1) {
                let first = d.split(['.', '\n']).next().unwrap_or("").trim();
                if first.encode_utf16().count() > 3 {
                    add(first);
                }
            }
        }
    }
    j(json!({ "success": true, "fields": out }), 200)
}

// ── POST /upload-url ────────────────────────────────────────────────────────

/// Everything the background job needs from the request.
struct UrlJob {
    url: String,
    owner: String,
    user_id: i64,
    admin: bool,
    display_name: Value,
    rating: Option<String>,
    tags: Vec<String>,
    comment: String,
    is_oc: bool,
    title: Option<String>,
    visibility: i64,
    slug: String,
    expiry: Option<String>,
}

async fn upload_url(st: AppState, mut req: Req) -> Res {
    if let Some(r) = upload_auth(&st, &mut req).await {
        return r;
    }
    let cfg = st.cfg();
    let r = upload_url_inner(&st, &cfg, &mut req).await;
    match r {
        Ok(res) => res,
        Err(e) => {
            let msg = crate::util::log_error(&e, "[UPLOAD-URL ERROR]");
            j(json!({ "success": false, "msg": format!("Upload failed: {msg}") }), 500)
        }
    }
}

async fn upload_url_inner(st: &AppState, cfg: &Config, req: &mut Req) -> anyhow::Result<Res> {
    if !cfg.truthy("websrv.web_url_upload") {
        return Ok(j(json!({ "success": false, "msg": "URL uploads are disabled" }), 403));
    }
    let post = req.post_value().await;
    let session: Session = req.session().cloned().unwrap_or_default();
    if !crate::cap::take(cfg, req, &session, crate::cap::Scope::Upload) {
        return Ok(j(json!({ "success": false, "msg": "Please complete the captcha.", "captcha_required": "upload" }), 403));
    }
    let field = |k: &str| js_str(post.get(k));
    let title = post.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(|t| crate::util::utf16_prefix(t, 500));
    let comment = field("comment").unwrap_or_default();
    if let Some(max) = cfg.int("main.comment_max_length") {
        if !comment.is_empty() && comment.encode_utf16().count() as i64 > max {
            return Ok(j(json!({ "success": false, "msg": format!("Comment too long (max {max} characters)") }), 400));
        }
    }
    let Some(input_url) = field("url").filter(|u| !u.trim().is_empty()) else {
        return Ok(j(json!({ "success": false, "msg": "URL is required" }), 400));
    };
    let is_shitpost = js_truthy(post.get("is_shitpost"));
    let is_oc = js_truthy(post.get("is_oc"));
    let rating = field("rating").filter(|r| ["sfw", "nsfw", "nsfl"].contains(&r.as_str()));
    if !is_shitpost && rating.is_none() {
        return Ok(j(json!({ "success": false, "msg": "Rating (sfw/nsfw/nsfl) is required" }), 400));
    }
    if rating.as_deref() == Some("nsfl") && !cfg.enable_nsfl() {
        return Ok(j(json!({ "success": false, "msg": "NSFL mode is currently disabled" }), 400));
    }
    let tags: Vec<String> = field("tags")
        .filter(|t| !t.is_empty())
        .map(|t| t.split(',').map(str::trim).filter(|t| !t.is_empty() && !["sfw", "nsfw", "nsfl"].contains(&t.to_lowercase().as_str())).map(str::to_string).collect())
        .unwrap_or_default();
    let min_tags = st.settings.min_tags();
    if !is_shitpost && min_tags > 0 && (tags.len() as i64) < min_tags {
        return Ok(j(json!({ "success": false, "msg": format!("At least {min_tags} tag{} required", if min_tags != 1 { "s" } else { "" }) }), 400));
    }
    let owner = s::session_owner_name(Some(&session)).unwrap_or_else(|| session.user.clone());
    if !session.is_admin_or_mod() {
        let since = (crate::util::now_secs() - 12 * 3600) as i32;
        let n: i64 = sql!("SELECT count(*) as count FROM items WHERE username = ", owner.as_str(), " AND stamp > ", since, " AND is_deleted = false")
            .fetch_scalar(&st.db)
            .await?
            .unwrap_or(0);
        if n >= 69 {
            return Ok(j(json!({ "success": false, "msg": "Upload limit reached (69 per 12 hours)" }), 429));
        }
    }

    let mut url = input_url.trim().to_string();
    if let Ok(mut u) = url::Url::parse(&url) {
        if u.query_pairs().any(|(k, _)| k == "igsh") {
            let rest: Vec<(String, String)> = u.query_pairs().filter(|(k, _)| k != "igsh").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            if rest.is_empty() {
                u.set_query(None);
            } else {
                u.query_pairs_mut().clear().extend_pairs(rest);
            }
            url = u.to_string();
        }
    }
    // Only public http(s) URLs (the old code handed any string to yt-dlp and curl)
    if validate_remote_url(&url).is_none() {
        return Ok(j(json!({ "success": false, "msg": "Invalid URL" }), 400));
    }
    let yt = YT_RE.captures(&url).map(|c| c[1].to_string());
    let bypass = st.settings.bypass_duplicate_check();
    if yt.is_none() && !bypass {
        if let Some(rep) = queue::check_repost_link(st, &url).await? {
            return Ok(j(json!({ "success": false, "msg": "This URL has already been uploaded", "repost": rep }), 409));
        }
    }
    let manual = st.settings.manual_approval();
    let visibility = target_visibility(cfg, req, &session, field("visibility"));
    let expiry = field("expiry")
        .filter(|v| !v.is_empty())
        .or_else(|| req.header("x-upload-expiry").filter(|h| !h.is_empty()).map(str::to_string))
        .or_else(|| field("expires_at").filter(|v| !v.is_empty()));
    let job = UrlJob {
        url,
        owner,
        user_id: session.id,
        admin: session.admin,
        display_name: session.display_name.clone().filter(|d| !d.is_empty()).map(Value::String).unwrap_or(Value::Null),
        rating,
        tags,
        comment,
        is_oc,
        title,
        visibility,
        slug: crate::util::generate_slug(11),
        expiry,
    };

    if let Some(video_id) = yt.filter(|_| cfg.enabled("websrv.enable_youtube_upload")) {
        return youtube(st, cfg, job, manual, &video_id).await;
    }

    // Regular URL: answer right away (proxy timeouts), download in the background
    let job_id = queue::genuuid();
    JOBS.lock().insert(
        job_id.clone(),
        Job { owner: session.id, state: json!({ "stage": "queued", "percent": 0, "speed": null, "eta": null, "done": false, "error": null }).as_object().cloned().unwrap_or_default() },
    );
    {
        let (st, job_id) = (st.clone(), job_id.clone());
        tokio::spawn(async move { background(&st, job, &job_id).await });
    }
    Ok(j(
        json!({ "success": true, "pending": true, "jobId": job_id, "msg": "URL processing started in background. You will receive a notification when it is finished." }),
        200,
    ))
}

async fn insert_item(st: &AppState, cfg: &Config, job: &UrlJob, src: &str, dest: &str, mime: &str, size: i64, checksum: &str, phash: Option<String>, manual: bool) -> anyhow::Result<i64> {
    let now = crate::util::now_secs();
    let expires_at = media::calculate_expires_at(cfg, job.expiry.as_deref(), now);
    let id: i32 = sql!(
        "INSERT INTO items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, is_oc, title, visibility, slug, expires_at) VALUES (",
        src, ", ", dest, ", ", mime, ", ", size as i32, ", ", checksum, ", ", phash, ", ", job.owner.as_str(), ", 'web', 'web', ", now as i32, ", ",
        !manual, ", ", job.is_oc, ", ", job.title.clone(), ", ", job.visibility as i16, ", ", job.slug.as_str(), ", ", expires_at, ") RETURNING id"
    )
    .fetch_scalar(&st.db)
    .await?
    .ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
    let id = id as i64;
    st.invalidate_upload_count_cache(&job.owner);
    if job.visibility == 2 {
        crate::private_items::add_private(st, id, dest, &job.owner);
    }
    if cfg.enabled("enable_comments") {
        if let Err(e) = sql!("INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", job.user_id as i32, ", ", id as i32, ") ON CONFLICT DO NOTHING")
            .execute(&st.db)
            .await
        {
            tracing::error!("[UPLOAD-URL] Auto-subscribe error: {e}");
        }
    }
    Ok(id)
}

/// Rating tag, user + automatic tags, approval notice, comment, OC tags, `new_item` broadcast.
async fn finish_item(st: &AppState, cfg: &Config, job: &UrlJob, item_id: i64, dest: &str, mime: &str, manual: bool) {
    let rating_id = job.rating.as_deref().map(|r| rating_tag(cfg, r));
    if let Some(rid) = rating_id {
        if let Err(e) = assign_tag(st, item_id, rid, job.user_id).await {
            tracing::error!("[UPLOAD-URL] rating tag failed: {e}");
        }
    }
    let mut all = job.tags.clone();
    for t in auto_tags_from_url(&job.url) {
        if !all.contains(&t) {
            all.push(t);
        }
    }
    all.dedup();
    for t in &all {
        add_tag(st, item_id, t, job.user_id).await;
    }
    if manual {
        queue::notify_admins(st, item_id).await;
    }
    save_comment(st, item_id, job.user_id, &job.comment).await;
    if job.is_oc {
        for t in ["oc", "original content"] {
            add_tag(st, item_id, t, job.user_id).await;
        }
    }
    if !manual {
        crate::f0cklib::clear_count_cache(st);
        let payload = json!({
            "id": item_id, "dest": dest, "mime": mime, "username": job.owner, "display_name": job.display_name,
            "tag_id": rating_id.unwrap_or(0), "is_oc": job.is_oc,
            "slug": if crate::settings::enable_item_slugs(&st.cfg()) { json!(job.slug) } else { Value::Null },
            "visibility": job.visibility,
            "is_album": false, "album_count": 0,
        });
        crate::social::helpers::notify(st, "new_item", &payload).await;
    }
}

async fn youtube(st: &AppState, cfg: &Config, mut job: UrlJob, manual: bool, video_id: &str) -> anyhow::Result<Res> {
    let yt_url = format!("https://www.youtube.com/watch?v={video_id}");
    let dest = format!("yt:{video_id}");
    let checksum = format!("yt_{video_id}_{}", crate::util::now_ms());
    job.url = yt_url.clone();
    let id = insert_item(st, cfg, &job, &yt_url, &dest, "video/youtube", 0, &checksum, None, manual).await?;
    queue::gen_thumbnail(cfg, &dest, "video/youtube", id, &yt_url, manual, 512).await;
    if job.rating.is_some() {
        queue::gen_blurred_thumbnail(cfg, id, manual).await;
    }
    finish_item(st, cfg, &job, id, &dest, "video/youtube", manual).await;
    let msg = if manual { "YouTube video embedded! Pending admin approval." } else { "YouTube video embedded!" };
    Ok(j(json!({ "success": true, "msg": msg, "itemid": id, "manual_approval": manual }), 200))
}

// ── background download ─────────────────────────────────────────────────────

/// A failed command: its message and stderr (for `sanitizeError`).
#[derive(Debug)]
struct CmdErr {
    msg: String,
    stderr: Option<String>,
}

impl CmdErr {
    fn plain(msg: impl Into<String>) -> CmdErr {
        CmdErr { msg: msg.into(), stderr: None }
    }
}

impl<E: std::fmt::Display> From<E> for CmdErr {
    fn from(e: E) -> CmdErr {
        CmdErr::plain(e.to_string())
    }
}

static ERROR_LINE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)ERROR:\s*(.+)$").unwrap());
static HTTP_ERR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)HTTP Error (\d+)").unwrap());
static STATUS_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)status code (\d{3})").unwrap());
static CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(4\d{2}|5\d{2})\b").unwrap());
static FAILED_CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"failed with code (\d+)").unwrap());

/// `sanitizeError(err)`: a short, user-facing reason.
fn sanitize_error(e: &CmdErr, url: &str) -> String {
    if let Some(stderr) = e.stderr.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(c) = ERROR_LINE_RE.captures(stderr) {
            return c[1].trim().to_string();
        }
        if stderr.starts_with("curl: ") {
            return stderr.to_string();
        }
        if let Some(last) = stderr.split('\n').map(str::trim).filter(|l| !l.is_empty() && !l.contains("WARNING:")).last() {
            return last.to_string();
        }
    }
    let msg = e.msg.as_str();
    let code = HTTP_ERR_RE
        .captures(msg)
        .or_else(|| STATUS_RE.captures(msg))
        .map(|c| c[1].to_string())
        .or_else(|| CODE_RE.captures(msg).map(|c| c[1].to_string()).filter(|c| c != "537"));
    if let Some(c) = code {
        return format!("Download/Process failed (HTTP {c})");
    }
    if msg.starts_with("Command '") {
        let code = FAILED_CODE_RE.captures(msg).map(|c| c[1].to_string()).unwrap_or_else(|| "1".into());
        return format!("Process failed (code {code})");
    }
    if msg.is_empty() { format!("Failed to process {url}") } else { msg.to_string() }
}

/// Run a tool; a non-zero exit is an error carrying its stderr.
async fn run(cmd: &str, args: &[&str], timeout: Duration) -> Result<crate::proc::Output, CmdErr> {
    let o = crate::proc::spawn_with(cmd, args, crate::proc::Opts { timeout, ignore_exit_code: true, quiet: true, ..Default::default() }).await?;
    if o.code != Some(0) {
        return Err(CmdErr { msg: format!("Command '{cmd}' failed with code {}", o.code.unwrap_or(1)), stderr: Some(o.stderr) });
    }
    Ok(o)
}

static ANSI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;]*m").unwrap());
static DL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[download\]\s+([\d.]+)%.*?at\s+([\d.]+\s*\S+/s)(?:.*?ETA\s+(\S+))?").unwrap());

/// `parseYtdlpLine(line)` → progress patch
fn parse_ytdlp_line(line: &str) -> Option<Value> {
    if let Some(c) = DL_RE.captures(line) {
        let pct = c[1].parse::<f64>().unwrap_or(0.0);
        return Some(json!({ "stage": "downloading", "percent": crate::f0cklib::recommend::jnum(pct), "speed": &c[2], "eta": c.get(3).map(|m| m.as_str()) }));
    }
    if line.contains("[download] Destination:") {
        return Some(json!({ "stage": "downloading" }));
    }
    if line.contains("[Merger]") || line.contains("[ffmpeg]") {
        return Some(json!({ "stage": "processing", "percent": 99 }));
    }
    if line.contains("[ExtractAudio]") {
        return Some(json!({ "stage": "extracting", "percent": 99 }));
    }
    None
}

/// yt-dlp with its stderr progress lines (split on `\r` and `\n`) fed into the job state.
async fn ytdlp_with_progress(args: &[&str], job_id: &str) -> Result<String, CmdErr> {
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new("yt-dlp")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| CmdErr::plain(format!("failed to start yt-dlp: {e}")))?;
    let mut stdout = child.stdout.take().ok_or_else(|| CmdErr::plain("no stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| CmdErr::plain("no stderr"))?;
    let out_task = tokio::spawn(async move {
        let mut b = Vec::new();
        let _ = stdout.read_to_end(&mut b).await;
        b
    });
    let id = job_id.to_string();
    let err_task = tokio::spawn(async move {
        let mut all = Vec::new();
        let mut line = Vec::new();
        let mut buf = [0u8; 4096];
        let handle = |l: &[u8]| {
            let s = String::from_utf8_lossy(l);
            let clean = ANSI_RE.replace_all(&s, "");
            if let Some(p) = parse_ytdlp_line(clean.trim()) {
                set_progress(&id, p);
            }
        };
        loop {
            let n = match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if all.len() < 1 << 20 {
                all.extend_from_slice(&buf[..n]);
            }
            for &b in &buf[..n] {
                if b == b'\n' || b == b'\r' {
                    handle(&line);
                    line.clear();
                } else if line.len() < 8192 {
                    line.push(b);
                }
            }
        }
        if !line.is_empty() {
            handle(&line);
        }
        String::from_utf8_lossy(&all).into_owned()
    });
    let status = match tokio::time::timeout(Duration::from_secs(3600), child.wait()).await {
        Ok(s) => s.map_err(|e| CmdErr::plain(e.to_string()))?,
        Err(_) => {
            let _ = child.kill().await;
            return Err(CmdErr::plain("Command 'yt-dlp' timed out"));
        }
    };
    let out = out_task.await.unwrap_or_default();
    let err = err_task.await.unwrap_or_default();
    if !status.success() {
        return Err(CmdErr { msg: format!("Command 'yt-dlp' failed with code {}", status.code().unwrap_or(1)), stderr: Some(err) });
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Accept a path yt-dlp printed only inside our temp dir and with our prefix.
async fn own_tmp_file(tmp: &Path, uuid: &str, printed: &str) -> Option<PathBuf> {
    let tmp_real = tokio::fs::canonicalize(tmp).await.unwrap_or_else(|_| tmp.to_path_buf());
    let real = tokio::fs::canonicalize(printed).await.ok()?;
    let ok = real.parent() == Some(tmp_real.as_path()) && real.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{uuid}."))) && real.is_file();
    ok.then_some(real)
}

/// Removes `<uuid>.*` leftovers from the temp dir when the job ends.
struct Leftovers {
    dir: PathBuf,
    prefix: String,
}

impl Drop for Leftovers {
    fn drop(&mut self) {
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                if e.file_name().to_string_lossy().starts_with(&self.prefix) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

async fn download(cfg: &Config, job: &UrlJob, job_id: &str, uuid: &str, max: f64) -> Result<PathBuf, CmdErr> {
    let tmp = cfg.paths.tmp.clone();
    let out_tpl = tmp.join(format!("{uuid}.%(ext)s")).to_string_lossy().into_owned();
    let max_k = format!("{}k", crate::util::js_num(max / 1024.0));
    // Configured SOCKS proxy, or the SSRF guard (crate::net_guard)
    let proxy = media::proxy_url(cfg);
    let mut base: Vec<&str> = vec![];
    if let Some(p) = &proxy {
        base.extend(["--proxy", p.as_str()]);
    }
    let js_runtime = media::ytdlp_js_runtime(cfg);
    base.extend(["--js-runtimes", js_runtime.as_str(), "--geo-bypass", "--extractor-args", "youtube:player-client=ios,web", "--newline", "--no-colors"]);
    let url = job.url.as_str();
    let last = |s: String| s.trim().split('\n').map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_string);

    tracing::info!("[UPLOAD-URL-ASYNC] Starting Stage 1 (constrained) download for {url} (user: {})", job.owner);
    set_progress(job_id, json!({ "stage": "downloading", "percent": 0 }));
    let mut a1 = base.clone();
    a1.extend([
        "-f", "bv*[height<=1080]+ba/b[height<=1080] / wv*+ba/w", "--max-filesize", &max_k, "--postprocessor-args", "ffmpeg:-bitexact", "-o", &out_tpl,
        "--print", "after_move:filepath", "--merge-output-format", "mp4", "--", url,
    ]);
    let printed = match ytdlp_with_progress(&a1, job_id).await {
        Ok(o) => last(o),
        Err(e) => {
            tracing::warn!("[UPLOAD-URL-ASYNC] Stage 1 failed: {}", e.msg);
            if url.to_lowercase().contains("instagram.com") {
                return Err(CmdErr::plain(sanitize_error(&e, url)));
            }
            set_progress(job_id, json!({ "stage": "downloading", "percent": 0, "speed": null, "eta": null }));
            let mut a2 = base.clone();
            a2.extend(["--max-filesize", &max_k, "-o", &out_tpl, "--print", "after_move:filepath", "--", url]);
            match ytdlp_with_progress(&a2, job_id).await {
                Ok(o) => last(o),
                Err(e2) => {
                    tracing::warn!("[UPLOAD-URL-ASYNC] Stage 2 failed: {}", e2.msg);
                    tracing::info!("[UPLOAD-URL-ASYNC] Starting Stage 3 (curl) fallback for {url}");
                    set_progress(job_id, json!({ "stage": "downloading", "percent": 0, "speed": null, "eta": null }));
                    return curl_fallback(cfg, url, uuid, max, proxy.as_deref()).await;
                }
            }
        }
    };
    let Some(p) = printed.filter(|p| !p.contains("larger than")) else {
        return Err(CmdErr::plain("File too large or download failed"));
    };
    own_tmp_file(&tmp, uuid, &p).await.ok_or_else(|| CmdErr::plain("File too large or download failed"))
}

async fn curl_fallback(cfg: &Config, url: &str, uuid: &str, max: f64, proxy: Option<&str>) -> Result<PathBuf, CmdErr> {
    let tmp = cfg.paths.tmp.join(format!("{uuid}.tmp"));
    let t = tmp.to_string_lossy().into_owned();
    let referer = url::Url::parse(url)
        .ok()
        .map(|u| {
            let mut host = u.host_str().unwrap_or("").to_string();
            if host.contains("imgur.com") {
                host = "imgur.com".into();
            }
            format!("{}://{host}/", u.scheme())
        })
        .unwrap_or_else(|| url.to_string());
    let max_b = crate::util::js_num(max);
    let mut a: Vec<&str> = vec![
        "-s", "-S", "-f", "-L", "--proto", "=http,https", "--proto-redir", "=http,https", "-o", &t, "--max-filesize", &max_b, "--connect-timeout", "30",
        "--max-time", "300", "--user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        "--referer", &referer, "-H", "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8", "-H",
        "Accept-Language: en-US,en;q=0.9",
    ];
    if let Some(p) = proxy {
        a.extend(["--proxy", p]);
    }
    a.extend(["--", url]);
    if let Err(e) = run("curl", &a, Duration::from_secs(330)).await {
        return Err(CmdErr::plain(sanitize_error(&e, url)));
    }
    let m = mime::detect(&tmp).await?;
    match mime::ext_for(cfg, &m) {
        Some(ext) => {
            let fin = cfg.paths.tmp.join(format!("{uuid}.{ext}"));
            tokio::fs::rename(&tmp, &fin).await?;
            Ok(fin)
        }
        None => {
            let _ = tokio::fs::remove_file(&tmp).await;
            Err(CmdErr::plain(format!("Failed to process {url}")))
        }
    }
}

async fn background(st: &AppState, job: UrlJob, job_id: &str) {
    let cfg = st.cfg();
    let uuid = queue::genuuid();
    let _leftovers = Leftovers { dir: cfg.paths.tmp.clone(), prefix: format!("{uuid}.") };
    match process(st, &cfg, &job, job_id, &uuid).await {
        Ok(Some(item_id)) => {
            set_progress(job_id, json!({ "stage": "done", "percent": 100, "done": true, "error": null, "itemId": item_id }));
            cleanup_job(job_id);
            notify_user(st, job.user_id, "upload_success", Some(item_id), json!({ "jobId": job_id })).await;
        }
        Ok(None) => {} // duplicate: already reported
        Err(e) => {
            tracing::error!("[UPLOAD-URL-ASYNC] Final Error: {e:?}");
            let msg = sanitize_error(&e, &job.url);
            set_progress(job_id, json!({ "stage": "error", "done": true, "error": msg }));
            cleanup_job(job_id);
            notify_user(st, job.user_id, "upload_error", None, json!({ "jobId": job_id, "url": job.url, "msg": msg })).await;
        }
    }
}

/// Replace the extension of a temp file (`<uuid>.<ext>`).
fn with_ext(p: &Path, ext: &str) -> PathBuf {
    p.with_extension(ext)
}

/// Download, check and store the file → item id (`None`: duplicate, already reported).
async fn process(st: &AppState, cfg: &Config, job: &UrlJob, job_id: &str, uuid: &str) -> Result<Option<i64>, CmdErr> {
    let mut max = cfg.f64("main.maxfilesize").unwrap_or(0.0);
    if job.admin {
        max = (max * cfg.f64("main.adminmultiplier").unwrap_or(1.0)).floor();
    }
    let mut source = download(cfg, job, job_id, uuid, max).await?;
    set_progress(job_id, json!({ "stage": "analyzing", "percent": 100, "speed": null, "eta": null }));
    let size = tokio::fs::metadata(&source).await?.len() as f64;
    if size > max {
        let _ = tokio::fs::remove_file(&source).await;
        return Err(CmdErr::plain(format!("File too large. Max: {}, Got: {}", crate::util::format_size_f(max), crate::util::format_size_f(size))));
    }
    let mut mime_t = mime::detect(&source).await?;
    if let Some(expected) = mime::ext_for(cfg, &mime_t) {
        let cur = source.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let jpg_pair = (cur == "jpg" && expected == "jpeg") || (cur == "jpeg" && expected == "jpg");
        if cur == "unknown_video" || (cur != expected && !jpg_pair) {
            let n = with_ext(&source, &expected);
            tokio::fs::rename(&source, &n).await?;
            source = n;
        }
    }
    if mime_t == "video/x-matroska" {
        let out = with_ext(&source, "mp4");
        run("ffmpeg", &["-i", &source.to_string_lossy(), "-codec", "copy", &out.to_string_lossy()], Duration::from_secs(600)).await?;
        let _ = tokio::fs::remove_file(&source).await;
        source = out;
        mime_t = "video/mp4".into();
    }
    if mime::needs_transcode(&mime_t) {
        let out = source.with_file_name(format!("{uuid}.conv.mp4"));
        media::transcode::transcode_to_mp4(&source, &out).await?;
        let _ = tokio::fs::remove_file(&source).await;
        source = out;
        mime_t = "video/mp4".into();
    }
    if source.extension().is_some_and(|e| e == "opus") {
        let out = with_ext(&source, "ogg");
        run("ffmpeg", &["-i", &source.to_string_lossy(), "-codec", "copy", &out.to_string_lossy()], Duration::from_secs(600)).await?;
        let _ = tokio::fs::remove_file(&source).await;
        source = out;
        mime_t = "audio/ogg".into();
    }
    if !cfg.mimes().contains_key(&mime_t) {
        let _ = tokio::fs::remove_file(&source).await;
        return Err(CmdErr::plain(format!("Unsupported file type: {mime_t}")));
    }

    let checksum = queue::sha256_file(&source).await?;
    let bypass = st.settings.bypass_duplicate_check();
    let duplicate = |item: i64, msg: &'static str| async move {
        set_progress(job_id, json!({ "stage": "error", "done": true, "error": msg, "itemId": item }));
        cleanup_job(job_id);
        notify_user(st, job.user_id, "upload_error", Some(item), json!({ "jobId": job_id, "url": job.url, "msg": msg })).await;
    };
    if !bypass {
        if let Some(rep) = queue::check_repost_sum(st, &checksum).await? {
            let _ = tokio::fs::remove_file(&source).await;
            duplicate(rep, "This file already exists").await;
            return Ok(None);
        }
    }
    let phash = queue::generate_phash(&source).await;
    if let (Some(h), false) = (&phash, bypass) {
        match queue::check_repost_phash(st, Some(h)).await {
            Ok(Some(rep)) => {
                let _ = tokio::fs::remove_file(&source).await;
                duplicate(rep, "This file is a visual duplicate").await;
                return Ok(None);
            }
            Ok(None) => {}
            Err(e) => tracing::error!("[UPLOAD-URL-ASYNC] PHash error: {e}"),
        }
    }

    let filename = source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let manual = st.settings.manual_approval();
    let dest_dir = if manual { cfg.paths.pending.join("b") } else { cfg.paths.b.clone() };
    tokio::fs::create_dir_all(&dest_dir).await?;
    let dest_path = dest_dir.join(&filename);
    let mut linked = false;
    if bypass {
        let like = format!("{}_bypass_%", crate::util::escape_like(&checksum));
        if let Some(r) = sql!("SELECT dest FROM items WHERE checksum = ", checksum.as_str(), " OR checksum LIKE ", like, " ORDER BY id DESC LIMIT 1").fetch_optional(&st.db).await? {
            let d = r.string("dest");
            if media::is_plain_name(&d) {
                if let Ok(real) = tokio::fs::canonicalize(cfg.paths.b.join(&d)).await {
                    linked = tokio::fs::symlink(&real, &dest_path).await.is_ok();
                }
            }
        }
    }
    set_progress(job_id, json!({ "stage": "saving", "percent": 100 }));
    if !linked {
        tokio::fs::copy(&source, &dest_path).await?;
        media::make_public(&dest_path).await;
    }
    let _ = tokio::fs::remove_file(&source).await;

    let insert_sum = if bypass { format!("{checksum}_bypass_{}", crate::util::now_ms()) } else { checksum };
    let item_id = insert_item(st, cfg, job, &job.url, &filename, &mime_t, size as i64, &insert_sum, phash, manual).await?;
    let t = queue::gen_thumbnail(cfg, &filename, &mime_t, item_id, &job.url, manual, 512).await;
    if mime_t.starts_with("audio/") && t.cover_extracted {
        let _ = sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", item_id as i32).execute(&st.db).await;
    }
    queue::gen_blurred_thumbnail(cfg, item_id, manual).await;
    finish_item(st, cfg, job, item_id, &filename, &mime_t, manual).await;
    if !manual && job.visibility == 0 {
        st.bot.notify_matrix(cfg, &format!("{} uploaded a new item {}", job.owner, crate::bot::item_url(st, item_id).await)).await;
    }
    Ok(Some(item_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_errors() {
        let e = CmdErr { msg: "Command 'yt-dlp' failed with code 1".into(), stderr: Some("WARNING: x\nERROR: [generic] Unsupported URL: y\n".into()) };
        assert_eq!(sanitize_error(&e, "u"), "[generic] Unsupported URL: y");
        let e = CmdErr { msg: "Command 'curl' failed with code 22".into(), stderr: Some("curl: (22) The requested URL returned error: 404".into()) };
        assert_eq!(sanitize_error(&e, "u"), "curl: (22) The requested URL returned error: 404");
        let e = CmdErr::plain("Command 'ffmpeg' failed with code 1");
        assert_eq!(sanitize_error(&e, "u"), "Process failed (code 1)");
        let e = CmdErr::plain("HTTP Error 403: Forbidden");
        assert_eq!(sanitize_error(&e, "u"), "Download/Process failed (HTTP 403)");
    }

    #[test]
    fn progress_lines() {
        let p = parse_ytdlp_line("[download]  47.3% of ~  58.23MiB at    3.22MiB/s ETA 00:13").unwrap();
        assert_eq!(p["percent"], json!(47.3));
        assert_eq!(p["speed"], "3.22MiB/s");
        assert_eq!(p["eta"], "00:13");
        assert_eq!(parse_ytdlp_line("[Merger] Merging").unwrap()["percent"], json!(99));
        assert!(parse_ytdlp_line("random").is_none());
    }

    #[test]
    fn youtube_tag() {
        assert_eq!(auto_tags_from_url("https://www.youtube.com/watch?v=x"), vec!["youtube"]);
        assert!(auto_tags_from_url("https://example.com/a").is_empty());
    }
}
