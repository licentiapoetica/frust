//! Link/media posting from chat (`src/inc/trigger/parser.mjs`): finds links (or Telegram media,
//! or a replied-to Matrix/Telegram file), downloads them with yt-dlp/curl/the Matrix media API,
//! checks size/mime/reposts and adds the item to the site.
//!
//! Flow per platform:
//! * IRC/Slack: any link in a channel whose name contains `main.upload_channel_keyword` (`w0bm`),
//!   or anywhere with the trigger (`!w`). IRC follows the site's rating/tag rules (tags only after `-t`).
//! * Telegram/Matrix: `!w -sfw|-nsfw|-nsfl -t tag1, tag2` (site rating/tag rules), account must be linked
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
static RE_NSFL: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(?:^|\s)-nsfl\b").unwrap());
static RE_TAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)-t\s+(\S.*)").unwrap());
static RE_FLAGS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*-(nsfw|sfw|nsfl)\b").unwrap());
static RE_T_FLAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*-t\b").unwrap());

const PC_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/121.0.0.0 Safari/537.36";
const FMT: &str = "bv*[height<=1080]+ba/b[height<=1080] / wv*+ba/w";
const FMT_IMGUR: &str = "bv*[height<=1080]+ba/b[height<=1080] / b[height<=1080]";
const TG_MEDIA_KEYS: [&str; 7] = ["video", "audio", "photo", "document", "animation", "voice", "video_note"];

/// Media groups (Telegram albums) whose first message carried `!w`.
static MEDIA_GROUPS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

/// Upload size limit in bytes before the admin multiplier: `main.maxfilesize`, or 150 MiB when it
/// is missing or not positive (same default as the web upload). Without the fallback every bot
/// upload was "too large".
pub(crate) fn max_file_size(cfg: &Config) -> f64 {
    cfg.f64("main.maxfilesize").filter(|m| *m > 0.0).unwrap_or(150.0 * 1024.0 * 1024.0)
}

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

/// What a chat upload must carry, from the same site config as the web upload
/// (`routes/uploads/upload.rs`): normally a rating plus the admin setting `min_tags`; in
/// `websrv.shitpost_mode` a rating only with `websrv.shitpost_require_rating` and
/// `websrv.shitpost_min_tags` tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rules {
    pub rating_required: bool,
    pub min_tags: usize,
    /// `-nsfl` is accepted (`enable_nsfl`).
    pub nsfl: bool,
    /// Without `-t`, read the rest of the message as tags (Telegram/Matrix). IRC lines are chat
    /// text, so there tags are only taken after `-t`.
    pub bare_tags: bool,
}

impl Rules {
    pub fn from_site(cfg: &Config, min_tags_setting: i64, bare_tags: bool) -> Rules {
        if crate::settings::shitpost_mode(cfg) {
            Rules {
                rating_required: cfg.get("websrv.shitpost_require_rating") == &Value::Bool(true),
                min_tags: crate::config::as_int(cfg.get("websrv.shitpost_min_tags")).unwrap_or(0).max(0) as usize,
                nsfl: cfg.enable_nsfl(),
                bare_tags,
            }
        } else {
            Rules { rating_required: true, min_tags: min_tags_setting.max(0) as usize, nsfl: cfg.enable_nsfl(), bare_tags }
        }
    }

    /// Usage line for replies, e.g. `!w <url> -sfw/-nsfw -t tag1, tag2 (rating and at least 2 tags required)`.
    pub fn usage(&self, trigger: &str, with_url: bool) -> String {
        let url = if with_url { " <url>" } else { "" };
        let flags = if self.nsfl { "-sfw/-nsfw/-nsfl" } else { "-sfw/-nsfw" };
        let rating = if self.rating_required { format!(" {flags}") } else { format!(" [{flags}]") };
        let tags = if self.min_tags > 0 { " -t tag1, tag2" } else { " [-t tag1, tag2]" };
        let need = match (self.rating_required, self.min_tags) {
            (true, 0) => " (rating required)".to_string(),
            (true, n) => format!(" (rating and at least {n} tag{} required)", if n == 1 { "" } else { "s" }),
            (false, 0) => String::new(),
            (false, n) => format!(" (at least {n} tag{} required)", if n == 1 { "" } else { "s" }),
        };
        format!("{trigger}{url}{rating}{tags}{need}")
    }
}

