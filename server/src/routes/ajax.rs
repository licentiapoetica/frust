//! XHR endpoints of the feed/item viewer — port of src/inc/routes/ajax.mjs.
//!
//! The old handlers passed `headers` to `res.reply`, which flummpress ignores, so these JSON
//! answers went out as `text/html`. They are sent as `application/json` now (a JSON body with
//! rendered HTML in it must not be sniffable as a page).

use crate::f0cklib::{self, FeedOpts};
use crate::http::{Req, Res, Router, json_resp};
use crate::session::Session;
use crate::settings as s;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/ajax\/item\/random", item_random);
    r.get(r"^\/ajax\/item\/(?<itemid>[a-zA-Z0-9_-]{11}|\d+)", item);
    r.exact(axum::http::Method::GET, "/ajax/halls", None, halls);
    r.get(r"^\/ajax\/items", items);
}

fn q<'a>(req: &'a Req, k: &str) -> Option<&'a str> {
    req.query(k)
}

/// `query.x || null`
fn q_nonempty(req: &Req, k: &str) -> Option<String> {
    req.query(k).filter(|v| !v.is_empty()).map(str::to_string)
}

fn is_guest(req: &Req) -> bool {
    req.session().map(|s| s.user.is_empty()).unwrap_or(true)
}

/// `ratings` cookie → valid ratings (`None` when the cookie is absent or empty).
fn ratings_cookie(req: &Req) -> Option<Vec<String>> {
    let r = req.cookie("ratings").filter(|r| !r.is_empty())?;
    Some(
        crate::util::decode_lossy(r)
            .split(['|', ','])
            .filter(|x| matches!(*x, "sfw" | "nsfw" | "nsfl" | "untagged"))
            .map(str::to_string)
            .collect(),
    )
}

/// `isGuest ? 0 : (query.mode !== undefined ? +query.mode : req.mode)`
fn req_mode(req: &Req) -> i64 {
    if is_guest(req) {
        return 0;
    }
    match req.query("mode") {
        Some(m) => crate::social::helpers::js_number(m).filter(|f| f.is_finite()).map(|f| f as i64).unwrap_or(0),
        None => req.mode,
    }
}

fn is_prefetch(req: &Req) -> bool {
    req.query("prefetch") == Some("1") || [req.header("x-purpose"), req.header("purpose"), req.header("sec-purpose")].iter().any(|h| *h == Some("prefetch"))
}

fn session_value(session: Option<&Session>) -> Value {
    session.map(Session::to_value).unwrap_or(Value::Bool(false))
}

fn json_body(v: &Value) -> Response {
    json_resp(v, 200)
}

fn enc(s: &str) -> String {
    crate::util::encode_uri_component(s)
}

/// URL of the item in its feed context (tag / hall / user / favs, optional mime).
struct Context<'a> {
    tag: Option<&'a str>,
    hall: Option<&'a str>,
    user: Option<&'a str>,
    user_hall: Option<&'a str>,
    user_hall_owner: Option<&'a str>,
    fav: bool,
    mime: Option<&'a str>,
}

fn context_url(itemid: &str, c: &Context) -> String {
    let mut url = format!("/{itemid}");
    if let Some(t) = c.tag {
        url = format!("/tag/{}/{itemid}", enc(t));
    }
    if let Some(h) = c.hall {
        url = format!("/h/{}/{itemid}", enc(h));
    }
    if let (Some(uh), Some(owner)) = (c.user_hall, c.user_hall_owner) {
        url = format!("/user/{}/hall/{}/{itemid}", enc(owner), enc(uh));
    } else if let Some(u) = c.user {
        url = if c.fav { format!("/user/{}/favs/{itemid}", enc(u)) } else { format!("/user/{}/{itemid}", enc(u)) };
    } else if c.fav {
        url = format!("/favs/{itemid}");
    }
    if let Some(m) = c.mime {
        let suffix = format!("/{itemid}");
        if let Some(base) = url.strip_suffix(&suffix) {
            url = format!("{base}/{m}/{itemid}");
        }
    }
    url
}

