//! Audit log page (`/mod/audit`, permission `mod_audit`): every moderation and admin action, with
//! filters (category, action, who, target, text, dates) and a summary of what matches; for admins
//! also CSV/JSON export of the filtered entries and the log's retention and manual pruning.
//!
//! Everything is plain links and forms (works without JavaScript); `views/mod/audit.js` loads
//! older entries while scrolling and adds new ones live (`audit_entry` events, see crate::audit).

use crate::audit::{self, Filter};
use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, html, json_resp, redirect, reply, with_header};
use crate::perms::P;
use crate::sql;
use crate::state::AppState;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, HashMap};

pub fn register(r: &mut Router) {
    use crate::http::Guard::{Auth, Perm};
    r.get_g(r"^\/mod\/audit\/export\/?$", Auth, export);
    r.post_g(r"^\/mod\/audit\/retention\/?$", Auth, set_retention);
    r.post_g(r"^\/mod\/audit\/prune\/?$", Auth, prune);
    r.get_g(r"^\/mod\/audit\/?$", Perm(P::ModAudit), page);
    // The admin bar's link
    r.get_g(r"^\/admin\/audit\/?$", Perm(P::ModAudit), |_st: AppState, _req: Req| async { Ok(redirect("/mod/audit")) });
}

const PAGE_SIZE: i64 = 50;
/// Most entries one export holds (newest first)
const EXPORT_MAX: i64 = 100_000;
/// Detail keys shown in their own row (reason, diff, uploader), not in the field list
const SHOWN_KEYS: [&str; 6] = ["reason", "old_content", "new_content", "item_id", "uploader_name", "uploader_id"];

fn filter_of(req: &Req) -> Filter {
    Filter::from_query(|k| req.query(k).map(str::to_string))
}

fn page_of(req: &Req) -> i64 {
    req.query("page").and_then(|p| p.trim().parse::<i64>().ok()).unwrap_or(1).clamp(1, 1_000_000)
}

/// A path segment, percent-encoded (`byte_serialize` writes spaces as `+`, which a path keeps).
fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>().replace('+', "%20")
}

fn fmt_time(created_at: Option<&str>) -> (String, String) {
    match created_at.and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok()) {
        Some(d) => (d.to_rfc3339(), crate::tpl::locale_string_str(&d.with_timezone(&chrono::Local), None)),
        None => (String::new(), String::new()),
    }
}

/// A detail value as text: strings as they are, everything else as JSON.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        v => crate::util::json_string(v),
    }
}

fn nonempty(v: Option<&Value>) -> Option<String> {
    v.map(text_of).filter(|s| !s.is_empty() && s != "null")
}

fn details_of(row: &mut Row) -> Map<String, Value> {
    match row.remove("details").unwrap_or(Value::Null) {
        Value::Object(m) => m,
        Value::String(s) => serde_json::from_str::<Value>(&s).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default(),
        _ => Map::new(),
    }
}

fn is_num(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit())
}

/// Where an entry's target lives on the site, when it has a page.
fn target_link(target_type: &str, target_id: &str, d: &Map<String, Value>) -> Option<String> {
    match target_type {
        "item" if is_num(target_id) => Some(format!("/{target_id}")),
        "comment" if is_num(target_id) => nonempty(d.get("item_id")).filter(|i| is_num(i)).map(|i| format!("/{i}#c{target_id}")),
        "album_item" => nonempty(d.get("postid")).filter(|i| is_num(i)).map(|i| format!("/{i}")),
        "user" => ["target_user", "new_user", "username", "user"]
            .iter()
            .find_map(|k| nonempty(d.get(*k)))
            .map(|u| format!("/user/{}", enc(&u))),
        "hall" if !target_id.is_empty() && target_id != "0" && !is_num(target_id) => Some(format!("/list/{}", enc(target_id))),
        _ => None,
    }
}

