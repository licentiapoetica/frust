//! Chat commands (`src/inc/trigger/*.mjs`, except `parser.mjs` → `parser.rs`).
//!
//! Dropped: `trigger/debug.mjs` `!f0ck debug <js>` (ran arbitrary JavaScript in a `vm` context with
//! access to the bot and the database — remote code execution for anyone matching an admin prefix),
//! the inactive `gib f0ck`, `!tags add`/`!tags remove` triggers and `!f0ck cache` (toggled a flag
//! the server no longer has).

use super::events::Runtime;
use super::pipeline;
use super::{Client, Kind, Message, User, get_level};
use crate::config::Config;
use crate::db::RowExt;
use crate::sql;
use anyhow::Result;
use futures::future::BoxFuture;
use serde_json::json;
use std::sync::Arc;

pub type Handler = fn(Arc<Runtime>, Message) -> BoxFuture<'static, Result<()>>;

/// Trigger pattern: a fixed regex or one built from the config.
pub enum Call {
    Re(&'static str),
    Dyn(fn(&Config) -> String),
}

impl Call {
    pub fn pattern(&self, cfg: &Config) -> String {
        match self {
            Call::Re(s) => s.to_string(),
            Call::Dyn(f) => f(cfg),
        }
    }
}

/// `new self.trigger({ name, call, level, active, clients, f })`
pub struct Trigger {
    pub name: &'static str,
    pub call: Call,
    pub level: i64,
    pub active: bool,
    pub clients: &'static [Kind],
    /// Shown by `!help` (None = hidden, like the old ignore list).
    pub help: Option<&'static str>,
    pub f: Handler,
}

macro_rules! h {
    ($f:path) => {
        |rt, e| Box::pin($f(rt, e))
    };
}

pub fn all() -> Vec<Trigger> {
    let t = |name, call, level, clients, help, f| Trigger { name, call, level, active: true, clients, help, f };
    vec![
        t("level", Call::Re(r"(?i)^!level (.*)"), 0, Kind::ALL, None, h!(level)),
        t("self", Call::Re(r"(?i)^!self$"), 0, Kind::ALL, None, h!(self_info)),
        t("delete", Call::Re(r"(?i)^!(del|rm) .*"), 100, Kind::ALL, Some("!del <id or slug> ..."), h!(delete)),
        t("recover", Call::Re(r"(?i)^!(recover) .*"), 100, Kind::ALL, None, h!(recover)),
        t("f0ckgag", Call::Dyn(f0ckgag_pattern), 0, Kind::ALL, None, h!(f0ckgag)),
        t("f0ck", Call::Re(r"(?i)^!f0ck .*"), 100, Kind::ALL, None, h!(f0ck)),
        t("help", Call::Re(r"(?i)^!help$"), 0, Kind::ALL, Some("!help"), h!(help)),
        t("invite_matrix", Call::Re(r"(?i)^!invite$"), 0, &[Kind::Matrix], Some("!invite"), h!(invite_matrix)),
        t("matrix_link", Call::Re(r"(?i)(^!link(?:\s+(.+))?$)|(^(?:[a-z0-9]{6}|[0-9a-f]{64})$)"), 0, &[Kind::Matrix], None, h!(matrix_link)),
        t("telegram_link", Call::Re(r"(?i)^!link(?:\s+(.+))?$"), 0, &[Kind::Tg], None, h!(telegram_link)),
        t("tags show", Call::Re(r"(?i)^!tags show [A-Za-z0-9_-]+$"), 100, Kind::ALL, None, h!(tags_show)),
        t("thumbnailer", Call::Re(r"(?i)^!thumb .*"), 100, Kind::ALL, None, h!(thumbnailer)),
        t("parser", Call::Dyn(super::parser::pattern), 0, Kind::ALL, None, h!(super::parser::run)),
    ]
}

fn url_full(cfg: &Config) -> String {
    cfg.url_full().to_string()
}

// ── debug.mjs (safe parts) ──────────────────────────────────────────────────

async fn level(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let name = e.message.trim().get(7..).unwrap_or("").trim();
    let prefix = e.cached_user(name).and_then(|u| u["prefix"].as_str().map(str::to_string)).unwrap_or_default();
    let (level, verified) = get_level(&rt.st.cfg(), &User { prefix, ..Default::default() });
    e.reply(&json!({ "level": level, "verified": verified }).to_string()).await;
    Ok(())
}

async fn self_info(_rt: Arc<Runtime>, e: Message) -> Result<()> {
    e.reply(&serde_json::to_string(&e.user)?).await;
    Ok(())
}

// ── delete.mjs ──────────────────────────────────────────────────────────────

/// An item id from a command argument: a numeric id or a slug (album items resolve to their item).
/// Ids 0 and 1 are never touched.
async fn resolve_item(st: &crate::state::AppState, arg: &str) -> Option<i32> {
    crate::f0cklib::resolve_numeric_item_id(st, arg).await.and_then(|id| i32::try_from(id).ok()).filter(|id| *id > 1)
}

/// `!del` — admins only (level 100, like the website, where only admins and moderators delete
/// items). The old uploader exception (same nick, channel and network within 10 minutes) is gone:
/// IRC nicks aren't authenticated, so anyone could take the nick and delete the upload.
async fn delete(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let st = &rt.st;
    let cfg = st.cfg();
    if get_level(&cfg, &e.user).0 < 100 {
        // the trigger level already keeps others out; this guards against a lowered level
        return Ok(());
    }
    let mut deleted = vec![];
    for arg in &e.args {
        let Some(id) = resolve_item(st, arg).await else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        let Some(item) = sql!("SELECT dest, mime FROM items WHERE id = ", id, " AND active = true LIMIT 1").fetch_optional(&st.db).await? else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        sql!("UPDATE items SET active = false, is_deleted = true WHERE id = ", id).execute(&st.db).await?;
        pipeline::move_to_deleted(st, &item.string("dest"), id as i64).await?;
        let thumb = format!("{id}.webp");
        pipeline::move_file(&cfg.paths.t.join(&thumb), &cfg.paths.deleted.join("t").join(&thumb)).await;
        if item.string("mime").starts_with("audio") {
            pipeline::move_file(&cfg.paths.ca.join(&thumb), &cfg.paths.deleted.join("ca").join(&thumb)).await;
        }
        deleted.push(arg.clone());
    }
    e.reply(&format!("deleted {}/{} f0cks ({})", deleted.len(), e.args.len(), deleted.join(","))).await;
    Ok(())
}

async fn recover(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let st = &rt.st;
    let cfg = st.cfg();
    let mut recovered = vec![];
    for arg in &e.args {
        let Some(id) = resolve_item(st, arg).await else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        let Some(item) = sql!("SELECT dest, mime FROM items WHERE id = ", id, " AND active = false LIMIT 1").fetch_optional(&st.db).await? else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        let dest = item.string("dest");
        if dest.is_empty() || dest.contains('/') || dest.contains("..") {
            continue;
        }
        pipeline::move_file(&cfg.paths.deleted.join("b").join(&dest), &cfg.paths.b.join(&dest)).await;
        let thumb = format!("{id}.webp");
        pipeline::move_file(&cfg.paths.deleted.join("t").join(&thumb), &cfg.paths.t.join(&thumb)).await;
        if item.string("mime").starts_with("audio") {
            pipeline::move_file(&cfg.paths.deleted.join("ca").join(&thumb), &cfg.paths.ca.join(&thumb)).await;
        }
        // fix: also clear is_deleted (the old code left recovered items flagged as deleted)
        sql!("UPDATE items SET active = true, is_deleted = false WHERE id = ", id).execute(&st.db).await?;
        recovered.push(arg.clone());
    }
    e.reply(&format!("recovered {}/{} f0cks ({})", recovered.len(), e.args.len(), recovered.join(","))).await;
    Ok(())
}

// ── f0ckgag.mjs ─────────────────────────────────────────────────────────────

/// Links to an item on this site. Group 3 is the id, the 11-character slug, or `b/<file>`; it must
/// be followed by the end of the path, so `/notifications` isn't read as a slug.
fn f0ckgag_pattern(cfg: &Config) -> String {
    let host = cfg.str("main.url.regex").filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| regex::escape(cfg.domain()));
    format!(
        r"(?i)(https?://{host})(/(?:video|image|audio|tag/[^/\s]+|user/[^/\s]+(?:/favs)?|h/[^/\s]+))?/([0-9]+|[A-Za-z0-9_-]{{11}}|(?:b/)[A-Za-z0-9_]{{8}}\.(?:jpg|webm|gif|mp4|png|mov|mp3|ogg|flac))(?:[^A-Za-z0-9_-]|$)"
    )
}

