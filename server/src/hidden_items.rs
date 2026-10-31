//! IDs of items that are not live (pending, deleted, purged); public media routes refuse them.

use crate::state::AppState;
use parking_lot::RwLock;
use std::collections::HashSet;
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(5);

static CACHE: RwLock<Option<(Instant, HashSet<i64>)>> = RwLock::new(None);

async fn refresh(st: &AppState) {
    match crate::sql!("select id::bigint from items where active = false").fetch_scalar_list(&st.db).await {
        Ok(ids) => *CACHE.write() = Some((Instant::now(), ids.into_iter().collect())),
        Err(e) => tracing::warn!("[HIDDEN ITEMS] refresh failed: {e}"),
    }
}

pub async fn is_hidden(st: &AppState, id: i64) -> bool {
    let fresh = CACHE.read().as_ref().map(|(t, _)| t.elapsed() < TTL).unwrap_or(false);
    if !fresh {
        refresh(st).await;
    }
    CACHE.read().as_ref().map(|(_, s)| s.contains(&id)).unwrap_or(false)
}

/// Call after an item changes state so the next lookup reloads.
pub fn invalidate() {
    *CACHE.write() = None;
}