/// One `audit_log` row (with `username`) for the page.
fn entry(mut row: Row, f: &Filter, names: &HashMap<i64, String>) -> Value {
    let d = details_of(&mut row);
    let id = row.i64("id").unwrap_or(0);
    let user_id = row.i64("user_id").unwrap_or(0);
    let action = row.string("action");
    let target_type = row.string("target_type");
    let target_id = row.string("target_id");
    let (iso, when) = fmt_time(row.str("created_at"));
    let category = audit::category_of(&action);
    let username = row.str("username").map(str::to_string).or_else(|| names.get(&user_id).cloned());

    // Old → new: the content diff, or the tag a tag change touched
    let (mut old, mut new) = (nonempty(d.get("old_content")), nonempty(d.get("new_content")));
    let toggle = action == "toggle_tag";
    if toggle {
        if d.get("action").and_then(Value::as_str) == Some("added") {
            new = nonempty(d.get("tag"));
        } else if let Some(from) = nonempty(d.get("from")) {
            old = Some(from);
            new = nonempty(d.get("tag"));
        }
    }

    let fields: Vec<Value> = d
        .iter()
        .filter(|(k, _)| !SHOWN_KEYS.contains(&k.as_str()) && !(toggle && ["tag", "from", "action"].contains(&k.as_str())))
        .map(|(k, v)| json!({ "key": k.replace('_', " "), "value": text_of(v) }))
        .filter(|f| f["value"].as_str().is_some_and(|v| !v.is_empty()))
        .collect();

    let uploader = nonempty(d.get("uploader_name")).map(|n| json!({ "name": n, "id": nonempty(d.get("uploader_id")), "href": format!("/user/{}", enc(&n)) }));
    let has_target = !target_id.is_empty() && target_id != "0";
    let with = |k: &'static str, v: String| {
        let mut g = f.clone();
        match k {
            "action" => {
                g.action = v;
                g.category.clear();
            }
            "category" => {
                g.category = v;
                g.action.clear();
            }
            _ => {}
        }
        g.href(&[])
    };
    let mut target_f = Filter { target_type: target_type.clone(), target_id: target_id.clone(), ..Default::default() };
    if !has_target {
        target_f.target_id.clear();
    }

    json!({
        "id": id,
        "created_at": iso,
        "created_at_fmt": when,
        "user_id": user_id,
        "username": username,
        "actor_href": Filter { actor: (user_id > 0).then_some(user_id), ..f.clone() }.href(&[]),
        "profile_href": username.as_deref().map(|u| format!("/user/{}", enc(u))),
        "action": action,
        "action_label": audit::action_label(&action),
        "action_href": with("action", action.clone()),
        "category": category,
        "category_label": audit::category_label(category),
        "category_href": with("category", category.to_string()),
        "target_type": target_type,
        "target_id": if has_target { target_id.clone() } else { String::new() },
        "target_link": target_link(&target_type, &target_id, &d),
        "history_href": (!target_type.is_empty()).then(|| target_f.href(&[])),
        "reason": nonempty(d.get("reason")),
        "old_content": old,
        "new_content": new,
        "uploader": uploader,
        "fields": fields,
        "raw": if d.is_empty() { Value::Null } else { json!(serde_json::to_string_pretty(&Value::Object(d)).unwrap_or_default()) },
    })
}

/// Names of these users.
async fn user_names(st: &AppState, ids: Vec<i64>) -> HashMap<i64, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    let rows = sql!(r#"SELECT id, "user" AS name FROM "user" WHERE id = ANY("#, ids, ")").fetch_all(&st.db).await.unwrap_or_default();
    rows.iter().filter_map(|r| Some((r.i64("id")?, r.string("name")))).collect()
}

async fn page(st: AppState, req: Req) -> Res {
    let f = filter_of(&req);
    let page = page_of(&req);
    let w = f.sql();
    let offset = (page - 1) * PAGE_SIZE;
    let rows = sql!(
        r#"SELECT al.id, al.user_id, al.action, al.target_type, al.target_id, al.details, al.created_at, u."user" AS username
           FROM audit_log al LEFT JOIN "user" u ON u.id = al.user_id WHERE true"#,
        @w,
        " ORDER BY al.created_at DESC, al.id DESC LIMIT ", PAGE_SIZE, " OFFSET ", offset
    )
    .fetch_all(&st.db)
    .await?;

    // What matches the filters, by action and by who did it
    let matched = sql!("SELECT al.action, al.user_id, count(*)::bigint AS n FROM audit_log al WHERE true", @w, " GROUP BY al.action, al.user_id")
        .fetch_all(&st.db)
        .await?;
    // Every action, person and target type in the log (the filters' choices)
    let all = sql!("SELECT action, user_id, target_type, count(*)::bigint AS n FROM audit_log GROUP BY action, user_id, target_type").fetch_all(&st.db).await?;

    let mut actor_ids: Vec<i64> = all.iter().filter_map(|r| r.i64("user_id")).collect();
    actor_ids.sort_unstable();
    actor_ids.dedup();
    let names = user_names(&st, actor_ids.clone()).await;
    let manage = if req.session().is_some_and(|s| s.admin) {
        let total_all: i64 = all.iter().filter_map(|r| r.i64("n")).sum();
        let oldest = sql!("SELECT min(created_at) AS t FROM audit_log").fetch_optional(&st.db).await?.unwrap_or_default();
        json!({
            "retention_days": st.settings.audit_retention_days(),
            "retention_max": crate::settings::AUDIT_RETENTION_MAX,
            "total": total_all,
            "oldest": fmt_time(oldest.str("t")).1,
        })
    } else {
        Value::Null
    };

    let data = page_data(&f, page, rows, &matched, &all, &names, manage);
    Ok(html(crate::tpl::render(&st, Some(&req), "mod/audit", data)))
}