/// The item part (id, slug or `b/<file>`) of the first link to this site in a message.
fn f0ckgag_target(re: &regex::Regex, msg: &str) -> Option<String> {
    re.captures(msg).and_then(|c| c.get(3)).map(|m| m.as_str().rsplit('/').next().unwrap_or("").to_string())
}

/// `new Date(stamp * 1e3).toString().slice(0, 24)`
fn js_date(stamp: i64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(stamp, 0).single().map(|d| d.format("%a %b %d %Y %H:%M:%S").to_string()).unwrap_or_default()
}

async fn f0ckgag(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let cfg = rt.st.cfg();
    let Some(re) = rt.regex(&f0ckgag_pattern(&cfg)) else { return Ok(()) };
    let Some(dat) = f0ckgag_target(&re, &e.message) else { return Ok(()) };
    let nsfl = cfg.nsfl_tag_id() as i32;
    let mut q = sql!(
        "SELECT i.id, i.mime, i.size, i.username, i.stamp,
           (SELECT t.tag FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ta.item_id = i.id AND ta.tag_id IN (1, 2, ", nsfl, ") ORDER BY ta.tag_id ASC LIMIT 1) AS rating
         FROM items i "
    );
    if dat.contains('.') {
        q.push("WHERE i.dest = ").bind(dat);
    } else {
        let Some(id) = crate::f0cklib::resolve_numeric_item_id(&rt.st, &dat).await.and_then(|i| i32::try_from(i).ok()) else {
            e.reply("no f0cks given! lol D:").await;
            return Ok(());
        };
        q.push("WHERE i.id = ").bind(id);
    }
    let Some(row) = q.push(" LIMIT 1").fetch_optional(&rt.st.db).await? else {
        e.reply("no f0cks given! lol D:").await;
        return Ok(());
    };
    let rating = row.str("rating").filter(|s| !s.is_empty()).unwrap_or("untagged").to_string();
    let id = row.i64("id").unwrap_or(0);
    let link = super::item_url(&rt.st, id).await.replace("http://", "https://");
    let size = format!("~{}", crate::util::format_size(row.val("size")));
    let date = js_date(row.i64("stamp").unwrap_or(0));
    let (user, mime) = (row.string("username"), row.string("mime"));
    match e.kind {
        Kind::Matrix => {
            let color = match rating.as_str() {
                "sfw" => "#00ff00",
                "nsfw" => "#ff0000",
                _ => "#888888",
            };
            let esc = crate::util::escape_html;
            let body = [format!("ID: {id}"), rating.clone(), format!("user: {user}"), size.clone(), mime.clone(), date.clone()].join(" - ");
            let html = [
                format!("ID: {id}"),
                format!("<font color=\"{color}\"><b>{}</b></font>", esc(&rating)),
                format!("user: {}", esc(&user)),
                esc(&size),
                esc(&mime),
                esc(&date),
            ]
            .join(" - ");
            e.reply_rich(&body, &html).await;
        }
        _ => {
            let r = if e.kind == Kind::Irc {
                let color = match rating.as_str() {
                    "sfw" => "green",
                    "nsfw" => "red",
                    _ => "brown",
                };
                format!("[color={color}]{rating}[/color]")
            } else {
                rating
            };
            e.reply(&[link, r, format!("user: {user}"), size, mime, date].join(" - ")).await;
        }
    }
    Ok(())
}