/// Upload rating, assigned as its rating tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rating {
    Sfw,
    Nsfw,
    Nsfl,
}

impl Rating {
    /// Tag id like the web upload: sfw = 1, nsfw = 2, nsfl = `nsfl_tag_id` (default 3).
    pub fn tag_id(self, cfg: &Config) -> i64 {
        match self {
            Rating::Sfw => 1,
            Rating::Nsfw => 2,
            Rating::Nsfl => cfg.nsfl_tag_id(),
        }
    }
}

/// Result of parsing `!w -sfw|-nsfw|-nsfl -t tags`.
#[derive(Debug, PartialEq, Eq)]
pub enum WArgs {
    /// `rating` is `None` only when the site doesn't require one.
    Ok { rating: Option<Rating>, tags: Vec<String> },
    /// Nothing given although something is required.
    Help,
    BothRatings,
    MissingRating,
    /// `-nsfl` while `enable_nsfl` is off.
    NsflDisabled,
    TooFewTags(usize),
}

fn is_rating_name(t: &str) -> bool {
    ["sfw", "nsfw", "nsfl"].contains(&t.to_lowercase().as_str())
}

pub fn parse_w_args(message: &str, trigger: &str, rules: Rules) -> WArgs {
    let has_sfw = RE_SFW.is_match(message);
    let has_nsfw = RE_NSFW.is_match(message);
    let has_nsfl = RE_NSFL.is_match(message);
    let tag_match = RE_TAGS.captures(message);
    if [has_sfw, has_nsfw, has_nsfl].iter().filter(|b| **b).count() > 1 {
        return WArgs::BothRatings;
    }
    if has_nsfl && !rules.nsfl {
        return WArgs::NsflDisabled;
    }
    let rating = if has_nsfl {
        Some(Rating::Nsfl)
    } else if has_nsfw {
        Some(Rating::Nsfw)
    } else if has_sfw {
        Some(Rating::Sfw)
    } else {
        None
    };
    let explicit_tags = tag_match.is_some();
    let input = match tag_match {
        Some(c) => c[1].to_string(),
        None if rules.bare_tags => {
            let trig = Regex::new(&format!(r"(?i){}(0bm)?\b", regex::escape(trigger))).ok();
            let s = RE_ALL.replace_all(message, "");
            let s = match trig {
                Some(t) => t.replace(&s, "").into_owned(),
                None => s.into_owned(),
            };
            let s = RE_FLAGS.replace_all(&s, "");
            RE_T_FLAG.replace_all(&s, "").trim().to_string()
        }
        None => String::new(),
    };
    // like the web upload: rating names are not tags (the rating comes from the flag)
    let mut tags: Vec<String> = Vec::new();
    for t in input.split(',').map(str::trim).filter(|t| !t.is_empty() && !is_rating_name(t)) {
        if !tags.iter().any(|x| x == t) {
            tags.push(t.to_string());
        }
    }
    let nothing_given = rating.is_none() && !explicit_tags && tags.is_empty();
    if nothing_given && (rules.rating_required || rules.min_tags > 0) {
        return WArgs::Help;
    }
    if rules.rating_required && rating.is_none() {
        return WArgs::MissingRating;
    }
    if tags.len() < rules.min_tags {
        return WArgs::TooFewTags(tags.len());
    }
    WArgs::Ok { rating, tags }
}

fn yt_dlp_base(cfg: &Config) -> Vec<String> {
    let runtime = crate::media::ytdlp_js_runtime(cfg);
    ["--js-runtimes", runtime.as_str(), "--geo-bypass", "--extractor-args", "youtube:player-client=ios,web"].iter().map(|s| s.to_string()).collect()
}