/// The page's data: `rows` this page's entries (with `username`), `matched` / `all` counts by
/// action and user (and target type) of the matches / the whole log, `names` the people's names.
fn page_data(f: &Filter, page: i64, rows: Vec<Row>, matched: &[Row], all: &[Row], names: &HashMap<i64, String>, manage: Value) -> Value {
    let offset = (page - 1) * PAGE_SIZE;
    let name_of = |id: i64| names.get(&id).cloned().unwrap_or_else(|| format!("#{id}"));

    // Choices: actions grouped by category, people, target types (with their counts)
    let mut action_n: BTreeMap<String, i64> = BTreeMap::new();
    let mut actor_n: BTreeMap<i64, i64> = BTreeMap::new();
    let mut type_n: BTreeMap<String, i64> = BTreeMap::new();
    for r in all {
        let n = r.i64("n").unwrap_or(0);
        *action_n.entry(r.string("action")).or_default() += n;
        *actor_n.entry(r.i64("user_id").unwrap_or(0)).or_default() += n;
        let t = r.string("target_type");
        if !t.is_empty() {
            *type_n.entry(t).or_default() += n;
        }
    }
    // A filtered action that isn't in the log (any more) stays selectable
    if !f.action.is_empty() {
        action_n.entry(f.action.clone()).or_default();
    }
    let categories: Vec<Value> = audit::CATEGORIES
        .iter()
        .filter_map(|(key, label)| {
            let mut actions: Vec<Value> = action_n
                .iter()
                .filter(|(a, _)| audit::category_of(a) == *key)
                .map(|(a, n)| json!({ "key": a, "label": audit::action_label(a), "count": n }))
                .collect();
            if actions.is_empty() && f.category != *key {
                return None;
            }
            actions.sort_by(|a, b| a["label"].as_str().cmp(&b["label"].as_str()));
            let count: i64 = actions.iter().filter_map(|a| a["count"].as_i64()).sum();
            Some(json!({ "key": key, "label": label, "count": count, "actions": actions }))
        })
        .collect();
    let mut actors: Vec<Value> = actor_n.iter().map(|(id, n)| json!({ "id": id, "name": name_of(*id), "count": n })).collect();
    actors.sort_by_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());
    let target_types: Vec<Value> = type_n.iter().map(|(t, n)| json!({ "key": t, "count": n })).collect();

    // Summary of the matches: total, top people, categories (each a link narrowing the filter)
    let mut total = 0;
    let mut by_actor: BTreeMap<i64, i64> = BTreeMap::new();
    let mut by_cat: BTreeMap<&'static str, i64> = BTreeMap::new();
    for r in matched {
        let n = r.i64("n").unwrap_or(0);
        total += n;
        *by_actor.entry(r.i64("user_id").unwrap_or(0)).or_default() += n;
        *by_cat.entry(audit::category_of(&r.string("action"))).or_default() += n;
    }
    let mut top: Vec<(i64, i64)> = by_actor.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let people = top.len();
    let top_actors: Vec<Value> = top
        .into_iter()
        .take(6)
        .map(|(id, n)| json!({ "name": name_of(id), "count": n, "href": Filter { actor: Some(id), ..f.clone() }.href(&[]), "active": f.actor == Some(id) }))
        .collect();
    let cat_summary: Vec<Value> = audit::CATEGORIES
        .iter()
        .filter_map(|(key, label)| {
            let n = *by_cat.get(key)?;
            let href = Filter { category: key.to_string(), action: String::new(), ..f.clone() }.href(&[]);
            Some(json!({ "key": key, "label": label, "count": n, "href": href, "active": f.category == *key }))
        })
        .collect();

    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let entries: Vec<Value> = rows.into_iter().map(|r| entry(r, &f, &names)).collect();
    let page_href = |p: i64| f.href(&if p > 1 { vec![("page", p.to_string())] } else { vec![] });

    json!({
        "entries": entries,
        "filter": {
            "category": f.category, "action": f.action, "actor": f.actor, "target_type": f.target_type,
            "target_id": f.target_id, "q": f.q, "from": f.from, "to": f.to,
        },
        "filter_active": !f.is_empty(),
        "filter_pairs": f.pairs().into_iter().map(|(k, v)| json!({ "name": k, "value": v })).collect::<Vec<_>>(),
        "back": page_href(page),
        "categories": categories,
        "actors": actors,
        "target_types": target_types,
        "summary": { "total": total, "people": people, "actors": top_actors, "categories": cat_summary },
        "page": page,
        "pages": pages,
        "first": offset + 1,
        "last": (offset + PAGE_SIZE).min(total),
        "prev_href": (page > 1).then(|| page_href(page - 1)),
        "next_href": (page < pages).then(|| page_href(page + 1)),
        // Export, retention and pruning: admins only
        "can_export": !manage.is_null(),
        "manage": manage,
        "tmp": null,
    })
}

