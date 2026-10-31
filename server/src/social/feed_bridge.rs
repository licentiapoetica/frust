//! Calls into the feed/item-page port (`f0cklib.getf0cks` / `f0cklib.getf0ck`), which another
//! agent is porting into `crate::f0cklib`.
//!
//! TODO(port): replace both bodies with calls to the feed port once it is merged. The options
//! object is passed exactly like the old JS call (`{ page, mode, session, exclude, user_id,
//! userHall, userHallOwner, mime, random }` / `{ itemid, ... }`), and the result must be the same
//! JSON the old functions returned (`{ success, items, pagination, link, tmp, ... }` /
//! `{ success, item, ..., message }`).

use crate::state::AppState;
use serde_json::{Value, json};

/// `f0cklib.getf0cks(opts)` — item grid. Stub: reports failure, so callers render an empty grid.
pub async fn get_f0cks(_st: &AppState, _opts: Value) -> Value {
    json!({ "success": false, "message": "TODO(port): getf0cks not ported yet" })
}

/// `f0cklib.getf0ck(opts)` — single item page data. Stub: reports "not found".
pub async fn get_f0ck(_st: &AppState, _opts: Value) -> Value {
    json!({ "success": false, "message": "TODO(port): getf0ck not ported yet" })
}
