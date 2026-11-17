//! Item views: `items.views`, one per viewer and item every [`WINDOW_MINUTES`] minutes (watching
//! again later counts again; going back and forth within the window doesn't).
//!
//! - A view is the item actually seen: the page reports it after 3 seconds on screen, or 3 seconds of
//!   a video / song played (`f0ckCountView`, f0ck-core.js): `POST /api/v2/view/<id>`. Without
//!   JavaScript the page load counts: the page's hidden lazy image `GET /api/v2/view/<id>/nojs.gif`
//!   (snippets/view-beacon.html; with JavaScript a hidden lazy image is never loaded).
//! - The viewer: a member or anonymous account by its id; a guest by a hash of the day, the IP and
//!   the browser (HMAC with the IP hash secret), so no IP is stored and the key changes every day.
//! - Not counted: the uploader on their own item, bots and scripts (user agent), requests from
//!   other sites (an embedded image or a form elsewhere), items that are gone.
//! - `item_view_marks` remembers who saw what lately; the hourly retention job (retention.rs)
//!   deletes marks older than the window.
//!
//! `user_video_views` (per member, every page load) stays for the "watched" marks.

use crate::db::RowExt;
use crate::http::{Req, Res, json_resp, raw, with_header};
use crate::sql;
use crate::state::AppState;
use crate::util::is_bot_ua as is_bot;
use axum::http::Method;
use serde_json::json;

/// One view per viewer and item in this many minutes.
pub const WINDOW_MINUTES: i32 = 10;

pub fn register(g: &mut crate::http::Group) {
    g.route(Method::POST, r"\/view\/(?<id>[0-9]+)$", None, count_view);
    g.get(r"\/view\/(?<id>[0-9]+)\/nojs\.gif$", count_view_nojs);
}

/// `POST /api/v2/view/:id` (sendBeacon / fetch from f0ckCountView): `{ success, counted }`.
async fn count_view(st: AppState, req: Req) -> Res {
    let counted = match req.param("id").and_then(|i| i.parse::<i32>().ok()) {
        Some(id) => count(&st, &req, id).await,
        None => false,
    };
    Ok(json_resp(&json!({ "success": true, "counted": counted }), 200))
}

/// `GET /api/v2/view/:id/nojs.gif`: the no-JavaScript page's beacon, a transparent pixel.
async fn count_view_nojs(st: AppState, req: Req) -> Res {
    if let Some(id) = req.param("id").and_then(|i| i.parse::<i32>().ok()) {
        count(&st, &req, id).await;
    }
    // the smallest transparent GIF
    const PIXEL: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\x00\x00\x00!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";
    let res = with_header(raw(200, PIXEL), "content-type", "image/gif");
    Ok(with_header(res, "cache-control", "no-store, private"))
}

/// Counts the view when it is one (see the module docs); true when it was counted.
async fn count(st: &AppState, req: &Req, item_id: i32) -> bool {
    let ua = req.header("user-agent").unwrap_or("");
    if is_bot(ua) || !same_site(req.header("sec-fetch-site"), req.header("origin"), req.header("referer"), &req.host_without_port()) {
        return false;
    }
    let item = match sql!("SELECT username FROM items WHERE id = ", item_id, " AND active = true AND is_deleted = false")
        .fetch_optional(&st.db)
        .await
    {
        Ok(Some(row)) => row,
        _ => return false,
    };
    let viewer = match req.session().filter(|s| s.id != 0) {
        Some(s) => {
            let uploader = item.string("username").to_lowercase();
            if !uploader.is_empty() && (uploader == s.user.to_lowercase() || uploader == s.login.to_lowercase()) {
                return false;
            }
            format!("u:{}", s.id)
        }
        None => {
            let secret = crate::security::ip_hash_secrets(&st.cfg()).current;
            let ip = req.real_ip();
            if secret.is_empty() || ip.is_empty() {
                return false;
            }
            guest_key(&secret, &chrono::Utc::now().format("%Y-%m-%d").to_string(), &ip, ua)
        }
    };
    // the mark is new, or older than the window: count it (one statement, so two tabs can't both)
    let q = sql!(
        "WITH mark AS (INSERT INTO item_view_marks (item_id, viewer, seen_at) VALUES (",
        item_id,
        ", ",
        viewer.as_str(),
        ", now()) ON CONFLICT (item_id, viewer) DO UPDATE SET seen_at = now() WHERE item_view_marks.seen_at < now() - make_interval(mins => ",
        WINDOW_MINUTES,
        ") RETURNING item_id) UPDATE items SET views = views + 1 WHERE id IN (SELECT item_id FROM mark) RETURNING views"
    );
    let counted = match q.fetch_optional(&st.db).await {
        Ok(r) => r.is_some(),
        Err(e) => {
            tracing::error!("[VIEWS] item {item_id}: {e}");
            false
        }
    };
    counted
}