// ── Export ───────────────────────────────────────────────────────────────────

/// A CSV cell: quoted when needed; text a spreadsheet would run as a formula gets a leading `'`.
fn csv_cell(s: &str) -> String {
    let s = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{s}") } else { s.to_string() };
    if s.contains([',', '"', '\n', '\r']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s }
}

/// `GET /mod/audit/export?format=csv|json&<filters>` (admins): the matching entries, newest first.
async fn export(st: AppState, req: Req) -> Res {
    let f = filter_of(&req);
    let w = f.sql();
    let mut rows = sql!(
        r#"SELECT al.id, al.user_id, al.action, al.target_type, al.target_id, al.details, al.created_at, u."user" AS username
           FROM audit_log al LEFT JOIN "user" u ON u.id = al.user_id WHERE true"#,
        @w,
        " ORDER BY al.created_at DESC, al.id DESC LIMIT ", EXPORT_MAX
    )
    .fetch_all(&st.db)
    .await?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M");
    let json_format = req.query("format") == Some("json");

    let records: Vec<Value> = rows
        .iter_mut()
        .map(|r| {
            let d = details_of(r);
            let action = r.string("action");
            json!({
                "id": r.i64("id"),
                "created_at": fmt_time(r.str("created_at")).0,
                "user_id": r.i64("user_id"),
                "user": r.val("username"),
                "action": action,
                "category": audit::category_of(&action),
                "target_type": r.val("target_type"),
                "target_id": r.val("target_id"),
                "details": d,
            })
        })
        .collect();

    let (ctype, ext, body) = if json_format {
        ("application/json; charset=utf-8", "json", serde_json::to_string_pretty(&records)?)
    } else {
        let mut out = String::from("id,created_at,user_id,user,action,category,target_type,target_id,details\r\n");
        for r in &records {
            let cells = ["id", "created_at", "user_id", "user", "action", "category", "target_type", "target_id"].map(|k| csv_cell(&text_of(&r[k])));
            out.push_str(&cells.join(","));
            out.push(',');
            out.push_str(&csv_cell(&crate::util::json_string(&r["details"])));
            out.push_str("\r\n");
        }
        ("text/csv; charset=utf-8", "csv", out)
    };
    let r = reply(200, ctype, body);
    Ok(with_header(r, "content-disposition", &format!("attachment; filename=\"audit-log-{stamp}.{ext}\"")))
}

// ── Admin: retention and pruning ─────────────────────────────────────────────

fn done(v: Value) -> Res {
    Ok(json_resp(&v, 200))
}

fn fail(msg: &str) -> Res {
    Ok(json_resp(&json!({ "success": false, "msg": msg }), 400))
}