/// The not-visible error snippet.
fn error_json(st: &AppState, req: &Req, data: &Value, itemid: &str, mode: i64, slug_fallback: bool) -> Response {
    let lang = req
        .session()
        .and_then(|s| s.language.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| super::index::req_lang(st, req));
    let tr = st.i18n.translator(&lang);
    let label = match mode {
        3 => None,
        1 => Some("NSFW"),
        2 => Some("Unrated"),
        4 => Some("NSFL"),
        _ => Some("SFW"),
    };
    let item = &data["item"];
    let item_id = if crate::config::truthy(&item["id"]) { item["id"].clone() } else { json!(itemid) };
    let item_slug = if crate::config::truthy(&item["slug"]) {
        item["slug"].clone()
    } else if slug_fallback {
        json!(itemid)
    } else {
        Value::Null
    };
    let html = crate::tpl::render(
        st,
        Some(req),
        "error-partial",
        json!({
            "message": tr.t("error.post_not_visible"),
            "tmp": null,
            "session": session_value(req.session()),
            "item_id": item_id,
            "item_slug": item_slug,
            "error_mode_label": label,
            "error_filter_hint": label.map(|l| tr.t_with("error.filter_hint", &[("mode", &format!("<strong>{l}</strong>"))])),
            "error_filter_hint_link": tr.t("error.filter_hint_link"),
            "error_see_anyways": if crate::config::truthy(&data["locked"]) { Value::Null } else { json!(tr.t("error.see_anyways")) },
        }),
    );
    json_body(&json!({ "html": html, "pagination": "", "error": true }))
}

/// Comments badge data, hall display data and template flags (shared by both item loaders).
async fn prepare_item(st: &AppState, req: &Req, data: &mut Value, itemid: &str, context: &str) {
    let cfg = st.cfg();
    let session = req.session();
    if session.is_some() || !cfg.truthy("main.hide_comments_from_public") {
        if let Some(s) = session.filter(|s| s.id != 0) {
            if !is_prefetch(req) {
                let (st2, uid, item) = (st.clone(), s.id, itemid.to_string());
                tokio::spawn(async move { crate::social::comments::mark_notifications_read(&st2, uid, &item).await });
            }
        }
        data["isSubscribed"] = json!(match session {
            Some(s) => crate::social::comments::get_subscription_status(st, s.id, itemid).await,
            None => false,
        });
    } else {
        data["isSubscribed"] = json!(false);
    }
    data["commentsJSON"] = Value::Null;
    data["comments"] = json!([]);
    data["session"] = session_value(session);
    data["url"] = json!({ "pathname": context });
    data["fullscreen"] = req.cookie("fullscreen").filter(|f| !f.is_empty()).map(|f| json!(f)).unwrap_or(json!(0));
    data["hidePagination"] = json!(true);

    if let Some(halls) = data["item"]["halls"].as_array().filter(|h| !h.is_empty()).cloned() {
        let cur = match &data["tmp"]["hall"] {
            Value::Object(m) => m.get("slug").cloned(),
            v if crate::config::truthy(v) => Some(v.clone()),
            _ => None,
        };
        let primary = halls.iter().find(|h| cur.as_ref().map(|c| &h["slug"] == c).unwrap_or(false)).unwrap_or(&halls[0]).clone();
        let others: Vec<Value> = halls.iter().filter(|h| h["slug"] != primary["slug"]).cloned().collect();
        data["item"]["primaryHall"] = primary;
        data["item"]["otherHalls"] = json!(others);
    } else if data["item"].is_object() {
        data["item"]["primaryHall"] = Value::Null;
        data["item"]["otherHalls"] = json!([]);
    }
    if data["item"].is_object() {
        // getf0ck already computed item.show_repost_row; keep its value
        let repost = data["item"].get("show_repost_row").cloned();
        super::index::item_template_flags(&cfg, data, session, session);
        match repost {
            Some(v) => data["item"]["show_repost_row"] = v,
            None => {
                if let Some(m) = data["item"].as_object_mut() {
                    m.shift_remove("show_repost_row");
                }
            }
        }
        if let Some(m) = data.as_object_mut() {
            m.shift_remove("show_repost_row");
        }
    }
}

