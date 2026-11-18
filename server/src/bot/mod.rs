//! Chat bots (IRC, Telegram, Matrix) — native replacement for the `cuffeo` package plus the
//! bot runtime of `src/index.mjs` (`self._trigger`, event registration), `src/inc/events/*` and
//! `src/inc/trigger/*`.
//!
//! Layout:
//! * `irc.rs`, `telegram.rs`, `matrix.rs` — protocol clients (`cuffeo/src/clients/*`).
//!   Each client runs its own reconnect loop with backoff and never blocks or crashes the web server.
//! * `net.rs` (TCP/TLS for IRC).
//! * `events.rs` — `src/inc/events/*` (message dispatch, CTCP, logging).
//! * `triggers.rs` — `src/inc/trigger/*` except the link parser; `parser.rs` — `trigger/parser.mjs`.
//! * `pipeline.rs` — the parts of the upload pipeline (`queue.mjs`, `lib_delete.mjs`, …) the
//!   triggers need; the heavy ones are `TODO(port)` stubs until `port/uploads` lands.
//!
//! [`BotHandle`] is the interface the web side uses (`st.bot.send(..)`, `st.bot.notify_matrix(..)`).

pub mod events;
pub mod irc;
pub mod matrix;
pub mod net;
pub mod parser;
pub mod pipeline;
pub mod telegram;
pub mod triggers;

use crate::config::Config;
use crate::state::AppState;
use parking_lot::RwLock;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Client type (`clients[].type` in config.yaml, `e.type` in the old triggers).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Irc,
    Tg,
    Matrix,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Irc => "irc",
            Kind::Tg => "tg",
            Kind::Matrix => "matrix",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "irc" => Some(Kind::Irc),
            "tg" => Some(Kind::Tg),
            "matrix" => Some(Kind::Matrix),
            _ => None,
        }
    }

    /// Default `clients` of a trigger (`["irc", "tg", "matrix"]`).
    pub const ALL: &'static [Kind] = &[Kind::Irc, Kind::Tg, Kind::Matrix];
}

/// `e.user` of the old message objects.
#[derive(Clone, Debug, Default, Serialize)]
pub struct User {
    /// Used for admin matching (`cfg.admins[].prefix`), e.g. `nick!user@host@network`,
    /// `username!123@Telegram`, `@a:b!@a:b`.
    pub prefix: String,
    pub nick: String,
    pub username: Option<String>,
    /// IRC services account / Telegram numeric id / Matrix id.
    pub account: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Message delivery mode (`msgmodes` of the IRC client; the other clients map them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Action,
    Notice,
}

/// A running protocol client.
#[derive(Clone)]
pub enum Client {
    Irc(Arc<irc::Irc>),
    Tg(Arc<telegram::Telegram>),
    Matrix(Arc<matrix::Matrix>),
}

impl Client {
    pub fn kind(&self) -> Kind {
        match self {
            Client::Irc(_) => Kind::Irc,
            Client::Tg(_) => Kind::Tg,
            Client::Matrix(_) => Kind::Matrix,
        }
    }

    /// Network name (`clients[].network` for IRC, fixed names for the others).
    pub fn network(&self) -> String {
        match self {
            Client::Irc(c) => c.network.clone(),
            Client::Tg(_) => "Telegram".into(),
            Client::Matrix(_) => "Matrix".into(),
        }
    }

    /// `client.sendmsg(mode, recipient, msg)` — errors are reported through the event channel.
    pub async fn sendmsg(&self, mode: Mode, target: &str, msg: &str) {
        match self {
            Client::Irc(c) => c.sendmsg(mode, target, msg),
            Client::Tg(c) => {
                c.send(target, msg, None, &json!({})).await;
            }
            Client::Matrix(c) => c.sendmsg(target, msg).await,
        }
    }
}

/// Matrix: the event a message replies to (`e.replyTo`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReplyTo {
    pub sender: String,
    pub message: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "mxcUrl")]
    pub mxc_url: Option<String>,
    #[serde(rename = "type")]
    pub msgtype: Option<String>,
    pub filename: Option<String>,
    pub mimetype: Option<String>,
}

/// An incoming chat message (the object built by `client.reply(tmp)` in cuffeo, plus
/// `cmd`/`args` added by `events/message.mjs`).
#[derive(Clone)]
pub struct Message {
    pub kind: Kind,
    pub network: String,
    pub channel: String,
    pub channelid: String,
    pub user: User,
    pub message: String,
    pub time: i64,
    /// Telegram: URL of an attached file (`e.media`).
    pub media: Option<String>,
    pub reply_to: Option<ReplyTo>,
    /// Raw protocol data (Telegram message object, Matrix event, IRC `{prefix, command, params}`).
    pub raw: Value,
    /// Where replies go (channel / chat id / room id; the sender for IRC private messages).
    pub target: String,
    pub client: Client,
    pub cmd: String,
    pub args: Vec<String>,
}

