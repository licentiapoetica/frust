//! Link/media posting from chat (`src/inc/trigger/parser.mjs`): finds links (or Telegram media,
//! or a replied-to Matrix/Telegram file), downloads them with yt-dlp/curl/the Matrix media API,
//! checks size/mime/reposts and adds the item to the site.
//!
//! Flow per platform (unchanged):
//! * IRC/Slack: any link in a channel whose name contains `main.upload_channel_keyword` (`w0bm`),
//!   or anywhere with the trigger (`!w`).
//! * Telegram/Matrix: `!w -sfw|-nsfw -t tag1, tag2` (min tags from settings), account must be linked
//!   (`!link`); Matrix only in `upload_channel_id` if set; Telegram also forwarded messages and the
//!   rest of a media group started with `!w`.

use super::events::Runtime;
use super::{Client, Kind, Message, get_level, pipeline};
use crate::config::Config;
use crate::db::RowExt;
use crate::sql;
use anyhow::Result;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ALL: &str = r"https?://([A-Za-z0-9_-]+(?:(?:\.[A-Za-z0-9_-]+)+))([A-Za-z0-9_.,@?^=%&:/~+#-]*[A-Za-z0-9_@?^=%&/~+#-])?";
static RE_ALL: Lazy<Regex> = Lazy::new(|| Regex::new(&format!("(?i){ALL}")).unwrap());
static RE_YT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:youtube\.com/\S*(?:(?:/e(?:mbed))?/|watch/?\?(?:\S*?&?v=))|youtu\.be/)([a-zA-Z0-9_-]{6,11})").unwrap()
});
static RE_IMGUR: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:https?:)?//([A-Za-z0-9_]+\.)?imgur\.com/\S+").unwrap());
static RE_4CHAN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)https?://i\.4cdn\.org/([A-Za-z0-9_]+)/([0-9]+)\.([A-Za-z0-9_]{3,4})").unwrap());
static RE_INSTA: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?im)(?:https?://www\.)?instagram\.com\S*?/(?:p|reel)/([A-Za-z0-9_]{11})/?").unwrap());
static RE_PH: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:https?://)?(?:[A-Za-z0-9_]+\.)?pornhub\.(?:com|org)/view_video\.php\?viewkey=([A-Za-z0-9_-]+)").unwrap());
static RE_IGNORE: Lazy<Regex> = Lazy::new(|| Regex::new(r"!i(gnore)?\b").unwrap());
static RE_SFW: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|\s)-sfw\b").unwrap());
static RE_NSFW: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|\s)-nsfw\b").unwrap());
static RE_TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)-t\s+(\S.+)").unwrap());
static RE_FLAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*-(nsfw|sfw)\b").unwrap());
static RE_T_FLAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*-t\b").unwrap());

const PC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36";
const FMT: &str = "bv*[height<=1080]+ba/b[height<=1080] / wv*+ba/w";
const FMT_IMGUR: &str = "bv*[height<=1080]+ba/b[height<=1080] / b[height<=1080]";
const TG_MEDIA_KEYS: [&str; 7] = ["video", "audio", "photo", "document", "animation", "voice", "video_note"];

/// Media groups (Telegram albums) whose first message carried `!w`.
static MEDIA_GROUPS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn trigger_word(cfg: &Config) -> String {
    cfg.str_or("main.trigger", "!w").to_string()
}