fn body_days(body: &Value) -> Option<i64> {
    match body.get("days")? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// `POST /mod/audit/retention` `days` (0 = keep forever): entries older than that are deleted, now
/// and from then on hourly (retention job).
async fn set_retention(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(days) = body_days(&body).filter(|d| (0..=crate::settings::AUDIT_RETENTION_MAX).contains(d)) else {
        return fail(&format!("Days: a whole number from 0 (keep forever) to {}", crate::settings::AUDIT_RETENTION_MAX));
    };
    let old = st.settings.audit_retention_days();
    if let Err(e) = crate::settings::save(&st, "audit_retention_days", &days.to_string()).await {
        tracing::error!("[AUDIT] Saving the retention failed: {e}");
        return fail("Saving failed");
    }
    st.settings.update(|s| s.audit_retention_days = days);
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    if old != days {
        audit::log(&st, uid, "audit_set_retention", "settings", "audit_retention_days".to_string(), json!({ "old_content": old.to_string(), "new_content": days.to_string() })).await;
    }
    let deleted = audit::prune_older_than(&st, days).await.unwrap_or(0);
    let msg = match (days, deleted) {
        (0, _) => "Audit entries are kept forever".to_string(),
        (d, 0) => format!("Audit entries older than {d} days are deleted"),
        (d, n) => format!("Audit entries older than {d} days are deleted ({n} deleted now)"),
    };
    done(json!({ "success": true, "msg": msg, "days": days, "deleted": deleted }))
}

/// `POST /mod/audit/prune` `days` (≥ 1), `confirm`: delete entries older than that once. The
/// deletion itself is logged.
async fn prune(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(days) = body_days(&body).filter(|d| (1..=crate::settings::AUDIT_RETENTION_MAX).contains(d)) else {
        return fail("Days: a whole number of at least 1");
    };
    if !body.get("confirm").is_some_and(crate::config::truthy) {
        return fail("Tick the box to confirm the deletion");
    }
    let deleted = match audit::prune_older_than(&st, days).await {
        Ok(n) => n,
        Err(e) => {
            tracing::error!("[AUDIT] Pruning failed: {e}");
            return fail("Deleting failed");
        }
    };
    let uid = req.session().map(|s| s.id).unwrap_or(0);
    audit::log(&st, uid, "audit_prune", "settings", "audit_log".to_string(), json!({ "days": days, "count": deleted })).await;
    done(json!({ "success": true, "msg": format!("Deleted {deleted} entries older than {days} days"), "deleted": deleted }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_cells() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("=1+1"), "'=1+1");
        assert_eq!(csv_cell("-x,y"), "\"'-x,y\"");
    }

    #[test]
    fn targets() {
        let d: Map<String, Value> = serde_json::from_value(json!({ "item_id": 5, "target_user": "a b", "postid": "9" })).unwrap();
        assert_eq!(target_link("item", "12", &d).as_deref(), Some("/12"));
        assert_eq!(target_link("item", "x", &d), None);
        assert_eq!(target_link("comment", "3", &d).as_deref(), Some("/5#c3"));
        assert_eq!(target_link("user", "7", &d).as_deref(), Some("/user/a%20b"));
        assert_eq!(target_link("album_item", "1", &d).as_deref(), Some("/9"));
        assert_eq!(target_link("hall", "best-of", &d).as_deref(), Some("/list/best-of"));
        assert_eq!(target_link("hall", "0", &d), None);
    }

    fn row(v: Value) -> Row {
        v.as_object().unwrap().clone()
    }

    /// The page renders from sample data, plain links and forms only (no inline styles or
    /// scripts, square corners). F0CKM_DUMP_AUDIT=<dir> writes it out to look at in a browser.
    #[test]
    fn page_renders() {
        use std::path::PathBuf;
        use std::sync::Arc;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
        let env = crate::tpl::build_env(root.join("views"), Arc::new(crate::i18n::I18n::new(root.join("locales"))));
        let f = Filter { category: "items".into(), q: "cat".into(), ..Default::default() };
        let rows = vec![
            row(json!({ "id": 42, "user_id": 1, "username": "bob", "action": "delete_item", "target_type": "item", "target_id": "123",
                "created_at": "2026-11-20T12:30:00+00:00", "details": { "reason": "dupe <b>", "filename": "cat.mp4", "uploader_name": "al", "uploader_id": 9 } })),
            row(json!({ "id": 41, "user_id": 2, "username": null, "action": "toggle_tag", "target_type": "item", "target_id": "120",
                "created_at": "2026-11-20T12:00:00+00:00", "details": { "tag": "cat", "from": "dog", "action": "renamed" } })),
            row(json!({ "id": 40, "user_id": 1, "username": "bob", "action": "brand_new_thing", "target_type": "system", "target_id": "0",
                "created_at": "2026-11-19T08:00:00+00:00", "details": {} })),
        ];
        let matched = vec![row(json!({ "action": "delete_item", "user_id": 1, "n": 120 })), row(json!({ "action": "toggle_tag", "user_id": 2, "n": 3 }))];
        let all = vec![
            row(json!({ "action": "delete_item", "user_id": 1, "target_type": "item", "n": 120 })),
            row(json!({ "action": "toggle_tag", "user_id": 2, "target_type": "item", "n": 3 })),
            row(json!({ "action": "brand_new_thing", "user_id": 1, "target_type": "system", "n": 1 })),
        ];
        let names: HashMap<i64, String> = [(1, "bob".to_string())].into();
        let manage = json!({ "retention_days": 90, "retention_max": 36500, "total": 124, "oldest": "1/2/2025, 3:04:05 AM" });
        let mut ctx = page_data(&f, 1, rows, &matched, &all, &names, manage);
        let base = json!({ "domain": "f0ck.test", "csrf_token": "tok", "frontend": "f0ckm", "session": { "user": "bob", "id": 1, "admin": true } });
        ctx.as_object_mut().unwrap().extend(base.as_object().unwrap().clone());
        let html = env.get_template("mod/audit.html").unwrap().render(crate::tpl::to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{e:#}"));
        if let Ok(dir) = std::env::var("F0CKM_DUMP_AUDIT") {
            std::fs::write(PathBuf::from(dir).join("audit.html"), &html).unwrap();
        }
        let page = &html[html.find("id=\"audit-page\"").unwrap()..];
        let page = &page[..page.find("<script src=\"/s/js/views/mod/audit.js").unwrap()];
        assert!(!page.contains("style=") && !page.contains("<script") && !page.contains("border-radius"));
        // entries: links that narrow the filter, the target and its history, escaped details
        assert!(page.contains(r#"<a class="audit-actor" href="/mod/audit?category=items&amp;actor=1&amp;q=cat""#), "{page}");
        assert!(page.contains(r#"<a class="audit-action" href="/mod/audit?action=delete_item&amp;q=cat""#));
        assert!(page.contains(r#"<a href="/123">123</a>"#));
        assert!(page.contains(r#"href="/mod/audit?target_type=item&amp;target_id=123""#));
        assert!(page.contains("dupe &lt;b&gt;") && !page.contains("dupe <b>"));
        assert!(page.contains(r#"<div class="diff-removed">- dog</div>"#) && page.contains(r#"<div class="diff-added">+ cat</div>"#));
        assert!(page.contains(">#2</a>"), "an actor without an account shows the id");
        assert!(page.contains(">Brand new thing</a>"));
        // summary, export with the filters, pager, admin forms
        assert!(page.contains("<strong>123</strong> entries match, by 2 people"));
        assert!(page.contains(r#"<input type="hidden" name="category" value="items"><input type="hidden" name="q" value="cat">"#));
        assert!(page.contains(r#"href="/mod/audit?category=items&amp;q=cat&amp;page=2" class="adm-btn" rel="next""#));
        assert!(page.contains("data-audit-more"));
        assert!(page.contains(r#"action="/mod/audit/retention""#) && page.contains(r#"name="days" class="adm-text audit-days" min="0" max="36500" step="1" value="90""#));
        assert!(page.contains(r#"<input type="checkbox" name="confirm" value="1" required>"#));
        assert!(page.contains(r#"<option value="other">Other (1)</option>"#));
        assert!(page.contains(r#"action="/mod/audit/export""#));

        // moderators: no export, no admin forms
        let mut ctx = page_data(&f, 1, vec![], &matched, &all, &names, Value::Null);
        ctx.as_object_mut().unwrap().extend(base.as_object().unwrap().clone());
        ctx["session"]["admin"] = json!(false);
        let html = env.get_template("mod/audit.html").unwrap().render(crate::tpl::to_jvalue(&ctx)).unwrap_or_else(|e| panic!("{e:#}"));
        assert!(!html.contains("/mod/audit/export") && !html.contains("/mod/audit/retention") && !html.contains("/mod/audit/prune"));
    }
}