// ── f0ck.mjs (admin commands) ───────────────────────────────────────────────

async fn dir_stats(dir: &std::path::Path) -> (usize, u64) {
    let (mut n, mut size) = (0, 0);
    if let Ok(mut rd) = tokio::fs::read_dir(dir).await {
        while let Ok(Some(ent)) = rd.next_entry().await {
            n += 1;
            size += tokio::fs::metadata(ent.path()).await.map(|m| m.len()).unwrap_or(0);
        }
    }
    (n, size)
}

async fn f0ck(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let st = &rt.st;
    let cfg = st.cfg();
    let fs = |n: f64| crate::util::format_size_f(n);
    match e.args.first().map(String::as_str) {
        Some("stats") => {
            let (b, t, ca) = (dir_stats(&cfg.paths.b).await, dir_stats(&cfg.paths.t).await, dir_stats(&cfg.paths.ca).await);
            e.reply(&format!(
                "{} f0cks: {}, {} thumbnails: {}, {} coverarts: {}",
                b.0,
                fs(b.1 as f64),
                t.0,
                fs(t.1 as f64),
                ca.0,
                fs(ca.1 as f64)
            ))
            .await;
        }
        Some("limit") => {
            let max = super::parser::max_file_size(&cfg);
            let mult = cfg.f64("main.adminmultiplier").unwrap_or(1.0);
            e.reply(&format!("up to {} ({} for admins)", fs(max), fs(max * mult))).await;
        }
        Some("thumb") => {
            let ids: Vec<i32> = sql!("SELECT id FROM items").fetch_scalar_list(&st.db).await?;
            let mut have = std::collections::HashSet::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&cfg.paths.t).await {
                while let Ok(Some(ent)) = rd.next_entry().await {
                    let name = ent.file_name().to_string_lossy().to_string();
                    if let Some(id) = name.strip_suffix(".webp").and_then(|s| s.parse::<i32>().ok()) {
                        have.insert(id);
                    }
                }
            }
            let missing = ids.iter().filter(|i| !have.contains(i)).count();
            e.reply(&format!("{missing}, {}, {}", ids.len(), have.len())).await;
        }
        Some("uptime") => {
            let out = crate::proc::spawn_with("sudo", &["-n", "systemctl", "status", "f0ck"], crate::proc::Opts { ignore_exit_code: true, quiet: true, ..Default::default() }).await?;
            if let Some(line) = out.stdout_str().lines().nth(2) {
                e.reply(&line.trim().replace("Active: active (running)", "i'm active")).await;
            }
        }
        Some("restart") => {
            e.reply("hay hay patron, hemen!").await;
            tokio::spawn(async {
                if let Err(err) = crate::proc::spawn("sudo", &["-n", "systemctl", "restart", "f0ck"]).await {
                    tracing::error!("[BOT] restart failed: {err:#}");
                }
            });
        }
        Some("clearTmp") => {
            if let Ok(mut rd) = tokio::fs::read_dir(&cfg.paths.tmp).await {
                while let Ok(Some(ent)) = rd.next_entry().await {
                    if ent.file_name() != ".empty" && ent.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
                        let _ = tokio::fs::remove_file(ent.path()).await;
                    }
                }
            }
            e.reply("cleared lol").await;
        }
        Some("status") => {
            let c = pipeline::count_f0cks(st).await?;
            e.reply(&format!("tagged: {}; untagged: {}; sfw: {}; nsfw: {}; total: {}", c.tagged, c.untagged, c.sfw, c.nsfw, c.tagged + c.untagged)).await;
        }
        Some("help") => {
            e.reply("cmds: stats, limit, thumb, uptime, restart, clearTmp, status").await;
        }
        _ => {}
    }
    Ok(())
}