impl Message {
    /// IRC: the services account the sender is identified to right now (fresh WHOIS: user mode +r
    /// / logged in), else `None`. Always `None` on the other networks, which authenticate senders.
    pub async fn irc_identified(&self) -> Option<String> {
        match &self.client {
            Client::Irc(c) => c.verify(&self.user.nick).await,
            _ => None,
        }
    }

    /// IRC: the bot's current nick (for `/msg <nick> ...` hints).
    pub fn irc_bot_nick(&self) -> String {
        match &self.client {
            Client::Irc(c) => c.nick(),
            _ => String::new(),
        }
    }

    /// IRC: a private message to the bot, not one in a channel.
    pub fn irc_private(&self) -> bool {
        self.kind == Kind::Irc && !self.channel.starts_with(['#', '&', '+', '!'])
    }

    /// `e.reply(msg)`. Telegram returns the sent message (`result` of sendMessage).
    pub async fn reply(&self, text: &str) -> Option<Value> {
        self.reply_opts(text, json!({})).await
    }

    /// `e.reply(msg, opt)` — `opt` is merged into the Telegram sendMessage body; ignored elsewhere.
    pub async fn reply_opts(&self, text: &str, opts: Value) -> Option<Value> {
        match &self.client {
            Client::Irc(c) => {
                c.sendmsg(Mode::Normal, &self.target, text);
                None
            }
            Client::Tg(c) => c.send(&self.target, text, self.raw["message_id"].as_i64(), &opts).await,
            Client::Matrix(c) => {
                c.send(&self.target, &matrix::Outgoing::Text(matrix::format(text)), "text").await;
                None
            }
        }
    }

    /// `e.reply({ body, formatted_body })` — HTML for Matrix, plain body everywhere else.
    pub async fn reply_rich(&self, body: &str, html: &str) {
        match &self.client {
            Client::Matrix(c) => c.send(&self.target, &matrix::Outgoing::Rich { body: body.into(), html: html.into() }, "text").await,
            _ => {
                self.reply(body).await;
            }
        }
    }

    pub async fn reply_action(&self, text: &str) {
        match &self.client {
            Client::Irc(c) => c.sendmsg(Mode::Action, &self.target, text),
            Client::Tg(c) => {
                c.send(&self.target, &format!("Uwe {text}"), self.raw["message_id"].as_i64(), &json!({})).await;
            }
            Client::Matrix(c) => c.send(&self.target, &matrix::Outgoing::Text(matrix::format(text)), "emote").await,
        }
    }

    pub async fn reply_notice(&self, text: &str) {
        match &self.client {
            Client::Irc(c) => c.sendmsg(Mode::Notice, &self.target, text),
            Client::Matrix(c) => c.send(&self.target, &matrix::Outgoing::Text(matrix::format(text)), "notice").await,
            _ => {
                self.reply(text).await;
            }
        }
    }

    /// IRC: `e.write(rawLine)`.
    pub fn write(&self, line: &str) {
        if let Client::Irc(c) = &self.client {
            c.send(line);
        }
    }

    /// Telegram: `e.editMessageText(chat_id, message_id, text, opt)`.
    pub async fn edit_message_text(&self, chat_id: &Value, message_id: i64, text: &str, opts: Value) {
        if let Client::Tg(c) = &self.client {
            c.edit_message_text(chat_id, message_id, text, &opts).await;
        }
    }

    /// Telegram: `e.deleteMessage(chat_id, message_id)`.
    pub async fn delete_message(&self, chat_id: &Value, message_id: i64) {
        if let Client::Tg(c) = &self.client {
            c.delete_message(chat_id, message_id).await;
        }
    }

    /// The client's user cache entry for a nick (`e.self.user.get(nick)`), as JSON.
    pub fn cached_user(&self, name: &str) -> Option<Value> {
        match &self.client {
            Client::Irc(c) => c.cached_user(name),
            Client::Tg(c) => c.cached_user(name),
            Client::Matrix(_) => None,
        }
    }
}

/// Events emitted by the clients (`client.emit("data", [type, data])`).
pub enum Event {
    Message(Message),
    CtcpVersion(Message),
    CtcpPing(Message),
    Error(String),
    Info(String),
    Debug(String),
    Motd(String),
    Connected(String),
    Notice(String),
    Ready(String),
}

pub type Emitter = mpsc::UnboundedSender<Event>;

/// `user_alias.alias` of a linked IRC account: the services account and the network
/// (`clients[].network`), as accounts of different networks are different people.
pub fn irc_alias(account: &str, network: &str) -> String {
    format!("{account}@{network}")
}

