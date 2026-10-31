//! Telegram Bot API client (`cuffeo/src/clients/tg.mjs`).
//!
//! Long polling with `getUpdates` (the old client short-polled every `pollrate` ms; `pollrate` is now
//! the pause after a failed poll). Messages older than 10 s are ignored like before. Media messages
//! get `media` = file URL (via `getFile`) and `message` = caption.

use super::{Backoff, Client, Emitter, Event, Kind, Message, User};
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

const ALLOWED_FILES: [&str; 4] = ["audio", "video", "photo", "document"];
const LONG_POLL_SECS: u64 = 25;

pub struct Telegram {
    token: String,
    api: String,
    pollrate: Duration,
    http: reqwest::Client,
    events: Emitter,
    me: Mutex<Value>,
    users: Mutex<HashMap<String, Value>>,
    last_update: AtomicI64,
    last_message: AtomicI64,
    this: Mutex<Option<std::sync::Weak<Telegram>>>,
}

/// `format(msg)`: HTML-escape, then `[b]`/`[i]`/`[pre]` → HTML, `[color]` stripped.
pub fn format(msg: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static B: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)\[b\](.*?)\[/b\]").unwrap());
    static I: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)\[i\](.*?)\[/i\]").unwrap());
    static C: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)\[color=(.*?)\](.*?)\[/color\]").unwrap());
    static P: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)\[pre\](.*?)\[/pre\]").unwrap());
    let s = msg.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let s = B.replace_all(&s, "<b>${1}</b>");
    let s = I.replace_all(&s, "<i>${1}</i>");
    let s = C.replace_all(&s, "${2}");
    P.replace_all(&s, "<pre>${1}</pre>").into_owned()
}

/// Remove the bot token from text that may be logged or shown (`/bot<token>` in API/file URLs).
pub fn sanitize_url(s: &str) -> String {
    use once_cell::sync::Lazy;
    static RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?i)/bot[0-9a-zA-Z_:-]+").unwrap());
    RE.replace_all(s, "/bot<token>").into_owned()
}

fn chat_id_value(target: &str) -> Value {
    target.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::String(target.to_string()))
}

/// `msg.length` in UTF-16 code units, like JavaScript.
fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Build the message object for a Telegram `message` (`reply(tmp)`), without the client handle.
pub fn message_fields(m: &Value) -> (String, String, User, String, i64) {
    let from = &m["from"];
    let id = js_id(&from["id"]);
    let username = from["username"].as_str().map(str::to_string);
    let user = User {
        prefix: format!("{}!{}@Telegram", username.as_deref().unwrap_or("undefined"), id),
        nick: from["first_name"].as_str().unwrap_or("").to_string(),
        username,
        account: Some(id),
        hostname: None,
    };
    let channel = m["chat"]["title"].as_str().unwrap_or("").to_string();
    let channelid = js_id(&m["chat"]["id"]);
    let text = m["text"].as_str().unwrap_or("").to_string();
    (channel, channelid, user, text, m["date"].as_i64().unwrap_or(0))
}

fn js_id(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => "undefined".into(),
    }
}

/// Which media key a message carries (first of audio/video/photo/document in message order) and
/// its `file_id` (largest photo size).
pub fn media_file_id(m: &Value) -> Option<String> {
    let obj = m.as_object()?;
    let key = obj.keys().find(|k| ALLOWED_FILES.contains(&k.as_str()))?;
    let media = if key == "photo" { obj[key].as_array()?.last()? } else { &obj[key] };
    media["file_id"].as_str().map(str::to_string)
}

impl Telegram {
    pub fn new(cfg: &Value, http: reqwest::Client, events: Emitter) -> Arc<Telegram> {
        let token = cfg["token"].as_str().unwrap_or("").to_string();
        let pollrate = crate::config::as_int(&cfg["pollrate"]).filter(|p| *p > 0).unwrap_or(1000) as u64;
        let tg = Arc::new(Telegram {
            api: format!("https://api.telegram.org/bot{token}"),
            token,
            pollrate: Duration::from_millis(pollrate),
            http,
            events,
            me: Mutex::new(json!({})),
            users: Mutex::new(HashMap::new()),
            last_update: AtomicI64::new(0),
            last_message: AtomicI64::new(0),
            this: Mutex::new(None),
        });
        *tg.this.lock() = Some(Arc::downgrade(&tg));
        tg
    }

    fn emit(&self, ev: Event) {
        let _ = self.events.send(ev);
    }

    fn arc(&self) -> Option<Arc<Telegram>> {
        self.this.lock().as_ref().and_then(|w| w.upgrade())
    }