// ── help.mjs ────────────────────────────────────────────────────────────────

pub fn help_text(rt: &Runtime, kind: Kind, level: i64, trigger_word: &str) -> String {
    let mut cmds: Vec<String> = rt
        .triggers
        .iter()
        .filter(|t| t.active && t.clients.contains(&kind) && t.level <= level)
        .filter_map(|t| t.help.map(str::to_string))
        .collect();
    let rules = |bare_tags| super::parser::Rules::from_site(&rt.st.cfg(), rt.st.settings.min_tags(), bare_tags);
    cmds.push(match kind {
        Kind::Matrix | Kind::Tg => rules(true).usage(trigger_word, false),
        Kind::Irc => rules(false).usage(trigger_word, true),
        Kind::Slack => format!("{trigger_word} <url>"),
    });
    cmds.sort();
    cmds.dedup();
    format_help(kind, &cmds)
}

fn format_help(kind: Kind, cmds: &[String]) -> String {
    if kind == Kind::Irc {
        // one line: the IRC client refuses messages with 5+ lines, and markdown doesn't render there
        return format!("[b]Available commands:[/b] {}", cmds.join(" | "));
    }
    format!("**Available Commands:**\n```\n{}\n```", cmds.join("\n"))
}

async fn help(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let cfg = rt.st.cfg();
    let level = get_level(&cfg, &e.user).0;
    let text = help_text(&rt, e.kind, level, cfg.str_or("main.trigger", "!w"));
    e.reply(&text).await;
    Ok(())
}