/// `getLevel(user)` from `src/inc/admin.mjs`: `(level, verified)` by exact prefix match against
/// `cfg.admins`.
pub fn get_level(cfg: &Config, user: &User) -> (i64, bool) {
    if user.prefix.is_empty() {
        return (0, false);
    }
    if let Some(admins) = cfg.get("admins").as_array() {
        if let Some(a) = admins.iter().find(|a| a["prefix"].as_str() == Some(user.prefix.as_str())) {
            return (crate::config::as_int(&a["level"]).unwrap_or(0), true);
        }
    }
    (0, false)
}

/// Public link to an item as the site itself links it: `<main.url.full>/<slug>` when item slugs are
/// enabled (`enable_item_slugs`) and the item has one, else `<main.url.full>/<id>`. Used for every
/// link the bot posts (replies, upload notices, webhooks).
pub async fn item_url(st: &AppState, id: i64) -> String {
    let cfg = st.cfg();
    let path = if crate::settings::enable_item_slugs(&cfg) {
        crate::sql!("SELECT slug FROM items WHERE id = ", id as i32, " LIMIT 1")
            .fetch_scalar::<String, _>(&st.db)
            .await
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    format!("{}/{}", cfg.url_full(), path.unwrap_or_else(|| id.to_string()))
}

/// Longest title the bot prints, in characters (the site allows 500).
const CHAT_TITLE_MAX: usize = 200;

/// An item title as one chat line, or None when there is none: line breaks and other control
/// characters become spaces (a newline would be a second IRC line), runs of whitespace collapse,
/// long titles end in `…`, and `@room` is broken with a zero-width space so an uploader can't make
/// the bot ping a whole Matrix room.
pub fn chat_title(title: Option<&str>) -> Option<String> {
    let flat = title?.split(|c: char| c.is_whitespace() || c.is_control()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return None;
    }
    let mut out: String = flat.chars().take(CHAT_TITLE_MAX).collect();
    // `out` is a prefix of `flat`, so a shorter byte length means it was cut
    if out.len() < flat.len() {
        out.push('…');
    }
    static ROOM: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?i)@(room)").unwrap());
    Some(ROOM.replace_all(&out, "@\u{200B}${1}").into_owned())
}

/// An item's title from the database, ready for chat (see [`chat_title`]).
pub async fn item_title(st: &AppState, id: i64) -> Option<String> {
    let t = crate::sql!("SELECT title FROM items WHERE id = ", id as i32, " LIMIT 1").fetch_scalar::<String, _>(&st.db).await.ok().flatten();
    chat_title(t.as_deref())
}

/// The bot's new-upload notice: `<who> uploaded a new <what> <link>`, plus ` - title: <title>` when
/// the item has one.
pub async fn upload_notice(st: &AppState, who: &str, what: &str, id: i64) -> String {
    let msg = format!("{who} uploaded a new {what} {}", item_url(st, id).await);
    match item_title(st, id).await {
        Some(t) => format!("{msg} - title: {t}"),
        None => msg,
    }
}

/// Exponential reconnect backoff: 5 s, 10 s, … capped at 5 minutes.
pub(crate) struct Backoff(u32);

impl Backoff {
    pub fn new() -> Backoff {
        Backoff(0)
    }
    pub fn reset(&mut self) {
        self.0 = 0;
    }
    pub fn next(&mut self) -> Duration {
        let secs = (5u64 << self.0.min(6)).min(300);
        self.0 = self.0.saturating_add(1);
        Duration::from_secs(secs)
    }
}

/// Handle used by routes to push messages to chat networks.
#[derive(Clone, Default)]
pub struct BotHandle {
    clients: Arc<RwLock<Vec<Client>>>,
}

impl BotHandle {
    pub fn new() -> BotHandle {
        BotHandle::default()
    }

    /// First running client of a type.
    pub fn client(&self, kind: Kind) -> Option<Client> {
        self.clients.read().iter().find(|c| c.kind() == kind).cloned()
    }

    pub fn clients(&self) -> Vec<Client> {
        self.clients.read().clone()
    }

    fn register(&self, c: Client) {
        self.clients.write().push(c);
    }

    /// Send a plain-text message to a channel/room on the given network
    /// (`"matrix"`, `"tg"`, `"irc"`, or an IRC network name). Errors are logged, never returned.
    pub async fn send(&self, network: &str, target: &str, message: &str) {
        let client = {
            let clients = self.clients.read();
            clients
                .iter()
                .find(|c| c.kind().as_str() == network)
                .or_else(|| clients.iter().find(|c| c.network().eq_ignore_ascii_case(network)))
                .cloned()
        };
        match client {
            // like matrixClient.send(room, msg) in the web routes (no DM handling)
            Some(Client::Matrix(m)) => m.send(target, &matrix::Outgoing::Text(message.to_string()), "text").await,
            Some(c) => c.sendmsg(Mode::Normal, target, message).await,
            None => tracing::info!("[BOT] ({network} not connected) {target}: {message}"),
        }
    }

