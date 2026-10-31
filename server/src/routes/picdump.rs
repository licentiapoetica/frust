//! `/picdump` (src/inc/routes/picdump.mjs).
//!
//! The old route rendered a `picdump` template that does not exist (the response was the
//! ENOENT error text, including a server path) and selected every image of the last week without
//! any active/deleted/visibility filter. It is answered with the regular 404 page instead.

use crate::http::{Req, Res, Router, html_code};
use crate::state::AppState;
use serde_json::json;

pub fn register(r: &mut Router) {
    r.get(r"^\/picdump$", picdump);
}

async fn picdump(st: AppState, req: Req) -> Res {
    Ok(html_code(crate::tpl::render(&st, Some(&req), "error", json!({ "message": "Not found", "tmp": null })), 404))
}
