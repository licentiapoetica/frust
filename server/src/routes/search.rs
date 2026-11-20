//! Tag/title/source search page (`/search`) — port of src/inc/routes/search.mjs.

use super::toptags::{member_session, no_store};
use crate::db::{Row, RowExt, Sql};
use crate::f0cklib::query;
use crate::http::{Req, Res, Router, html};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

/// f0cks a page without `websrv.eps`
const EPS: i64 = 20;

/// f0cks on a page of a search: as many as on a feed's page (`websrv.eps`)
fn eps(cfg: &crate::config::Config) -> i64 {
    cfg.int("websrv.eps").filter(|&n| n > 0).unwrap_or(EPS)
}

pub fn register(r: &mut Router) {
    // /search?q=<search>, its pages /search/p/<n>?q=… (like a feed's /p/<n>)
    r.get(r"^\/search(?:\/p\/(?<page>\d+))?\/?$", search);
    // more pages of a search's grid for f0ckm's endless scrolling
    r.get(r"^\/ajax\/search\/?$", ajax_search);
}

/// What a search page asks for: the search (`q`, or the old `tag`), strict (`match=all`, or the
/// old `mode=strict`) and the page (`/search/p/<n>`, or the old `page=`, `page || 1`).
fn page_request(req: &Req) -> (String, bool, i64) {
    let input = req.query("q").or_else(|| req.query("tag")).unwrap_or("").to_string();
    let strict = req.query("match") == Some("all") || req.query("mode") == Some("strict");
    let page = req
        .param("page")
        .or_else(|| req.query("page"))
        .filter(|p| !p.is_empty())
        .map(|p| crate::social::helpers::js_number(p).filter(|f| f.is_finite() && *f != 0.0).map(|f| f.trunc() as i64).unwrap_or(1))
        .unwrap_or(1);
    (input, strict, page)
}

/// A page of a search's results: /search?q=<search>, /search/p/<n>?q=… (`&match=all` for a strict one).
fn page_url(input: &str, strict: bool, page: i64) -> String {
    let path = if page > 1 { format!("/search/p/{page}") } else { "/search".to_string() };
    format!("{path}{}", item_query(input, strict))
}

/// The pagination `link` of a search: /search/p/<n>?q=…
fn page_link(input: &str, strict: bool) -> Value {
    json!({ "main": "/search/", "path": "p/", "suffix": item_query(input, strict) })
}

/// How the visitor wants the results: `?view=grid` / `?view=list` picks it (and keeps it in the
/// `search_view` cookie), else the cookie. None: the frontend's own default (the grid in f0ckm, w0bm
/// and f0ckv2).
fn view_pref(st: &AppState, req: &Req) -> Option<&'static str> {
    let pick = |v: &str| match v {
        "grid" => Some("grid"),
        "list" => Some("list"),
        _ => None,
    };
    if let Some(v) = req.query("view").and_then(pick) {
        let opts = crate::http::cookie_options(&st.cfg(), crate::http::CookieExpiry::MaxAge(31_536_000), false);
        req.set_cookie(format!("search_view={v}; {opts}"));
        return Some(v);
    }
    req.cookie("search_view").and_then(pick)
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

/// A search as this visitor runs it: the parsed query and everything its SQL needs.
struct Prepared {
    q: query::Query,
    related_id: Option<i64>,
    owner: Option<String>,
    admin: bool,
    user_id: i64,
    src_col: &'static str,
    vis: Sql,
    seed: String,
    strict: bool,
    input: String,
    slugs: bool,
    nsfl: i64,
    /// f0cks a page ([`eps`])
    eps: i64,
}