// ── matrix_invite.mjs ───────────────────────────────────────────────────────

async fn invite_matrix(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let Client::Matrix(matrix) = &e.client else { return Ok(()) };
    let st = &rt.st;
    let mxid = e.user.account.clone().unwrap_or_default();
    if sql!("SELECT 1 FROM invite_tokens WHERE created_by_matrix = ", mxid.clone(), " LIMIT 1").fetch_optional(&st.db).await?.is_some() {
        matrix.sendmsg(&mxid, "You have already generated an invite token.").await;
        return Ok(());
    }
    let token = crate::util::random_hex(5).to_uppercase();
    sql!("INSERT INTO invite_tokens (token, created_at, created_by_matrix) VALUES (", token.clone(), ", ", crate::util::now_secs(), ", ", mxid.clone(), ")")
        .execute(&st.db)
        .await?;
    let full = url_full(&st.cfg());
    matrix.sendmsg(&mxid, &format!("**Your Invite Token:** {token}\n**Register here:** {full}/register?token={token}")).await;
    e.reply("I've sent you a direct message with your invite token.").await;
    Ok(())
}

// ── matrix_link.mjs / telegram_link.mjs ─────────────────────────────────────

enum LinkResult {
    Invalid,
    AlreadyLinked,
    Taken,
    Linked,
}

/// Link a chat account (`alias`, `kind` = `matrix`/`telegram`) to the website user owning `token`.
/// Fix: tokens expire (`link_token.expires_at`, 10 minutes) — the old code accepted them forever.
async fn link_account(rt: &Runtime, token: &str, alias: &str, kind: &str, case_insensitive: bool) -> Result<LinkResult> {
    let db = &rt.st.db;
    let Some(user_id) = sql!("SELECT user_id FROM link_token WHERE token = ", token, " AND (expires_at IS NULL OR expires_at > LOCALTIMESTAMP) LIMIT 1")
        .fetch_optional(db)
        .await?
        .and_then(|r| r.i64("user_id"))
    else {
        return Ok(LinkResult::Invalid);
    };
    let alias_cmp = if case_insensitive { "lower(alias) = lower(" } else { "alias = (" };
    let mut q = sql!("SELECT 1 FROM user_alias WHERE userid = ", user_id as i32, " AND type = ", kind, " AND ");
    q.push(alias_cmp).bind(alias).push(")");
    if q.fetch_optional(db).await?.is_some() {
        return Ok(LinkResult::AlreadyLinked);
    }
    let mut q = sql!("SELECT 1 FROM user_alias WHERE type = ", kind, " AND ");
    q.push(alias_cmp).bind(alias).push(")");
    if q.fetch_optional(db).await?.is_some() {
        return Ok(LinkResult::Taken);
    }
    let mut tx = db.begin().await?;
    sql!("INSERT INTO user_alias (userid, alias, type) VALUES (", user_id as i32, ", ", alias, ", ", kind, ") ON CONFLICT DO NOTHING").execute(&mut *tx).await?;
    sql!("DELETE FROM link_token WHERE token = ", token).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(LinkResult::Linked)
}

