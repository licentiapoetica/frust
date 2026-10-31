//! Chat bots (IRC, Telegram, Matrix, Slack) — native replacement for the `cuffeo` package.
//!
//! TODO(port): clients and triggers are not ported yet. `BotHandle` is the interface the web side
//! uses; until the clients exist it only logs.

use std::sync::Arc;

/// Handle used by routes to push messages to chat networks.
#[derive(Clone, Default)]
pub struct BotHandle {
    inner: Arc<()>,
}

impl BotHandle {
    pub fn new() -> BotHandle {
        BotHandle { inner: Arc::new(()) }
    }

    /// Send a plain-text message to a channel/room on the given network
    /// (`"matrix"`, `"tg"`, `"irc"`, `"slack"`). Errors are logged, never returned.
    pub async fn send(&self, network: &str, target: &str, message: &str) {
        let _ = &self.inner;
        tracing::info!("[BOT] ({network} not connected) {target}: {message}");
    }

    /// Matrix upload notification (`clients[type=matrix].notification_channel_id`).
    pub async fn notify_matrix(&self, cfg: &crate::config::Config, message: &str) {
        if let Some(ch) = cfg.client("matrix").and_then(|c| c["notification_channel_id"].as_str()).filter(|c| !c.is_empty()) {
            self.send("matrix", ch, message).await;
        }
    }
}