/// The JSON envelope of an item load.
fn item_envelope(st: &AppState, req: &Req, data: &Value, itemid: &str, numeric_fallback: Option<String>, item_html: String, pagination: String, page: Value) -> Value {
    let cfg = st.cfg();
    let item = &data["item"];
    let t = |k: &str| crate::config::truthy(&item[k]);
    let js = |v: &Value| crate::tpl::js_to_string(&crate::tpl::to_jvalue(v));
    let nsfl = cfg.nsfl_tag_id();
    let numeric_id = if t("id") { json!(js(&item["id"])) } else { numeric_fallback.map(Value::String).unwrap_or(Value::Null) };
    let mode = if t("is_nsfl") {
        "nsfl"
    } else if t("is_nsfw") {
        "nsfw"
    } else if t("is_sfw") {
        "sfw"
    } else {
        "null"
    };
    let tag_id = if t("tag_id") {
        item["tag_id"].clone()
    } else if t("is_nsfl") {
        json!(nsfl)
    } else if t("is_nsfw") {
        json!(2)
    } else if t("is_sfw") {
        json!(1)
    } else {
        Value::Null
    };
    let thumb = if t("thumb") {
        item["thumb"].clone()
    } else if t("thumbnail") {
        item["thumbnail"].clone()
    } else if numeric_id.is_string() {
        json!(format!("/t/{}.webp", numeric_id.as_str().unwrap_or("")))
    } else {
        Value::Null
    };
    let anon = crate::config::truthy(&data["is_anonymized"]) || s::is_anonymize_session(&cfg, req.session());
    let first = |keys: &[&str], dflt: Value| keys.iter().map(|k| &item[*k]).find(|v| crate::config::truthy(v)).cloned().unwrap_or(dflt);
    let user = if anon { json!("anonymous") } else { first(&["author_display_name", "display_name", "username"], json!("anonymous")) };
    let username = if anon { json!("anonymous") } else { first(&["username"], json!("anonymous")) };
    json!({
        "html": item_html,
        "pagination": pagination,
        "title": data.get("title").cloned().unwrap_or(Value::Null),
        "id": itemid,
        "numeric_id": numeric_id,
        "slug": if t("slug") { item["slug"].clone() } else { Value::Null },
        "page": page,
        "thumb": thumb,
        "mode": mode,
        "tag_id": tag_id,
        "mime": first(&["matching_sub_mime", "mime"], Value::Null),
        "user": user,
        "username": username,
        "dest": first(&["matching_sub_dest", "dest"], Value::Null),
    })
}

// ── /ajax/item/random ───────────────────────────────────────────────────────