/// Trigger pattern: any link, or a message starting with the trigger word.
pub fn pattern(cfg: &Config) -> String {
    format!(r"(?i){ALL}|^{}(0bm)?\b", regex::escape(&trigger_word(cfg)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Site {
    Mxc,
    Pornhub,
    Instagram,
    Imgur,
    YouTube,
    FourChan,
    Generic,
}

pub fn site(link: &str) -> Site {
    if link.starts_with("mxc://") {
        Site::Mxc
    } else if RE_PH.is_match(link) {
        Site::Pornhub
    } else if RE_INSTA.is_match(link) {
        Site::Instagram
    } else if RE_IMGUR.is_match(link) {
        Site::Imgur
    } else if RE_YT.is_match(link) {
        Site::YouTube
    } else if RE_4CHAN.is_match(link) {
        Site::FourChan
    } else {
        Site::Generic
    }
}

/// Links in a text, excluding links to this site.
pub fn extract_links(text: &str, domain: &str) -> Vec<String> {
    RE_ALL.find_iter(text).map(|m| m.as_str().to_string()).filter(|l| domain.is_empty() || !l.contains(domain)).collect()
}

/// Result of parsing `!w -sfw|-nsfw -t tags`.
#[derive(Debug, PartialEq, Eq)]
pub enum WArgs {
    Ok { nsfw: bool, tags: Vec<String> },
    Help,
    BothRatings,
    MissingRating,
    TooFewTags(usize),
}

pub fn parse_w_args(message: &str, trigger: &str, min_tags: usize) -> WArgs {
    let has_sfw = RE_SFW.is_match(message);
    let has_nsfw = RE_NSFW.is_match(message);
    let tag_match = RE_TAGS.captures(message);
    if !has_sfw && !has_nsfw && tag_match.is_none() {
        return WArgs::Help;
    }
    if has_sfw && has_nsfw {
        return WArgs::BothRatings;
    }
    if !has_sfw && !has_nsfw {
        return WArgs::MissingRating;
    }
    let input = match tag_match {
        Some(c) => c[1].to_string(),
        None => {
            let trig = Regex::new(&format!(r"(?i){}(0bm)?\b", regex::escape(trigger))).ok();
            let s = RE_ALL.replace_all(message, "");
            let s = match trig {
                Some(t) => t.replace(&s, "").into_owned(),
                None => s.into_owned(),
            };
            let s = RE_FLAGS.replace_all(&s, "");
            RE_T_FLAG.replace_all(&s, "").trim().to_string()
        }
    };
    let mut tags: Vec<String> = Vec::new();
    for t in input.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !tags.iter().any(|x| x == t) {
            tags.push(t.to_string());
        }
    }
    if tags.len() < min_tags {
        return WArgs::TooFewTags(tags.len());
    }
    WArgs::Ok { nsfw: has_nsfw, tags }
}

fn yt_dlp_base(cfg: &Config) -> Vec<String> {
    let runtime = cfg.str_or("main.ytdlp_js_runtime", "node");
    ["--js-runtimes", runtime, "--geo-bypass", "--extractor-args", "youtube:player-client=ios,web"].iter().map(|s| s.to_string()).collect()
}

fn socks(cfg: &Config) -> Option<String> {
    cfg.str("main.socks").filter(|s| !s.is_empty() && *s != "undefined").map(str::to_string)
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// yt-dlp arguments for the metadata probe (`--dump-json`).
pub fn meta_args(cfg: &Config, s: Site, link: &str) -> Vec<String> {
    let proxy: Vec<String> = socks(cfg).map(|p| vec!["--proxy".into(), p]).unwrap_or_default();
    let base = yt_dlp_base(cfg);
    let mut a: Vec<String> = match s {
        Site::Pornhub => [base, strings(&["--referer", "https://www.pornhub.com", "--no-progress", "--no-warnings", "--user-agent", PC_UA])].concat(),
        Site::Instagram | Site::FourChan => [proxy, base, strings(&["-f", FMT])].concat(),
        Site::Imgur => [proxy, base].concat(),
        Site::YouTube => [proxy, base, strings(&["-f", FMT, "-I", "1"])].concat(),
        Site::Generic | Site::Mxc => [base, strings(&["-f", FMT])].concat(),
    };
    a.extend(strings(&["--skip-download", "--dump-json", "--", link]));
    a
}

/// yt-dlp arguments for the download.
pub fn download_args(cfg: &Config, s: Site, link: &str, max_bytes: f64, out_template: &str) -> Vec<String> {
    let proxy: Vec<String> = socks(cfg).map(|p| vec!["--proxy".into(), p]).unwrap_or_default();
    let base = yt_dlp_base(cfg);
    let mut a: Vec<String> = match s {
        Site::Pornhub => [proxy, strings(&["--no-playlist", "--referer", "https://www.pornhub.com", "--user-agent", PC_UA, "-f", FMT])].concat(),
        Site::Instagram | Site::FourChan => [proxy, base, strings(&["-f", FMT])].concat(),
        Site::Imgur => [proxy, base, strings(&["-f", FMT_IMGUR])].concat(),
        Site::YouTube => [proxy, base, strings(&["-f", FMT, "-I", "1"])].concat(),
        Site::Generic | Site::Mxc => [base, strings(&["-f", FMT])].concat(),
    };
    a.extend([
        "--max-filesize".to_string(),
        format!("{}k", crate::util::js_num(max_bytes / 1024.0)),
        "--postprocessor-args".into(),
        "ffmpeg:-bitexact".into(),
        "-o".into(),
        out_template.to_string(),
        "--print".into(),
        "after_move:filepath".into(),
        "--merge-output-format".into(),
        "mp4".into(),
        "--".into(),
        link.to_string(),
    ]);
    a
}

/// First JSON line of yt-dlp output.
pub fn extract_json(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('{') || l.starts_with('['))
        .find_map(|l| serde_json::from_str(l).ok())
        .or_else(|| serde_json::from_str(stdout).ok())
}

fn referer_for(link: &str, force_host: Option<&str>) -> String {
    match url::Url::parse(link) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("");
            let host = force_host.map(str::to_string).unwrap_or_else(|| {
                let parts: Vec<&str> = host.split('.').collect();
                if parts.len() >= 2 { parts[parts.len() - 2..].join(".") } else { host.to_string() }
            });
            format!("{}://{host}/", u.scheme())
        }
        Err(_) => link.to_string(),
    }
}

