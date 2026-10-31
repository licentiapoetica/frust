//! Matrix client (`cuffeo/src/clients/matrix.mjs`, which used matrix-js-sdk) on the plain
//! client-server API: `/sync` long polling, room state cache (names, member display names),
//! sending `m.room.message`, joining the configured rooms, DMs via `createRoom`, reply resolution
//! (`e.replyTo`) and authenticated media download.
//!
//! Like the old client: own messages and messages older than 60 s are ignored, only
//! `m.text`/`m.notice`/`m.emote` are delivered, no end-to-end encryption.

use super::{Backoff, Client, Emitter, Event, Kind, Message, ReplyTo, User};
use anyhow::{Context, Result, bail};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const SYNC_TIMEOUT_MS: u64 = 30_000;
const MAX_AGE_MS: i64 = 60_000;
const RECENT_EVENTS: usize = 100;

/// Outgoing message body.
pub enum Outgoing {
    /// Plain text; `*`/`**` markup becomes HTML (`send()` "auto-formatting").
    Text(String),
    /// `{ body, formatted_body }`
    Rich { body: String, html: String },
}

#[derive(Default)]
struct Room {
    name: Option<String>,
    alias: Option<String>,
    members: HashMap<String, String>,
    recent: VecDeque<Value>,
}

pub struct Matrix {
    base: String,
    token: String,
    user_id: Mutex<String>,
    channels: Vec<String>,
    http: reqwest::Client,
    events: Emitter,
    rooms: Mutex<HashMap<String, Room>>,
    since: Mutex<Option<String>>,
    me: Mutex<Value>,
    txn: AtomicU64,
    this: Mutex<Option<std::sync::Weak<Matrix>>>,
}

/// `format(msg)`: `[b]` → `**`, `[i]` → `*`.
pub fn format(msg: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static B: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[b\](.*?)\[/b\]").unwrap());
    static I: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[i\](.*?)\[/i\]").unwrap());
    let s = B.replace_all(msg, "**${1}**");
    I.replace_all(&s, "*${1}*").into_owned()
}

/// Content of an `m.room.message` (`send(roomId, msg, type)`).
/// Fix: the text is HTML-escaped before `*`/`**` become `<i>`/`<b>` — the old code put raw message
/// text into `formatted_body`, so user-controlled text could inject HTML.
pub fn build_content(msg: &Outgoing, kind: &str) -> Value {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static B: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*(.*?)\*\*").unwrap());
    static I: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*(.*?)\*").unwrap());
    let msgtype = match kind {
        "emote" => "m.emote",
        "notice" => "m.notice",
        _ => "m.text",
    };
    let mut c = json!({ "msgtype": msgtype, "body": "" });
    match msg {
        Outgoing::Rich { body, html } => {
            c["body"] = body.clone().into();
            if !html.is_empty() {
                c["format"] = "org.matrix.custom.html".into();
                c["formatted_body"] = html.clone().into();
            }
        }
        Outgoing::Text(body) => {
            c["body"] = body.clone().into();
            if body.contains('*') {
                let esc = crate::util::escape_html(body);
                let html = B.replace_all(&esc, "<b>${1}</b>");
                let html = I.replace_all(&html, "<i>${1}</i>");
                c["format"] = "org.matrix.custom.html".into();
                c["formatted_body"] = html.into_owned().into();
            }
        }
    }
    c
}

/// `mxc://server/id` → (server, id). Rejects anything that could change the request path.
pub fn parse_mxc(mxc: &str) -> Option<(&str, &str)> {
    let rest = mxc.strip_prefix("mxc://")?;
    let (server, id) = rest.split_once('/')?;
    let ok_server = !server.is_empty() && server.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c));
    let ok_id = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c));
    (ok_server && ok_id).then_some((server, id))
}

fn enc(s: &str) -> String {
    crate::util::encode_uri_component(s)
}

/// The event's reply target (`m.relates_to.m.in_reply_to.event_id`).
fn in_reply_to(content: &Value) -> Option<&str> {
    content["m.relates_to"]["m.in_reply_to"]["event_id"].as_str()
}