async fn item_random(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    let guest = is_guest(&req);
    let mode = req_mode(&req);
    let ratings: Option<Vec<String>> = if guest {
        Some(vec!["sfw".into()])
    } else if mode == 2 || mode == 3 {
        None
    } else {
        ratings_cookie(&req)
    };
    let tag = q_nonempty(&req, "tag");
    let hall = q_nonempty(&req, "hall");
    let user = q_nonempty(&req, "user");
    let user_hall = q_nonempty(&req, "userHall");
    let user_hall_owner = q_nonempty(&req, "userHallOwner");
    let fav = q(&req, "fav") == Some("true");
    let strict = q(&req, "strict") == Some("1");
    let cookie_mime = req.cookie("mime").map(|m| crate::util::decode_lossy(m).trim().to_string()).filter(|m| !m.is_empty());
    let mime = match req.query("mime") {
        Some(m) => Some(m.to_string()).filter(|m| !m.is_empty()),
        None => cookie_mime,
    };
    let excluded = session.map(|s| s.excluded_tags.clone()).unwrap_or_default();
    let base = FeedOpts {
        user: user.clone(),
        tag: tag.clone(),
        hall: hall.clone(),
        user_hall: user_hall.clone(),
        user_hall_owner: user_hall_owner.clone(),
        mime: mime.clone(),
        fav,
        mode: Some(mode),
        ratings: ratings.clone().filter(|r| !r.is_empty()),
        strict,
        session,
        exclude: excluded.clone(),
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin),
        ..Default::default()
    };
    let random = f0cklib::get_random(&st, &base).await?;
    let itemid = match random.get("itemid").filter(|v| crate::config::truthy(v)) {
        Some(v) => crate::tpl::js_to_string(&crate::tpl::to_jvalue(v)),
        None => {
            return Ok(json_body(&json!({ "html": "", "error": true, "success": false, "message": "No items found" })));
        }
    };
    let ctx = context_url(
        &itemid,
        &Context {
            tag: tag.as_deref(),
            hall: hall.as_deref(),
            user: user.as_deref(),
            user_hall: user_hall.as_deref(),
            user_hall_owner: user_hall_owner.as_deref(),
            fav,
            mime: mime.as_deref(),
        },
    );
    let opts = FeedOpts {
        itemid: Some(itemid.clone()),
        ratings: ratings.clone(),
        bypass_filter: q(&req, "bypass") == Some("1"),
        random: true,
        strict: strict || session.and_then(|s| s.strict_mode).unwrap_or(false),
        subf0ck: q_nonempty(&req, "subf0ck"),
        lang: Some(super::index::req_lang(&st, &req)),
        ..base.clone()
    };
    let mut data = f0cklib::getf0ck(&st, &opts).await?;
    if !data["success"].as_bool().unwrap_or(false) {
        return Ok(error_json(&st, &req, &data, &itemid, mode, false));
    }
    prepare_item(&st, &req, &mut data, &itemid, &ctx).await;
    let item_html = crate::tpl::render(&st, Some(&req), "ajax-item", data.clone());
    let pagination = crate::tpl::render(&st, Some(&req), "snippets/pagination", data.clone());

    let mut page = Value::Null;
    if s::is_onara_enabled_for(&cfg, &req.cookies, session, q(&req, "onara") == Some("1")) {
        let o = FeedOpts {
            target_item_id: Some(if crate::config::truthy(&data["item"]["id"]) { crate::tpl::js_to_string(&crate::tpl::to_jvalue(&data["item"]["id"])) } else { itemid.clone() }),
            target_item_pinned: Some(crate::config::truthy(&data["item"]["is_pinned"])),
            tagger: q_nonempty(&req, "tagger"),
            ..base.clone()
        };
        if let Ok(p) = f0cklib::get_item_page(&st, &o).await {
            page = json!(p);
        }
    }
    let mut out = item_envelope(&st, &req, &data, &itemid, Some(itemid.clone()), item_html, pagination, page);
    out["is_random"] = json!(true);
    Ok(json_body(&out))
}

// ── /ajax/item/:itemid ──────────────────────────────────────────────────────