fn socks_host(cfg: &Config) -> Option<String> {
    socks(cfg).map(|s| s.split_once("://").map(|(_, h)| h.to_string()).unwrap_or(s))
}

/// `getMimeViaCurl(link)`: extension from the `Content-Type` of a HEAD request.
async fn ext_via_curl(cfg: &Config, link: &str) -> Option<String> {
    let referer = referer_for(link, None);
    let mut args = vec!["-I", "-s", "-f", "-L", "--max-time", "60", "--user-agent", PC_UA, "--referer", &referer];
    let proxy = socks_host(cfg);
    if let Some(p) = &proxy {
        args.extend(["--socks5-hostname", p.as_str()]);
    }
    args.push(link);
    let out = crate::proc::spawn("curl", &args).await.ok()?.stdout_str();
    let ct = out.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case("content-type").then(|| v.trim().to_string())
    })?;
    let mime = ct.split(';').next().unwrap_or("").trim().to_lowercase();
    cfg.mimes().get(&mime).and_then(|v| v.as_str()).map(str::to_string)
}

async fn file_mime(path: &Path) -> Result<String> {
    Ok(crate::proc::spawn("file", &["--mime-type", "-b", &path.to_string_lossy()]).await?.stdout_str().trim().to_string())
}

/// Message shown while downloading; on Telegram it is edited with the outcome.
struct Progress<'a> {
    e: &'a Message,
    sent: Option<Value>,
}

impl Progress<'_> {
    async fn say(&self, text: &str) {
        match (&self.e.kind, &self.sent) {
            (Kind::Tg, Some(m)) if m["message_id"].is_i64() => self.e.edit_message_text(&m["chat"]["id"], m["message_id"].as_i64().unwrap_or(0), text, json!({})).await,
            _ => {
                self.e.reply(text).await;
            }
        }
    }
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

