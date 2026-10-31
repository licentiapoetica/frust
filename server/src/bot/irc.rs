//! IRC client (`cuffeo/src/clients/irc.mjs` + `irc/*.mjs`).
//!
//! Plain TCP or TLS (`ssl`, `selfSigned`), SASL PLAIN (`sasl`), server password, nick/user/realname,
//! auto-join of `channels`, PING/PONG, WHO/WHOIS user cache (used for `e.user.account`), CTCP
//! VERSION/PING events, invites. Reconnects with exponential backoff; a dead connection is detected
//! by a read timeout + our own PING.

use super::{Backoff, Client, Emitter, Event, Message, Mode, User};
use base64::Engine;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

/// Re-WHOIS a cached user after 30 minutes (`_recachetime`).
const RECACHE_SECS: i64 = 60 * 30;
/// Max bytes of message text per PRIVMSG line (the server adds our prefix when relaying; 512 total).
const MAX_LINE_TEXT: usize = 400;
/// Old limit: refuse messages with 5 or more lines.
const MAX_LINES: usize = 5;
/// No data for this long → send PING; no data for twice as long → reconnect.
const IDLE_PING: Duration = Duration::from_secs(180);

#[derive(Clone, Debug)]
pub struct IrcOpts {
    pub host: String,
    pub port: u16,
    pub ssl: bool,
    pub self_signed: bool,
    pub sasl: bool,
    pub nickname: String,
    pub username: String,
    pub realname: String,
    pub password: String,
    pub channels: Vec<String>,
}

impl IrcOpts {
    pub fn from_config(c: &Value) -> (String, IrcOpts) {
        let s = |k: &str, d: &str| c[k].as_str().filter(|v| !v.is_empty()).unwrap_or(d).to_string();
        let channels = match &c["channels"] {
            Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            Value::String(s) => s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
            _ => vec![],
        };
        let network = s("network", "test");
        let opts = IrcOpts {
            host: s("host", "127.0.0.1"),
            port: crate::config::as_int(&c["port"]).and_then(|p| u16::try_from(p).ok()).filter(|p| *p > 0).unwrap_or(6667),
            ssl: crate::config::truthy(&c["ssl"]),
            self_signed: crate::config::truthy(&c["selfSigned"]),
            sasl: crate::config::truthy(&c["sasl"]),
            nickname: s("nickname", "test"),
            username: s("username", "test"),
            realname: s("realname", "test"),
            password: c["password"].as_str().unwrap_or("").to_string(),
            channels,
        };
        (network, opts)
    }
}

/// WHOIS cache entry (`server.user`).
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct IrcUser {
    pub nickname: Option<String>,
    pub username: Option<String>,
    pub hostname: Option<String>,
    pub realname: Option<String>,
    pub account: Option<String>,
    pub prefix: Option<String>,
    pub registered: bool,
    pub oper: bool,
    /// channel → mode prefix (`@`, `+`, …)
    pub channels: Vec<(String, String)>,
    pub cached: i64,
}

#[derive(Default)]
struct State {
    nick: String,
    motd: String,
    me: IrcUser,
    /// channel → member nicks (from WHO)
    channels: HashMap<String, Vec<String>>,
    users: HashMap<String, IrcUser>,
    who_chan: Option<String>,
    who_nicks: Vec<String>,
    cap_ls: String,
    cap_done: bool,
    registered: bool,
}

pub struct Irc {
    pub network: String,
    opts: IrcOpts,
    events: Emitter,
    out: Mutex<Option<mpsc::UnboundedSender<String>>>,
    state: Mutex<State>,
    me_arc: Mutex<Option<std::sync::Weak<Irc>>>,
}

/// A parsed IRC line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IrcMsg {
    /// IRCv3 message tags (unparsed)
    pub tags: Option<String>,
    /// Source without the leading `:`
    pub prefix: Option<String>,
    pub command: String,
    pub params: Vec<String>,
}