impl Matrix {
    pub fn new(cfg: &Value, events: Emitter) -> Arc<Matrix> {
        let base = cfg["baseUrl"].as_str().filter(|s| !s.is_empty()).unwrap_or("https://matrix.org").trim_end_matches('/').to_string();
        let http = reqwest::Client::builder().connect_timeout(Duration::from_secs(30)).build().unwrap_or_default();
        let m = Arc::new(Matrix {
            base,
            token: cfg["token"].as_str().unwrap_or("").to_string(),
            user_id: Mutex::new(cfg["userId"].as_str().unwrap_or("").to_string()),
            channels: cfg["channels"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            http,
            events,
            rooms: Mutex::new(HashMap::new()),
            since: Mutex::new(None),
            me: Mutex::new(json!({})),
            txn: AtomicU64::new(0),
            this: Mutex::new(None),
        });
        *m.this.lock() = Some(Arc::downgrade(&m));
        m
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn arc(&self) -> Option<Arc<Matrix>> {
        self.this.lock().as_ref().and_then(|w| w.upgrade())
    }

    pub fn user_id(&self) -> String {
        self.user_id.lock().clone()
    }

    pub fn me(&self) -> Value {
        self.me.lock().clone()
    }

    /// Client-server API request; retries once on rate limiting.
    async fn api(&self, method: reqwest::Method, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value> {
        for attempt in 0..2 {
            let mut req = self.http.request(method.clone(), format!("{}{path}", self.base)).bearer_auth(&self.token).timeout(timeout);
            if let Some(b) = body {
                req = req.json(b);
            }
            let res = req.send().await.map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
            let status = res.status();
            let v: Value = res.json().await.unwrap_or(Value::Null);
            if status.as_u16() == 429 && attempt == 0 {
                let ms = v["retry_after_ms"].as_u64().unwrap_or(1000).min(30_000);
                tokio::time::sleep(Duration::from_millis(ms)).await;
                continue;
            }
            if !status.is_success() {
                bail!("{} {}: {}", status.as_u16(), v["errcode"].as_str().unwrap_or(""), v["error"].as_str().unwrap_or(""));
            }
            return Ok(v);
        }
        bail!("rate limited")
    }

    pub async fn run(self: Arc<Self>) {
        let mut backoff = Backoff::new();
        loop {
            if self.token.is_empty() {
                self.emit(Event::Error("[Matrix] no token configured".into()));
                return;
            }
            match self.session(&mut backoff).await {
                Ok(()) => {}
                Err(e) => self.emit(Event::Error(format!("[Matrix] {e:#}"))),
            }
            let wait = backoff.next();
            self.emit(Event::Info(format!("[Matrix] reconnecting in {}s", wait.as_secs())));
            tokio::time::sleep(wait).await;
        }
    }

    async fn session(&self, backoff: &mut Backoff) -> Result<()> {
        if self.since.lock().is_none() {
            let who = self.api(reqwest::Method::GET, "/_matrix/client/v3/account/whoami", None, Duration::from_secs(30)).await.context("whoami")?;
            if let Some(uid) = who["user_id"].as_str() {
                *self.user_id.lock() = uid.to_string();
            }
            // initial sync: build the room cache, don't deliver old messages
            let filter = enc(&json!({ "room": { "timeline": { "limit": 10 } } }).to_string());
            let v = self.api(reqwest::Method::GET, &format!("/_matrix/client/v3/sync?timeout=0&filter={filter}"), None, Duration::from_secs(120)).await.context("initial sync")?;
            self.process_sync(&v, false);
            *self.since.lock() = v["next_batch"].as_str().map(str::to_string);
            self.prepared().await;
        }
        loop {
            let since = self.since.lock().clone().unwrap_or_default();
            let path = format!("/_matrix/client/v3/sync?timeout={SYNC_TIMEOUT_MS}&since={}", enc(&since));
            let v = self.api(reqwest::Method::GET, &path, None, Duration::from_millis(SYNC_TIMEOUT_MS + 30_000)).await.context("sync")?;
            backoff.reset();
            if let Some(nb) = v["next_batch"].as_str() {
                *self.since.lock() = Some(nb.to_string());
            }
            self.process_sync(&v, true);
        }
    }

    /// `sync` state PREPARED: profile, `ready` event, join the configured rooms.
    async fn prepared(&self) {
        let uid = self.user_id();
        let profile = self.api(reqwest::Method::GET, &format!("/_matrix/client/v3/profile/{}", enc(&uid)), None, Duration::from_secs(30)).await.unwrap_or(Value::Null);
        *self.me.lock() = json!({
            "nickname": profile["displayname"].as_str().unwrap_or(&uid),
            "username": uid,
            "account": uid,
            "prefix": format!("{uid}!matrix"),
            "id": uid,
        });
        self.emit(Event::Ready("matrix".into()));
        for ch in &self.channels {
            match self.api(reqwest::Method::POST, &format!("/_matrix/client/v3/join/{}", enc(ch)), Some(&json!({})), Duration::from_secs(30)).await {
                Ok(_) => tracing::info!("[Matrix] Joined room: {ch}"),
                Err(e) => tracing::error!("[Matrix] Failed to join room {ch}: {e:#}"),
            }
        }
    }

    fn apply_state(room: &mut Room, ev: &Value) {
        match ev["type"].as_str() {
            Some("m.room.name") => room.name = ev["content"]["name"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
            Some("m.room.canonical_alias") => room.alias = ev["content"]["alias"].as_str().map(str::to_string),
            Some("m.room.member") => {
                if let Some(uid) = ev["state_key"].as_str() {
                    match ev["content"]["membership"].as_str() {
                        Some("join") | Some("invite") => {
                            room.members.insert(uid.to_string(), ev["content"]["displayname"].as_str().unwrap_or(uid).to_string());
                        }
                        _ => {
                            room.members.remove(uid);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Update the room cache from a sync response and deliver new messages.
    fn process_sync(&self, v: &Value, deliver: bool) {
        let Some(joined) = v["rooms"]["join"].as_object() else { return };
        let me = self.user_id();
        let now = crate::util::now_ms();
        let mut out = vec![];
        {
            let mut rooms = self.rooms.lock();
            for (room_id, r) in joined {
                let room = rooms.entry(room_id.clone()).or_default();
                for ev in r["state"]["events"].as_array().into_iter().flatten() {
                    Matrix::apply_state(room, ev);
                }
                for ev in r["timeline"]["events"].as_array().into_iter().flatten() {
                    if ev.get("state_key").is_some() {
                        Matrix::apply_state(room, ev);
                        continue;
                    }
                    if ev["type"] != "m.room.message" {
                        continue;
                    }
                    room.recent.push_back(ev.clone());
                    if room.recent.len() > RECENT_EVENTS {
                        room.recent.pop_front();
                    }
                    if deliver && is_deliverable(ev, &me, now) {
                        out.push((room_id.clone(), ev.clone()));
                    }
                }
            }
        }
        if out.is_empty() {
            return;
        }
        let Some(this) = self.arc() else { return };
        for (room_id, ev) in out {
            let this = this.clone();
            tokio::spawn(async move {
                let msg = this.reply(&room_id, &ev).await;
                this.emit(Event::Message(msg));
            });
        }
    }

    fn cached_event(&self, room_id: &str, event_id: &str) -> Option<Value> {
        self.rooms.lock().get(room_id)?.recent.iter().find(|e| e["event_id"] == event_id).cloned()
    }

    pub fn mxc_to_http(&self, mxc: &str) -> Option<String> {
        parse_mxc(mxc).map(|(s, id)| format!("{}/_matrix/media/v3/download/{s}/{id}", self.base))
    }

    async fn resolve_reply(&self, room_id: &str, event_id: &str) -> Option<ReplyTo> {
        let ev = match self.cached_event(room_id, event_id) {
            Some(e) => e,
            None => match self.api(reqwest::Method::GET, &format!("/_matrix/client/v3/rooms/{}/event/{}", enc(room_id), enc(event_id)), None, Duration::from_secs(30)).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("[Matrix] Failed to fetch remote event {event_id}: {e:#}");
                    return None;
                }
            },
        };
        let c = &ev["content"];
        let msgtype = c["msgtype"].as_str();
        let is_media = matches!(msgtype, Some("m.image" | "m.video" | "m.audio" | "m.file"));
        let mxc = if is_media { c["url"].as_str().map(str::to_string) } else { None };
        Some(ReplyTo {
            sender: ev["sender"].as_str().unwrap_or("").to_string(),
            message: c["body"].as_str().map(str::to_string),
            url: mxc.as_deref().and_then(|m| self.mxc_to_http(m)),
            mxc_url: mxc,
            msgtype: msgtype.map(str::to_string),
            filename: c["filename"].as_str().or(c["body"].as_str()).map(str::to_string),
            mimetype: c["info"]["mimetype"].as_str().map(str::to_string),
        })
    }

    /// `reply(event, room)`
    async fn reply(self: &Arc<Self>, room_id: &str, ev: &Value) -> Message {
        let sender = ev["sender"].as_str().unwrap_or("").to_string();
        let (room_name, sender_name) = {
            let rooms = self.rooms.lock();
            let room = rooms.get(room_id);
            (
                room.and_then(|r| r.name.clone().or_else(|| r.alias.clone())).unwrap_or_else(|| "Room".into()),
                room.and_then(|r| r.members.get(&sender).cloned()).unwrap_or_else(|| sender.clone()),
            )
        };
        let c = &ev["content"];
        let mut message = c["body"].as_str().unwrap_or("").to_string();
        if c["msgtype"] == "m.emote" {
            message = format!("* {sender_name} {message}");
        }
        let reply_to = match in_reply_to(c) {
            Some(id) => self.resolve_reply(room_id, id).await,
            None => None,
        };
        Message {
            kind: Kind::Matrix,
            network: "Matrix".into(),
            channel: room_name,
            channelid: room_id.to_string(),
            user: User { prefix: format!("{sender}!{sender}"), nick: sender_name, username: Some(sender.clone()), account: Some(sender), hostname: None },
            message,
            time: ev["origin_server_ts"].as_i64().unwrap_or(0) / 1000,
            media: None,
            reply_to,
            raw: ev.clone(),
            target: room_id.to_string(),
            client: Client::Matrix(self.clone()),
            cmd: String::new(),
            args: vec![],
        }
    }

    /// `send(roomId, msg, type)` — errors are logged.
    pub async fn send(&self, room_id: &str, msg: &Outgoing, kind: &str) {
        if room_id.is_empty() {
            return tracing::error!("[Matrix] Failed to send message: No roomId provided for send");
        }
        let content = build_content(msg, kind);
        let txn = format!("f0ckm{}.{}", crate::util::now_ms(), self.txn.fetch_add(1, Ordering::Relaxed));
        let path = format!("/_matrix/client/v3/rooms/{}/send/m.room.message/{}", enc(room_id), enc(&txn));
        if let Err(e) = self.api(reqwest::Method::PUT, &path, Some(&content), Duration::from_secs(30)).await {
            tracing::error!("[Matrix] Failed to send message: {e:#}");
        }
    }

    /// `sendmsg(mode, recipient, msg)`: a user id (`@x:y`) gets a fresh DM room, anything else is a room.
    pub async fn sendmsg(&self, recipient: &str, msg: &str) {
        if recipient.starts_with('@') {
            tracing::info!("[Matrix] Sending DM to {recipient}");
            let body = json!({ "visibility": "private", "invite": [recipient], "is_direct": true });
            match self.api(reqwest::Method::POST, "/_matrix/client/v3/createRoom", Some(&body), Duration::from_secs(30)).await {
                Ok(r) => {
                    if let Some(room) = r["room_id"].as_str() {
                        self.send(room, &Outgoing::Text(msg.to_string()), "text").await;
                        tracing::info!("[Matrix] Sent DM to {recipient} in new room {room}");
                    }
                }
                Err(e) => tracing::error!("[Matrix] Failed to send DM to {recipient}: {e:#}"),
            }
            return;
        }
        self.send(recipient, &Outgoing::Text(msg.to_string()), "text").await;
    }

    /// `download(mxcUrl)`: authenticated media (v1), then legacy v3. At most `max_bytes`.
    pub async fn download(&self, mxc: &str, max_bytes: u64) -> Result<Vec<u8>> {
        let (server, id) = parse_mxc(mxc).with_context(|| format!("Invalid MXC URL format: {mxc}"))?;
        let mut last = anyhow::anyhow!("All download strategies failed.");
        for url in [
            format!("{}/_matrix/client/v1/media/download/{server}/{id}", self.base),
            format!("{}/_matrix/media/v3/download/{server}/{id}", self.base),
        ] {
            match self.fetch_limited(&url, max_bytes).await {
                Ok(b) => return Ok(b),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    async fn fetch_limited(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>> {
        use futures::StreamExt;
        let res = self.http.get(url).bearer_auth(&self.token).timeout(Duration::from_secs(600)).send().await.map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
        if !res.status().is_success() {
            bail!("HTTP Error {}", res.status());
        }
        if res.content_length().is_some_and(|l| l > max_bytes) {
            bail!("file too large");
        }
        let mut out = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
            if out.len() as u64 + chunk.len() as u64 > max_bytes {
                bail!("file too large");
            }
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

/// Room.timeline filter of the old client.
fn is_deliverable(ev: &Value, me: &str, now_ms: i64) -> bool {
    let c = &ev["content"];
    ev["sender"].as_str().is_some_and(|s| s != me)
        && c["body"].as_str().is_some_and(|b| !b.is_empty())
        && matches!(c["msgtype"].as_str(), Some("m.text" | "m.notice" | "m.emote"))
        && now_ms - ev["origin_server_ts"].as_i64().unwrap_or(0) <= MAX_AGE_MS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_formatting() {
        let c = build_content(&Outgoing::Text("plain".into()), "text");
        assert_eq!(c, json!({ "msgtype": "m.text", "body": "plain" }));
        let c = build_content(&Outgoing::Text(format("[b]bold[/b] <x> [i]it[/i]")), "notice");
        assert_eq!(c["msgtype"], "m.notice");
        assert_eq!(c["body"], "**bold** <x> *it*");
        assert_eq!(c["formatted_body"], "<b>bold</b> &lt;x&gt; <i>it</i>");
        assert_eq!(c["format"], "org.matrix.custom.html");
        let c = build_content(&Outgoing::Rich { body: "b".into(), html: "<b>b</b>".into() }, "emote");
        assert_eq!(c, json!({ "msgtype": "m.emote", "body": "b", "format": "org.matrix.custom.html", "formatted_body": "<b>b</b>" }));
    }

    #[test]
    fn mxc_parsing() {
        assert_eq!(parse_mxc("mxc://matrix.org/AbC_12-x"), Some(("matrix.org", "AbC_12-x")));
        assert_eq!(parse_mxc("mxc://matrix.org/../../x"), None);
        assert_eq!(parse_mxc("mxc://matrix.org/a?b"), None);
        assert_eq!(parse_mxc("https://x/y"), None);
    }

    #[test]
    fn sync_processing() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let m = Matrix::new(&json!({ "token": "t", "userId": "@bot:x" }), tx);
        let now = crate::util::now_ms();
        let sync = json!({
            "next_batch": "s1",
            "rooms": { "join": { "!r:x": {
                "state": { "events": [
                    { "type": "m.room.name", "state_key": "", "content": { "name": "Uploads" } },
                    { "type": "m.room.member", "state_key": "@al:x", "content": { "membership": "join", "displayname": "Al" } }
                ]},
                "timeline": { "events": [
                    { "type": "m.room.message", "event_id": "$1", "sender": "@al:x", "origin_server_ts": now,
                      "content": { "msgtype": "m.video", "body": "clip.mp4", "url": "mxc://x/vid", "info": { "mimetype": "video/mp4" } } }
                ]}
            }}}
        });
        m.process_sync(&sync, false);
        let rooms = m.rooms.lock();
        let r = rooms.get("!r:x").unwrap();
        assert_eq!(r.name.as_deref(), Some("Uploads"));
        assert_eq!(r.members.get("@al:x").map(String::as_str), Some("Al"));
        assert_eq!(r.recent.len(), 1);
        drop(rooms);
        assert_eq!(m.mxc_to_http("mxc://x/vid").as_deref(), Some("https://matrix.org/_matrix/media/v3/download/x/vid"));
    }

    #[test]
    fn delivery_filter() {
        let now = 1_000_000;
        let ev = |sender: &str, msgtype: &str, ts: i64| json!({ "sender": sender, "origin_server_ts": ts, "content": { "msgtype": msgtype, "body": "hi" } });
        assert!(is_deliverable(&ev("@a:x", "m.text", now - 1000), "@bot:x", now));
        assert!(!is_deliverable(&ev("@bot:x", "m.text", now), "@bot:x", now));
        assert!(!is_deliverable(&ev("@a:x", "m.image", now), "@bot:x", now));
        assert!(!is_deliverable(&ev("@a:x", "m.text", now - 61_000), "@bot:x", now));
        assert_eq!(in_reply_to(&json!({ "m.relates_to": { "m.in_reply_to": { "event_id": "$e" } } })), Some("$e"));
    }

    #[tokio::test]
    async fn reply_resolution_from_cache() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let m = Matrix::new(&json!({ "token": "t", "userId": "@bot:x" }), tx);
        let now = crate::util::now_ms();
        let sync = json!({ "rooms": { "join": { "!r:x": { "timeline": { "events": [
            { "type": "m.room.message", "event_id": "$v", "sender": "@al:x", "origin_server_ts": now,
              "content": { "msgtype": "m.video", "body": "clip.mp4", "url": "mxc://x/vid", "info": { "mimetype": "video/mp4" } } }
        ]}}}}});
        m.process_sync(&sync, false);
        let ev = json!({ "type": "m.room.message", "event_id": "$w", "sender": "@bo:x", "origin_server_ts": now,
            "content": { "msgtype": "m.text", "body": "> <@al:x> clip.mp4\n\n!w -sfw -t a", "m.relates_to": { "m.in_reply_to": { "event_id": "$v" } } } });
        let msg = m.reply("!r:x", &ev).await;
        assert_eq!(msg.channel, "Room");
        assert_eq!(msg.user.prefix, "@bo:x!@bo:x");
        let r = msg.reply_to.unwrap();
        assert_eq!(r.sender, "@al:x");
        assert_eq!(r.mxc_url.as_deref(), Some("mxc://x/vid"));
        assert_eq!(r.mimetype.as_deref(), Some("video/mp4"));
        assert_eq!(r.filename.as_deref(), Some("clip.mp4"));
    }
}
