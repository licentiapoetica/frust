//! Slack client. The old `cuffeo` client used the RTM API (`rtm.start`), which Slack has retired;
//! this one uses **Socket Mode** (`apps.connections.open` + websocket, envelopes acked by id) for
//! incoming messages and the Web API (`chat.postMessage`) for sending.
//!
//! Config (`clients[type=slack]`): `token` = bot token (`xoxb-…`, scopes `chat:write`,
//! `channels:read`, `users:read`, plus the `message.*` event subscriptions), `appToken` = app-level
//! token (`xapp-…`, scope `connections:write`).

use super::ws::{self, WsMessage};
use super::{Backoff, Client, Emitter, Event, Kind, Message, User};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

const API: &str = "https://slack.com/api";

#[derive(Clone, Debug, serde::Serialize)]
pub struct SlackUser {
    pub account: String,
    pub nickname: String,
}

pub struct Slack {
    token: String,
    app_token: String,
    http: reqwest::Client,
    events: Emitter,
    channels: Mutex<HashMap<String, String>>,
    users: Mutex<HashMap<String, SlackUser>>,
    this: Mutex<Option<std::sync::Weak<Slack>>>,
}

/// `format(msg)`: bbcode → Slack mrkdwn.
pub fn format(msg: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static B: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[b\](.*?)\[/b\]").unwrap());
    static S: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[s\](.*?)\[/s\]").unwrap());
    static I: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[i\](.*?)\[/i\]").unwrap());
    static C: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[color=(.*?)\](.*?)\[/color\]").unwrap());
    let s = B.replace_all(msg, "*${1}*");
    let s = S.replace_all(&s, "~${1}~");
    let s = I.replace_all(&s, "_${1}_");
    C.replace_all(&s, "${2}").into_owned()
}

/// Message text as sent: multi-line messages go into a code block.
pub fn outgoing_text(text: &str) -> String {
    let t = if text.contains('\n') { format!("```{text}```") } else { text.to_string() };
    format(&t)
}

/// A Socket Mode envelope: (envelope_id to ack, message event if it is a plain user message).
pub fn parse_envelope(v: &Value) -> (Option<String>, Option<Value>) {
    let id = v["envelope_id"].as_str().map(str::to_string);
    if v["type"] != "events_api" {
        return (id, None);
    }
    let ev = &v["payload"]["event"];
    let plain = ev["type"] == "message"
        && ev.get("subtype").is_none()
        && ev.get("bot_id").is_none()
        && ev["user"].is_string()
        && ev["channel"].is_string()
        && ev["text"].is_string();
    (id, plain.then(|| ev.clone()))
}

impl Slack {
    pub fn new(cfg: &Value, http: reqwest::Client, events: Emitter) -> Arc<Slack> {
        let s = Arc::new(Slack {
            token: cfg["token"].as_str().unwrap_or("").to_string(),
            app_token: cfg["appToken"].as_str().unwrap_or("").to_string(),
            http,
            events,
            channels: Mutex::new(HashMap::new()),
            users: Mutex::new(HashMap::new()),
            this: Mutex::new(None),
        });
        *s.this.lock() = Some(Arc::downgrade(&s));
        s
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn arc(&self) -> Option<Arc<Slack>> {
        self.this.lock().as_ref().and_then(|w| w.upgrade())
    }

    async fn call(&self, method: &str, token: &str, body: &Value) -> anyhow::Result<Value> {
        let v: Value = self
            .http
            .post(format!("{API}/{method}"))
            .bearer_auth(token)
            .json(body)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{method}: {}", e.without_url()))?
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("{method}: {}", e.without_url()))?;
        if v["ok"].as_bool() != Some(true) {
            anyhow::bail!("{method}: {}", v["error"].as_str().unwrap_or("request failed"));
        }
        Ok(v)
    }

    pub async fn run(self: Arc<Self>) {
        let mut backoff = Backoff::new();
        loop {
            if self.token.is_empty() || self.app_token.is_empty() {
                self.emit(Event::Error("[slack] token and appToken (Socket Mode) are required".into()));
                return;
            }
            match self.session(&mut backoff).await {
                Ok(()) => self.emit(Event::Debug("[slack] stream ended".into())),
                Err(e) => self.emit(Event::Error(format!("[slack] {e:#}"))),
            }
            let wait = backoff.next();
            self.emit(Event::Info(format!("reconnecting slack in {}s", wait.as_secs())));
            tokio::time::sleep(wait).await;
        }
    }

    async fn session(&self, backoff: &mut Backoff) -> anyhow::Result<()> {
        let open = self.call("apps.connections.open", &self.app_token, &json!({})).await?;
        let url = open["url"].as_str().ok_or_else(|| anyhow::anyhow!("apps.connections.open: no url"))?;
        let (mut rd, mut wr) = ws::connect(url).await?;
        // read in a separate task: a timed-out read in the middle of a frame would desync the stream
        let (tx, mut rx) = tokio::sync::mpsc::channel::<anyhow::Result<WsMessage>>(16);
        let reader = tokio::spawn(async move {
            loop {
                let m = rd.next().await;
                let stop = m.is_err();
                if tx.send(m).await.is_err() || stop {
                    break;
                }
            }
        });
        let res = self.pump(&mut rx, &mut wr, backoff).await;
        reader.abort();
        res
    }