/// `--proxy` for sites that went through `main.socks`: the SOCKS proxy if configured, else the
/// SSRF guard.
fn socks_or_guard(cfg: &Config) -> Vec<String> {
    crate::media::proxy_url(cfg).map(|p| vec!["--proxy".into(), p]).unwrap_or_default()
}

/// `--proxy` for the other sites: always the SSRF guard (they never used the SOCKS proxy).
fn guard() -> Vec<String> {
    crate::net_guard::ytdlp_args()
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// yt-dlp arguments for the metadata probe (`--dump-json`).
pub fn meta_args(cfg: &Config, s: Site, link: &str) -> Vec<String> {
    let proxy = socks_or_guard(cfg);
    let base = yt_dlp_base(cfg);
    let mut a: Vec<String> = match s {
        Site::Pornhub => [guard(), base, strings(&["--referer", "https://www.pornhub.com", "--no-progress", "--no-warnings", "--user-agent", PC_UA])].concat(),
        Site::Instagram | Site::FourChan => [proxy, base, strings(&["-f", FMT])].concat(),
        Site::Imgur => [proxy, base].concat(),
        Site::YouTube => [proxy, base, strings(&["-f", FMT, "-I", "1"])].concat(),
        Site::Generic | Site::Mxc => [guard(), base, strings(&["-f", FMT])].concat(),
    };
    a.extend(strings(&["--skip-download", "--dump-json", "--", link]));
    a
}

/// yt-dlp arguments for the download.
pub fn download_args(cfg: &Config, s: Site, link: &str, max_bytes: f64, out_template: &str) -> Vec<String> {
    let proxy = socks_or_guard(cfg);
    let base = yt_dlp_base(cfg);
    let mut a: Vec<String> = match s {
        Site::Pornhub => [guard(), strings(&["--no-playlist", "--referer", "https://www.pornhub.com", "--user-agent", PC_UA, "-f", FMT])].concat(),
        Site::Instagram | Site::FourChan => [proxy, base, strings(&["-f", FMT])].concat(),
        Site::Imgur => [proxy, base, strings(&["-f", FMT_IMGUR])].concat(),
        Site::YouTube => [proxy, base, strings(&["-f", FMT, "-I", "1"])].concat(),
        Site::Generic | Site::Mxc => [guard(), base, strings(&["-f", FMT])].concat(),
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

/// `getMimeViaCurl(link)`: extension from the `Content-Type` of a HEAD request.
async fn ext_via_curl(cfg: &Config, link: &str) -> Option<String> {
    let referer = referer_for(link, None);
    let mut args = vec!["-I", "-s", "-f", "-L", "--max-time", "60", "--user-agent", PC_UA, "--referer", &referer];
    let proxy = crate::media::proxy_url(cfg);
    if let Some(p) = &proxy {
        args.extend(["--proxy", p.as_str()]);
    }
    args.extend(["--", link]);
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

/// The website account (`id`, `user`) linked to the sender's Telegram/Matrix/IRC account
/// (`!link`). On IRC the account comes from a fresh WHOIS (`irc_identified`), never from the nick.
/// Always `None` on Slack, which has no account linking.
pub(crate) async fn linked_user(st: &crate::state::AppState, e: &Message) -> Result<Option<(i64, String)>> {
    let (kind, alias) = match e.kind {
        Kind::Matrix => ("matrix", e.user.account.clone().unwrap_or_default()),
        Kind::Tg => ("telegram", e.user.account.clone().unwrap_or_default()),
        Kind::Irc => match e.irc_identified().await {
            Some(account) => ("irc", super::irc_alias(&account, &e.network)),
            None => return Ok(None),
        },
        Kind::Slack => return Ok(None),
    };
    alias_user(st, kind, &alias).await
}

/// The website account (`id`, `user`) a chat account (`user_alias`, `kind` + `alias`) is linked to.
pub(crate) async fn alias_user(st: &crate::state::AppState, kind: &str, alias: &str) -> Result<Option<(i64, String)>> {
    if alias.is_empty() {
        return Ok(None);
    }
    let row = sql!(
        r#"SELECT "user".id, "user"."user" FROM user_alias JOIN "user" ON "user".id = user_alias.userid
           WHERE lower(user_alias.alias) = lower("#,
        alias,
        ") AND user_alias.type = ",
        kind,
        " LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    Ok(row.and_then(|r| r.i64("id").map(|id| (id, r.string("user")))))
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
    let forwarded = e.raw.get("forward_origin").is_some() || e.raw.get("forward_from").is_some() || e.raw.get("forward_date").is_some();
    let media_group = e.raw["media_group_id"].as_str().map(str::to_string);
    let keyword = cfg.str_or("main.upload_channel_keyword", "w0bm").to_lowercase();
    if (e.kind == Kind::Matrix || !e.channel.to_lowercase().contains(&keyword)) && !is_w && !forwarded {
        return Ok(());
    }

    // Rating and tags follow the site's upload rules (`Rules::from_site`). They apply to `!w` on
    // Matrix/Telegram and to every IRC upload (links in upload channels too). Slack and forwarded
    // Telegram messages upload without them.
    let irc = e.kind == Kind::Irc;
    let rules = Rules::from_site(&cfg, st.settings.min_tags(), !irc);
    let usage = rules.usage(&trigger, irc);
    let mut rating: Option<Rating> = None;
    let mut tags: Vec<String> = vec![];
    if irc || (is_w && matches!(e.kind, Kind::Matrix | Kind::Tg)) {
        match parse_w_args(&e.message, &trigger, rules) {
            WArgs::Ok { rating: r, tags: t } => {
                rating = r;
                tags = t;
            }
            WArgs::Help => {
                e.reply(&usage).await;
                return Ok(());
            }
            WArgs::BothRatings => {
                e.reply(if rules.nsfl { "pick one: -sfw, -nsfw or -nsfl" } else { "pick one: -sfw or -nsfw, not both" }).await;
                return Ok(());
            }
            WArgs::NsflDisabled => {
                e.reply("NSFL mode is currently disabled").await;
                return Ok(());
            }
            WArgs::MissingRating => {
                e.reply(&format!("missing rating: {usage}")).await;
                return Ok(());
            }
            WArgs::TooFewTags(n) => {
                e.reply(&format!("at least {} tags required (got {n}): {usage}", rules.min_tags)).await;
                return Ok(());
            }
        }
    }

    // linked website account (required for !w on Matrix/Telegram and for every IRC upload: an
    // IRC upload is credited to the linked website user, never to the nick, which anyone can take)
    let mut website_user: Option<(i64, String)> = None;
    if irc {
        let Some(account) = e.irc_identified().await else {
            e.reply(&format!(
                "{}: you must be identified with NickServ (user mode +r) to upload. Identify, link your account ({full}/settings#linked) and try again.",
                e.user.nick
            ))
            .await;
            return Ok(());
        };
        match alias_user(st, "irc", &super::irc_alias(&account, &e.network)).await {
            Ok(Some(u)) => website_user = Some(u),
            Ok(None) => {
                e.reply(&format!(
                    "{}: the IRC account {account} isn't linked to a website account. Generate a token at {full}/settings#linked, then send /msg {} !link <TOKEN>",
                    e.user.nick,
                    e.irc_bot_nick()
                ))
                .await;
                return Ok(());
            }
            Err(err) => {
                tracing::error!("[PARSER] DB Link check error: {err:#}");
                return Ok(());
            }
        }
    }
    if matches!(e.kind, Kind::Matrix | Kind::Tg) {
        match linked_user(st, &e).await {
            Ok(u) => website_user = u,
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

/// `main.bot_youtube_download`: download YouTube links with yt-dlp instead of embedding them
/// (default off).
pub fn youtube_download(cfg: &Config) -> bool {
    cfg.bool("main.bot_youtube_download").unwrap_or(false)
}

/// Row for `Upload::add_item`.
struct NewItem {
    src: String,
    dest: String,
    mime: String,
    size: i64,
    checksum: String,
    phash: Option<String>,
    /// Source link, for the thumbnail.
    link: String,
    manual: bool,
}

struct Upload {
    rt: Arc<Runtime>,
    cfg: Arc<Config>,
    website_user: Option<(i64, String)>,
    rating: Option<Rating>,
    tags: Vec<String>,
}

impl Upload {
    /// Handle one link. `Ok(false)` stops processing the remaining links (like the old `return`).
    async fn process(&self, e: &Message, link: &str) -> Result<bool> {
        let st = &self.rt.st;
        let cfg = &self.cfg;
        let shown = super::telegram::sanitize_url(link);
        let s = site(link);

        if s != Site::Mxc {
            if let Some(id) = pipeline::check_repost_link(st, &shown).await.unwrap_or(None) {
                e.reply(&format!("repost motherf0cker (link): {}", super::item_url(st, id).await)).await;
                return Ok(true);
            }
        }
        if s == Site::YouTube && !youtube_download(cfg) {
            return self.embed_youtube(e, link).await;
        }
        let uuid = pipeline::gen_uuid();
        let level = get_level(cfg, &e.user).0;
        let base_max = max_file_size(cfg);
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
        let proxy = crate::media::proxy_url(cfg);
        if let Some(p) = &proxy {
            args.extend(["--proxy", p.as_str()]);
        }
        args.extend(["--", link]);
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
            progress.say(&format!("repost motherf0cker (checksum): {}", super::item_url(st, id).await)).await;
            return Ok(false);
        }

        let filename = source.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let manual = st.settings.manual_approval();
        let dest_dir = if manual { cfg.paths.pending.join("b") } else { cfg.paths.b.clone() };
        tokio::fs::copy(&source, dest_dir.join(&filename)).await?;
        let _ = tokio::fs::remove_file(&source).await;

        let src = if e.media.is_some() || link.contains("api.telegram.org") || link.starts_with("mxc://") { String::new() } else { trunc(link, 255) };
        let link = self.add_item(e, NewItem { src, dest: filename, mime, size: size as i64, checksum, phash, link: link.to_string(), manual }).await?;

        let speed = crate::util::calc_speed(size as f64, secs as f64);
        let speed = if speed.is_finite() { format!("{speed:.2} Mbit/s") } else { "yes".into() };
        let out = format!(
            "{}size: {} | speed: {speed} | link: {link}",
            if manual { "[approval pending] " } else { "" },
            crate::util::format_size_f(size as f64)
        );
        if let (Kind::Tg, Some(m)) = (e.kind, &progress.sent) {
            e.delete_message(&m["chat"]["id"], m["message_id"].as_i64().unwrap_or(0)).await;
        }
        e.reply(&out).await;
        Ok(true)
    }

    /// Name the item is stored under: the linked website account, else the chat name.
    fn username(&self, e: &Message) -> String {
        match &self.website_user {
            Some((_, u)) => u.clone(),
            None if e.kind == Kind::Tg => e.user.username.clone().filter(|u| !u.is_empty()).unwrap_or_else(|| e.user.nick.clone()),
            None => if e.user.nick.is_empty() { e.user.username.clone().unwrap_or_default() } else { e.user.nick.clone() },
        }
    }

    /// Insert the item, then comment subscription, rating/tags, thumbnails and notifications.
    /// Returns the item URL.
    async fn add_item(&self, e: &Message, it: NewItem) -> Result<String> {
        let st = &self.rt.st;
        let cfg = &self.cfg;
        let username = self.username(e);
        let slug = crate::util::generate_slug(11);
        let item_id: i64 = sql!(
            "INSERT INTO items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, slug) VALUES (",
            it.src, ", ", it.dest.clone(), ", ", it.mime.clone(), ", ", it.size as i32, ", ", it.checksum, ", ", it.phash, ", ",
            trunc(&username, 40), ", ", trunc(&e.channel, 255), ", ", trunc(&e.network, 40), ", ", crate::util::now_secs() as i32, ", ",
            !it.manual, ", ", slug.clone(), ") RETURNING id"
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

        if self.rating.is_some() || !self.tags.is_empty() {
            if let Err(err) = self.assign_tags(item_id).await {
                tracing::error!("[PARSER] Failed to assign tags: {err:#}");
            }
        }

        let thumb = async {
            pipeline::gen_thumbnail(st, &it.dest, &it.mime, item_id, &it.link, it.manual).await?;
            pipeline::gen_blurred_thumbnail(st, item_id, it.manual).await
        };
        if thumb.await.is_err() {
            if let Err(err) = pipeline::placeholder_thumbnail(st, item_id, it.manual).await {
                tracing::error!("[PARSER] placeholder thumbnail failed for {item_id}: {err:#}");
            }
        }

        let url = super::item_url(st, item_id).await;
        if it.manual {
            pipeline::notify_admins(st, item_id).await;
        } else {
            // live feed: the main page adds the item without a reload (same event as web uploads)
            crate::f0cklib::clear_count_cache(st);
            let payload = json!({
                "id": item_id, "dest": it.dest, "mime": it.mime, "username": trunc(&username, 40), "display_name": Value::Null,
                "tag_id": self.rating.map(|r| r.tag_id(cfg)).unwrap_or(0), "is_oc": false, "slug": slug, "visibility": 0,
                "is_album": false, "album_count": 0,
            });
            crate::social::helpers::notify(st, "new_item", &payload).await;
            st.bot.notify_matrix(cfg, &format!("{username} uploaded a new video {url}")).await;
        }
        Ok(url)
    }

    /// YouTube link without `main.bot_youtube_download`: a `video/youtube` embed item, the same
    /// as a YouTube link in the web upload (`websrv.enable_youtube_upload`). Nothing is downloaded.
    async fn embed_youtube(&self, e: &Message, link: &str) -> Result<bool> {
        let st = &self.rt.st;
        let Some(video_id) = RE_YT.captures(link).map(|c| c[1].to_string()) else { return Ok(true) };
        if !self.cfg.enabled("websrv.enable_youtube_upload") {
            e.reply("youtube embeds are disabled").await;
            return Ok(true);
        }
        let yt_url = format!("https://www.youtube.com/watch?v={video_id}");
        if let Some(id) = pipeline::check_repost_link(st, &yt_url).await.unwrap_or(None) {
            e.reply(&format!("repost motherf0cker (link): {}", super::item_url(st, id).await)).await;
            return Ok(true);
        }
        let manual = st.settings.manual_approval();
        let item = NewItem {
            src: yt_url.clone(),
            dest: format!("yt:{video_id}"),
            mime: "video/youtube".into(),
            size: 0,
            checksum: format!("yt_{video_id}_{}", crate::util::now_ms()),
            phash: None,
            link: yt_url,
            manual,
        };
        match self.add_item(e, item).await {
            Ok(url) => {
                e.reply(&format!("{}youtube | link: {url}", if manual { "[approval pending] " } else { "" })).await;
                Ok(true)
            }
            Err(err) => {
                tracing::error!("[PARSER] YouTube embed {video_id}: {err:#}");
                e.reply("something went wrong lol").await;
                Ok(false)
            }
        }
    }

    /// Rating tag (sfw = 1, nsfw = 2, nsfl = `nsfl_tag_id`) + user tags, attributed to the linked
    /// website user.
    async fn assign_tags(&self, item_id: i64) -> Result<()> {
        let db = &self.rt.st.db;
        let user_id = self.website_user.as_ref().map(|(id, _)| *id as i32).unwrap_or(1);
        let item = item_id as i32;
        if let Some(rating) = self.rating {
            let rating_tag = rating.tag_id(&self.cfg) as i32;
            sql!("INSERT INTO tags_assign (tag_id, item_id, user_id) VALUES (", rating_tag, ", ", item, ", ", user_id, ")").execute(db).await?;
        }
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
    fn youtube_download_needs_opt_in() {
        assert!(!youtube_download(&test_cfg(json!({}))));
        assert!(!youtube_download(&test_cfg(json!({ "main": { "bot_youtube_download": false } }))));
        assert!(youtube_download(&test_cfg(json!({ "main": { "bot_youtube_download": true } }))));
    }

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

    const TG: Rules = Rules { rating_required: true, min_tags: 3, nsfl: false, bare_tags: true };
    const IRC: Rules = Rules { rating_required: true, min_tags: 0, nsfl: false, bare_tags: false };

    #[test]
    fn w_arguments() {
        let r2 = Rules { min_tags: 2, ..TG };
        assert_eq!(parse_w_args("!w", "!w", TG), WArgs::Help);
        assert_eq!(parse_w_args("!w -sfw -nsfw -t a", "!w", Rules { min_tags: 0, ..TG }), WArgs::BothRatings);
        assert_eq!(parse_w_args("!w -t a,b,c", "!w", TG), WArgs::MissingRating);
        assert_eq!(parse_w_args("!w -sfw -t a, b", "!w", TG), WArgs::TooFewTags(2));
        assert_eq!(parse_w_args("!w -nsfw -t big cat, dog,  dog ,x", "!w", TG), WArgs::Ok { rating: Some(Rating::Nsfw), tags: vec!["big cat".into(), "dog".into(), "x".into()] });
        assert_eq!(parse_w_args("!w https://x.org/v.mp4 -sfw a, b", "!w", r2), WArgs::Ok { rating: Some(Rating::Sfw), tags: vec!["a".into(), "b".into()] });
        // rating names don't count as tags (like the web upload)
        assert_eq!(parse_w_args("!w -sfw -t sfw, a", "!w", r2), WArgs::TooFewTags(1));
    }
    #[test]
    fn builds_ytdlp_args() {
        let cfg = test_cfg(json!({ "main": { "socks": "socks5://127.0.0.1:9050" } }));
        let a = meta_args(&cfg, Site::YouTube, "https://youtu.be/x");
        assert_eq!(&a[..2], &["--proxy", "socks5://127.0.0.1:9050"]);
        assert_eq!(&a[a.len() - 2..], &["--", "https://youtu.be/x"]);
        assert!(a.windows(2).any(|w| w == ["-I", "1"]));
        let a = download_args(&cfg, Site::Generic, "https://e.org/v", 104857600.0 * 3.5, "/tmp/u.%(ext)s");
        // Sites that never used main.socks still go through the SSRF guard, not the SOCKS proxy
        assert!(!a.iter().any(|x| x.contains("9050")));
        assert!(a.windows(2).any(|w| w == ["--max-filesize", "358400k"]));
        assert_eq!(a.last().unwrap(), "https://e.org/v");
        assert_eq!(referer_for("https://i.imgur.com/a.mp4", None), "https://imgur.com/");
    }

    #[test]
    fn irc_arguments() {
        let p = |m: &str, r: Rules| parse_w_args(m, "!w", r);
        assert_eq!(p("look https://x.org/v.mp4", IRC), WArgs::Help);
        assert_eq!(p("!w https://x.org/v.mp4 -t a", IRC), WArgs::MissingRating);
        // the chat text around the link never becomes tags
        assert_eq!(p("lol https://x.org/v.mp4 -nsfw", IRC), WArgs::Ok { rating: Some(Rating::Nsfw), tags: vec![] });
        assert_eq!(p("https://x.org/v.mp4 -sfw -t cat, loop", IRC), WArgs::Ok { rating: Some(Rating::Sfw), tags: vec!["cat".into(), "loop".into()] });
        assert_eq!(p("https://x.org/v.mp4 -sfw -nsfw", IRC), WArgs::BothRatings);
        assert_eq!(p("https://x.org/v.mp4 -sfwx", IRC), WArgs::Help);
        // site requires tags → IRC too
        let irc3 = Rules { min_tags: 3, ..IRC };
        assert_eq!(p("lol https://x.org/v.mp4 -sfw", irc3), WArgs::TooFewTags(0));
        assert_eq!(p("https://x.org/v.mp4 -sfw -t a, b, c", irc3), WArgs::Ok { rating: Some(Rating::Sfw), tags: vec!["a".into(), "b".into(), "c".into()] });
    }

    #[test]
    fn site_rules() {
        let cfg = test_cfg(json!({}));
        assert_eq!(Rules::from_site(&cfg, 3, false), Rules { rating_required: true, min_tags: 3, nsfl: false, bare_tags: false });
        let cfg = test_cfg(json!({ "websrv": { "shitpost_mode": true } }));
        let r = Rules::from_site(&cfg, 3, false);
        assert_eq!(r, Rules { rating_required: false, min_tags: 0, nsfl: false, bare_tags: false });
        // shitpost mode without requirements: a plain link uploads untagged
        assert_eq!(parse_w_args("https://x.org/v.mp4", "!w", r), WArgs::Ok { rating: None, tags: vec![] });
        let cfg = test_cfg(json!({ "websrv": { "shitpost_mode": true, "shitpost_require_rating": true, "shitpost_min_tags": 1 } }));
        let r = Rules::from_site(&cfg, 3, false);
        assert_eq!(r, Rules { rating_required: true, min_tags: 1, nsfl: false, bare_tags: false });
        assert_eq!(r.usage("!w", true), "!w <url> -sfw/-nsfw -t tag1, tag2 (rating and at least 1 tag required)");
        assert_eq!(IRC.usage("!w", true), "!w <url> -sfw/-nsfw [-t tag1, tag2] (rating required)");
    }

    #[test]
    fn nsfl_rating() {
        let on = Rules { nsfl: true, ..IRC };
        let p = |m: &str, r: Rules| parse_w_args(m, "!w", r);
        assert_eq!(p("https://x.org/v.mp4 -nsfl", IRC), WArgs::NsflDisabled);
        assert_eq!(p("https://x.org/v.mp4 -nsfl", on), WArgs::Ok { rating: Some(Rating::Nsfl), tags: vec![] });
        assert_eq!(p("https://x.org/v.mp4 -nsfl -nsfw", on), WArgs::BothRatings);
        // the flag isn't read as a tag on Telegram/Matrix either
        assert_eq!(parse_w_args("!w -nsfl gore, x", "!w", Rules { nsfl: true, min_tags: 2, ..TG }), WArgs::Ok { rating: Some(Rating::Nsfl), tags: vec!["gore".into(), "x".into()] });
        assert_eq!(on.usage("!w", true), "!w <url> -sfw/-nsfw/-nsfl [-t tag1, tag2] (rating required)");
        // site config: enable_nsfl, nsfl_tag_id
        let cfg = test_cfg(json!({ "enable_nsfl": true, "nsfl_tag_id": 7 }));
        assert!(Rules::from_site(&cfg, 0, false).nsfl);
        assert_eq!(Rating::Nsfl.tag_id(&cfg), 7);
        assert_eq!(Rating::Nsfl.tag_id(&test_cfg(json!({}))), 3);
        assert!(!Rules::from_site(&test_cfg(json!({})), 0, false).nsfl);
    }

    #[test]
    fn size_limit_fallback() {
        assert_eq!(max_file_size(&test_cfg(json!({ "main": { "maxfilesize": 1000 } }))), 1000.0);
        assert_eq!(max_file_size(&test_cfg(json!({ "main": {} }))), 150.0 * 1024.0 * 1024.0);
        assert_eq!(max_file_size(&test_cfg(json!({ "main": { "maxfilesize": 0 } }))), 150.0 * 1024.0 * 1024.0);
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