    /// POST a Bot API method; `Err` carries the API description. URLs (with the token) are never
    /// part of the error text.
    async fn call(&self, method: &str, body: &Value, timeout: Duration) -> anyhow::Result<Value> {
        let res = self
            .http
            .post(format!("{}/{method}", self.api))
            .json(body)
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{method}: {}", e.without_url()))?;
        let v: Value = res.json().await.map_err(|e| anyhow::anyhow!("{method}: {}", e.without_url()))?;
        if v["ok"].as_bool() != Some(true) {
            anyhow::bail!("{method}: {}", v["description"].as_str().unwrap_or("request failed"));
        }
        Ok(v["result"].clone())
    }

    pub async fn run(self: Arc<Self>) {
        let mut backoff = Backoff::new();
        loop {
            if self.token.is_empty() {
                self.emit(Event::Error("[tg] no token configured".into()));
                return;
            }
            match self.connect().await {
                Ok(()) => {
                    self.emit(Event::Ready("tg".into()));
                    backoff.reset();
                    loop {
                        match self.poll().await {
                            Ok(()) => backoff.reset(),
                            Err(e) => {
                                self.emit(Event::Error(format!("[tg] {e:#}")));
                                break;
                            }
                        }
                    }
                }
                Err(e) => self.emit(Event::Error(format!("[tg] {e:#}"))),
            }
            tokio::time::sleep(self.pollrate.max(backoff.next())).await;
        }
    }

    /// `getMe`
    async fn connect(&self) -> anyhow::Result<()> {
        let r = self.call("getMe", &json!({}), Duration::from_secs(30)).await?;
        let id = js_id(&r["id"]);
        *self.me.lock() = json!({
            "nickname": r["first_name"],
            "username": r["username"],
            "account": id,
            "prefix": format!("{}!{}", r["username"].as_str().unwrap_or("undefined"), id),
            "id": id,
        });
        Ok(())
    }

    pub fn me(&self) -> Value {
        self.me.lock().clone()
    }

    pub async fn get_file(&self, file_id: &str) -> Option<String> {
        let r = self.call("getFile", &json!({ "file_id": file_id }), Duration::from_secs(30)).await.ok()?;
        r["file_path"].as_str().map(|p| format!("https://api.telegram.org/file/bot{}/{p}", self.token))
    }

    async fn poll(&self) -> anyhow::Result<()> {
        let body = json!({
            "offset": self.last_update.load(Ordering::Relaxed),
            "timeout": LONG_POLL_SECS,
            "allowed_updates": ["message"],
        });
        let updates = self.call("getUpdates", &body, Duration::from_secs(LONG_POLL_SECS + 15)).await?;
        let Some(list) = updates.as_array() else { return Ok(()) };
        if let Some(last) = list.last().and_then(|u| u["update_id"].as_i64()) {
            self.last_update.store(last + 1, Ordering::Relaxed);
        }
        for u in list {
            self.handle_update(u).await;
        }
        Ok(())
    }

    async fn handle_update(&self, u: &Value) {
        let Some(me) = self.arc() else { return };
        if let Some(m) = u.get("message") {
            let date = m["date"].as_i64().unwrap_or(0);
            let mid = m["message_id"].as_i64().unwrap_or(0);
            if date < crate::util::now_secs() - 10 || mid == self.last_message.load(Ordering::Relaxed) {
                return;
            }
            self.last_message.store(mid, Ordering::Relaxed);
            let from = &m["from"];
            let key = from["username"].as_str().or(from["first_name"].as_str()).unwrap_or("").to_string();
            self.users.lock().entry(key).or_insert_with(|| {
                json!({
                    "nick": from["first_name"],
                    "username": from["username"],
                    "account": js_id(&from["id"]),
                    "prefix": format!("{}!{}@Telegram", from["username"].as_str().unwrap_or("undefined"), js_id(&from["id"])),
                    "id": from["id"],
                })
            });
            let mut msg = me.reply(m);
            if let Some(fid) = media_file_id(m) {
                msg.media = self.get_file(&fid).await;
                msg.message = m["caption"].as_str().unwrap_or("").to_string();
            }
            self.emit(Event::Message(msg));
        }
    }

    /// `reply(tmp)`
    fn reply(self: &Arc<Self>, m: &Value) -> Message {
        let (channel, channelid, user, text, date) = message_fields(m);
        Message {
            kind: Kind::Tg,
            network: "Telegram".into(),
            channel,
            channelid: channelid.clone(),
            user,
            message: text,
            time: date,
            media: None,
            reply_to: None,
            raw: m.clone(),
            target: channelid,
            client: Client::Tg(self.clone()),
            cmd: String::new(),
            args: vec![],
        }
    }