    async fn pump(
        &self,
        rx: &mut tokio::sync::mpsc::Receiver<anyhow::Result<WsMessage>>,
        wr: &mut ws::WsWriter,
        backoff: &mut Backoff,
    ) -> anyhow::Result<()> {
        let mut pinged = false;
        loop {
            let msg = match tokio::time::timeout(Duration::from_secs(if pinged { 30 } else { 120 }), rx.recv()).await {
                Ok(Some(m)) => {
                    pinged = false;
                    m?
                }
                Ok(None) => return Ok(()),
                Err(_) if !pinged => {
                    // keepalive; Slack pings us regularly, so silence means a dead connection
                    pinged = true;
                    wr.send(ws::OP_PING, b"").await?;
                    continue;
                }
                Err(_) => anyhow::bail!("connection timed out"),
            };
            match msg {
                WsMessage::Ping(p) => wr.send(ws::OP_PONG, &p).await?,
                WsMessage::Close => return Ok(()),
                WsMessage::Binary(_) | WsMessage::Pong => {}
                WsMessage::Text(t) => {
                    let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                    match v["type"].as_str() {
                        Some("hello") => {
                            backoff.reset();
                            self.emit(Event::Ready("slack".into()));
                        }
                        Some("disconnect") => return Ok(()),
                        _ => {}
                    }
                    let (envelope, event) = parse_envelope(&v);
                    if let Some(id) = envelope {
                        wr.send(ws::OP_TEXT, json!({ "envelope_id": id }).to_string().as_bytes()).await?;
                    }
                    if let (Some(ev), Some(this)) = (event, self.arc()) {
                        tokio::spawn(async move {
                            let msg = this.reply(&ev).await;
                            this.emit(Event::Message(msg));
                        });
                    }
                }
            }
        }
    }

    async fn channel_name(&self, id: &str) -> String {
        if let Some(n) = self.channels.lock().get(id) {
            return n.clone();
        }
        let name = self
            .call("conversations.info", &self.token, &json!({ "channel": id }))
            .await
            .ok()
            .and_then(|r| r["channel"]["name"].as_str().map(str::to_string))
            .unwrap_or_else(|| id.to_string());
        self.channels.lock().insert(id.to_string(), name.clone());
        name
    }

    async fn user(&self, id: &str) -> SlackUser {
        if let Some(u) = self.users.lock().get(id) {
            return u.clone();
        }
        let u = match self.call("users.info", &self.token, &json!({ "user": id })).await {
            Ok(r) => SlackUser {
                account: r["user"]["name"].as_str().unwrap_or(id).to_string(),
                nickname: r["user"]["real_name"].as_str().or(r["user"]["name"].as_str()).unwrap_or(id).to_string(),
            },
            Err(_) => SlackUser { account: id.to_string(), nickname: id.to_string() },
        };
        self.users.lock().insert(id.to_string(), u.clone());
        u
    }

    async fn reply(self: &Arc<Self>, ev: &Value) -> Message {
        let channel_id = ev["channel"].as_str().unwrap_or("").to_string();
        let user_id = ev["user"].as_str().unwrap_or("").to_string();
        let channel = self.channel_name(&channel_id).await;
        let u = self.user(&user_id).await;
        Message {
            kind: Kind::Slack,
            network: "Slack".into(),
            channel,
            channelid: channel_id.clone(),
            user: User {
                prefix: format!("{}!{user_id}@Slack", u.account),
                nick: u.nickname.clone(),
                username: Some(u.nickname),
                account: Some(u.account),
                hostname: None,
            },
            message: ev["text"].as_str().unwrap_or("").to_string(),
            time: crate::util::now_secs(),
            media: None,
            reply_to: None,
            raw: ev.clone(),
            target: channel_id,
            client: Client::Slack(self.clone()),
            cmd: String::new(),
            args: vec![],
        }
    }

    /// `send(channel, text)` via `chat.postMessage`.
    pub async fn send(&self, channel: &str, text: &str) {
        let text = outgoing_text(text);
        if text.len() > 40_000 {
            return self.emit(Event::Error("message too long, slack limit reached".into()));
        }
        if let Err(e) = self.call("chat.postMessage", &self.token, &json!({ "channel": channel, "text": text })).await {
            self.emit(Event::Error(format!("[slack] {e:#}")));
        }
    }

    pub fn cached_user(&self, id: &str) -> Option<Value> {
        self.users.lock().get(id).and_then(|u| serde_json::to_value(u).ok())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(format("[b]a[/b] [s]b[/s] [i]c[/i] [color=red]d[/color]"), "*a* ~b~ _c_ d");
        assert_eq!(outgoing_text("a\nb"), "```a\nb```");
    }

    #[test]
    fn envelopes() {
        let v = json!({ "type": "events_api", "envelope_id": "e1", "payload": { "event": { "type": "message", "user": "U1", "channel": "C1", "text": "hi" } } });
        let (id, ev) = parse_envelope(&v);
        assert_eq!(id.as_deref(), Some("e1"));
        assert_eq!(ev.unwrap()["text"], "hi");
        let v = json!({ "type": "events_api", "envelope_id": "e2", "payload": { "event": { "type": "message", "subtype": "message_changed", "channel": "C1" } } });
        assert_eq!(parse_envelope(&v), (Some("e2".into()), None));
        let v = json!({ "type": "events_api", "envelope_id": "e3", "payload": { "event": { "type": "message", "bot_id": "B", "user": "U", "channel": "C", "text": "x" } } });
        assert!(parse_envelope(&v).1.is_none());
        assert_eq!(parse_envelope(&json!({ "type": "hello" })), (None, None));
    }
}
