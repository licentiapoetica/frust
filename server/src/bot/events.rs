//! Bot runtime: the `self` object of `src/index.mjs` (trigger registry) and the event handlers of
//! `src/inc/events/*.mjs` (message → trigger dispatch, CTCP replies, error/info logging).

use super::triggers::{self, Trigger};
use super::{Event, Kind, Message, get_level};
use crate::state::AppState;
use parking_lot::Mutex;
use regex::Regex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Runtime {
    pub st: AppState,
    pub triggers: Vec<Trigger>,
    regex_cache: Mutex<HashMap<String, Option<Regex>>>,
}

const VERSIONS: [&str; 2] = ["AmIRC.1 (8 Bit) for Commodore Amiga 500", "HexChat 0.72 [x86] / Windows 95c [500MHz]"];

/// `parseArgs(msg)`: first word without a leading `.`, `/` or `!` → `cmd`, the rest → `args`.
pub fn parse_args(msg: &str) -> (String, Vec<String>) {
    let mut words = msg.split_whitespace();
    let cmd = words.next().unwrap_or("");
    let cmd = cmd.strip_prefix(['.', '/', '!']).unwrap_or(cmd).to_string();
    (cmd, words.map(str::to_string).collect())
}

/// Matrix reply fallback (`> <@user:x> quoted` lines + blank line) is stripped from the message.
pub fn strip_reply_fallback(msg: &str) -> String {
    let lines: Vec<&str> = msg.split('\n').collect();
    let i = lines.iter().take_while(|l| l.starts_with('>') || l.trim().is_empty()).count();
    if i > 0 { lines[i..].join("\n").trim().to_string() } else { msg.to_string() }
}

impl Runtime {
    pub fn new(st: AppState) -> Runtime {
        Runtime { st, triggers: triggers::all(), regex_cache: Mutex::new(HashMap::new()) }
    }

    /// Compile (and cache) a regex; invalid patterns are logged once and never match.
    pub fn regex(&self, pattern: &str) -> Option<Regex> {
        let mut cache = self.regex_cache.lock();
        if let Some(r) = cache.get(pattern) {
            return r.clone();
        }
        let r = Regex::new(pattern).map_err(|e| tracing::error!("[BOT] invalid trigger regex {pattern:?}: {e}")).ok();
        if cache.len() > 64 {
            cache.clear();
        }
        cache.insert(pattern.to_string(), r.clone());
        r
    }

    pub async fn dispatch(self: Arc<Self>, mut rx: mpsc::UnboundedReceiver<Event>) {
        while let Some(ev) = rx.recv().await {
            let rt = self.clone();
            tokio::spawn(async move { rt.handle(ev).await });
        }
    }

    async fn handle(self: Arc<Self>, ev: Event) {
        match ev {
            Event::Message(m) => self.on_message(m).await,
            Event::CtcpVersion(e) => {
                tracing::info!("{} -> {} -> {}: ctcp:version {}", e.network, e.channel, e.user.nick, e.message);
                let v = VERSIONS[rand::random::<usize>() % VERSIONS.len()];
                e.write(&format!("NOTICE {} :\u{1}VERSION {v}\u{1}", e.user.nick));
            }
            Event::CtcpPing(e) => {
                tracing::info!("{} -> {} -> {}: ctcp:ping {}", e.network, e.channel, e.user.nick, e.message);
                e.write(&format!("NOTICE {} :{}", e.user.nick, e.message));
            }
            Event::Error(e) => tracing::error!("{e}"),
            Event::Info(e) | Event::Debug(e) | Event::Notice(e) => tracing::debug!("{e}"),
            Event::Motd(_) => {}
            Event::Connected(e) => tracing::info!("[BOT] connected: {e}"),
            Event::Ready(e) => tracing::info!("[BOT] {e} ready"),
        }
    }

    /// Triggers that fire for a message (`events/message.mjs`).
    pub fn matching(&self, e: &Message) -> Vec<&Trigger> {
        if e.media.is_some() {
            return self.triggers.iter().filter(|t| t.name == "parser").collect();
        }
        let cfg = self.st.cfg();
        let level = get_level(&cfg, &e.user).0;
        self.triggers
            .iter()
            .filter(|t| t.active && t.clients.contains(&e.kind) && t.level <= level)
            .filter(|t| self.regex(&t.call.pattern(&cfg)).is_some_and(|r| r.is_match(&e.message)))
            .collect()
    }

    async fn on_message(self: Arc<Self>, mut e: Message) {
        tracing::info!("{} -> {} -> {}: {}", e.network, e.channel, e.user.nick, e.message);
        if e.kind == Kind::Matrix {
            e.message = strip_reply_fallback(&e.message);
        }
        let (cmd, args) = parse_args(&e.message);
        e.cmd = cmd;
        e.args = args;
        for t in self.matching(&e) {
            let (name, f) = (t.name, t.f);
            let rt = self.clone();
            let e = e.clone();
            tokio::spawn(async move {
                let (network, channel, nick) = (e.network.clone(), e.channel.clone(), e.user.nick.clone());
                let reply_to = e.clone();
                match f(rt, e).await {
                    Ok(()) => tracing::debug!("triggered > {name}"),
                    Err(err) => {
                        tracing::error!("{network} -> {channel} -> {nick}: {name}: {err:#}");
                        reply_to.reply(&format!("{name}: An error occured.")).await;
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args() {
        assert_eq!(parse_args("!del 1  2"), ("del".to_string(), vec!["1".to_string(), "2".to_string()]));
        assert_eq!(parse_args(".tags show 5").0, "tags");
        assert_eq!(parse_args("").0, "");
    }

    #[test]
    fn reply_fallback() {
        assert_eq!(strip_reply_fallback("> <@a:x> hi\n> more\n\n!w -sfw -t a"), "!w -sfw -t a");
        assert_eq!(strip_reply_fallback("plain\n> not a quote"), "plain\n> not a quote");
    }
}
