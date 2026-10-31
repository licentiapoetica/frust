//! In-memory index of private (visibility 2) and unavailable (visibility 3) items, used to gate
//! direct media URLs (port of src/inc/private_items.mjs).

use crate::db::RowExt;
use crate::http::{Req, json_raw, raw};
use crate::sql;
use crate::state::{AppState, PrivateItems};
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::json;

pub struct PrivateHit {
    pub is_unavailable: bool,
    pub owner: String,
}

pub async fn init(st: &AppState) {
    let rows = match sql!("SELECT id, dest, LOWER(username) as username, visibility FROM items WHERE visibility IN (2, 3) AND is_deleted = false")
        .fetch_all(&st.db)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[BOOT] Failed to load private/unavailable items into cache: {e}");
            return;
        }
    };
    let mut p = PrivateItems::default();
    for r in rows {
        let u = r.string("username");
        let id = r.i64("id");
        let dest = r.str("dest").filter(|d| !d.is_empty()).map(str::to_string);
        match r.i64("visibility") {
            Some(2) => {
                if let Some(d) = dest {
                    p.private_dests.insert(d, u.clone());
                }
                if let Some(id) = id {
                    p.private_ids.insert(id, u);
                }
            }
            Some(3) => {
                if let Some(d) = dest {
                    p.unavailable_dests.insert(d, u.clone());
                }
                if let Some(id) = id {
                    p.unavailable_ids.insert(id, u);
                }
            }
            _ => {}
        }
    }
    *st.caches.private_items.write() = p;
}

pub fn add_private(st: &AppState, id: i64, dest: &str, username: &str) {
    let mut p = st.caches.private_items.write();
    let u = username.to_lowercase();
    if !dest.is_empty() {
        p.private_dests.insert(dest.to_string(), u.clone());
        p.unavailable_dests.remove(dest);
    }
    p.private_ids.insert(id, u);
    p.unavailable_ids.remove(&id);
}

pub fn remove_private(st: &AppState, id: i64, dest: &str) {
    let mut p = st.caches.private_items.write();
    p.private_dests.remove(dest);
    p.private_ids.remove(&id);
}

pub fn add_unavailable(st: &AppState, id: i64, dest: &str, username: &str) {
    let mut p = st.caches.private_items.write();
    let u = username.to_lowercase();
    if !dest.is_empty() {
        p.unavailable_dests.insert(dest.to_string(), u.clone());
        p.private_dests.remove(dest);
    }
    p.unavailable_ids.insert(id, u);
    p.private_ids.remove(&id);
}

pub fn remove_unavailable(st: &AppState, id: i64, dest: &str) {
    let mut p = st.caches.private_items.write();
    p.unavailable_dests.remove(dest);
    p.unavailable_ids.remove(&id);
}

fn leading_id(file: &str) -> Option<i64> {
    let digits: String = file.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Private/unavailable lookup for /b/<dest>, /t/<id>…, /ca/<id>….
pub fn from_path(st: &AppState, path: &str) -> Option<PrivateHit> {
    let p = st.caches.private_items.read();
    let clean = |s: &str| {
        let d = crate::util::decode_lossy(s);
        d.split(['?', '#']).next().unwrap_or("").to_string()
    };
    if let Some(rest) = path.strip_prefix("/b/") {
        let dest = clean(rest);
        if let Some(o) = p.private_dests.get(&dest) {
            return Some(PrivateHit { is_unavailable: false, owner: o.clone() });
        }
        if let Some(o) = p.unavailable_dests.get(&dest) {
            return Some(PrivateHit { is_unavailable: true, owner: o.clone() });
        }
        return None;
    }
    let rest = path.strip_prefix("/t/").or_else(|| path.strip_prefix("/ca/"))?;
    let id = leading_id(&clean(rest))?;
    if let Some(o) = p.private_ids.get(&id) {
        return Some(PrivateHit { is_unavailable: false, owner: o.clone() });
    }
    p.unavailable_ids.get(&id).map(|o| PrivateHit { is_unavailable: true, owner: o.clone() })
}

pub fn is_private_path(st: &AppState, path: &str) -> bool {
    from_path(st, path).is_some()
}

fn no_cache(mut r: Response) -> Response {
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, no-store, must-revalidate"));
    r
}

pub fn render_502(st: &AppState, req: &Req) -> Response {
    if req.is_xhr() {
        return no_cache(json_raw(&json!({ "success": false, "msg": "Bad Gateway" }), 502));
    }
    let mut r = raw(502, crate::gate::gate_or_nginx(st, req));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    no_cache(r)
}

pub fn render_451(req: &Req) -> Response {
    if req.is_xhr() || req.accepts_json() {
        return no_cache(json_raw(&json!({ "success": false, "is_unavailable": true, "msg": "451 - Unavailable For Legal Reasons" }), 451));
    }
    let body = "<html>\r\n<head><title>451 Unavailable For Legal Reasons</title></head>\r\n<body>\r\n<center><h1>451 Unavailable For Legal Reasons</h1></center>\r\n<hr><center>nginx</center>\r\n</body>\r\n</html>\r\n";
    let mut r = raw(451, body);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html"));
    no_cache(r)
}
