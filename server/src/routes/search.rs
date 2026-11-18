//! Tag/title/source search page (`/search`) — port of src/inc/routes/search.mjs.

use super::toptags::{member_session, no_store};
use crate::db::{Row, Sql};
use crate::f0cklib::query;
use crate::http::{Req, Res, Router, html};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

const EPS: i64 = 20;

pub fn register(r: &mut Router) {
    r.get(r"^\/search(\/)?$", search);
}

/// Page numbers around the current one (`cheat`) plus prev/next.
fn pagination(act_page: i64, pages: i64) -> Value {
    let cheat: Vec<i64> = ((act_page - 3).max(1)..=(act_page + 3).min(pages)).collect();
    json!({
        "start": 1,
        "end": pages,
        "prev": if act_page > 1 { json!(act_page - 1) } else { Value::Null },
        "next": if act_page < pages { json!(act_page + 1) } else { Value::Null },
        "page": act_page,
        "cheat": cheat,
        "uff": false,
    })
}

struct Found {
    total: i64,
    rows: Vec<Value>,
    /// the page shown (the last one when asked for more)
    page: i64,
    explain: Vec<String>,
    notes: Vec<String>,
}

/// A search in the language of f0cklib/query.rs: 20 f0cks a page, newest first unless `sort:` says
/// otherwise; without a sort, the f0cks of a page are ranked by how well their tag fits the first
/// word (`score`, f0cklib/search.rs). Rows: id, slug, username, mime, title, dest, duration, views,
/// stamp, tag (the matching tag, else the first one), display_name, avatar, avatar_file, score.
async fn run_search(st: &AppState, req: &Req, input: &str, strict: bool, page: i64) -> anyhow::Result<Found> {
    let session = req.session();
    let mut q = parse_for(st, req, input, strict);

    // the tags looked for count towards the visitor's interests
    if let Some(s) = session.filter(|s| s.id != 0) {
        for t in q.wanted_tags().iter().take(3) {
            crate::f0cklib::recommend::spawn_update_user_tag_affinity(st, s.id, t, 2.0);
        }
    }

    let Ok(related_id) = related_id(st, &mut q).await? else {
        return Ok(Found { total: 0, rows: vec![], page: 1, explain: q.explain, notes: q.notes });
    };

    let owner = crate::settings::session_owner_name(session);
    let admin = session.map(|s| s.admin).unwrap_or(false);
    let viewer = query::Viewer { user_id: session.map(|s| s.id).unwrap_or(0), owner: owner.as_deref(), admin, src_col: crate::remote::src_sql(&st.cfg(), admin) };
    let vis = visitor_filter(&st.cfg(), session);
    let day = crate::util::now_secs() / 86_400;
    let seed = format!("{input}|{day}");
    let (count, _) = search_sql(&q, &viewer, &vis, related_id, &seed, 0);
    let total = count.fetch_scalar::<i64, _>(&st.db).await?.unwrap_or(0);
    let act = pages_of(total).max(1).min(page);
    let (_, sel) = search_sql(&q, &viewer, &vis, related_id, &seed, offset_of(act));
    let rows: Vec<Row> = sel.fetch_all(&st.db).await?;

    let word = q.first_word().map(str::to_string);
    let mut rows: Vec<Value> = rows
        .into_iter()
        .map(|mut r| {
            if let Some(w) = &word {
                let sc = if strict { 1.0 } else { crate::f0cklib::search::score(r.get("tag").and_then(Value::as_str).unwrap_or(""), w) };
                r.insert("score".into(), json!(sc));
            }
            Value::Object(r)
        })
        .collect();
    // newest first, then (without a sort of their own) the best fitting tags of the page on top
    if word.is_some() && !strict && q.sort.is_none() && related_id.is_none() {
        rows.sort_by(|a, b| {
            let (x, y) = (a["score"].as_f64().unwrap_or(0.0), b["score"].as_f64().unwrap_or(0.0));
            y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    Ok(Found { total, rows, page: act, explain: q.explain, notes: q.notes })
}

/// What a visitor may find at all. Private f0cks only for their owner, unavailable ones only for
/// admins (f0cklib::visibility_filter). Guests and anonymous accounts get the same limits as in
/// their feeds:
/// - guests see SFW only, plus whatever `websrv.public_nsfw` / `public_untagged` open up;
/// - anonymous accounts see the ratings of `anonymous_permissions.allowed_modes`;
/// - both see only the media types of `allowed_mimes`.
/// Members see everything and narrow it down with `rating:` / `type:`.
fn visitor_filter(cfg: &crate::config::Config, session: Option<&crate::session::Session>) -> Sql {
    let mut vis = crate::f0cklib::visibility_filter(session);
    if crate::settings::is_anon_session(session) {
        // guests: no mode (the public one); anonymous accounts: "all" of what they may see
        let mode = session.map(|_| 3);
        let base = crate::f0cklib::compute_base_mode(cfg, mode, None, session);
        vis.push(" and (").push(&base).push(") ");
        let (_, mime) = crate::f0cklib::resolve_mime_sql(cfg, None, session, "items");
        vis.push(" ").append(&mime).push(" ");
    }
    vis
}

/// The search as this visitor may run it: `who_ok` (names hidden by anonymizing), and no
/// `commenter:` for guests while comments are hidden from the public
/// (`main.hide_comments_from_public`).
fn parse_for(st: &AppState, req: &Req, input: &str, strict: bool) -> query::Query {
    let cfg = st.cfg();
    let session = req.session();
    let who_ok = !crate::settings::is_anonymize_session(&cfg, session);
    let mut q = query::parse(input, strict, who_ok);
    if crate::settings::is_guest_session(session) && cfg.truthy("main.hide_comments_from_public") {
        let mut dropped = Vec::new();
        for g in &mut q.groups {
            g.retain(|a| {
                let keep = !matches!(a.term, query::Term::Commenter(_));
                if !keep {
                    dropped.push(query::describe(a.neg, &a.term));
                }
                keep
            });
        }
        if !dropped.is_empty() {
            q.groups.retain(|g| !g.is_empty());
            q.explain.retain(|e| !dropped.contains(e));
            q.notes.push("commenter: needs an account here (comments are for members)".into());
        }
    }
    q
}

/// `related:<id or slug>` as an item id: Ok(None) without one, Err(()) (and a note) when it names no f0ck.
async fn related_id(st: &AppState, q: &mut query::Query) -> anyhow::Result<Result<Option<i64>, ()>> {
    let Some(r) = q.related.clone() else { return Ok(Ok(None)) };
    let id: Option<i64> = sql!("SELECT id FROM items WHERE id::text = ", r.as_str(), " OR slug = ", r.as_str(), " LIMIT 1").fetch_scalar(&st.db).await?;
    Ok(match id {
        Some(id) => Ok(Some(id)),
        None => {
            q.notes.push(format!("there is no f0ck \u{201c}{r}\u{201d}"));
            Err(())
        }
    })
}

/// What a search would find, without running it: the parsed query (what it understood, what it
/// left out) and how many f0cks it finds. The search boxes show it while typing
/// (GET /api/v2/search/preview).
pub async fn preview(st: &AppState, req: &Req, input: &str, strict: bool) -> anyhow::Result<(query::Query, Option<i64>)> {
    let session = req.session();
    let mut q = parse_for(st, req, input, strict);
    if q.is_empty() {
        return Ok((q, None));
    }
    let Ok(related_id) = related_id(st, &mut q).await? else { return Ok((q, Some(0))) };
    let owner = crate::settings::session_owner_name(session);
    let admin = session.map(|s| s.admin).unwrap_or(false);
    let viewer = query::Viewer { user_id: session.map(|s| s.id).unwrap_or(0), owner: owner.as_deref(), admin, src_col: crate::remote::src_sql(&st.cfg(), admin) };
    let vis = visitor_filter(&st.cfg(), session);
    let (count, _) = search_sql(&q, &viewer, &vis, related_id, "", 0);
    let n = count.fetch_scalar::<i64, _>(&st.db).await?.unwrap_or(0);
    Ok((q, Some(n)))
}

/// The count and the page query of a search (`vis`: f0cklib::visibility_filter of the visitor).
fn search_sql(q: &query::Query, viewer: &query::Viewer, vis: &Sql, related_id: Option<i64>, seed: &str, offset: i64) -> (Sql, Sql) {
    // no deleted or expired f0cks
    let mut cond = sql!(
        "WHERE items.active = true AND items.is_deleted IS NOT TRUE AND (items.expires_at IS NULL OR items.expires_at > ",
        crate::util::now_secs(),
        ") ",
        @vis
    );
    cond.append(&query::conditions(q, viewer, related_id));

    let mut count = Sql::new("SELECT count(*) FROM items ");
    count.append(&cond);

    // the tag to show: the one matching the first word, else the item's first (not a rating)
    let rating = query::rating_tags_sql();
    let mut sel = Sql::new(
        r#"SELECT items.id, items.slug, items.username, items.mime, items.title, items.dest, items.duration, items.views, items.stamp, COALESCE("#,
    );
    if let Some(m) = q.first_tag().and_then(query::tag_match) {
        sel.push("(SELECT t.tag FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ta.item_id = items.id AND ").append(&m).push(" ORDER BY length(t.tag), t.tag LIMIT 1), ");
    }
    sel.push(&format!(
        "(SELECT t.tag FROM tags_assign ta JOIN tags t ON t.id = ta.tag_id WHERE ta.item_id = items.id AND t.normalized NOT IN {rating} ORDER BY t.tag LIMIT 1)) AS tag, \
         au.display_name, au.avatar, au.avatar_file "
    ));
    // the uploader's display name and avatar (one user, by name or login)
    sel.push(
        r#"FROM items LEFT JOIN LATERAL (SELECT uo.display_name, uo.avatar, uo.avatar_file FROM "user" au_u JOIN user_options uo ON uo.user_id = au_u.id
           WHERE au_u."user" = items.username OR au_u.login = items.username LIMIT 1) au ON TRUE "#,
    );
    sel.append(&cond);
    sel.append(&query::order_by(q.sort, related_id, seed));
    sel.push(" OFFSET ").bind(offset).push(" LIMIT ").bind(EPS);
    (count, sel)
}

fn pages_of(total: i64) -> i64 {
    (total + EPS - 1) / EPS
}

fn offset_of(act_page: i64) -> i64 {
    ((act_page - 1) * EPS).max(0)
}

async fn search(st: AppState, req: Req) -> Res {
    let tag: String = req.query("tag").unwrap_or("").to_string();
    let mode = req.query("mode").map(str::to_string);
    // `page || 1`; a non-numeric page made the old query fail, use 1 instead
    let page: i64 = match req.query("page").filter(|p| !p.is_empty()) {
        Some(p) => crate::social::helpers::js_number(p).filter(|f| f.is_finite() && *f != 0.0).map(|f| f.trunc() as i64).unwrap_or(1),
        None => 1,
    };
    let mut total: i64 = 0;
    let mut result: Value = Value::Null;
    let mut pag: Value = Value::Null;
    let mut link: Value = Value::Null;
    let mut explain: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    if !tag.trim().is_empty() {
        let found = run_search(&st, &req, &tag, mode.as_deref() == Some("strict"), page).await?;
        total = found.total;
        result = Value::Array(found.rows);
        explain = found.explain;
        notes = found.notes;
        pag = pagination(found.page, pages_of(total));
        let strict = if mode.as_deref() == Some("strict") { "&mode=strict" } else { "" };
        link = json!({ "main": format!("/search/?tag={}{strict}", crate::util::encode_uri_component(&tag)), "path": "&page=" });
    }

    let totals = super::ranking::count_f0cks(&st).await?;
    let mut data = json!({
        "result": result,
        "totals": totals,
        "searchstring": tag,
        "count": total,
        "session": member_session(&req),
        "mode": mode,
        // The template's strict checkbox; `mode` itself is replaced by the feed mode when rendering
        "search_mode": mode,
        // what the search understood ("tagged cat", "uploaded by al", "sorted by views") and the
        // parts it couldn't use (f0cklib/query.rs); frontends may show them
        "search_explain": explain,
        "search_notes": notes,
    });
    if result.is_null() {
        if let Some(m) = data.as_object_mut() {
            m.shift_remove("result");
        }
    }
    // `pagination`/`link` stay undefined when there was no search
    if !pag.is_null() {
        data["pagination"] = pag;
    }
    if !link.is_null() {
        data["link"] = link;
    }
    // Without a search term the old page received `searchstring: []`
    if tag.is_empty() {
        data["searchstring"] = json!([]);
    }
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), "search", data))))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F0CKM_SEARCH_SQL=<file>: write the SQL of sample searches (values inlined) to run by hand
    /// against a database (`psql -f`).
    /// Guests find only what their feed shows them: public, SFW (unless `public_nsfw`), and the
    /// types of `anonymous_permissions.allowed_mimes`; members aren't narrowed down.
    #[test]
    fn guests_search_what_they_may_see() {
        let cfg = crate::media::test_cfg("websrv: {}\n");
        let guest = visitor_filter(&cfg, None).inline();
        assert!(guest.contains("coalesce(items.visibility, 0) = 0"), "{guest}");
        assert!(guest.contains("tag_id = 1"), "SFW only: {guest}");
        assert!(!guest.contains("tag_id in (1, 2)"), "{guest}");
        assert!(!guest.contains("items.mime"), "all types allowed: {guest}");

        let open = crate::media::test_cfg("websrv: { public_nsfw: true }\nanonymous_permissions: { allowed_mimes: [image, video] }\n");
        let guest = visitor_filter(&open, None).inline();
        assert!(guest.contains("tag_id in (1, 2)"), "public NSFW: {guest}");
        assert!(guest.contains("items.mime ilike 'image/%'") && guest.contains("items.mime ilike 'video/%'") && !guest.contains("audio"), "{guest}");

        let member = crate::session::Session { user: "bob".into(), id: 1, ..Default::default() };
        let m = visitor_filter(&cfg, Some(&member)).inline();
        assert!(!m.contains("tag_id") && !m.contains("items.mime"), "{m}");
    }

    #[test]
    fn search_sql_samples() {
        let Ok(out) = std::env::var("F0CKM_SEARCH_SQL") else { return };
        let samples = [
            ("cat", false),
            ("black cat", false),
            ("black cat, funny", true),
            ("\"black cat\" -dog c?t*", false),
            ("cat OR dog | bird", false),
            ("user:x type:video sort:views", false),
            ("fav:x -rating:nsfw views:>1 favs:>=1 comments:0..5 tags:>1", false),
            ("commenter:x tagger:x hall:* is:oc has:title has:comments has:favs has:tags is:fav is:mine", false),
            ("duration:>1m size:<10mb width:>=100 height:>=100 id:1..1000 ext:webm", false),
            ("year:2026 after:2025-01 before:2027-01-01 age:<1y sort:random", false),
            ("type:gif sort:long", false),
            ("sort:short", false),
            ("sort:favs", false),
            ("sort:comments", false),
            ("sort:old", false),
            ("title:a src:http", false),
            ("100%_cat", false),
        ];
        let viewer = query::Viewer { user_id: 1, owner: Some("x"), admin: false, src_col: "items.src" };
        let vis = crate::f0cklib::visibility_filter(None);
        let mut sql = String::new();
        let guest_vis = visitor_filter(&crate::media::test_cfg("websrv: {}\n"), None);
        let samples = samples.iter().map(|&(i, s)| (i, s, &vis)).chain([("cat type:video sort:random", false, &guest_vis)]);
        for (input, strict, vis) in samples {
            let q = query::parse(input, strict, true);
            let (count, sel) = search_sql(&q, &viewer, vis, None, "seed", 0);
            sql.push_str(&format!("\\echo '== {}'\n{};\n{};\n", input.replace('\'', ""), count.inline(), sel.inline()));
        }
        let q = query::parse("related:x sort:related", false, true);
        let (count, sel) = search_sql(&q, &viewer, &vis, Some(47), "seed", 0);
        sql.push_str(&format!("\\echo '== related 47'\n{};\n{};\n", count.inline(), sel.inline()));
        std::fs::write(out, sql).unwrap();
    }
}
