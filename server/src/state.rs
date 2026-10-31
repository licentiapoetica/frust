//! Shared application state.

use crate::config::Config;
use crate::i18n::I18n;
use crate::settings::Settings;
use crate::tpl::Templates;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Small TTL cache.
pub struct TtlCache<V: Clone> {
    map: DashMap<String, (Instant, V)>,
    ttl: Duration,
    max: usize,
}

impl<V: Clone> TtlCache<V> {
    pub fn new(ttl: Duration, max: usize) -> Self {
        TtlCache { map: DashMap::new(), ttl, max }
    }

    pub fn get(&self, key: &str) -> Option<V> {
        let hit = self.map.get(key).map(|e| (e.0, e.1.clone()));
        match hit {
            Some((exp, v)) if exp > Instant::now() => Some(v),
            Some(_) => {
                self.map.remove(key);
                None
            }
            None => None,
        }
    }

    pub fn set(&self, key: &str, v: V) {
        self.map.insert(key.to_string(), (Instant::now() + self.ttl, v));
        if self.map.len() > self.max {
            let now = Instant::now();
            self.map.retain(|_, e| e.0 > now);
        }
    }

    pub fn remove(&self, key: &str) {
        self.map.remove(key);
    }

    pub fn clear(&self) {
        self.map.clear();
    }
}

/// In-memory owner maps for private (visibility 2) and unavailable (visibility 3) items.
#[derive(Default)]
pub struct PrivateItems {
    pub private_dests: HashMap<String, String>,
    pub private_ids: HashMap<i64, String>,
    pub unavailable_dests: HashMap<String, String>,
    pub unavailable_ids: HashMap<i64, String>,
}

pub struct Caches {
    /// sha256(session cookie) → session row (5 s)
    pub session: TtlCache<crate::db::Row>,
    /// username → uploads in the last 12 h (30 s)
    pub upload_count: TtlCache<i64>,
    /// lower(owner) → own pending uploads (30 s)
    pub pending_count: TtlCache<i64>,
    /// `SELECT id, name, slug, rating FROM halls`
    pub halls: RwLock<Value>,
    pub private_items: RwLock<PrivateItems>,
    /// Feed count cache (f0cklib): key → total (90 s)
    pub feed_count: TtlCache<i64>,
}

pub struct Inner {
    pub cfg: ArcSwap<Config>,
    pub db: PgPool,
    pub settings: Settings,
    pub tpl: Templates,
    pub i18n: Arc<I18n>,
    pub caches: Caches,
    pub git_hash: String,
    /// Random per-process secret (impersonation fallback key).
    pub process_secret: String,
    pub http: reqwest::Client,
    pub bot: crate::bot::BotHandle,
    pub router: std::sync::OnceLock<Arc<crate::http::Router>>,
}

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

impl Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.0
    }
}

impl AppState {
    /// Current config snapshot.
    pub fn cfg(&self) -> Arc<Config> {
        self.cfg.load_full()
    }

    /// HMAC key for the admin impersonation cookie.
    pub fn impersonation_secret(&self) -> String {
        self.cfg().str("main.secret").filter(|s| !s.is_empty()).map(str::to_string).unwrap_or_else(|| self.process_secret.clone())
    }

    pub fn invalidate_session_cache(&self, session_hash: &str) {
        self.caches.session.remove(session_hash);
    }

    pub fn invalidate_upload_count_cache(&self, username: &str) {
        self.caches.upload_count.remove(username);
        self.caches.pending_count.remove(&username.to_lowercase());
    }

    pub fn halls(&self) -> Value {
        self.caches.halls.read().clone()
    }

    pub async fn update_halls_cache(&self) {
        match crate::sql!("SELECT id, name, slug, rating FROM halls ORDER BY name ASC").fetch_all(&self.db).await {
            Ok(rows) => *self.caches.halls.write() = Value::Array(rows.into_iter().map(Value::Object).collect()),
            Err(e) => tracing::error!("[HALLS CACHE] Failed to update: {e}"),
        }
    }
}

impl Caches {
    pub fn new() -> Caches {
        Caches {
            session: TtlCache::new(Duration::from_secs(5), 2000),
            upload_count: TtlCache::new(Duration::from_secs(30), 5000),
            pending_count: TtlCache::new(Duration::from_secs(30), 5000),
            halls: RwLock::new(Value::Array(vec![])),
            private_items: RwLock::new(PrivateItems::default()),
            feed_count: TtlCache::new(Duration::from_secs(90), 500),
        }
    }
}