/// Parse one IRC line (RFC 1459 / IRCv3 framing).
pub fn parse(line: &str) -> Option<IrcMsg> {
    let mut rest = line.trim_end_matches(['\r', '\n']);
    let mut tags = None;
    if let Some(r) = rest.strip_prefix('@') {
        let (t, r) = r.split_once(' ')?;
        tags = Some(t.to_string());
        rest = r.trim_start_matches(' ');
    }
    let mut prefix = None;
    if let Some(r) = rest.strip_prefix(':') {
        let (p, r) = r.split_once(' ')?;
        prefix = Some(p.to_string());
        rest = r.trim_start_matches(' ');
    }
    let (command, mut rest) = match rest.split_once(' ') {
        Some((c, r)) => (c, r),
        None => (rest, ""),
    };
    if command.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    loop {
        rest = rest.trim_start_matches(' ');
        if rest.is_empty() {
            break;
        }
        if let Some(trailing) = rest.strip_prefix(':') {
            params.push(trailing.to_string());
            break;
        }
        match rest.split_once(' ') {
            Some((p, r)) => {
                params.push(p.to_string());
                rest = r;
            }
            None => {
                params.push(rest.to_string());
                break;
            }
        }
    }
    Some(IrcMsg { tags, prefix, command: command.to_ascii_uppercase(), params })
}

/// `parsePrefix`: `nick!user@host` → (nick, user, host).
pub fn parse_prefix(prefix: &str) -> Option<(String, String, String)> {
    let p = prefix.strip_prefix(':').unwrap_or(prefix);
    let at = p.rfind('@')?;
    let (left, host) = (&p[..at], &p[at + 1..]);
    let bang = left.rfind('!')?;
    Some((left[..bang].to_string(), left[bang + 1..].to_string(), host.to_string()))
}

fn color_code(name: &str) -> Option<&'static str> {
    const COLORS: [&str; 16] = [
        "white", "black", "navy", "green", "red", "brown", "purple", "orange", "yellow", "lightgreen", "teal", "cyan", "blue", "magenta", "gray", "lightgray",
    ];
    const CODES: [&str; 16] = ["00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13", "14", "15"];
    COLORS.iter().position(|c| *c == name).map(|i| CODES[i])
}

/// `format(msg)`: `[b]` → bold, `[i]` → italic, `[color=x]` → mIRC colour codes.
pub fn format(msg: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static B: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[b\](.*?)\[/b\]").unwrap());
    static I: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[i\](.*?)\[/i\]").unwrap());
    static C: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[color=(.*?)\](.*?)\[/color\]").unwrap());
    let s = B.replace_all(msg, "\x02${1}\x02");
    let s = I.replace_all(&s, "\x1D${1}\x1D");
    C.replace_all(&s, |c: &regex::Captures| match color_code(&c[1]) {
        Some(code) => format!("\x03{code}{}\x0F", &c[2]),
        None => c[2].to_string(),
    })
    .into_owned()
}