    /// Matrix upload notification (`clients[type=matrix].notification_channel_id`).
    pub async fn notify_matrix(&self, cfg: &Config, message: &str) {
        if let Some(ch) = cfg.client("matrix").and_then(|c| c["notification_channel_id"].as_str()).filter(|c| !c.is_empty()) {
            self.send("matrix", ch, message).await;
        }
    }
}

/// Start every `enabled: true` client from `cfg.clients` and the event dispatcher.
/// Returns immediately; connecting happens in background tasks with reconnect/backoff.
pub fn start(st: &AppState) {
    let cfg = st.cfg();
    let (tx, rx) = mpsc::unbounded_channel::<Event>();
    let mut started = 0;
    for c in cfg.get("clients").as_array().cloned().unwrap_or_default() {
        if !crate::config::truthy(&c["enabled"]) {
            continue;
        }
        let ty = c["type"].as_str().unwrap_or("");
        let client = match Kind::parse(ty) {
            Some(Kind::Irc) => {
                let cl = irc::Irc::new(&c, tx.clone());
                tokio::spawn(cl.clone().run());
                Client::Irc(cl)
            }
            Some(Kind::Tg) => {
                let cl = telegram::Telegram::new(&c, st.http.clone(), tx.clone());
                tokio::spawn(cl.clone().run());
                Client::Tg(cl)
            }
            Some(Kind::Matrix) => {
                let cl = matrix::Matrix::new(&c, tx.clone());
                tokio::spawn(cl.clone().run());
                Client::Matrix(cl)
            }
            None => {
                tracing::error!("[BOT] not supported client: {ty}");
                continue;
            }
        };
        tracing::info!("[BOT] starting {ty} client ({})", client.network());
        st.bot.register(client);
        started += 1;
    }
    if started == 0 {
        tracing::info!("[BOT] no chat clients enabled");
        return;
    }
    let rt = Arc::new(events::Runtime::new(st.clone()));
    for t in rt.triggers.iter() {
        tracing::debug!("[BOT] registering trigger > {}", t.name);
    }
    tokio::spawn(rt.dispatch(rx));
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn chat_title_is_one_safe_line() {
        assert_eq!(chat_title(None), None);
        assert_eq!(chat_title(Some(" \n\t ")), None);
        assert_eq!(chat_title(Some("  cat\r\nPRIVMSG #x :hi\u{0}  dog ")).as_deref(), Some("cat PRIVMSG #x :hi dog"));
        assert_eq!(chat_title(Some("hey @room and @ROOM")).as_deref(), Some("hey @\u{200B}room and @\u{200B}ROOM"));
        let long = "ä".repeat(CHAT_TITLE_MAX + 5);
        let t = chat_title(Some(&long)).unwrap();
        assert_eq!(t.chars().count(), CHAT_TITLE_MAX + 1);
        assert!(t.ends_with('…'));
        assert_eq!(chat_title(Some(&"x".repeat(CHAT_TITLE_MAX))).unwrap().chars().count(), CHAT_TITLE_MAX);
    }

    pub(crate) fn test_cfg(raw: Value) -> Config {
        let p = std::path::PathBuf::from("/tmp/f0ckm-test");
        let paths = crate::config::Paths {
            a: p.clone(),
            b: p.clone(),
            c: p.clone(),
            t: p.clone(),
            ca: p.clone(),
            s: p.clone(),
            emojis: p.clone(),
            koepfe: p.clone(),
            fonts: p.clone(),
            memes: p.clone(),
            e: p.clone(),
            pending: p.clone(),
            deleted: p.clone(),
            logs: p.clone(),
            tmp: p.clone(),
            import: p.clone(),
        };
        Config { raw, paths, base: p }
    }

    #[test]
    fn backoff_grows_and_caps() {
        let mut b = Backoff::new();
        let v: Vec<u64> = (0..9).map(|_| b.next().as_secs()).collect();
        assert_eq!(v, vec![5, 10, 20, 40, 80, 160, 300, 300, 300]);
        b.reset();
        assert_eq!(b.next().as_secs(), 5);
    }

    #[test]
    fn level_by_prefix() {
        let cfg = test_cfg(json!({"admins": [{"prefix": "a!b@c@net", "level": 100}, {}]}));
        let u = User { prefix: "a!b@c@net".into(), ..Default::default() };
        assert_eq!(get_level(&cfg, &u), (100, true));
        let u = User { prefix: "x!b@c@net".into(), ..Default::default() };
        assert_eq!(get_level(&cfg, &u), (0, false));
        assert_eq!(get_level(&cfg, &User::default()), (0, false));
    }
}
