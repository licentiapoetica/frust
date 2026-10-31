//! Calls into the feed/item-page port for user halls (`f0cklib.getf0cks` / `f0cklib.getf0ck`),
//! taking the old JS-style options object.

use crate::f0cklib::{self, FeedOpts};
use crate::session::Session;
use crate::state::AppState;
use serde_json::{Value, json};

fn opts_from<'a>(session: Option<&'a Session>, o: &Value) -> FeedOpts<'a> {
    let s = |k: &str| o.get(k).and_then(|v| v.as_str()).filter(|v| !v.is_empty()).map(str::to_string);
    let i = |k: &str| o.get(k).and_then(crate::config::as_int);
    FeedOpts {
        session,
        mode: i("mode"),
        page: i("page"),
        mime: s("mime"),
        exclude: o.get("exclude").and_then(Value::as_array).map(|a| a.iter().filter_map(crate::config::as_int).collect()).unwrap_or_default(),
        user_id: i("user_id"),
        user_hall: s("userHall"),
        user_hall_owner: s("userHallOwner"),
        random: o.get("random").map(crate::config::truthy).unwrap_or(false),
        itemid: s("itemid").or_else(|| i("itemid").map(|n| n.to_string())),
        ..Default::default()
    }
}

fn failure(e: anyhow::Error) -> Value {
    tracing::error!("[USER HALLS] feed error: {e:#}");
    json!({ "success": false, "message": "Internal error" })
}

/// `f0cklib.getf0cks(opts)` — item grid.
pub async fn get_f0cks(st: &AppState, session: Option<&Session>, opts: Value) -> Value {
    f0cklib::getf0cks(st, &opts_from(session, &opts)).await.unwrap_or_else(failure)
}

/// `f0cklib.getf0ck(opts)` — single item page data.
pub async fn get_f0ck(st: &AppState, session: Option<&Session>, opts: Value) -> Value {
    f0cklib::getf0ck(st, &opts_from(session, &opts)).await.unwrap_or_else(failure)
}