/// Split text into chunks of at most `max` bytes on character boundaries.
pub fn split_bytes(s: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = s;
    while rest.len() > max {
        let mut cut = max;
        while cut > 0 && !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if cut == 0 {
            break;
        }
        out.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// Build the raw lines for `sendmsg(mode, recipient, msg)`. `Err` when the message has too many lines.
pub fn build_lines(mode: Mode, recipient: &str, msg: &str) -> Result<Vec<String>, &'static str> {
    let lines: Vec<&str> = msg.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    if lines.len() >= MAX_LINES {
        return Err("[irc] too many lines");
    }
    let mut out = Vec::new();
    for line in lines {
        // a stray CR or NUL would end the line early on some servers (command injection)
        let line = format(line).replace(['\r', '\0'], "");
        for chunk in split_bytes(&line, MAX_LINE_TEXT) {
            out.push(match mode {
                Mode::Normal => format!("PRIVMSG {recipient} :{chunk}"),
                Mode::Action => format!("PRIVMSG {recipient} :\x01ACTION {chunk}\x01"),
                Mode::Notice => format!("NOTICE {recipient} :{chunk}"),
            });
        }
    }
    Ok(out)
}

fn now() -> i64 {
    crate::util::now_secs()
}

impl Irc {
    pub fn new(cfg: &Value, events: Emitter) -> Arc<Irc> {
        let (network, opts) = IrcOpts::from_config(cfg);
        Irc::with_opts(network, opts, events)
    }

    pub fn with_opts(network: String, opts: IrcOpts, events: Emitter) -> Arc<Irc> {
        let state = State { nick: opts.nickname.clone(), ..Default::default() };
        let irc = Arc::new(Irc { network, opts, events, out: Mutex::new(None), state: Mutex::new(state), me_arc: Mutex::new(None) });
        *irc.me_arc.lock() = Some(Arc::downgrade(&irc));
        irc
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn arc(&self) -> Option<Arc<Irc>> {
        self.me_arc.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// Queue a raw line (CR/LF stripped — one command per call).
    pub fn send(&self, data: &str) {
        let line = data.split(['\r', '\n']).next().unwrap_or("").to_string();
        match self.out.lock().as_ref() {
            Some(tx) => {
                let _ = tx.send(line);
            }
            None => self.emit(Event::Info(format!("[irc] nope: {line}"))),
        }
    }

    pub fn sendmsg(&self, mode: Mode, recipient: &str, msg: &str) {
        match build_lines(mode, recipient, msg) {
            Ok(lines) => lines.iter().for_each(|l| self.send(l)),
            Err(e) => self.emit(Event::Error(e.into())),
        }
    }

    pub fn join(&self, channels: &[String]) {
        if !channels.is_empty() {
            self.send(&format!("JOIN {}", channels.join(",")));
        }
    }

    pub fn part(&self, channels: &[String], msg: Option<&str>) {
        self.send(&format!("PART {} {}", channels.join(","), msg.unwrap_or("part")));
    }

    pub fn whois(&self, users: &[String], force: bool) {
        let list: Vec<String> = {
            let st = self.state.lock();
            users
                .iter()
                .filter(|u| !u.trim().is_empty())
                .filter(|u| force || st.users.get(u.as_str()).map(|x| x.cached).unwrap_or(0) < now() - RECACHE_SECS)
                .cloned()
                .collect()
        };
        if list.is_empty() {
            return;
        }
        self.emit(Event::Info(format!("[irc] whois > {}", list.join(","))));
        self.send(&format!("WHOIS {}", list.join(",")));
    }

    pub fn cached_user(&self, nick: &str) -> Option<Value> {
        self.state.lock().users.get(nick).and_then(|u| serde_json::to_value(u).ok())
    }

    pub fn nick(&self) -> String {
        self.state.lock().nick.clone()
    }

    /// Reconnect loop. Never returns.
    pub async fn run(self: Arc<Self>) {
        let mut backoff = Backoff::new();
        loop {
            match self.session().await {
                Ok(registered) => {
                    if registered {
                        backoff.reset();
                    }
                    self.emit(Event::Error("[irc] stream ended, reconnecting in progress".into()));
                }
                Err(e) => self.emit(Event::Error(format!("[irc] {}: {e:#}", self.network))),
            }
            *self.out.lock() = None;
            let wait = backoff.next();
            self.emit(Event::Info(format!("[irc] reconnecting to {} in {}s", self.network, wait.as_secs())));
            tokio::time::sleep(wait).await;
        }
    }

    /// One connection. Returns whether registration (001) succeeded.
    async fn session(&self) -> anyhow::Result<bool> {
        let o = &self.opts;
        let stream = super::net::connect(&o.host, o.port, o.ssl, o.self_signed).await?;
        let (rd, mut wr) = tokio::io::split(stream);
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        {
            let mut st = self.state.lock();
            st.nick = o.nickname.clone();
            st.cap_ls.clear();
            st.cap_done = false;
            st.registered = false;
            st.channels.clear();
        }
        *self.out.lock() = Some(tx);
        let writer = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let mut buf = line.into_bytes();
                buf.extend_from_slice(b"\r\n");
                if wr.write_all(&buf).await.is_err() {
                    break;
                }
            }
            let _ = wr.shutdown().await;
        });

        // CAP LS first so registration waits for SASL
        if o.sasl {
            self.send("CAP LS");
        } else if !o.password.is_empty() {
            self.send(&format!("PASS {}", o.password));
        }
        self.send(&format!("NICK {}", o.nickname));
        self.send(&format!("USER {} 0 * :{}", o.username, o.realname));
        self.emit(Event::Info("[irc] connected!".into()));

        let mut reader = BufReader::new(rd);
        let mut buf = Vec::with_capacity(1024);
        let mut pinged = false;
        let result = loop {
            // a timed-out read keeps its partial data in `buf`; only clear after a full line
            let read = tokio::time::timeout(IDLE_PING, (&mut reader).take(16 * 1024).read_until(b'\n', &mut buf)).await;
            match read {
                Err(_) => {
                    if pinged {
                        break Err(anyhow::anyhow!("ping timeout"));
                    }
                    pinged = true;
                    self.send(&format!("PING :{}", o.host));
                }
                Ok(Ok(0)) => break Ok(()),
                Ok(Ok(_)) => {
                    pinged = false;
                    let line = String::from_utf8_lossy(&buf).trim_end_matches(['\r', '\n']).to_string();
                    buf.clear();
                    if let Some(msg) = parse(&line) {
                        self.handle(&msg);
                    }
                }
                Ok(Err(e)) => break Err(e.into()),
            }
        };
        *self.out.lock() = None;
        writer.abort();
        let registered = self.state.lock().registered;
        result.map(|_| registered)
    }

    /// `reply(tmp)`: the message object handed to events/triggers.
    fn reply(&self, msg: &IrcMsg) -> Option<Message> {
        let prefix = msg.prefix.clone().unwrap_or_default();
        let (nick, username, host) = parse_prefix(&prefix).unwrap_or_default();
        let target = msg.params.first()?.clone();
        let text = msg.params.get(1).cloned().unwrap_or_default();
        let st = self.state.lock();
        let account = st.users.get(&nick).and_then(|u| u.account.clone());
        // private message to us → answer the sender, not ourselves
        let reply_target = if target.eq_ignore_ascii_case(&st.nick) { nick.clone() } else { target.clone() };
        drop(st);
        let me = self.arc()?;
        Some(Message {
            kind: super::Kind::Irc,
            network: self.network.clone(),
            channel: target.clone(),
            channelid: target,
            user: User { prefix: format!("{prefix}@{}", self.network), nick, username: Some(username), account, hostname: Some(host) },
            message: text.replacen('\u{2}', "", 1),
            time: now(),
            media: None,
            reply_to: None,
            raw: json!({ "prefix": msg.prefix, "command": msg.command, "params": msg.params }),
            target: reply_target,
            client: Client::Irc(me),
            cmd: String::new(),
            args: vec![],
        })
    }

    fn user_entry<'a>(st: &'a mut State, nick: &str) -> &'a mut IrcUser {
        st.users.entry(nick.to_string()).or_default()
    }

    /// Protocol handlers (`irc/*.mjs`).
    pub fn handle(&self, msg: &IrcMsg) {
        let p = |i: usize| msg.params.get(i).map(String::as_str).unwrap_or("");
        match msg.command.as_str() {
            "PING" => self.send(&format!("PONG :{}", msg.params.last().map(String::as_str).unwrap_or(""))),
            "CAP" => {
                // CAP <target> <sub> [*] :<caps>
                let sub = p(1).to_ascii_uppercase();
                let more = p(2) == "*" && msg.params.len() > 3;
                let caps = if more { p(3) } else { p(2) };
                match sub.as_str() {
                    "LS" => {
                        let all = {
                            let mut st = self.state.lock();
                            st.cap_ls.push(' ');
                            st.cap_ls.push_str(caps);
                            if more {
                                return;
                            }
                            std::mem::take(&mut st.cap_ls)
                        };
                        if all.split_whitespace().any(|c| c == "sasl" || c.starts_with("sasl=")) {
                            self.send("CAP REQ :sasl");
                        } else {
                            self.emit(Event::Error("[irc] server does not support SASL".into()));
                            self.cap_end();
                        }
                    }
                    "ACK" if caps.split_whitespace().any(|c| c == "sasl") => self.send("AUTHENTICATE PLAIN"),
                    "NAK" => self.cap_end(),
                    _ => {}
                }
            }
            "AUTHENTICATE" => {
                if p(0) == "+" {
                    let o = &self.opts;
                    let token = base64::engine::general_purpose::STANDARD.encode(format!("{}\0{}\0{}", o.username, o.username, o.password));
                    self.send(&format!("AUTHENTICATE {token}"));
                }
            }
            // logged in / SASL success / failure → finish capability negotiation
            "900" | "903" => self.cap_end(),
            "902" | "904" | "905" | "906" | "907" => {
                self.emit(Event::Error(format!("[irc] SASL failed: {}", msg.params.last().map(String::as_str).unwrap_or(""))));
                self.cap_end();
            }
            "433" => {
                // nick in use: append "_" and retry (only while registering)
                let next = {
                    let mut st = self.state.lock();
                    if st.registered {
                        return;
                    }
                    st.nick.push('_');
                    st.nick.clone()
                };
                self.send(&format!("NICK {next}"));
            }
            "001" => {
                {
                    let mut st = self.state.lock();
                    st.registered = true;
                    if !p(0).is_empty() {
                        st.nick = p(0).to_string();
                    }
                }
                self.join(&self.opts.channels);
                self.emit(Event::Connected(p(1).to_string()));
            }
            "INVITE" => {
                let channel = p(1).to_string();
                let nick = msg.prefix.as_deref().and_then(parse_prefix).map(|x| x.0).unwrap_or_default();
                if channel.is_empty() || self.state.lock().channels.contains_key(&channel) {
                    return;
                }
                self.join(std::slice::from_ref(&channel));
                if let Some(me) = self.arc() {
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        me.send(&format!("PRIVMSG {channel} :Hi. Wurde von {nick} eingeladen."));
                    });
                }
            }
            "JOIN" => {
                let nick = msg.prefix.as_deref().and_then(parse_prefix).map(|x| x.0).unwrap_or_default();
                let chan = p(0).to_string();
                if nick.eq_ignore_ascii_case(&self.nick()) {
                    self.state.lock().channels.entry(chan.clone()).or_default();
                    self.send(&format!("WHO {chan}"));
                } else if !nick.is_empty() {
                    self.whois(&[nick], false);
                }
            }
            "PART" | "KICK" => {
                let me = self.nick();
                let gone = if msg.command == "KICK" { p(1).to_string() } else { msg.prefix.as_deref().and_then(parse_prefix).map(|x| x.0).unwrap_or_default() };
                if gone.eq_ignore_ascii_case(&me) {
                    self.state.lock().channels.remove(p(0));
                }
            }
            "NICK" => {
                let old = msg.prefix.as_deref().and_then(parse_prefix).map(|x| x.0).unwrap_or_default();
                let new = p(0).to_string();
                {
                    let mut st = self.state.lock();
                    st.users.remove(&old);
                    if st.nick.eq_ignore_ascii_case(&old) {
                        st.nick = new.clone();
                    }
                }
                self.whois(&[new], true);
            }
            "375" => self.state.lock().motd = format!("{}\n", p(1)),
            "372" => {
                let mut st = self.state.lock();
                st.motd.push_str(p(1));
                st.motd.push('\n');
            }
            "376" => {
                let motd = {
                    let mut st = self.state.lock();
                    st.motd.push_str(p(1));
                    st.motd.push('\n');
                    st.motd.clone()
                };
                self.emit(Event::Motd(motd));
            }
            "PRIVMSG" => {
                let text = p(1);
                let Some(m) = self.reply(msg) else { return };
                if text == "\u{1}VERSION\u{1}" {
                    self.emit(Event::CtcpVersion(m));
                } else if text.len() > 6 && text[..6].eq_ignore_ascii_case("\u{1}PING ") && text.ends_with('\u{1}') {
                    self.emit(Event::CtcpPing(m));
                } else {
                    self.emit(Event::Message(m));
                }
            }
            "NOTICE" => self.emit(Event::Notice(p(1).to_string())),
            // WHO
            "352" => {
                let mut st = self.state.lock();
                st.who_chan = Some(p(1).to_string());
                st.who_nicks.push(p(5).to_string());
            }
            "315" => {
                let nicks = {
                    let mut st = self.state.lock();
                    let mut nicks = std::mem::take(&mut st.who_nicks);
                    if let Some(chan) = st.who_chan.take() {
                        st.channels.insert(chan, nicks.clone());
                    }
                    let mut seen = std::collections::HashSet::new();
                    nicks.retain(|n| seen.insert(n.clone()));
                    nicks
                };
                for batch in nicks.chunks(10) {
                    self.whois(batch, false);
                }
            }
            // WHOIS
            "307" => {
                let mut st = self.state.lock();
                let u = Irc::user_entry(&mut st, p(1));
                u.account = Some(p(1).to_string());
                u.registered = true;
            }
            "311" => {
                let mut st = self.state.lock();
                let u = Irc::user_entry(&mut st, p(1));
                u.nickname = Some(p(1).to_string());
                u.username = Some(p(2).to_string());
                u.hostname = Some(p(3).to_string());
                u.realname = Some(p(5).to_string());
                u.prefix = Some(format!("{}!{}@{}", p(1), p(2), p(3)));
            }
            "313" => Irc::user_entry(&mut self.state.lock(), p(1)).oper = true,
            "319" => {
                let mut st = self.state.lock();
                let u = Irc::user_entry(&mut st, p(1));
                for chan in p(2).split_whitespace() {
                    let (mode, name) = chan.split_once('#').unwrap_or(("", chan));
                    let key = format!("#{name}");
                    u.channels.retain(|(c, _)| c != &key);
                    u.channels.push((key, mode.to_string()));
                }
            }
            "330" => {
                let mut st = self.state.lock();
                let u = Irc::user_entry(&mut st, p(1));
                u.account = Some(p(2).to_string());
                u.registered = true;
            }
            "318" => {
                let mut st = self.state.lock();
                let mut u = st.users.remove(p(1)).unwrap_or_default();
                u.cached = now();
                if p(0) == p(1) {
                    st.me = u;
                } else {
                    st.users.insert(p(1).to_string(), u);
                }
            }
            "ERROR" => self.emit(Event::Error(format!("[irc] server error: {}", p(0)))),
            _ => {}
        }
    }

    fn cap_end(&self) {
        let done = std::mem::replace(&mut self.state.lock().cap_done, true);
        if !done {
            self.send("CAP END");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(sasl: bool) -> (Arc<Irc>, mpsc::UnboundedReceiver<String>, mpsc::UnboundedReceiver<Event>) {
        let (etx, erx) = mpsc::unbounded_channel();
        let opts = IrcOpts {
            host: "irc.example.net".into(),
            port: 6697,
            ssl: true,
            self_signed: false,
            sasl,
            nickname: "bot".into(),
            username: "botuser".into(),
            realname: "Bot".into(),
            password: "pw".into(),
            channels: vec!["#a".into(), "#b".into()],
        };
        let irc = Irc::with_opts("net".into(), opts, etx);
        let (tx, rx) = mpsc::unbounded_channel();
        *irc.out.lock() = Some(tx);
        (irc, rx, erx)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<String>) -> Vec<String> {
        let mut v = vec![];
        while let Ok(l) = rx.try_recv() {
            v.push(l);
        }
        v
    }

    #[test]
    fn parses_lines() {
        let m = parse(":nick!user@host PRIVMSG #chan :hello :world").unwrap();
        assert_eq!(m.prefix.as_deref(), Some("nick!user@host"));
        assert_eq!(m.command, "PRIVMSG");
        assert_eq!(m.params, vec!["#chan", "hello :world"]);

        let m = parse("PING :irc.example.net\r\n").unwrap();
        assert_eq!(m.prefix, None);
        assert_eq!(m.params, vec!["irc.example.net"]);

        let m = parse("@time=2020-01-01T00:00:00Z :srv 001 bot :Welcome to IRC").unwrap();
        assert_eq!(m.tags.as_deref(), Some("time=2020-01-01T00:00:00Z"));
        assert_eq!(m.command, "001");
        assert_eq!(m.params, vec!["bot", "Welcome to IRC"]);

        let m = parse(":srv CAP * LS :sasl multi-prefix").unwrap();
        assert_eq!(m.params, vec!["*", "LS", "sasl multi-prefix"]);

        let m = parse(":srv 352 bot #c ~u host srv nick H :0 Real Name").unwrap();
        assert_eq!(m.params[5], "nick");

        let m = parse("QUIT").unwrap();
        assert!(m.params.is_empty());
        assert!(parse("").is_none());
        assert!(parse(":onlyprefix").is_none());
    }

    #[test]
    fn parses_prefix() {
        assert_eq!(parse_prefix(":a!b@c"), Some(("a".into(), "b".into(), "c".into())));
        assert_eq!(parse_prefix("a!~b@c.d.e"), Some(("a".into(), "~b".into(), "c.d.e".into())));
        assert_eq!(parse_prefix("irc.server.net"), None);
    }

    #[test]
    fn formats_bbcode() {
        assert_eq!(format("[b]x[/b] [i]y[/i]"), "\x02x\x02 \x1Dy\x1D");
        assert_eq!(format("[color=green]sfw[/color]"), "\x0303sfw\x0F");
        assert_eq!(format("[color=nope]t[/color]"), "t");
    }

    #[test]
    fn builds_and_splits_messages() {
        assert_eq!(build_lines(Mode::Normal, "#c", "a\r\nb").unwrap(), vec!["PRIVMSG #c :a", "PRIVMSG #c :b"]);
        assert_eq!(build_lines(Mode::Action, "#c", "waves").unwrap(), vec!["PRIVMSG #c :\x01ACTION waves\x01"]);
        assert_eq!(build_lines(Mode::Notice, "n", "hi").unwrap(), vec!["NOTICE n :hi"]);
        assert!(build_lines(Mode::Normal, "#c", "1\n2\n3\n4\n5").is_err());
        // CR inside a line can't inject a second command
        assert_eq!(build_lines(Mode::Normal, "#c", "a\rQUIT").unwrap(), vec!["PRIVMSG #c :aQUIT"]);
        let long = "ä".repeat(300); // 600 bytes
        let lines = build_lines(Mode::Normal, "#c", &long).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| l.len() <= MAX_LINE_TEXT + "PRIVMSG #c :".len()));
        assert_eq!(split_bytes("abc", 2), vec!["ab", "c"]);
    }

    #[tokio::test]
    async fn registration_and_ping() {
        let (irc, mut out, mut ev) = client(false);
        irc.handle(&parse("PING :abc").unwrap());
        assert_eq!(drain(&mut out), vec!["PONG :abc"]);
        irc.handle(&parse(":srv 433 * bot :Nickname is already in use").unwrap());
        assert_eq!(drain(&mut out), vec!["NICK bot_"]);
        irc.handle(&parse(":srv 001 bot_ :Welcome").unwrap());
        assert_eq!(drain(&mut out), vec!["JOIN #a,#b"]);
        assert!(matches!(ev.try_recv(), Ok(Event::Connected(s)) if s == "Welcome"));
        assert_eq!(irc.nick(), "bot_");
        irc.handle(&parse(":bot_!u@h JOIN #a").unwrap());
        assert_eq!(drain(&mut out), vec!["WHO #a"]);
    }

    #[tokio::test]
    async fn sasl_flow() {
        let (irc, mut out, _ev) = client(true);
        irc.handle(&parse(":srv CAP * LS :multi-prefix sasl").unwrap());
        assert_eq!(drain(&mut out), vec!["CAP REQ :sasl"]);
        irc.handle(&parse(":srv CAP * ACK :sasl").unwrap());
        assert_eq!(drain(&mut out), vec!["AUTHENTICATE PLAIN"]);
        irc.handle(&parse("AUTHENTICATE +").unwrap());
        let expected = base64::engine::general_purpose::STANDARD.encode("botuser\0botuser\0pw");
        assert_eq!(drain(&mut out), vec![format!("AUTHENTICATE {expected}")]);
        irc.handle(&parse(":srv 900 bot bot!u@h botuser :You are now logged in").unwrap());
        irc.handle(&parse(":srv 903 bot :SASL authentication successful").unwrap());
        assert_eq!(drain(&mut out), vec!["CAP END"]);
    }

    #[tokio::test]
    async fn messages_and_whois_cache() {
        let (irc, mut out, mut ev) = client(false);
        irc.handle(&parse(":srv 311 bot alice ~al host.example * :Alice A").unwrap());
        irc.handle(&parse(":srv 330 bot alice alice_acc :is logged in as").unwrap());
        irc.handle(&parse(":srv 319 bot alice :@#a +#b").unwrap());
        irc.handle(&parse(":srv 318 bot alice :End of /WHOIS list.").unwrap());
        let u = irc.cached_user("alice").unwrap();
        assert_eq!(u["account"], "alice_acc");
        assert_eq!(u["prefix"], "alice!~al@host.example");
        assert_eq!(u["channels"][0], json!(["#a", "@"]));

        irc.handle(&parse(":alice!~al@host.example PRIVMSG #a :\u{2}hi\u{2} there").unwrap());
        match ev.try_recv() {
            Ok(Event::Message(m)) => {
                assert_eq!(m.message, "hi\u{2} there");
                assert_eq!(m.channel, "#a");
                assert_eq!(m.target, "#a");
                assert_eq!(m.user.nick, "alice");
                assert_eq!(m.user.prefix, "alice!~al@host.example@net");
                assert_eq!(m.user.account.as_deref(), Some("alice_acc"));
                m.reply("[b]ok[/b]").await;
                assert_eq!(drain(&mut out), vec!["PRIVMSG #a :\x02ok\x02"]);
            }
            _ => panic!("expected message"),
        }
        // private message → reply to the sender
        irc.handle(&parse(":bob!b@h PRIVMSG bot :yo").unwrap());
        match ev.try_recv() {
            Ok(Event::Message(m)) => assert_eq!(m.target, "bob"),
            _ => panic!("expected message"),
        }
        irc.handle(&parse(":bob!b@h PRIVMSG bot :\u{1}VERSION\u{1}").unwrap());
        assert!(matches!(ev.try_recv(), Ok(Event::CtcpVersion(_))));
        irc.handle(&parse(":bob!b@h PRIVMSG bot :\u{1}PING 123\u{1}").unwrap());
        assert!(matches!(ev.try_recv(), Ok(Event::CtcpPing(_))));
    }

    #[tokio::test]
    async fn who_batches_whois() {
        let (irc, mut out, _ev) = client(false);
        for i in 0..12 {
            irc.handle(&parse(&format!(":srv 352 bot #a u h srv n{i} H :0 R")).unwrap());
        }
        irc.handle(&parse(":srv 352 bot #a u h srv n0 H :0 R").unwrap());
        irc.handle(&parse(":srv 315 bot #a :End of /WHO list.").unwrap());
        let lines = drain(&mut out);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "WHOIS n0,n1,n2,n3,n4,n5,n6,n7,n8,n9");
        assert_eq!(lines[1], "WHOIS n10,n11");
    }
}