async fn matrix_link(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let Client::Matrix(matrix) = &e.client else { return Ok(()) };
    let explicit = e.message.to_lowercase().starts_with("!link");
    let raw = e.args.first().cloned().unwrap_or_else(|| e.message.clone());
    let token = raw.replacen("!link", "", 1).trim().to_uppercase();
    let alias = e.user.account.clone().unwrap_or_default();
    if token.is_empty() {
        matrix.sendmsg(&alias, "Please paste your link token here.").await;
        if !e.channelid.contains(&alias) {
            e.reply("I've sent you a DM. Please paste your token there.").await;
        }
        return Ok(());
    }
    match link_account(&rt, &token, &alias, "matrix", true).await {
        Ok(LinkResult::Invalid) => {
            // a random 6-character message that isn't a token is ignored silently
            if explicit {
                e.reply("Invalid or expired token. Please generate a new one.").await;
            }
        }
        Ok(LinkResult::AlreadyLinked) => {
            e.reply("This Matrix account is already linked to your website profile.").await;
        }
        Ok(LinkResult::Taken) => {
            e.reply("This Matrix account is already linked to a different website profile.").await;
        }
        Ok(LinkResult::Linked) => {
            e.reply_rich(
                &format!("Successfully linked Matrix account {alias} to your website profile!"),
                &format!("Successfully linked Matrix account <b>{}</b> to your website profile!", crate::util::escape_html(&alias)),
            )
            .await;
        }
        Err(err) => {
            tracing::error!("[Matrix Link] Error: {err:#}");
            if explicit {
                e.reply("An internal error occurred while linking your account.").await;
            }
        }
    }
    Ok(())
}

async fn telegram_link(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let full = url_full(&rt.st.cfg());
    let token = e.args.first().map(|s| s.trim().to_uppercase()).unwrap_or_default();
    if token.is_empty() {
        e.reply(&format!("Please generate a link token on the website at {full}/settings and then send: !link <TOKEN>")).await;
        return Ok(());
    }
    let alias = e.user.account.clone().unwrap_or_default();
    let text = match link_account(&rt, &token, &alias, "telegram", false).await {
        Ok(LinkResult::Invalid) => "Invalid or expired token. Please generate a new one at the website settings page.".to_string(),
        Ok(LinkResult::AlreadyLinked) => "This Telegram account is already linked to your website profile.".to_string(),
        Ok(LinkResult::Taken) => "This Telegram account is already linked to a different website profile.".to_string(),
        Ok(LinkResult::Linked) => {
            let name = e.user.username.as_ref().map(|u| format!("@{u}")).unwrap_or_else(|| e.user.nick.clone());
            format!("Successfully linked Telegram account {name} (ID: {alias}) to your website profile!")
        }
        Err(err) => {
            tracing::error!("[Telegram Link] Error: {err:#}");
            "An internal error occurred while linking your account.".to_string()
        }
    };
    e.reply(&text).await;
    Ok(())
}

// ── tags.mjs ────────────────────────────────────────────────────────────────

async fn tags_show(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let arg = e.args.get(1).map(String::as_str).unwrap_or("");
    let Some(id) = crate::f0cklib::resolve_numeric_item_id(&rt.st, arg).await.filter(|i| *i > 0) else {
        e.reply("lol no").await;
        return Ok(());
    };
    let link = super::item_url(&rt.st, id).await;
    let tags: Vec<String> = pipeline::get_tags(&rt.st, id).await?.iter().map(|t| t.string("tag")).collect();
    if tags.is_empty() {
        e.reply(&format!("item {link} has no tags!")).await;
    } else {
        e.reply(&format!("item {link} is tagged as: {}", tags.join(", "))).await;
    }
    Ok(())
}