impl Prepared {
    /// None (with the notes in `q`) when `related:` names no f0ck.
    async fn new(st: &AppState, req: &Req, input: &str, strict: bool) -> anyhow::Result<Result<Prepared, query::Query>> {
        let session = req.session();
        let mut q = parse_for(st, req, input, strict);
        let Ok(related_id) = related_id(st, &mut q).await? else { return Ok(Err(q)) };
        let cfg = st.cfg();
        let admin = session.map(|s| s.admin).unwrap_or(false);
        let day = crate::util::now_secs() / 86_400;
        Ok(Ok(Prepared {
            related_id,
            owner: crate::settings::session_owner_name(session),
            admin,
            user_id: session.map(|s| s.id).unwrap_or(0),
            src_col: crate::remote::src_sql(&cfg, admin),
            vis: visitor_filter(&cfg, session),
            seed: format!("{input}|{day}"),
            strict,
            input: input.to_string(),
            slugs: crate::settings::enable_item_slugs(&cfg),
            nsfl: cfg.nsfl_tag_id(),
            eps: eps(&cfg),
            q,
        }))
    }

    fn viewer(&self) -> query::Viewer<'_> {
        query::Viewer { user_id: self.user_id, owner: self.owner.as_deref(), admin: self.admin, src_col: self.src_col }
    }

    /// The f0cks `offset..offset + limit` in the order the search page shows them: the SQL order,
    /// then each page of [`Prepared::eps`] ranked by [`rank_page`] (`offset` is a page start).
    async fn rows(&self, st: &AppState, offset: i64, limit: i64) -> anyhow::Result<Vec<Value>> {
        let (_, sel) = search_sql(&self.q, &self.viewer(), &self.vis, self.related_id, &self.seed, self.nsfl, offset, limit);
        let rows: Vec<Row> = sel.fetch_all(&st.db).await?;
        let mut rows: Vec<Value> = rows.into_iter().map(|r| self.row(r)).collect();
        for page in rows.chunks_mut(self.eps as usize) {
            self.rank_page(page);
        }
        Ok(rows)
    }

    /// A result row with its `score` (how well its tag fits the first word) and `href`: the f0ck
    /// in this search, so its item page walks through the results ([`nav`]). Plus what the feeds'
    /// grids read (`rating_class`, `rating_label`, `is_audio`, `thumb_size`), so the results can be
    /// shown as one.
    fn row(&self, mut r: Row) -> Value {
        let (class, label) = match r.i64("tag_id") {
            Some(1) => ("sfw", "SFW"),
            Some(2) => ("nsfw", "NSFW"),
            Some(n) if n == self.nsfl => ("nsfl", "NSFL"),
            _ => ("untagged", "?"),
        };
        r.insert("rating_class".into(), json!(class));
        r.insert("rating_label".into(), json!(label));
        let audio = r.str("mime").is_some_and(|m| m.starts_with("audio/"));
        r.insert("is_audio".into(), json!(audio));
        r.insert("thumb_size".into(), json!(1));
        if let Some(w) = self.q.first_word() {
            let sc = if self.strict { 1.0 } else { crate::f0cklib::search::score(r.get("tag").and_then(Value::as_str).unwrap_or(""), w) };
            r.insert("score".into(), json!(sc));
        }
        let href = format!("/search/{}{}", js(&self.item_ref(&r)), item_query(&self.input, self.strict));
        r.insert("href".into(), json!(href));
        Value::Object(r)
    }

    /// The slug of a row's f0ck when slugs are on and it has one, else its id.
    fn item_ref(&self, r: &serde_json::Map<String, Value>) -> Value {
        match r.get("slug") {
            Some(s) if self.slugs && crate::config::truthy(s) => s.clone(),
            _ => r.get("id").cloned().unwrap_or(Value::Null),
        }
    }

    /// Newest first, then (without a sort of its own) the best fitting tags of a page on top.
    fn rank_page(&self, page: &mut [Value]) {
        if self.q.first_word().is_some() && !self.strict && self.q.sort.is_none() && self.related_id.is_none() {
            page.sort_by(|a, b| {
                let (x, y) = (a["score"].as_f64().unwrap_or(0.0), b["score"].as_f64().unwrap_or(0.0));
                y.partial_cmp(&x).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
    }
}

fn js(v: &Value) -> String {
    crate::tpl::js_to_string(&crate::tpl::to_jvalue(v))
}

/// The query string of a f0ck's page in a search: `?q=<search>`, `&match=all` for a strict one.
/// (Not `mode=strict` as on the search page: `mode` is the feed mode everywhere else, and f0ckm.js
/// drops `strict=1` from the links it follows.)
fn item_query(input: &str, strict: bool) -> String {
    format!("?q={}{}", crate::util::encode_uri_component(input), if strict { "&match=all" } else { "" })
}

/// A search in the language of f0cklib/query.rs: 20 f0cks a page, newest first unless `sort:` says
/// otherwise; without a sort, the f0cks of a page are ranked by how well their tag fits the first
/// word (`score`, f0cklib/search.rs). Rows: id, slug, username, mime, title, dest, duration, views,
/// stamp, is_pinned, is_oc, has_coverart, is_album, album_count, tag_id (the rating), tag (the
/// matching tag, else the first one), display_name, avatar, avatar_file, score, href, rating_class,
/// rating_label, is_audio, thumb_size.
async fn run_search(st: &AppState, req: &Req, input: &str, strict: bool, page: i64) -> anyhow::Result<Found> {
    let p = match Prepared::new(st, req, input, strict).await? {
        Ok(p) => p,
        Err(q) => return Ok(Found { total: 0, rows: vec![], page: 1, explain: q.explain, notes: q.notes }),
    };

    // the tags looked for count towards the visitor's interests
    if let Some(s) = req.session().filter(|s| s.id != 0) {
        for t in p.q.wanted_tags().iter().take(3) {
            crate::f0cklib::recommend::spawn_update_user_tag_affinity(st, s.id, t, 2.0);
        }
    }

    let (count, _) = search_sql(&p.q, &p.viewer(), &p.vis, p.related_id, &p.seed, p.nsfl, 0, p.eps);
    let total = count.fetch_scalar::<i64, _>(&st.db).await?.unwrap_or(0);
    let act = pages_of(total, p.eps).max(1).min(page);
    let rows = p.rows(st, offset_of(act, p.eps), p.eps).await?;
    Ok(Found { total, rows, page: act, explain: p.q.explain, notes: p.q.notes })
}

/// Walking through a search from one of its f0cks (`/search/<f0ck>?q=<search>`, and the item loads
/// of f0ckm.js with `search=`): the item page's `pagination` and `link` for the f0cks around it in
/// the order the search page shows them, like a tag or a user's uploads. `pagination.next` is the
/// one before it on the search page and `prev` the one after (the item pages' newer/older), `end` /
/// `start` the first / last result. Plus `search_ctx`: the search (`q`, `strict`), where the f0ck
/// is in it (`position` of `total`) and the search page it is on (`results`). None when the search
/// doesn't find this f0ck (any more), so the page stays a plain item page.
pub async fn nav(st: &AppState, req: &Req, input: &str, strict: bool, item_id: i64, page_id: &Value) -> anyhow::Result<Option<Value>> {
    if input.trim().is_empty() {
        return Ok(None);
    }
    let Ok(p) = Prepared::new(st, req, input, strict).await? else { return Ok(None) };
    if p.q.is_empty() {
        return Ok(None);
    }

    // where the f0ck is in the SQL order (pages are cut from it; only their insides are re-ranked)
    let rank = rank_sql(&p.q, &p.viewer(), &p.vis, p.related_id, &p.seed, item_id);
    let Some(r) = rank.fetch_optional(&st.db).await? else { return Ok(None) };
    let (rn, total) = (r.i64("rn").unwrap_or(1), r.i64("total").unwrap_or(0));
    let eps = p.eps;
    let page = (rn - 1) / eps + 1;
    let pages = pages_of(total, eps);

    // its page and the ones before and after it
    // (and the first and the last result, when they are further away)
    let first_page = (page - 1).max(1);
    let last_page = (page + 1).min(pages);
    let (window, first, last) = tokio::try_join!(
        p.rows(st, offset_of(first_page, eps), (last_page - first_page + 1) * eps),
        async { Ok(if first_page > 1 { p.rows(st, 0, eps).await?.into_iter().next() } else { None }) },
        async { Ok(if last_page < pages { p.rows(st, offset_of(pages, eps), eps).await?.into_iter().last() } else { None }) },
    )?;
    let Some(idx) = window.iter().position(|r| r["id"].as_i64() == Some(item_id)) else { return Ok(None) };
    let first = first.or_else(|| window.first().cloned());
    let last = last.or_else(|| window.last().cloned());

    let item_ref = |r: Option<&Value>| r.and_then(Value::as_object).map(|m| p.item_ref(m)).unwrap_or(Value::Null);
    let before = idx.checked_sub(1).and_then(|i| window.get(i));
    let after = window.get(idx + 1);
    let near: Vec<Value> = window[idx.saturating_sub(7)..(idx + 8).min(window.len())].iter().map(|r| r["id"].clone()).collect();
    let suffix = item_query(input, strict);
    Ok(Some(json!({
        "pagination": {
            "start": item_ref(last.as_ref()),
            "end": item_ref(first.as_ref()),
            "next": item_ref(before),
            "prev": item_ref(after),
            "page": page_id,
            "cheat": near,
        },
        "link": { "main": "/search/", "mainDisplay": "/search/", "path": "", "suffix": suffix },
        "search_ctx": {
            "q": input,
            "strict": strict,
            "position": offset_of(first_page, eps) + idx as i64 + 1,
            "total": total,
            "page": page,
            "results": page_url(input, strict, page),
        },
    })))
}

/// Up to `limit` of a search's f0cks in random order, as (id, the slug when slugs are on and it has
/// one, else the id): random in a search (f0ckm.js' random pool, /random from a search's pages).
/// None when the search is empty or `related:` names no f0ck.
pub async fn random_refs(st: &AppState, req: &Req, input: &str, strict: bool, limit: i64) -> anyhow::Result<Option<Vec<(i64, Value)>>> {
    if input.trim().is_empty() {
        return Ok(None);
    }
    let Ok(p) = Prepared::new(st, req, input, strict).await? else { return Ok(None) };
    if p.q.is_empty() {
        return Ok(None);
    }
    let mut sel = Sql::new("SELECT items.id, items.slug FROM items ");
    sel.append(&search_cond(&p.q, &p.viewer(), &p.vis, p.related_id)).push(" ORDER BY random() LIMIT ").bind(limit);
    let rows: Vec<Row> = sel.fetch_all(&st.db).await?;
    Ok(Some(rows.into_iter().filter_map(|r| Some((r.i64("id")?, p.item_ref(&r)))).collect()))
}

/// The search a page of one is in: a result (/search/<f0ck>?q=…: the search and the f0ck) or the
/// results (/search?q=…, /search/p/<n>?q=…, the old ?tag=…&mode=strict). For /random, which only
/// sees the page it was clicked on (Referer).
pub fn context_of(url: &url::Url) -> Option<(String, bool, Option<String>)> {
    let path = url.path().trim_end_matches('/');
    let rest = path.strip_prefix("/search")?;
    let item = match rest.strip_prefix('/') {
        None if rest.is_empty() => None,
        Some(seg) if seg.starts_with("p/") && seg[2..].bytes().all(|b| b.is_ascii_digit()) && seg.len() > 2 => None,
        Some(seg) if !seg.is_empty() && !seg.contains('/') => Some(seg.to_string()),
        _ => return None,
    };
    let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let q = get("q").or_else(|| get("tag")).filter(|q| !q.trim().is_empty())?;
    let strict = get("match").as_deref() == Some("all") || get("mode").as_deref() == Some("strict");
    Some((q, strict, item))
}

/// A f0ck's page in a search: /search/<f0ck>?q=<search>[&match=all]
pub fn item_url(item_ref: &Value, input: &str, strict: bool) -> String {
    format!("/search/{}{}", js(item_ref), item_query(input, strict))
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
    let (count, _) = search_sql(&q, &viewer, &vis, related_id, "", 3, 0, EPS);
    let n = count.fetch_scalar::<i64, _>(&st.db).await?.unwrap_or(0);
    Ok((q, Some(n)))
}

/// What a search finds (`vis`: f0cklib::visibility_filter of the visitor), as a WHERE clause on `items`.
fn search_cond(q: &query::Query, viewer: &query::Viewer, vis: &Sql, related_id: Option<i64>) -> Sql {
    // no deleted or expired f0cks
    let mut cond = sql!(
        "WHERE items.active = true AND items.is_deleted IS NOT TRUE AND (items.expires_at IS NULL OR items.expires_at > ",
        crate::util::now_secs(),
        ") ",
        @vis
    );
    cond.append(&query::conditions(q, viewer, related_id));
    cond
}

/// Where a f0ck is in a search: `rn` (1-based, in the SQL order) and the search's `total`; no row
/// when the search doesn't find it.
fn rank_sql(q: &query::Query, viewer: &query::Viewer, vis: &Sql, related_id: Option<i64>, seed: &str, item_id: i64) -> Sql {
    let mut rank = Sql::new("SELECT s.rn, s.total FROM (SELECT items.id, row_number() OVER (");
    rank.append(&query::order_by(q.sort, related_id, seed))
        .push(") AS rn, count(*) OVER () AS total FROM items ")
        .append(&search_cond(q, viewer, vis, related_id))
        .push(") s WHERE s.id = ")
        .bind(item_id);
    rank
}

/// The count and the query of `limit` results from `offset` of a search.
#[allow(clippy::too_many_arguments)]
fn search_sql(q: &query::Query, viewer: &query::Viewer, vis: &Sql, related_id: Option<i64>, seed: &str, nsfl: i64, offset: i64, limit: i64) -> (Sql, Sql) {
    let cond = search_cond(q, viewer, vis, related_id);

    let mut count = Sql::new("SELECT count(*) FROM items ");
    count.append(&cond);

    // the tag to show: the one matching the first word, else the item's first (not a rating)
    let rating = query::rating_tags_sql();
    let mut sel = Sql::new(
        r#"SELECT items.id, items.slug, items.username, items.mime, items.title, items.dest, items.duration, items.views, items.stamp,
           items.is_pinned, items.is_oc, items.has_coverart, items.is_album, items.album_count, "#,
    );
    // the rating, as the feeds have it (`tag_id`: the lowest of sfw / nsfw / nsfl)
    sel.push("(SELECT min(ra.tag_id) FROM tags_assign ra WHERE ra.item_id = items.id AND ra.tag_id IN (1, 2, ").bind(nsfl).push(")) AS tag_id, COALESCE(");
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
    sel.push(" OFFSET ").bind(offset).push(" LIMIT ").bind(limit);
    (count, sel)
}

fn pages_of(total: i64, eps: i64) -> i64 {
    (total + eps - 1) / eps
}

fn offset_of(act_page: i64, eps: i64) -> i64 {
    ((act_page - 1) * eps).max(0)
}

async fn search(st: AppState, req: Req) -> Res {
    let (tag, strict, page) = page_request(&req);
    // the template's strict checkbox (`mode` itself is replaced by the feed mode when rendering)
    let mode = strict.then(|| "strict".to_string());
    let mut total: i64 = 0;
    let mut result: Value = Value::Null;
    let mut pag: Value = Value::Null;
    let mut link: Value = Value::Null;
    let mut explain: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut view_urls: Value = Value::Null;

    if !tag.trim().is_empty() {
        let found = run_search(&st, &req, &tag, strict, page).await?;
        total = found.total;
        result = Value::Array(found.rows);
        explain = found.explain;
        notes = found.notes;
        pag = pagination(found.page, pages_of(total, eps(&st.cfg())));
        link = page_link(&tag, strict);
        let here = page_url(&tag, strict, found.page);
        view_urls = json!({ "grid": format!("{here}&view=grid"), "list": format!("{here}&view=list") });
    }

    let totals = super::ranking::count_f0cks(&st).await?;
    let mut data = json!({
        "result": result,
        "totals": totals,
        "searchstring": tag,
        "count": total,
        "session": member_session(&req),
        "mode": mode,
        "search_mode": mode,
        // what the search understood ("tagged cat", "uploaded by al", "sorted by views") and the
        // parts it couldn't use (f0cklib/query.rs); frontends may show them
        "search_explain": explain,
        "search_notes": notes,
        // grid or list: the visitor's pick (None: the frontend's default) and the links to switch
        "search_view": view_pref(&st, &req),
        "search_view_urls": view_urls,
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

/// GET /ajax/search?q=<search>&page=<n>[&match=all]: a page of a search's grid for f0ckm's endless
/// scrolling, as /ajax/items answers for feeds: `html` (the grid's tiles), `pagination` (the page
/// links), `currentPage`, `hasMore`. Past the last page: `success: false`.
async fn ajax_search(st: AppState, req: Req) -> Res {
    let (input, strict, page) = page_request(&req);
    let none = || Ok(crate::http::json_resp(&json!({ "success": false, "html": "", "hasMore": false }), 200));
    if input.trim().is_empty() {
        return none();
    }
    let found = run_search(&st, &req, &input, strict, page).await?;
    if found.page != page || found.rows.is_empty() {
        return none();
    }
    let pages = pages_of(found.total, eps(&st.cfg()));
    let link = page_link(&input, strict);
    let pag = pagination(found.page, pages);
    let items_html = crate::tpl::render(&st, Some(&req), "snippets/items-grid", json!({ "items": found.rows, "link": link }));
    let pagination_html = crate::tpl::render(&st, Some(&req), "snippets/pagination", json!({ "pagination": pag, "link": link }));
    Ok(crate::http::json_resp(
        &json!({
            "success": true,
            "html": items_html,
            "pagination": pagination_html,
            "currentPage": found.page,
            "hasMore": found.page < pages,
            "total": found.total,
        }),
        200,
    ))
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

    /// The search of the page /random was clicked on: a result or the results, old addresses too.
    #[test]
    fn context_of_search_pages() {
        let c = |u: &str| context_of(&url::Url::parse(&format!("http://x{u}")).unwrap());
        assert_eq!(c("/search/eFqWVn9cYGi?q=type%3Avideo%20cat&match=all"), Some(("type:video cat".into(), true, Some("eFqWVn9cYGi".into()))));
        assert_eq!(c("/search/7?q=cat"), Some(("cat".into(), false, Some("7".into()))));
        assert_eq!(c("/search?q=cat"), Some(("cat".into(), false, None)));
        assert_eq!(c("/search/p/3?q=cat"), Some(("cat".into(), false, None)));
        assert_eq!(c("/search/?tag=cat&mode=strict&page=2"), Some(("cat".into(), true, None)));
        assert_eq!(c("/search?q="), None);
        assert_eq!(c("/search"), None);
        assert_eq!(c("/searching?q=cat"), None);
        assert_eq!(c("/tag/cat/7"), None);
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
            let (count, sel) = search_sql(&q, &viewer, vis, None, "seed", 3, 0, EPS);
            sql.push_str(&format!("\\echo '== {}'\n{};\n{};\n", input.replace('\'', ""), count.inline(), sel.inline()));
            // where a f0ck is in it (nav)
            sql.push_str(&format!("{};\n", rank_sql(&q, &viewer, vis, None, "seed", 1).inline()));
        }
        let q = query::parse("related:x sort:related", false, true);
        let (count, sel) = search_sql(&q, &viewer, &vis, Some(47), "seed", 3, 0, EPS);
        sql.push_str(&format!("\\echo '== related 47'\n{};\n{};\n", count.inline(), sel.inline()));
        std::fs::write(out, sql).unwrap();
    }
}