async fn item(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    let itemid = req.param("itemid").unwrap_or("").to_string();
    let tag = q_nonempty(&req, "tag");
    let hall = q_nonempty(&req, "hall");
    let user = q_nonempty(&req, "user");
    let user_hall = q_nonempty(&req, "userHall");
    let user_hall_owner = q_nonempty(&req, "userHallOwner");
    let fav = q(&req, "fav") == Some("true");
    let qmime = q_nonempty(&req, "mime");
    let ctx = context_url(
        &itemid,
        &Context {
            tag: tag.as_deref(),
            hall: hall.as_deref(),
            user: user.as_deref(),
            user_hall: user_hall.as_deref(),
            user_hall_owner: user_hall_owner.as_deref(),
            fav,
            mime: qmime.as_deref(),
        },
    );
    let guest = is_guest(&req);
    let random = q(&req, "random") == Some("1") || req.cookie("random_mode") == Some("1");
    let mode = req_mode(&req);
    let ratings: Option<Vec<String>> = if guest {
        Some(vec!["sfw".into()])
    } else if mode == 2 || mode == 3 {
        None
    } else {
        ratings_cookie(&req)
    };
    let truthy_flag = |k: &str| matches!(q(&req, k), Some("1" | "true"));
    let bypass = truthy_flag("force") || truthy_flag("allow") || truthy_flag("bypass") || q(&req, "see_anyways") == Some("1");
    let explicit_strict = truthy_flag("strict");
    let strict = explicit_strict || session.and_then(|s| s.strict_mode).unwrap_or(false);
    let mime = qmime.clone().or_else(|| req.cookie("mime").filter(|m| !m.is_empty()).map(str::to_string));
    let excluded = session.map(|s| s.excluded_tags.clone()).unwrap_or_default();
    let base = FeedOpts {
        user: user.clone(),
        tag: tag.clone(),
        hall: hall.clone(),
        user_hall: user_hall.clone(),
        user_hall_owner: user_hall_owner.clone(),
        mime,
        fav,
        mode: Some(mode),
        ratings: ratings.clone(),
        strict,
        session,
        exclude: excluded,
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin),
        ..Default::default()
    };
    let opts = FeedOpts {
        itemid: Some(itemid.clone()),
        bypass_filter: bypass,
        ids: q_nonempty(&req, "ids").and_then(|i| f0cklib::clean_ids(&json!(i))),
        random,
        subf0ck: q_nonempty(&req, "subf0ck"),
        lang: Some(super::index::req_lang(&st, &req)),
        ..base.clone()
    };
    let mut data = f0cklib::getf0ck(&st, &opts).await?;
    if !data["success"].as_bool().unwrap_or(false) {
        // `(query.mode !== undefined ? +query.mode : req.mode) ?? 0` — not forced to 0 for guests
        let label_mode = match req.query("mode") {
            Some(m) => crate::social::helpers::js_number(m).filter(|f| f.is_finite()).map(|f| f as i64).unwrap_or(0),
            None => req.mode,
        };
        return Ok(error_json(&st, &req, &data, &itemid, label_mode, true));
    }
    prepare_item(&st, &req, &mut data, &itemid, &ctx).await;
    let item_html = crate::tpl::render(&st, Some(&req), "ajax-item", data.clone());
    let pagination = crate::tpl::render(&st, Some(&req), "snippets/pagination", data.clone());

    let mut page = Value::Null;
    if s::is_onara_enabled_for(&cfg, &req.cookies, session, q(&req, "onara") == Some("1")) || q(&req, "get_page") == Some("1") {
        let o = FeedOpts {
            target_item_id: Some(if crate::config::truthy(&data["item"]["id"]) { crate::tpl::js_to_string(&crate::tpl::to_jvalue(&data["item"]["id"])) } else { itemid.clone() }),
            target_item_pinned: Some(crate::config::truthy(&data["item"]["is_pinned"])),
            tagger: q_nonempty(&req, "tagger"),
            ..base.clone()
        };
        if let Ok(p) = f0cklib::get_item_page(&st, &o).await {
            page = json!(p);
        }
    }
    let numeric = itemid.bytes().all(|b| b.is_ascii_digit()).then(|| itemid.clone());
    let out = item_envelope(&st, &req, &data, &itemid, numeric, item_html, pagination, page);
    Ok(json_body(&out))
}

// ── /ajax/halls ─────────────────────────────────────────────────────────────

async fn halls(st: AppState, _req: Req) -> Res {
    let halls = crate::social::halls::get_halls(&st).await;
    Ok(json_body(&json!(halls)))
}

// ── /ajax/items ─────────────────────────────────────────────────────────────