// ── thumbnails.mjs ──────────────────────────────────────────────────────────

async fn thumbnailer(rt: Arc<Runtime>, e: Message) -> Result<()> {
    let st = &rt.st;
    let mut done = vec![];
    for arg in &e.args {
        let Some(id) = resolve_item(st, arg).await else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        let Some(item) = sql!("SELECT id, dest, mime, src FROM items WHERE id = ", id, " AND active = true LIMIT 1").fetch_optional(&st.db).await? else {
            e.reply(&format!("f0ck {arg}: f0ck not found")).await;
            continue;
        };
        pipeline::gen_thumbnail(st, &item.string("dest"), &item.string("mime"), id as i64, &item.string("src"), false).await?;
        done.push(arg.clone());
    }
    e.reply(&format!("thumbnails: {}/{} ({})", done.len(), e.args.len(), done.join(","))).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bot::tests::test_cfg;

    #[test]
    fn patterns_compile_and_match() {
        let cfg = test_cfg(json!({ "main": { "url": { "full": "https://f0ck.me", "domain": "f0ck.me", "regex": "f0ck\\.me" }, "trigger": "!w" } }));
        for t in all() {
            regex::Regex::new(&t.call.pattern(&cfg)).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        }
        let re = |name: &str| regex::Regex::new(&all().into_iter().find(|t| t.name == name).unwrap().call.pattern(&cfg)).unwrap();
        assert!(re("delete").is_match("!rm 123"));
        assert!(!re("delete").is_match("!rm"));
        assert!(re("matrix_link").is_match("ab12cd"));
        assert!(re("matrix_link").is_match("!link AB12CD"));
        // current link tokens: 64 upper-case hex characters, pasted on their own in the DM
        let token = crate::util::generate_token().to_uppercase();
        assert!(re("matrix_link").is_match(&token));
        assert!(!re("matrix_link").is_match(&format!("{token}0")));
        assert!(!re("matrix_link").is_match("just some chat message"));
        assert!(re("tags show").is_match("!tags show 12"));
        assert!(re("tags show").is_match("!tags show AbC_dEf-123"));
        assert_eq!(all().into_iter().find(|t| t.name == "delete").unwrap().level, 100);
        let g = re("f0ckgag");
        let t = |m: &str| f0ckgag_target(&g, m);
        assert_eq!(t("look https://f0ck.me/video/1234 lol").as_deref(), Some("1234"));
        assert_eq!(t("https://f0ck.me/AbC_dEf-123").as_deref(), Some("AbC_dEf-123"));
        assert_eq!(t("see https://f0ck.me/tag/cats/AbC_dEf-123?x=1").as_deref(), Some("AbC_dEf-123"));
        assert_eq!(t("https://f0ck.me/b/abcdefgh.mp4").as_deref(), Some("abcdefgh.mp4"));
        // longer path words aren't slugs
        assert_eq!(t("https://f0ck.me/notifications"), None);
        assert!(g.is_match("https://f0ck.me/user/x/favs/77"));
        assert!(!g.is_match("https://example.com/1234"));
    }

    #[test]
    fn help_fits_irc() {
        let cmds: Vec<String> = ["!del <id> ...", "!help", "!w <url> or attachment"].iter().map(|s| s.to_string()).collect();
        let text = format_help(Kind::Irc, &cmds);
        let lines = crate::bot::irc::build_lines(crate::bot::Mode::Normal, "#c", &text).expect("IRC refuses the help text");
        assert_eq!(lines, vec!["PRIVMSG #c :\x02Available commands:\x02 !del <id> ... | !help | !w <url> or attachment"]);
        assert!(format_help(Kind::Matrix, &cmds).contains("```\n!del <id> ...\n!help\n"));
    }
}