/// A guest's viewer key for a day: no IP in it, a new one every day.
fn guest_key(secret: &str, day: &str, ip: &str, ua: &str) -> String {
    let h = crate::util::hmac_sha256_hex(secret.as_bytes(), format!("view|{day}|{ip}|{ua}").as_bytes());
    format!("g:{}", &h[..40])
}

/// The request comes from the site's own pages: the browser says so (Sec-Fetch-Site), else the
/// Origin or the Referer is this host. Refuses another site embedding the beacon or posting to it.
fn same_site(fetch_site: Option<&str>, origin: Option<&str>, referer: Option<&str>, host: &str) -> bool {
    if let Some(site) = fetch_site {
        return site == "same-origin";
    }
    let host_of = |u: &str| url::Url::parse(u).ok().and_then(|u| u.host_str().map(str::to_lowercase));
    match origin.or(referer) {
        Some(u) => host_of(u).is_some_and(|h| h == host.to_lowercase()),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bots_are_not_viewers() {
        for ua in ["", "Googlebot/2.1 (+http://www.google.com/bot.html)", "Mozilla/5.0 (compatible; bingbot/2.0)", "facebookexternalhit/1.1",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0 Safari/537.36", "curl/8.5.0", "python-requests/2.31",
            "Discordbot/2.0", "TelegramBot (like TwitterBot)"] {
            assert!(is_bot(ua), "{ua}");
        }
        for ua in ["Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"] {
            assert!(!is_bot(ua), "{ua}");
        }
    }

    #[test]
    fn only_the_sites_own_pages() {
        assert!(same_site(Some("same-origin"), None, None, "f0ck.test"));
        assert!(!same_site(Some("cross-site"), Some("https://f0ck.test"), None, "f0ck.test"));
        assert!(!same_site(Some("none"), None, None, "f0ck.test"), "typed into the address bar");
        assert!(same_site(None, Some("https://F0ck.test"), None, "f0ck.test"));
        assert!(same_site(None, None, Some("https://f0ck.test/7"), "f0ck.test"));
        assert!(!same_site(None, None, Some("https://evil.test/f0ck.test"), "f0ck.test"));
        assert!(!same_site(None, None, None, "f0ck.test"));
    }

    #[test]
    fn guest_keys_hide_the_ip_and_change_daily() {
        let a = guest_key("s3cret", "2026-11-16", "203.0.113.9", "Firefox");
        assert!(a.starts_with("g:") && a.len() == 42 && !a.contains("203.0.113.9"));
        assert_eq!(a, guest_key("s3cret", "2026-11-16", "203.0.113.9", "Firefox"));
        assert_ne!(a, guest_key("s3cret", "2026-11-17", "203.0.113.9", "Firefox"));
        assert_ne!(a, guest_key("s3cret", "2026-11-16", "203.0.113.10", "Firefox"));
        assert_ne!(a, guest_key("s3cret", "2026-11-16", "203.0.113.9", "Chrome"));
    }
}