async fn items(st: AppState, req: Req) -> Res {
    let session = req.session();
    let guest = is_guest(&req);
    let page = req.query("page").and_then(crate::social::helpers::js_parse_int).filter(|&p| p != 0).unwrap_or(1);
    let random = q(&req, "random") == Some("1") || req.cookie("random_mode") == Some("1");
    let ratings = if guest { Some(vec!["sfw".to_string()]) } else { ratings_cookie(&req) };
    let explicit_strict = matches!(q(&req, "strict"), Some("1" | "true"));
    let tag = q_nonempty(&req, "tag");
    let hall = q_nonempty(&req, "hall");
    let newer = q_nonempty(&req, "newer");
    let opts = FeedOpts {
        page: Some(page),
        tag: tag.clone(),
        hall: hall.clone(),
        user: q_nonempty(&req, "user"),
        user_hall: q_nonempty(&req, "userHall"),
        user_hall_owner: q_nonempty(&req, "userHallOwner"),
        mime: q_nonempty(&req, "mime").or_else(|| req.cookie("mime").filter(|m| !m.is_empty()).map(str::to_string)),
        mode: Some(req_mode(&req)),
        ratings,
        session,
        user_id: session.map(|s| s.id),
        is_admin: session.map(|s| s.admin),
        exclude: session.map(|s| s.excluded_tags.clone()).unwrap_or_default(),
        fav: q(&req, "fav") == Some("true"),
        ids: q_nonempty(&req, "ids").and_then(|i| f0cklib::clean_ids(&json!(i))),
        total: req.query("total").and_then(crate::social::helpers::js_parse_int),
        random,
        strict: explicit_strict || session.and_then(|s| s.strict_mode).unwrap_or(false),
        newer: newer.clone(),
        tagger: q_nonempty(&req, "tagger"),
        lang: Some(super::index::req_lang(&st, &req)),
        ..Default::default()
    };
    let mut data = f0cklib::getf0cks(&st, &opts).await?;
    let ok = |d: &Value| d["success"].as_bool().unwrap_or(false);

    if !ok(&data) && crate::config::truthy(&data["is_private"]) {
        let lang = session.and_then(|s| s.language.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| super::index::req_lang(&st, &req));
        let msg = st.i18n.translator(&lang).t("profile.private_favorites");
        let html = format!(r#"<div class="private-favs-msg private-favs-msg-ajax">{msg}</div>"#);
        return Ok(json_body(&json!({ "success": false, "is_private": true, "html": html, "hasMore": false })));
    }
    if !ok(&data) && page == 1 && newer.is_none() {
        data["items"] = json!([]);
        data["pagination"] = json!({ "start": 1, "end": 1, "current": 1, "page": 1, "cheat": [1], "prev": null, "next": null });
        data["total"] = json!(0);
        data["success"] = json!(true);
        if !crate::config::truthy(&data["link"]) {
            data["link"] = if let Some(h) = &hall {
                json!({ "main": format!("/h/{}/", enc(h)), "path": "p/", "suffix": "" })
            } else if let Some(t) = &tag {
                json!({ "main": format!("/tag/{}/", enc(t)), "path": "p/", "suffix": "" })
            } else {
                json!({ "main": "/", "path": "p/", "suffix": "" })
            };
        }
    }
    if !ok(&data) {
        return Ok(json_body(&json!({ "success": false, "html": "", "hasMore": false })));
    }

    let has_items = data["items"].as_array().map(|a| !a.is_empty()).unwrap_or(false);
    let items_html = if has_items {
        crate::tpl::render(&st, Some(&req), "snippets/items-grid", json!({ "items": data["items"], "link": data["link"] }))
    } else {
        r#"<div class="no-results">No f0cks found :(</div>"#.to_string()
    };
    let pagination_html = crate::tpl::render(&st, Some(&req), "snippets/pagination", json!({ "pagination": data["pagination"], "link": data["link"] }));
    let lower = |k: &str| q_nonempty(&req, k).map(|v| json!(v.to_lowercase())).unwrap_or(Value::Null);
    let title_html = crate::tpl::render(
        &st,
        Some(&req),
        "snippets/page-title",
        json!({
            "tmp": {
                "user": lower("user"),
                "tag": lower("tag"),
                "hall": lower("hall"),
                "mime": lower("mime"),
                "mode": req.query("mode"),
                "view_mode": if q(&req, "fav") == Some("true") { "favs" } else { "f0cks" },
            },
            "total": if crate::config::truthy(&data["total"]) { data["total"].clone() } else { json!(0) },
            "session": session_value(session.filter(|s| !s.user.is_empty())),
        }),
    );
    let next = data["pagination"].get("next").cloned();
    // `pagination.next !== null` (a missing `next` counts as more)
    let has_more = !matches!(next, Some(Value::Null));
    let mut r = json_body(&json!({
        "success": true,
        "html": items_html,
        "titleHtml": title_html,
        "pagination": pagination_html,
        "hasMore": has_more,
        "nextPage": next.unwrap_or(Value::Null),
        "currentPage": data["pagination"]["page"],
    }));
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache, must-revalidate, proxy-revalidate"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert(header::EXPIRES, HeaderValue::from_static("0"));
    Ok(r)
}
