//! Scraper guard admin page (`/admin/scrapers`), see crate::scraper_guard: the settings, the
//! busiest clients right now, blocks (lift them), and what clients did over the last days
//! (`scraper_activity`), to set the limits from.
//!
//! The forms are plain posts with `back` (crate::nojs turns the JSON reply into a redirect without
//! JavaScript); scrapers.js sends them with fetch and refreshes the page's live parts.

use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_raw};
use crate::scraper_guard::{self as sg, GuardConfig, Mode};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};
use std::collections::HashMap;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/scrapers\/?$", Guard::Auth, page);
    r.post_g(r"^\/api\/v2\/admin\/scrapers\/settings\/?$", Guard::Auth, save);
    r.post_g(r"^\/api\/v2\/admin\/scrapers\/lift\/?$", Guard::Auth, lift);
}

/// Periods the statistics can cover (days).
const PERIODS: [i64; 4] = [1, 7, 30, 90];

/// "Over N per hour" columns of the statistics.
const THRESHOLDS: [i64; 7] = [50, 100, 250, 500, 1000, 1500, 3000];

/// Group of a client key in SQL: member, onion or guest.
const GROUP_SQL: &str = "CASE WHEN client LIKE 'u:%' THEN 'member' WHEN client LIKE 'tor:%' OR onion THEN 'onion' ELSE 'guest' END";

fn group_of(key: &str) -> &'static str {
    if key.starts_with("u:") {
        "member"
    } else if key.starts_with("tor:") {
        "onion"
    } else {
        "guest"
    }
}

/// How a client is shown: members by name, guests by the start of their IP hash, .onion visitors
/// by circuit.
fn label(key: &str, names: &HashMap<i64, String>) -> Value {
    let group = group_of(key);
    let (text, user) = match key.split_once(':') {
        Some(("u", id)) => {
            let name = id.parse::<i64>().ok().and_then(|i| names.get(&i).cloned());
            (name.clone().unwrap_or_else(|| format!("user #{id}")), name)
        }
        Some(("tor", c)) => (format!("circuit {c}"), None),
        Some(("ip", h)) => (format!("ip {}", &h[..h.len().min(8)]), None),
        _ => (key.to_string(), None),
    };
    json!({ "key": key, "group": group, "text": text, "user": user })
}

async fn user_names(st: &AppState, keys: &[String]) -> HashMap<i64, String> {
    let ids: Vec<i64> = keys.iter().filter_map(|k| k.strip_prefix("u:")?.parse().ok()).collect();
    if ids.is_empty() {
        return HashMap::new();
    }
    match sql!(r#"SELECT id, "user" FROM "user" WHERE id = ANY("#, ids, ")").fetch_all(&st.db).await {
        Ok(rows) => rows.iter().filter_map(|r| Some((r.i64("id")?, r.string("user")))).collect(),
        Err(e) => {
            tracing::error!("[SCRAPER] user names: {e}");
            HashMap::new()
        }
    }
}

/// "3 h 12 min" until `until` (unix seconds).
fn remaining(until: i64, now: i64) -> String {
    let s = (until - now).max(0);
    let (h, m) = (s / 3600, (s % 3600) / 60);
    if h > 0 { format!("{h} h {m} min") } else { format!("{} min", m.max(1)) }
}

fn pct(part: i64, whole: i64) -> i64 {
    if whole > 0 { (part * 100 + whole / 2) / whole } else { 0 }
}

fn n(r: &Row, k: &str) -> i64 {
    r.i64(k).unwrap_or(0)
}