    /// `send(chatid, msg, reply, opt)` → the sent message (`result`), if any.
    pub async fn send(&self, chat_id: &str, msg: &str, reply_to: Option<i64>, opts: &Value) -> Option<Value> {
        let len = js_len(msg);
        if len == 0 || len > 2048 {
            self.emit(Event::Error("msg to short or to long lol".into()));
            return None;
        }
        let mut body = json!({ "chat_id": chat_id_value(chat_id), "text": format(msg), "parse_mode": "HTML" });
        if let (Some(b), Some(o)) = (body.as_object_mut(), opts.as_object()) {
            for (k, v) in o {
                b.insert(k.clone(), v.clone());
            }
        }
        if let Some(r) = reply_to {
            body["reply_to_message_id"] = r.into();
        }
        match self.call("sendMessage", &body, Duration::from_secs(30)).await {
            Ok(r) => Some(r),
            Err(e) => {
                self.emit(Event::Error(format!("[tg] {e:#}")));
                None
            }
        }
    }

    pub async fn edit_message_text(&self, chat_id: &Value, message_id: i64, text: &str, opts: &Value) {
        let mut body = json!({ "chat_id": chat_id, "message_id": message_id, "text": text });
        if let (Some(b), Some(o)) = (body.as_object_mut(), opts.as_object()) {
            for (k, v) in o {
                b.insert(k.clone(), v.clone());
            }
        }
        if let Err(e) = self.call("editMessageText", &body, Duration::from_secs(30)).await {
            self.emit(Event::Error(format!("[tg] {e:#}")));
        }
    }

    pub async fn delete_message(&self, chat_id: &Value, message_id: i64) {
        if let Err(e) = self.call("deleteMessage", &json!({ "chat_id": chat_id, "message_id": message_id }), Duration::from_secs(30)).await {
            self.emit(Event::Error(format!("[tg] {e:#}")));
        }
    }

    pub fn cached_user(&self, name: &str) -> Option<Value> {
        self.users.lock().get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_html() {
        assert_eq!(format("a<b>&[b]x[/b]"), "a&lt;b&gt;&amp;<b>x</b>");
        assert_eq!(format("[i]multi\nline[/i] [color=red]r[/color] [pre]c[/pre]"), "<i>multi\nline</i> r <pre>c</pre>");
    }

    #[test]
    fn hides_token() {
        assert_eq!(sanitize_url("https://api.telegram.org/file/bot123:AA-b_c/videos/x.mp4"), "https://api.telegram.org/file/bot<token>/videos/x.mp4");
    }

    #[test]
    fn parses_message() {
        let m = json!({
            "message_id": 5, "date": 1700000000,
            "from": { "id": 42, "first_name": "Al", "username": "al" },
            "chat": { "id": -100123, "title": "w0bm uploads" },
            "text": "!w -sfw -t a,b"
        });
        let (channel, channelid, user, text, date) = message_fields(&m);
        assert_eq!(channel, "w0bm uploads");
        assert_eq!(channelid, "-100123");
        assert_eq!(user.prefix, "al!42@Telegram");
        assert_eq!(user.account.as_deref(), Some("42"));
        assert_eq!(user.nick, "Al");
        assert_eq!(text, "!w -sfw -t a,b");
        assert_eq!(date, 1700000000);
        assert_eq!(chat_id_value(&channelid), json!(-100123));
        assert_eq!(chat_id_value("@chan"), json!("@chan"));

        let anon = json!({ "from": { "id": 7, "first_name": "X" }, "chat": { "id": 7 } });
        assert_eq!(message_fields(&anon).2.prefix, "undefined!7@Telegram");
        assert_eq!(message_fields(&anon).0, "");
    }

    #[test]
    fn finds_media() {
        let photo: Value = serde_json::from_str(r#"{"message_id":1,"photo":[{"file_id":"small"},{"file_id":"big"}],"caption":"c"}"#).unwrap();
        assert_eq!(media_file_id(&photo).as_deref(), Some("big"));
        let doc: Value = serde_json::from_str(r#"{"animation":{"file_id":"anim"},"document":{"file_id":"doc"}}"#).unwrap();
        assert_eq!(media_file_id(&doc).as_deref(), Some("doc"));
        assert_eq!(media_file_id(&json!({"text": "hi"})), None);
        assert_eq!(js_len("😀"), 2);
    }
}