pub async fn run(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let st = &rt.st;
    let cfg = st.cfg();
    let trigger = trigger_word(&cfg);
    let full = cfg.url_full().to_string();
    let mut links = extract_links(&e.message, cfg.domain());
    if let Some(m) = &e.media {
        links.push(m.clone());
    }

    let upload_room = cfg.client("matrix").and_then(|c| c["upload_channel_id"].as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    if e.kind == Kind::Matrix && upload_room.as_ref().is_some_and(|r| *r != e.channelid) {
        return Ok(());
    }

    if links.is_empty() {
        if let (Kind::Matrix, Some(r)) = (e.kind, &e.reply_to) {
            let in_upload_room = upload_room.as_ref().is_some_and(|r| *r == e.channelid);
            let level = get_level(&cfg, &e.user).0;
            if e.user.account.as_deref() != Some(r.sender.as_str()) && !in_upload_room && level <= 10 {
                e.reply("You can only !w your own messages in this room.").await;
                return Ok(());
            }
            if let Some(m) = &r.mxc_url {
                links.push(m.clone());
            } else if let Some(u) = &r.url {
                links.push(u.clone());
            } else if let Some(text) = &r.message {
                links.extend(extract_links(text, cfg.domain()));
            }
        }
        if let (Kind::Tg, Some(replied), Client::Tg(tg)) = (e.kind, e.raw.get("reply_to_message").cloned(), &e.client) {
            let key = replied.as_object().and_then(|o| o.keys().find(|k| TG_MEDIA_KEYS.contains(&k.as_str())).cloned());
            if let Some(key) = key {
                let media = if key == "photo" { replied[&key].as_array().and_then(|a| a.last()).cloned().unwrap_or_default() } else { replied[&key].clone() };
                match media["file_id"].as_str() {
                    Some(fid) => match tg.get_file(fid).await {
                        Some(url) => links.push(url),
                        None => {
                            e.reply("Could not resolve the replied-to file. It may be too large (>20MB) for the Telegram Bot API.").await;
                            return Ok(());
                        }
                    },
                    None => {
                        e.reply("Failed to fetch the replied-to media file.").await;
                        return Ok(());
                    }
                }
            } else if let Some(text) = replied["text"].as_str().or(replied["caption"].as_str()) {
                links.extend(extract_links(text, cfg.domain()));
            }
        }
    }
    if links.is_empty() || RE_IGNORE.is_match(&e.message) {
        return Ok(());
    }

    let is_w = rt.regex(&format!(r"(?i){}(0bm)?\b", regex::escape(&trigger))).is_some_and(|r| r.is_match(&e.message));
    let min_tags = st.settings.min_tags().max(0) as usize;
    let mut rating: Option<bool> = None; // Some(nsfw)
    let mut tags: Vec<String> = vec![];
    if is_w && matches!(e.kind, Kind::Matrix | Kind::Tg) {
        match parse_w_args(&e.message, &trigger, min_tags) {
            WArgs::Ok { nsfw, tags: t } => {
                rating = Some(nsfw);
                tags = t;
            }
            WArgs::Help => {
                e.reply(&format!("{trigger} -sfw/-nsfw -t tag1,tag2,tag3 (at least {min_tags} tags required)")).await;
                return Ok(());
            }
            WArgs::BothRatings => {
                e.reply("pick one: -sfw or -nsfw, not both").await;
                return Ok(());
            }
            WArgs::MissingRating => {
                e.reply(&format!("missing rating: {trigger} -sfw/-nsfw -t tag1,tag2,tag3")).await;
                return Ok(());
            }
            WArgs::TooFewTags(n) => {
                e.reply(&format!("at least {min_tags} tags required (got {n}): {trigger} -sfw/-nsfw -t tag1, tag2, tag3")).await;
                return Ok(());
            }
        }
    }

    let forwarded = e.raw.get("forward_origin").is_some() || e.raw.get("forward_from").is_some() || e.raw.get("forward_date").is_some();
    let media_group = e.raw["media_group_id"].as_str().map(str::to_string);
    let keyword = cfg.str_or("main.upload_channel_keyword", "w0bm").to_lowercase();
    if (e.kind == Kind::Matrix || !e.channel.to_lowercase().contains(&keyword)) && !is_w && !forwarded {
        return Ok(());
    }

    // linked website account (required for !w on Matrix/Telegram)
    let mut website_user: Option<(i64, String)> = None;
    if matches!(e.kind, Kind::Matrix | Kind::Tg) {
        let (alias, kind) = match e.kind {
            Kind::Matrix => (e.user.account.clone().unwrap_or_default(), "matrix"),
            _ => (e.user.account.clone().unwrap_or_default(), "telegram"),
        };
        match sql!(
            r#"SELECT "user".id, "user"."user" FROM user_alias JOIN "user" ON "user".id = user_alias.userid
               WHERE lower(user_alias.alias) = lower("#,
            alias,
            ") AND user_alias.type = ",
            kind,
            " LIMIT 1"
        )
        .fetch_optional(&st.db)
        .await
        {
            Ok(Some(r)) => website_user = r.i64("id").map(|id| (id, r.string("user"))),
            Ok(None) => {}
            Err(err) => tracing::error!("[PARSER] DB Link check error: {err:#}"),
        }
        if is_w && website_user.is_none() {
            let lt = if e.kind == Kind::Tg { "Telegram" } else { "matrix" };
            e.reply(&format!(
                "You must link your account to use this command. Go to {full}/settings to link your {lt} account. Then send !link <TOKEN> to this bot."
            ))
            .await;
            return Ok(());
        }
    }

    if e.kind == Kind::Tg {
        let group_known = media_group.as_ref().is_some_and(|g| MEDIA_GROUPS.lock().contains(g));
        if !is_w && !forwarded && !group_known {
            return Ok(());
        }
        if let (Some(g), true) = (&media_group, is_w) {
            let mut groups = MEDIA_GROUPS.lock();
            if groups.len() > 1000 {
                groups.clear();
            }
            groups.insert(g.clone());
        }
    }

    let ctx = Upload { rt: rt.clone(), cfg: cfg.clone(), website_user, rating, tags };
    for link in links {
        if !ctx.process(&e, &link).await? {
            break;
        }
    }
    Ok(())
}

struct Upload {
    rt: Arc<Runtime>,
    cfg: Arc<Config>,
    website_user: Option<(i64, String)>,
    rating: Option<bool>,
    tags: Vec<String>,
}

impl Upload {
    /// Handle one link. `Ok(false)` stops processing the remaining links (like the old `return`).
    async fn process(&self, e: &Message, link: &str) -> Result<bool> {
        let st = &self.rt.st;
        let cfg = &self.cfg;
        let full = cfg.url_full();
        let shown = super::telegram::sanitize_url(link);
        let s = site(link);

        if s != Site::Mxc {
            if let Some(id) = pipeline::check_repost_link(st, &shown).await.unwrap_or(None) {
                e.reply(&format!("repost motherf0cker (link): {full}/{id}")).await;
                return Ok(true);
            }
        }
        let uuid = pipeline::gen_uuid();
        let level = get_level(cfg, &e.user).0;
        let base_max = cfg.f64("main.maxfilesize").unwrap_or(0.0);
        let max_bytes = if level > 50 { base_max * cfg.f64("main.adminmultiplier").unwrap_or(1.0) } else { base_max };

        // extension probe
        let ext = match s {
            Site::Mxc => {
                let r = e.reply_to.as_ref();
                let mime = r.and_then(|r| r.mimetype.clone());
                match mime.and_then(|m| cfg.mimes().get(&m).and_then(|v| v.as_str()).map(str::to_string)) {
                    Some(x) => Some(x),
                    None => {
                        let fname = r.and_then(|r| r.filename.clone()).unwrap_or_default();
                        let fext = fname.rsplit_once('.').map(|(_, x)| x.to_lowercase()).unwrap_or_default();
                        Some(if !fext.is_empty() && cfg.mimes().values().any(|v| v.as_str() == Some(fext.as_str())) { fext } else { "bin".into() })
                    }
                }
            }
            _ => {
                let args = meta_args(cfg, s, link);
                let argv: Vec<&str> = args.iter().map(String::as_str).collect();
                let probed = match crate::proc::spawn_with("yt-dlp", &argv, crate::proc::Opts { quiet: true, timeout: std::time::Duration::from_secs(180), ..Default::default() }).await {
                    Ok(out) => extract_json(&out.stdout_str()).and_then(|m| m["ext"].as_str().map(str::to_string)),
                    Err(err) => {
                        tracing::warn!("[METADATA ERROR] yt-dlp failed for {shown}: {}", super::telegram::sanitize_url(&err.to_string()));
                        None
                    }
                };
                match probed {
                    Some(x) => Some(x),
                    None => ext_via_curl(cfg, link).await,
                }
            }
        };
        let ext_ok = ext.as_deref().is_some_and(|x| cfg.mimes().values().any(|v| v.as_str() == Some(x.to_lowercase().as_str())));
        if s != Site::Mxc && !ext_ok {
            // fix: the old message echoed the raw link — for Telegram files that includes the bot token
            let msg = trunc(&format!("lol, go f0ck yourself (mime schmime: {} for {shown})", ext.as_deref().unwrap_or("undefined")), 1024);
            e.reply(&msg).await;
            return Ok(false);
        }

        let progress = Progress { e, sent: e.reply_opts("[charging my lazor] downloading", json!({ "disable_notification": true })).await };
        let start = std::time::Instant::now();
        let source = match self.download(e, s, link, &uuid, ext.as_deref(), max_bytes).await {
            Ok(p) => p,
            Err(msg) => {
                progress.say(&msg).await;
                return Ok(false);
            }
        };
        let secs = start.elapsed().as_secs();
        match self.finish(e, &progress, link, source.clone(), &uuid, max_bytes, secs).await {
            Ok(cont) => Ok(cont),
            Err(err) => {
                let _ = tokio::fs::remove_file(&source).await;
                tracing::error!("[PARSER] {shown}: {err:#}");
                progress.say("something went wrong lol").await;
                Ok(false)
            }
        }
    }

    /// Download to `tmp/`; `Err` is the message for the user.
    async fn download(&self, e: &Message, s: Site, link: &str, uuid: &str, ext: Option<&str>, max_bytes: f64) -> std::result::Result<PathBuf, String> {
        let cfg = &self.cfg;
        let tmp = cfg.paths.tmp.clone();
        let fail = "something went wrong lol".to_string();
        if s == Site::Mxc {
            let Client::Matrix(m) = &e.client else { return Err(fail) };
            let bytes = m.download(link, max_bytes as u64).await.map_err(|err| format!("Matrix download failed: {err}"))?;
            let ext = ext.filter(|x| *x != "bin" && x.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or("bin");
            let dest = tmp.join(format!("{uuid}.{ext}"));
            tokio::fs::write(&dest, bytes).await.map_err(|_| fail.clone())?;
            return Ok(dest);
        }
        let template = tmp.join(format!("{uuid}.%(ext)s")).to_string_lossy().to_string();
        let args = download_args(cfg, s, link, max_bytes, &template);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        let res = crate::proc::spawn_with("yt-dlp", &argv, crate::proc::Opts { quiet: true, timeout: std::time::Duration::from_secs(1800), ..Default::default() }).await;
        let path = match res {
            Ok(out) => out.stdout_str().lines().map(str::trim).filter(|l| !l.is_empty()).last().map(str::to_string),
            Err(err) => {
                tracing::warn!("[PARSER] yt-dlp download failed for {}: {}", super::telegram::sanitize_url(link), super::telegram::sanitize_url(&err.to_string()));
                if s == Site::Imgur {
                    return self.curl_fallback(link, uuid, max_bytes).await.ok_or(fail);
                }
                return Err(fail);
            }
        };
        let Some(path) = path else { return Err(fail) };
        if path.contains("larger than") {
            return Err("too large lol".into());
        }
        // only accept files yt-dlp wrote into our tmp dir
        let p = PathBuf::from(&path);
        let canon_tmp = tokio::fs::canonicalize(&tmp).await.unwrap_or(tmp.clone());
        match tokio::fs::canonicalize(&p).await {
            Ok(c) if c.starts_with(&canon_tmp) => Ok(c),
            _ => Err(fail),
        }
    }

    /// Imgur: plain curl download when yt-dlp fails.
    async fn curl_fallback(&self, link: &str, uuid: &str, max_bytes: f64) -> Option<PathBuf> {
        let cfg = &self.cfg;
        let tmpfile = cfg.paths.tmp.join(format!("{uuid}.tmp"));
        let out = tmpfile.to_string_lossy().to_string();
        let referer = referer_for(link, Some("imgur.com"));
        let max = format!("{}", max_bytes as u64);
        let mut args = vec![
            "-s", "-f", "-L", "-o", &out, "--max-filesize", &max, "--connect-timeout", "30", "--max-time", "300", "--user-agent", PC_UA, "--referer", &referer,
            "-H", "Accept: text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8", "-H", "Accept-Language: en-US,en;q=0.9",
        ];
        let proxy = socks_host(cfg);
        if let Some(p) = &proxy {
            args.extend(["--socks5-hostname", p.as_str()]);
        }
        args.push(link);
        if crate::proc::spawn("curl", &args).await.is_err() {
            let _ = tokio::fs::remove_file(&tmpfile).await;
            return None;
        }
        let mime = file_mime(&tmpfile).await.ok()?;
        match cfg.mimes().get(&mime).and_then(|v| v.as_str()) {
            Some(ext) => {
                let dest = cfg.paths.tmp.join(format!("{uuid}.{ext}"));
                tokio::fs::rename(&tmpfile, &dest).await.ok()?;
                Some(dest)
            }
            None => {
                tracing::error!("[PARSER] Imgur fallback downloaded unsupported MIME type: {mime:?}");
                let _ = tokio::fs::remove_file(&tmpfile).await;
                None
            }
        }
    }

    /// Checks, conversion, repost detection, insert. `Err` → generic failure message.
    #[allow(clippy::too_many_arguments)]
    async fn finish(&self, e: &Message, progress: &Progress<'_>, link: &str, mut source: PathBuf, uuid: &str, max_bytes: f64, secs: u64) -> Result<bool> {
        let st = &self.rt.st;
        let cfg = &self.cfg;
        let full = cfg.url_full();
        let size = tokio::fs::metadata(&source).await?.len();
        if size as f64 > max_bytes {
            let _ = tokio::fs::remove_file(&source).await;
            progress.say(&format!("too large lol. ({} / {})", crate::util::format_size_f(size as f64), crate::util::format_size_f(max_bytes))).await;
            return Ok(false);
        }

        let mut mime = file_mime(&source).await?;
        let tmp = &cfg.paths.tmp;
        if mime == "video/x-matroska" {
            let out = tmp.join(format!("{uuid}.remux.mp4"));
            crate::proc::spawn("ffmpeg", &["-y", "-i", &source.to_string_lossy(), "-codec", "copy", &out.to_string_lossy()]).await?;
            let _ = tokio::fs::remove_file(&source).await;
            source = out;
            mime = "video/mp4".into();
        }
        if pipeline::needs_transcode(&mime) {
            let out = tmp.join(format!("{uuid}.conv.mp4"));
            pipeline::transcode_to_mp4(&source, &out).await?;
            let _ = tokio::fs::remove_file(&source).await;
            source = out;
            mime = "video/mp4".into();
        }
        if source.extension().is_some_and(|x| x == "opus") {
            let out = source.with_extension("ogg");
            crate::proc::spawn("ffmpeg", &["-y", "-i", &source.to_string_lossy(), "-codec", "copy", &out.to_string_lossy()]).await?;
            let _ = tokio::fs::remove_file(&source).await;
            source = out;
            mime = "audio/ogg".into();
        }

        if !cfg.mimes().contains_key(&mime) {
            let _ = tokio::fs::remove_file(&source).await;
            progress.say(&format!("lol, go f0ck yourself ({mime})")).await;
            return Ok(false);
        }
        let category = mime.split('/').next().unwrap_or("");
        let allowed = cfg.str_list("allowedMimes").unwrap_or_default();
        if !allowed.iter().any(|a| a == &mime || a == category) {
            let _ = tokio::fs::remove_file(&source).await;
            progress.say(&format!("lol, go f0ck yourself (blocked type: {mime})")).await;
            return Ok(false);
        }

        let checksum = pipeline::sha256_file(&source).await?;
        let mut repost = pipeline::check_repost_sum(st, &checksum).await?;
        let mut phash = None;
        if repost.is_none() {
            phash = pipeline::generate_phash(st, &source).await;
            if let Some(h) = &phash {
                repost = pipeline::check_repost_phash(st, h).await.unwrap_or(None);
            }
        }
        if let Some(id) = repost {
            let _ = tokio::fs::remove_file(&source).await;
            progress.say(&format!("repost motherf0cker (checksum): {full}/{id}")).await;
            return Ok(false);
        }

        let filename = source.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let manual = st.settings.manual_approval();
        let dest_dir = if manual { cfg.paths.pending.join("b") } else { cfg.paths.b.clone() };
        tokio::fs::copy(&source, dest_dir.join(&filename)).await?;
        let _ = tokio::fs::remove_file(&source).await;

        let username = match &self.website_user {
            Some((_, u)) => u.clone(),
            None if e.kind == Kind::Tg => e.user.username.clone().filter(|u| !u.is_empty()).unwrap_or_else(|| e.user.nick.clone()),
            None => if e.user.nick.is_empty() { e.user.username.clone().unwrap_or_default() } else { e.user.nick.clone() },
        };
        let src = if e.media.is_some() || link.contains("api.telegram.org") || link.starts_with("mxc://") { String::new() } else { trunc(link, 255) };
        let item_id: i64 = sql!(
            "INSERT INTO items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, slug) VALUES (",
            src, ", ", filename.clone(), ", ", mime.clone(), ", ", size as i64 as i32, ", ", checksum, ", ", phash, ", ",
            trunc(&username, 40), ", ", trunc(&e.channel, 255), ", ", trunc(&e.network, 40), ", ", crate::util::now_secs() as i32, ", ",
            !manual, ", ", crate::util::generate_slug(11), ") RETURNING id"
        )
        .fetch_scalar::<i32, _>(&st.db)
        .await?
        .unwrap_or(0) as i64;

        if cfg.enabled("enable_comments") {
            if let Some((uid, _)) = &self.website_user {
                let _ = sql!("INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", *uid as i32, ", ", item_id as i32, ") ON CONFLICT DO NOTHING")
                    .execute(&st.db)
                    .await;
            }
        }

        if let Some(nsfw) = self.rating {
            if let Err(err) = self.assign_tags(item_id, nsfw).await {
                tracing::error!("[PARSER] Failed to assign tags: {err:#}");
            }
        }

        let thumb = async {
            pipeline::gen_thumbnail(st, &filename, &mime, item_id, link, manual).await?;
            pipeline::gen_blurred_thumbnail(st, item_id, manual).await
        };
        if thumb.await.is_err() {
            if let Err(err) = pipeline::placeholder_thumbnail(st, item_id, manual).await {
                tracing::error!("[PARSER] placeholder thumbnail failed for {item_id}: {err:#}");
            }
        }

        if manual {
            pipeline::notify_admins(st, item_id).await;
        } else {
            st.bot.notify_matrix(cfg, &format!("{username} uploaded a new video {full}/{item_id}")).await;
        }

        let speed = crate::util::calc_speed(size as f64, secs as f64);
        let speed = if speed.is_finite() { format!("{speed:.2} Mbit/s") } else { "yes".into() };
        let out = format!(
            "{}size: {} | speed: {speed} | link: {full}/{item_id}",
            if manual { "[approval pending] " } else { "" },
            crate::util::format_size_f(size as f64)
        );
        if let (Kind::Tg, Some(m)) = (e.kind, &progress.sent) {
            e.delete_message(&m["chat"]["id"], m["message_id"].as_i64().unwrap_or(0)).await;
        }
        e.reply(&out).await;
        Ok(true)
    }

    /// Rating tag (sfw = 1, nsfw = 2) + user tags, attributed to the linked website user.
    async fn assign_tags(&self, item_id: i64, nsfw: bool) -> Result<()> {
        let db = &self.rt.st.db;
        let user_id = self.website_user.as_ref().map(|(id, _)| *id as i32).unwrap_or(1);
        let item = item_id as i32;
        let rating_tag: i32 = if nsfw { 2 } else { 1 };
        sql!("INSERT INTO tags_assign (tag_id, item_id, user_id) VALUES (", rating_tag, ", ", item, ", ", user_id, ")").execute(db).await?;
        for name in &self.tags {
            let name = trunc(name, 255);
            let id = match sql!("SELECT id FROM tags WHERE tag = ", name.clone(), " LIMIT 1").fetch_scalar::<i32, _>(db).await? {
                Some(id) => id,
                None => sql!("INSERT INTO tags (tag) VALUES (", name, ") RETURNING id").fetch_scalar::<i32, _>(db).await?.unwrap_or(0),
            };
            if id > 0 {
                sql!("INSERT INTO tags_assign (tag_id, item_id, user_id) VALUES (", id, ", ", item, ", ", user_id, ")").execute(db).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::tests::test_cfg;

    #[test]
    fn finds_links() {
        let l = extract_links("look https://youtu.be/dQw4w9WgXcQ and https://f0ck.me/123 and http://x.org/a.webm.", "f0ck.me");
        assert_eq!(l, vec!["https://youtu.be/dQw4w9WgXcQ", "http://x.org/a.webm"]);
        assert!(extract_links("no links", "").is_empty());
    }

    #[test]
    fn classifies_sites() {
        assert_eq!(site("mxc://x/y"), Site::Mxc);
        assert_eq!(site("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), Site::YouTube);
        assert_eq!(site("https://youtu.be/dQw4w9WgXcQ"), Site::YouTube);
        assert_eq!(site("https://i.imgur.com/abc.mp4"), Site::Imgur);
        assert_eq!(site("https://i.4cdn.org/wsg/1234.webm"), Site::FourChan);
        assert_eq!(site("https://www.instagram.com/reel/ABCDEFGHIJK/"), Site::Instagram);
        assert_eq!(site("https://www.pornhub.com/view_video.php?viewkey=ph123"), Site::Pornhub);
        assert_eq!(site("https://example.com/v.mp4"), Site::Generic);
    }

    #[test]
    fn w_arguments() {
        assert_eq!(parse_w_args("!w", "!w", 3), WArgs::Help);
        assert_eq!(parse_w_args("!w -sfw -nsfw -t a", "!w", 0), WArgs::BothRatings);
        assert_eq!(parse_w_args("!w -t a,b,c", "!w", 3), WArgs::MissingRating);
        assert_eq!(parse_w_args("!w -sfw -t a, b", "!w", 3), WArgs::TooFewTags(2));
        assert_eq!(parse_w_args("!w -nsfw -t big cat, dog,  dog ,x", "!w", 3), WArgs::Ok { nsfw: true, tags: vec!["big cat".into(), "dog".into(), "x".into()] });
        assert_eq!(parse_w_args("!w https://x.org/v.mp4 -sfw a, b", "!w", 2), WArgs::Ok { nsfw: false, tags: vec!["a".into(), "b".into()] });
    }

    #[test]
    fn builds_ytdlp_args() {
        let cfg = test_cfg(json!({ "main": { "socks": "socks5://127.0.0.1:9050" } }));
        let a = meta_args(&cfg, Site::YouTube, "https://youtu.be/x");
        assert_eq!(&a[..2], &["--proxy", "socks5://127.0.0.1:9050"]);
        assert_eq!(&a[a.len() - 2..], &["--", "https://youtu.be/x"]);
        assert!(a.windows(2).any(|w| w == ["-I", "1"]));
        let a = download_args(&cfg, Site::Generic, "https://e.org/v", 104857600.0 * 3.5, "/tmp/u.%(ext)s");
        assert!(!a.contains(&"--proxy".to_string()));
        assert!(a.windows(2).any(|w| w == ["--max-filesize", "358400k"]));
        assert_eq!(a.last().unwrap(), "https://e.org/v");
        assert_eq!(referer_for("https://i.imgur.com/a.mp4", None), "https://imgur.com/");
        assert_eq!(socks_host(&cfg).as_deref(), Some("127.0.0.1:9050"));
    }

    #[test]
    fn yt_dlp_json() {
        assert_eq!(extract_json("WARNING: x\n{\"ext\":\"mp4\"}\n").unwrap()["ext"], "mp4");
        assert!(extract_json("nope").is_none());
    }

    #[test]
    fn trigger_pattern() {
        let cfg = test_cfg(json!({ "main": { "trigger": "!w" } }));
        let re = Regex::new(&pattern(&cfg)).unwrap();
        assert!(re.is_match("!w -sfw -t a"));
        assert!(re.is_match("!W0bm"));
        assert!(re.is_match("see https://x.org/v.mp4"));
        assert!(!re.is_match("!wat"));
        assert!(!re.is_match("hello"));
    }
}