async fn page(st: AppState, req: Req) -> Res {
    let g = sg::settings(&st);
    let now = crate::util::now_secs();
    let days = req.query("days").and_then(|d| d.parse::<i64>().ok()).filter(|d| PERIODS.contains(d)).unwrap_or(7);
    let since = sql!("now() - make_interval(days => ", days as i32, ")");

    let live = sg::live(&g, 30);
    let blocks = sql!(
        r#"SELECT client, to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI') AS created,
            extract(epoch from expires_at)::bigint AS until, estimate, limit_at, enforced, onion, user_agent,
            lifted_at IS NOT NULL AS lifted, (SELECT "user" FROM "user" WHERE id = lifted_by) AS lifted_by
        FROM scraper_blocks ORDER BY created_at DESC LIMIT 100"#
    )
    .fetch_all(&st.db)
    .await?;
    let grp = GROUP_SQL;
    let mut groups_q = sql!("SELECT ");
    groups_q.push(grp).push(
        " AS grp, count(*) AS hours, count(DISTINCT client) AS clients, sum(requests)::bigint AS requests,
            percentile_disc(0.5) WITHIN GROUP (ORDER BY items + files) AS p50,
            percentile_disc(0.9) WITHIN GROUP (ORDER BY items + files) AS p90,
            percentile_disc(0.99) WITHIN GROUP (ORDER BY items + files) AS p99,
            max(items + files) AS max
        FROM scraper_activity WHERE hour > ",
    );
    groups_q.append(&since).push(" GROUP BY 1");
    let groups = groups_q.fetch_all(&st.db).await?;

    let mut over_q = sql!("WITH c AS (SELECT ");
    over_q.push(grp).push(" AS grp, client, max(items + files) AS m FROM scraper_activity WHERE hour > ");
    over_q.append(&since).push(" GROUP BY 1, 2) SELECT grp");
    for t in THRESHOLDS {
        over_q.push(&format!(", count(*) FILTER (WHERE m > {t}) AS o{t}"));
    }
    over_q.push(", count(*) FILTER (WHERE m > CASE grp WHEN 'member' THEN ").bind(g.member_limit as i64).push(" ELSE ").bind(g.guest_limit as i64);
    over_q.push(" END) AS over_limit FROM c GROUP BY grp");
    let over: HashMap<String, Row> = over_q.fetch_all(&st.db).await?.into_iter().map(|r| (r.string("grp"), r)).collect();

    let mut mix_q = sql!(
        "WITH c AS (SELECT client, max(items + files) AS m, sum(requests) AS r, sum(items) AS i, sum(files) AS f, sum(thumbs) AS t,
            sum(assets) AS a, sum(pages) AS p, sum(api) AS ap, sum(referer) AS rf FROM scraper_activity WHERE hour > "
    );
    mix_q.append(&since).push(
        " GROUP BY client)
        SELECT CASE WHEN m >= 1000 THEN 4 WHEN m >= 250 THEN 3 WHEN m >= 50 THEN 2 WHEN m >= 1 THEN 1 ELSE 0 END AS b,
            count(*) AS clients, sum(r)::bigint AS r, sum(i)::bigint AS i, sum(f)::bigint AS f, sum(t)::bigint AS t,
            sum(a)::bigint AS a, sum(p)::bigint AS p, sum(ap)::bigint AS ap, sum(rf)::bigint AS rf
        FROM c GROUP BY 1 ORDER BY 1",
    );
    let mix = mix_q.fetch_all(&st.db).await?;

    let mut top_q = sql!(
        "SELECT client, bool_or(onion) AS onion, max(items + files) AS max_h, count(*) AS hours, sum(items + files) AS things,
            sum(requests) AS requests, sum(thumbs) AS thumbs, sum(assets) AS assets, sum(referer) AS referer, max(peak) AS peak,
            (array_agg(user_agent ORDER BY hour DESC))[1] AS ua, (array_agg(ua_class ORDER BY hour DESC))[1] AS ua_class,
            to_char(max(hour) AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:00') AS last
        FROM scraper_activity WHERE hour > "
    );
    top_q.append(&since).push(" GROUP BY client ORDER BY max_h DESC, requests DESC LIMIT 30");
    let top = top_q.fetch_all(&st.db).await?;

    let keys: Vec<String> = live
        .iter()
        .map(|l| l.key.clone())
        .chain(blocks.iter().map(|b| b.string("client")))
        .chain(top.iter().map(|t| t.string("client")))
        .collect();
    let names = user_names(&st, &keys).await;

    let live_json: Vec<Value> = live
        .iter()
        .map(|l| {
            let a = &l.act;
            let limit = if l.key.starts_with("u:") { g.member_limit } else { g.guest_limit } as u64;
            json!({
                "who": label(&l.key, &names),
                "estimate": l.estimate,
                "limit": limit,
                "meter": l.estimate.min(limit),
                "over": l.estimate > limit,
                "requests": a.requests, "items": a.items, "files": a.files, "thumbs": a.thumbs, "assets": a.assets,
                "pages": a.pages, "api": a.api, "referer_pct": pct(a.referer as i64, a.requests as i64),
                "onion": a.onion,
                "ua": a.user_agent, "ua_class": sg::ua_class(&a.user_agent),
                "blocked": l.blocked.map(|(until, enforced)| json!({ "left": remaining(until as i64, now), "enforced": enforced })),
            })
        })
        .collect();

    let mut active = Vec::new();
    let mut history = Vec::new();
    for b in &blocks {
        let until = n(b, "until");
        let running = until > now && !b.bool("lifted");
        let enforced = b.bool("enforced");
        let status = if b.bool("lifted") {
            format!("lifted{}", b.str("lifted_by").map(|u| format!(" by {u}")).unwrap_or_default())
        } else if !enforced {
            "log only (not blocked)".to_string()
        } else if running {
            format!("blocked, {} left", remaining(until, now))
        } else {
            "ran out".to_string()
        };
        let row = json!({
            "who": label(&b.string("client"), &names),
            "created": b.string("created"),
            "estimate": n(b, "estimate"), "limit": n(b, "limit_at"),
            "enforced": enforced, "onion": b.bool("onion"),
            "ua": b.string("user_agent"), "ua_class": sg::ua_class(&b.string("user_agent")),
            "status": status,
        });
        if running && enforced { active.push(row) } else { history.push(row) }
    }

    let group_names = [("guest", "Guests (per IP)"), ("member", "Signed-in users"), ("onion", "Tor (per circuit)")];
    let groups_json: Vec<Value> = group_names
        .iter()
        .filter_map(|(key, title)| {
            let r = groups.iter().find(|r| r.string("grp") == *key)?;
            let o = over.get(*key);
            let limit = if *key == "member" { g.member_limit } else { g.guest_limit };
            Some(json!({
                "key": key, "title": title,
                "clients": n(r, "clients"), "hours": n(r, "hours"), "requests": n(r, "requests"),
                "p50": n(r, "p50"), "p90": n(r, "p90"), "p99": n(r, "p99"), "max": n(r, "max"),
                "over": THRESHOLDS.iter().map(|t| o.map(|o| n(o, &format!("o{t}"))).unwrap_or(0)).collect::<Vec<_>>(),
                "over_limit": o.map(|o| n(o, "over_limit")).unwrap_or(0),
                "limit": limit,
            }))
        })
        .collect();

    let bucket_names = ["no items/files", "1–49", "50–249", "250–999", "1000+"];
    let mix_json: Vec<Value> = mix
        .iter()
        .map(|m| {
            let r = n(m, "r");
            json!({
                "bucket": bucket_names.get(n(m, "b") as usize).copied().unwrap_or("?"),
                "clients": n(m, "clients"), "requests": r,
                "items": pct(n(m, "i"), r), "files": pct(n(m, "f"), r), "thumbs": pct(n(m, "t"), r),
                "assets": pct(n(m, "a"), r), "pages": pct(n(m, "p"), r), "api": pct(n(m, "ap"), r), "referer": pct(n(m, "rf"), r),
            })
        })
        .collect();

    let top_json: Vec<Value> = top
        .iter()
        .map(|t| {
            let r = n(t, "requests");
            json!({
                "who": label(&t.string("client"), &names),
                "max_h": n(t, "max_h"), "peak": n(t, "peak"), "hours": n(t, "hours"), "things": n(t, "things"), "requests": r,
                "thumbs": pct(n(t, "thumbs"), r), "assets": pct(n(t, "assets"), r), "referer": pct(n(t, "referer"), r),
                "ua": t.string("ua"), "ua_class": t.string("ua_class"), "last": t.string("last"),
            })
        })
        .collect();

    let data = json!({
        "session": req.session_value(),
        "sg": {
            "mode": g.mode.as_str(),
            "saved": st.settings.scraper_guard().is_some(),
            "window_minutes": g.window_minutes, "guest_limit": g.guest_limit, "member_limit": g.member_limit,
            "block_hours": g.block_hours, "status": g.status,
            "live_count": sg::live_count(),
            "live": live_json,
            "active": active,
            "history": history,
            "days": days,
            "periods": PERIODS,
            "thresholds": THRESHOLDS,
            "groups": groups_json,
            "mix": mix_json,
            "top": top_json,
            "tor_port": st.cfg().int("websrv.tor_port").unwrap_or(1338),
            "tor_on": st.cfg().truthy("websrv.enable_tor_hs"),
        },
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/scrapers", data)))
}

fn reply(ok: bool, msg: &str, code: u16) -> Res {
    Ok(json_raw(&json!({ "success": ok, "msg": msg }), code))
}

/// A field of the posted form / JSON body as text.
fn field(body: &Value, k: &str) -> Option<String> {
    match body.get(k)? {
        Value::String(s) => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

async fn save(st: AppState, mut req: Req) -> Res {
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    let body = req.post().await?.clone();
    let old = sg::settings(&st);
    let Some(mode) = field(&body, "mode").as_deref().and_then(Mode::parse) else { return reply(false, "Pick a mode", 400) };
    let num = |k: &str, def: u32| -> Result<u32, String> {
        match field(&body, k).filter(|v| !v.is_empty()) {
            None => Ok(def),
            Some(v) => v.parse::<u32>().map_err(|_| format!("{k}: not a number")),
        }
    };
    let parsed = (|| -> Result<GuardConfig, String> {
        Ok(GuardConfig {
            mode,
            window_minutes: num("window_minutes", old.window_minutes)?,
            guest_limit: num("guest_limit", old.guest_limit)?,
            member_limit: num("member_limit", old.member_limit)?,
            block_hours: num("block_hours", old.block_hours)?,
            status: num("status", old.status as u32)?.min(u16::MAX as u32) as u16,
        })
    })();
    let g = match parsed {
        Ok(g) => g.clean(),
        Err(m) => return reply(false, &m, 400),
    };
    let stored = serde_json::to_string(&g).unwrap_or_default();
    if let Err(e) = crate::settings::save(&st, sg::KEY, &stored).await {
        tracing::error!("[SCRAPER] saving settings failed: {e:#}");
        return reply(false, "Save failed", 500);
    }
    st.settings.update(|s| s.scraper_guard = Some(g.clone()));
    crate::audit::log(&st, uid, "update_scraper_guard", "system", "0".to_string(), serde_json::to_value(&g).unwrap_or(Value::Null)).await;
    let msg = match g.mode {
        Mode::Off => "Saved: the scraper guard is off",
        Mode::Log => "Saved: counting and logging, nobody is blocked",
        Mode::Block => "Saved: clients over the limit are blocked",
    };
    reply(true, msg, 200)
}

/// A client key as the guard writes it.
fn valid_key(k: &str) -> bool {
    match k.split_once(':') {
        Some(("u" | "tor", v)) => !v.is_empty() && v.len() <= 20 && v.bytes().all(|b| b.is_ascii_digit()),
        Some(("ip", v)) => !v.is_empty() && v.len() <= 64 && v.bytes().all(|b| b.is_ascii_hexdigit()),
        _ => false,
    }
}

async fn lift(st: AppState, mut req: Req) -> Res {
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    let body = req.post().await?.clone();
    let Some(key) = field(&body, "client").filter(|k| valid_key(k)) else { return reply(false, "Unknown client", 400) };
    match sg::lift(&st, &key, uid).await {
        Ok(true) => {
            crate::audit::log(&st, uid, "scraper_unblock", "system", key.clone(), json!({ "client": key })).await;
            reply(true, "Block lifted", 200)
        }
        Ok(false) => reply(false, "No active block for that client", 404),
        Err(e) => {
            tracing::error!("[SCRAPER] lifting {key} failed: {e:#}");
            reply(false, "Lifting the block failed", 500)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_keys_and_labels() {
        assert!(valid_key("u:12") && valid_key("tor:4294967295") && valid_key("ip:0123456789abcdef"));
        assert!(!valid_key("u:") && !valid_key("ip:xyz") && !valid_key("x:1") && !valid_key("u:1;drop"));
        let names = HashMap::from([(12, "alice".to_string())]);
        assert_eq!(label("u:12", &names)["text"], "alice");
        assert_eq!(label("u:13", &names)["text"], "user #13");
        assert_eq!(label("ip:0123456789abcdef", &names)["text"], "ip 01234567");
        assert_eq!(label("tor:7", &names)["group"], "onion");
        assert_eq!(remaining(3600 * 3 + 120, 0), "3 h 2 min");
        assert_eq!(remaining(10, 0), "1 min");
    }
}
