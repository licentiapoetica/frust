//! Feed, item and profile pages plus the small static pages (port of src/inc/routes/index.mjs).

use crate::db::{Row, RowExt};
use crate::f0cklib::{self, FeedOpts};
use crate::http::{CookieExpiry, Req, Res, Router, cookie_options, html, html_code, raw, redirect, reply};
use crate::session::Session;
use crate::settings as s;
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, header};
use axum::response::Response;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/user\/(?<user>[^/]+)\/?$", user_profile);
    r.get(r"^\/favs(?:\/p\/(?<page>\d+))?\/?(?:\?.*)?$", favs_redirect);
    r.get(r"^\/favs\/(?<itemid>[a-zA-Z0-9_-]{11}|\d+)$", favs_item);
    r.get(
        r"^\/user\/(?<user>[^/]+)\/(?<itemid>(?!(?:f0cks|uploads|favs|comments|tags|halls)$)[a-zA-Z0-9_-]+)$",
        generic,
    );
    r.get(
        r"^(?!\/(s|b|t|ca|a|4|login|register|settings|about|terms|rules|api|logout|auth|admin|comments|notifications|feed)\/)\/?(?:\/tag\/(?<tag>.+?))?(?:\/h\/(?<hall>.+?))?(?:\/user\/(?<user>.+?)\/(?<mode>f0cks|uploads|favs))?(?:\/(?<mime>(?:video|audio|image)(?:,(?:video|audio|image))*))?(?:\/p\/(?<page>\d+))?(?:\/(?<itemid>[a-zA-Z0-9_-]{11}|\d+))?\/?(?:\?.*)?$",
        generic,
    );
    r.get(r"^\/(about)$", about);
    r.get(r"^\/(terms)$", terms);
    r.get(r"^\/privacy\/?$", privacy);
    r.get(r"^\/(rules)$", rules);
    r.get(r"^\/mode\/(\d)", mode);
    r.get(r"^\/strict\/(0|1)$", strict);
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// Page language, as the template renderer resolves it.
pub fn req_lang(st: &AppState, req: &Req) -> String {
    let cfg = st.cfg();
    let dflt = cfg.str_or("websrv.language", "en").to_string();
    let raw = req
        .session()
        .and_then(|s| s.language.as_deref())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .or_else(|| req.cookie("language").map(str::trim).filter(|l| !l.is_empty()))
        .map(str::to_string)
        .unwrap_or_else(|| dflt.clone());
    if crate::i18n::VALID_LANGS.contains(&raw.as_str()) { raw } else { dflt }
}

/// `createI18n(req.session?.language || req.lang || 'en')` — note: no cookie / config fallback.
fn err_lang(req: &Req) -> String {
    req.session().and_then(|s| s.language.clone()).filter(|l| !l.is_empty()).unwrap_or_else(|| "en".into())
}

/// `ratings` cookie → list of valid ratings (`None` when absent/empty).
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

fn session_value(session: Option<&Session>) -> Value {
    session.map(Session::to_value).unwrap_or(Value::Bool(false))
}

fn truthy_q(req: &Req, k: &str) -> bool {
    req.query(k).map(|v| !v.is_empty()).unwrap_or(false)
}

/// `req.cookies.mime !== undefined ? req.cookies.mime : (query mime || params.mime || null)`
fn feed_mime(req: &Req) -> Option<String> {
    if let Some(m) = req.cookie("mime") {
        return Some(m.to_string());
    }
    req.query("mime").filter(|m| !m.is_empty()).or_else(|| req.param("mime").filter(|m| !m.is_empty())).map(str::to_string)
}

fn min_xd(req: &Req, session: Option<&Session>) -> f64 {
    match req.query("min_xd") {
        Some(v) => {
            let t = v.trim();
            if t.is_empty() { 0.0 } else { t.parse::<f64>().unwrap_or(f64::NAN) }
        }
        None => session.map(|s| s.min_xd_score as f64).unwrap_or(0.0),
    }
}

fn error_page(st: &AppState, req: &Req, code: u16, data: Value) -> Response {
    html_code(crate::tpl::render(st, Some(req), "error", data), code)
}

// ── /user/:user ─────────────────────────────────────────────────────────────

async fn user_profile(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let session = req.session();
    let raw_user = req.param("user").unwrap_or("");
    if s::is_anonymize_session(&cfg, session) {
        let requested = crate::util::decode_lossy(raw_user).to_lowercase();
        let is_self = session.map(|s| requested == s.user.to_lowercase() || requested == s.login.to_lowercase()).unwrap_or(false);
        if !is_self {
            return Ok(redirect(if session.is_some() { "/" } else { "/login" }));
        }
    }
    let user = crate::util::decode_lossy(raw_user);
    let mime = match req.cookie("mime") {
        Some(m) => Some(m.to_string()),
        None => req.query("mime").filter(|m| !m.is_empty()).map(str::to_string),
    };

    let row = sql!(
        r#"select "user".id, "user".login, "user".user, "user".admin, "user".is_moderator, "user".banned, "user".ban_expires, "user".created_at, user_options.*, user_options.display_name
      from "user"
      left join user_options on "user".id = user_options.user_id
      where "user".user ilike "#,
        user.as_str(),
        r#" OR "user".login ilike "#,
        user.as_str(),
        " limit 1"
    )
    .fetch_optional(&st.db)
    .await?;

    let mut user_data: Row = match row {
        Some(r) => r,
        None => {
            let ghost = sql!(
                "SELECT username, MIN(stamp) as first_upload FROM items WHERE username ILIKE ",
                user.as_str(),
                " AND active = true GROUP BY username LIMIT 1"
            )
            .fetch_optional(&st.db)
            .await?;
            match ghost {
                Some(g) => {
                    let first = g.i64("first_upload").unwrap_or(0);
                    let created = chrono::DateTime::from_timestamp(first, 0).unwrap_or_default();
                    let mut m = Row::new();
                    m.insert("id".into(), Value::Null);
                    m.insert("user".into(), json!(user));
                    m.insert("created_at".into(), json!(created.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()));
                    m.insert("activated".into(), json!(true));
                    m.insert("banned".into(), json!(false));
                    m.insert("is_ghost".into(), json!(true));
                    m
                }
                None => {
                    return Ok(error_page(&st, &req, 404, json!({ "message": "this user does not exists", "tmp": null })));
                }
            }
        }
    };
    let is_ghost = user_data.bool("is_ghost");

    let random = req.cookie("random_mode") == Some("1");
    let ratings = if req.mode == 3 { None } else { ratings_cookie(&req) };
    let mut count = serde_json::Map::new();
    count.insert("f0cks".into(), json!(0));
    count.insert("favs".into(), json!(0));
    count.insert("tags".into(), json!(0));

    let base = FeedOpts {
        user: Some(user.clone()),
        mode: Some(req.mode),
        ratings: ratings.clone(),
        mime: mime.clone(),
        session,
        user_id: session.map(|s| s.id),
        random,
        ..Default::default()
    };
    // The profile shows the first 12 items of each list.
    let first12 = |mut v: Value, key: &str, count: &mut serde_json::Map<String, Value>| -> Value {
        if let Some(items) = v.get("items").and_then(Value::as_array).cloned() {
            let total = v.get("total").filter(|t| !t.is_null()).cloned().unwrap_or(json!(items.len()));
            count.insert(key.into(), total);
            v["items"] = Value::Array(items.into_iter().take(12).collect());
        }
        v
    };
    let f0cks = match f0cklib::getf0cks(&st, &base).await {
        Ok(v) => first12(v, "f0cks", &mut count),
        Err(e) => {
            tracing::error!("[PROFILE] getf0cks failed for user: {user} {e}");
            count.insert("f0cks".into(), json!(0));
            json!(false)
        }
    };
    let mut favs = Value::Null;
    if !is_ghost {
        let o = FeedOpts { fav: true, is_admin: session.map(|s| s.admin), ..base.clone() };
        favs = match f0cklib::getf0cks(&st, &o).await {
            Ok(v) => first12(v, "favs", &mut count),
            Err(_) => {
                count.insert("favs".into(), json!(0));
                json!(false)
            }
        };
        let uid = user_data.i64("id").unwrap_or(0) as i32;
        let q_comments = sql!(
            "select count(*) from comments c join items i on c.item_id = i.id where c.user_id = ",
            uid,
            " and c.is_deleted = false and i.active = true and i.is_deleted = false"
        );
        let q_tags = sql!("select count(*) from tags_assign where user_id = ", uid, " and tag_id > 2");
        let q_halls = sql!("select count(*) from user_halls where user_id = ", uid);
        let res = tokio::try_join!(
            q_comments.fetch_scalar::<i64, _>(&st.db),
            q_tags.fetch_scalar::<i64, _>(&st.db),
            q_halls.fetch_scalar::<i64, _>(&st.db)
        );
        match res {
            Ok((c, t, h)) => {
                count.insert("comments".into(), json!(c.unwrap_or(0)));
                count.insert("tags".into(), json!(t.unwrap_or(0)));
                count.insert("halls".into(), json!(h.unwrap_or(0)));
            }
            Err(_) => {
                let c = count.get("comments").cloned().filter(crate::config::truthy).unwrap_or(json!(0));
                count.insert("comments".into(), c);
                count.insert("tags".into(), json!(0));
            }
        }
    }

    let created = user_data
        .str("created_at")
        .and_then(|c| chrono::DateTime::parse_from_rfc3339(c).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    let lang = req_lang(&st, &req);
    let now = chrono::Utc::now();
    match created {
        Some(c) => {
            let ago = crate::util::time_ago(&st.i18n.translator(&lang), (now - c).num_seconds());
            user_data.insert("timestamp".into(), json!({ "timeago": ago, "timefull": c.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string() }));
            user_data.insert("age_days".into(), json!((now - c).num_milliseconds().div_euclid(86_400_000)));
        }
        None => {
            // new Date(null) is the epoch in JS
            let c = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap_or_default();
            let ago = crate::util::time_ago(&st.i18n.translator(&lang), (now - c).num_seconds());
            user_data.insert("timestamp".into(), json!({ "timeago": ago, "timefull": c.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string() }));
            user_data.insert("age_days".into(), json!((now - c).num_milliseconds().div_euclid(86_400_000)));
        }
    }

    if session.is_none() {
        user_data.insert("banned".into(), json!(false));
        user_data.shift_remove("ban_expires");
        user_data.shift_remove("ban_duration");
    } else if user_data.bool("banned") {
        let dur = match user_data.str("ban_expires").and_then(|e| chrono::DateTime::parse_from_rfc3339(e).ok()) {
            None if user_data.val("ban_expires").is_null() => "Permanent".to_string(),
            None => "Expiration pending refresh".to_string(),
            Some(exp) => {
                let diff = (exp.with_timezone(&chrono::Utc) - now).num_milliseconds() / 1000;
                if diff <= 0 {
                    "Expiration pending refresh".to_string()
                } else {
                    const EPOCHS: [(&str, i64); 7] =
                        [("year", 31536000), ("month", 2592000), ("week", 604800), ("day", 86400), ("hour", 3600), ("minute", 60), ("second", 1)];
                    let mut out = "Expires in ".to_string();
                    for (name, secs) in EPOCHS {
                        let n = diff / secs;
                        if n >= 1 {
                            out.push_str(&format!("{n} {name}{}", if n == 1 { "" } else { "s" }));
                            break;
                        }
                    }
                    out
                }
            }
        };
        user_data.insert("ban_duration".into(), json!(dur));
    }

    let domain = cfg.str_or("main.url.domain", "").to_string();
    let uname = user_data.string("user");
    let image = if user_data.bool("avatar_file") {
        format!("https://{domain}/a/{}", user_data.string("avatar_file"))
    } else if user_data.bool("avatar") {
        format!("https://{domain}/t/{}.webp", user_data.string("avatar"))
    } else {
        format!("https://{domain}/a/default.png")
    };
    let n = |k: &str| crate::tpl::js_to_string(&minijinja::Value::from_serialize(count.get(k).unwrap_or(&Value::Null)));
    let description = if is_ghost { format!("{} legacy uploads", n("f0cks")) } else { format!("{} uploads, {} favorites", n("f0cks"), n("favs")) };
    let mut data = json!({
        "user": Value::Object(user_data),
        "f0cks": f0cks,
        "count": Value::Object(count),
        "favs": favs,
        "tmp": null,
        "session": session_value(session),
        "page_meta": {
            "title": uname,
            "description": description,
            "url": format!("https://{domain}/user/{}", crate::util::encode_uri_component(&uname)),
            "image": image,
        }
    });
    if is_ghost {
        // favs stayed undefined for ghosts
        data.as_object_mut().map(|m| m.shift_remove("favs"));
    }
    let view = if req.is_xhr() { "user-partial" } else { "user" };
    Ok(html(crate::tpl::render(&st, Some(&req), view, data)))
}

// ── favourites shortcuts ────────────────────────────────────────────────────

fn location_302(target: &str) -> Response {
    let mut r = raw(302, "");
    if let Ok(v) = HeaderValue::from_str(target) {
        r.headers_mut().insert(header::LOCATION, v);
    }
    r
}

async fn favs_redirect(_st: AppState, req: Req) -> Res {
    if let Some(s) = req.session().filter(|s| !s.user.is_empty()) {
        let target = if s.is_anon && !s.login.is_empty() { &s.login } else { &s.user };
        return Ok(location_302(&format!("/user/{}/favs", crate::util::encode_uri_component(&target.to_lowercase()))));
    }
    Ok(location_302("/login"))
}

async fn favs_item(st: AppState, mut req: Req) -> Res {
    req.params.insert("mode".into(), "favs".into());
    generic(st, req).await
}

// ── feed / item pages ───────────────────────────────────────────────────────

fn no_store(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
    h.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    h.insert(header::EXPIRES, HeaderValue::from_static("0"));
    h.insert("surrogate-control", HeaderValue::from_static("no-store"));
    r
}

async fn generic(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let p = |k: &str| req.param(k).filter(|v| !v.is_empty()).map(str::to_string);
    let is_item = p("itemid").is_some();

    if cfg.is_false("websrv.halls_enabled") && p("hall").is_some() {
        return Ok(error_page(&st, &req, 404, json!({ "message": "Not found", "tmp": null })));
    }

    let mut session: Option<Session> = req.session.clone();
    let fav = p("mode").as_deref() == Some("favs");
    if fav && p("user").map(|u| u.to_lowercase() == "anonymous").unwrap_or(false) {
        if let Some(s) = session.as_ref().filter(|s| s.is_anon && !s.login.is_empty()) {
            return Ok(location_302(&format!("/user/{}/favs", crate::util::encode_uri_component(&s.login.to_lowercase()))));
        }
    }
    if s::is_anonymize_session(&cfg, session.as_ref()) && p("user").is_some() && p("mode").is_some() {
        let target = crate::util::decode_lossy(&p("user").unwrap_or_default()).to_lowercase();
        let is_self = session
            .as_ref()
            .filter(|s| !s.user.is_empty())
            .map(|s| target == s.user.to_lowercase() || (!s.login.is_empty() && target == s.login.to_lowercase()))
            .unwrap_or(false);
        if !is_self {
            return Ok(redirect(if session.is_some() { "/" } else { "/login" }));
        }
    }
    // ?strict in the URL updates the session's strict mode (for this request)
    if let Some(sess) = session.as_mut() {
        if let Some(v) = req.query("strict") {
            sess.strict_mode = Some(v == "1");
        }
    }
    let sref = session.as_ref();
    let req_tag = p("tag").map(|t| crate::util::decode_lossy(&t));

    if let (Some(tag), Some(s)) = (&req_tag, sref) {
        if s.id != 0 && !tag.starts_with("title:") {
            f0cklib::recommend::spawn_update_user_tag_affinity(&st, s.id, tag, 2.0);
        }
    }

    let bypass = ["force", "allow", "bypass", "see_anyways"].iter().any(|k| req.query(k) == Some("1"));
    let ratings = if req.mode == 2 || req.mode == 3 { None } else { ratings_cookie(&req) };
    let strict = truthy_q(&req, "strict") || sref.and_then(|s| s.strict_mode).unwrap_or(false);
    let random = req.cookie("random_mode") == Some("1");
    let lang = req_lang(&st, &req);
    let page = p("page").map(|pg| pg.parse::<i64>().unwrap_or(i64::MAX));
    let opts = FeedOpts {
        user: p("user"),
        tag: req_tag.clone(),
        mime: feed_mime(&req),
        page,
        itemid: p("itemid"),
        hall: p("hall"),
        fav,
        mode: Some(req.mode),
        bypass_filter: bypass,
        ratings: ratings.clone(),
        session: sref,
        user_id: sref.map(|s| s.id),
        is_admin: sref.map(|s| s.admin),
        exclude: sref.map(|s| s.excluded_tags.clone()).unwrap_or_default(),
        strict,
        random,
        min_xd_score: if is_item { 0.0 } else { min_xd(&req, sref) },
        lang: Some(lang),
        tagger: req.query("tagger").filter(|t| !t.is_empty()).map(str::to_string),
        subf0ck: req.query("subf0ck").filter(|t| !t.is_empty()).map(str::to_string),
        ..Default::default()
    };
    let mut data = if is_item { f0cklib::getf0ck(&st, &opts).await? } else { f0cklib::getf0cks(&st, &opts).await? };

    if !data["success"].as_bool().unwrap_or(false) {
        let is_private = crate::config::truthy(&data["is_private"]);
        let message = data["message"].as_str().unwrap_or("").to_string();
        if is_private && sref.is_none() && (is_item || message == "403 - private upload") {
            return Ok(crate::private_items::render_502(&st, &req));
        }
        let tr = st.i18n.translator(&err_lang(&req));
        if is_private && (message == "private favorites" || fav) {
            return Ok(error_page(
                &st,
                &req,
                403,
                json!({
                    "message": tr.t("profile.private_favorites"),
                    "domain": cfg.str_or("main.url.domain", ""),
                    "tmp": null,
                    "session": session_value(sref),
                    "error_filter_hint": null,
                    "error_filter_hint_link": null,
                }),
            ));
        }
        if !is_item {
            // Empty grid instead of an error page
            data["items"] = json!([]);
            data["pagination"] = json!({ "start": 1, "end": 1, "current": 1, "page": 1, "cheat": [1], "prev": null, "next": null });
            data["total"] = json!(0);
            data["success"] = json!(true);
            if !crate::config::truthy(&data["link"]) {
                data["link"] = if let Some(h) = p("hall") {
                    json!({ "main": format!("/h/{}/", crate::util::encode_uri_component(&h)), "path": "p/", "suffix": "" })
                } else if let Some(t) = &req_tag {
                    json!({ "main": format!("/tag/{}/", crate::util::encode_uri_component(t)), "path": "p/", "suffix": "" })
                } else {
                    json!({ "main": "/", "path": "p/", "suffix": "" })
                };
            }
            if !data["tmp"].is_object() {
                data["tmp"] = json!({});
            }
            if let Some(h) = p("hall") {
                if !crate::config::truthy(&data["tmp"]["hall"]) {
                    let row = sql!("SELECT id, name, slug, description FROM halls WHERE slug = ", h.as_str(), " LIMIT 1").fetch_optional(&st.db).await?;
                    data["tmp"]["hall"] = row.map(Value::Object).unwrap_or(json!(h));
                }
            }
            if let Some(t) = &req_tag {
                if !crate::config::truthy(&data["tmp"]["tag"]) {
                    data["tmp"]["tag"] = json!(t);
                }
            }
        } else {
            let code = if data["item"].is_null() { 404 } else { 200 };
            let label = match req.mode {
                3 => None,
                1 => Some("NSFW"),
                2 => Some("Untagged"),
                4 => Some("NSFL"),
                _ => Some("SFW"),
            };
            let item = data["item"].clone();
            let item_id = if crate::config::truthy(&item["id"]) { item["id"].clone() } else { json!(p("itemid")) };
            let item_slug = if crate::config::truthy(&item["slug"]) { item["slug"].clone() } else { json!(p("itemid")) };
            let mut d = json!({
                "message": tr.t("error.post_not_visible"),
                "item": item,
                "item_id": item_id,
                "item_slug": item_slug,
                "domain": cfg.str_or("main.url.domain", ""),
                "tmp": null,
                "session": session_value(sref),
                "error_mode_label": label,
                "error_filter_hint": label.map(|l| tr.t_with("error.filter_hint", &[("mode", &format!("<strong>{l}</strong>"))])),
                "error_filter_hint_link": tr.t("error.filter_hint_link"),
                "error_see_anyways": tr.t("error.see_anyways"),
            });
            if d["item"].is_null() {
                d.as_object_mut().map(|m| m.shift_remove("item"));
            }
            return Ok(error_page(&st, &req, code, d));
        }
    }

    if is_item {
        data["hidePagination"] = json!(true);
        let item = &mut data["item"];
        if let Some(halls) = item["halls"].as_array().filter(|h| !h.is_empty()).cloned() {
            let cur = match &data["tmp"]["hall"] {
                Value::Object(m) => m.get("slug").cloned(),
                v if crate::config::truthy(v) => Some(v.clone()),
                _ => None,
            };
            let item = &mut data["item"];
            let primary = halls.iter().find(|h| cur.as_ref().map(|c| &h["slug"] == c).unwrap_or(false)).unwrap_or(&halls[0]).clone();
            let others: Vec<Value> = halls.iter().filter(|h| h["slug"] != primary["slug"]).cloned().collect();
            item["primaryHall"] = primary;
            item["otherHalls"] = json!(others);
        } else if item.is_object() {
            item["primaryHall"] = Value::Null;
            item["otherHalls"] = json!([]);
        }
        let item_id = data["item"]["id"].as_i64().unwrap_or(0);
        if sref.is_some() || !cfg.truthy("main.hide_comments_from_public") {
            let prefetch = [req.header("x-purpose"), req.header("purpose"), req.header("sec-purpose")].iter().any(|h| *h == Some("prefetch"));
            if let Some(s) = sref.filter(|s| s.id != 0) {
                if !prefetch {
                    let (db, uid) = (st.db.clone(), s.id);
                    tokio::spawn(async move {
                        let q = sql!("UPDATE notifications SET is_read = true WHERE user_id = ", uid, " AND item_id = ", item_id as i32, " AND is_read = false");
                        if let Err(e) = q.execute(&db).await {
                            tracing::error!("[F0CKLIB] Error marking notifications as read: {e}");
                        }
                    });
                }
            }
            data["isSubscribed"] = json!(match sref {
                Some(s) => subscription_status(&st, s.id, item_id).await,
                None => false,
            });
            let score = f0cklib::item_xd_score(&st, item_id).await;
            let (tier, label) = f0cklib::xd_score_meta(score as f64);
            data["item"]["xd_score"] = json!(score);
            data["item"]["xd_tier"] = json!(tier);
            data["item"]["xd_label"] = json!(label);
        } else {
            data["isSubscribed"] = json!(false);
            data["item"]["xd_score"] = json!(0);
            data["item"]["xd_tier"] = json!(0);
            data["item"]["xd_label"] = json!("");
        }
        data["commentsJSON"] = Value::Null;
        data["comments"] = json!([]);
    } else if data.get("total").map(Value::is_null).unwrap_or(true) {
        data["total"] = json!(0);
    }

    // Only regular sessions get the member UI
    let page_session = sref.filter(|s| !s.user.is_empty());
    data["session"] = session_value(page_session);

    if is_item && data["item"].is_object() {
        item_template_flags(&cfg, &mut data, page_session, req.session());
    }

    let tpl_req = req_with_session(&req, session.clone());
    if req.is_xhr() && !is_item {
        return Ok(no_store(html(crate::tpl::render(&st, Some(&tpl_req), "index-partial", data))));
    }

    let onara = s::is_onara_enabled_for(&cfg, &req.cookies, sref, false);
    if is_item && onara {
        return Ok(no_store(onara_page(&st, &req, &tpl_req, &opts, data).await?));
    }
    let view = if is_item { "item" } else { "index" };
    Ok(no_store(html(crate::tpl::render(&st, Some(&tpl_req), view, data))))
}

/// The template renderer reads the session from the request; use the one with this request's
/// strict-mode update.
fn req_with_session(req: &Req, session: Option<Session>) -> Req {
    let mut r = Req::new(
        req.method.clone(),
        &req.original_url.parse().unwrap_or_else(|_| axum::http::Uri::from_static("/")),
        req.headers.clone(),
        axum::body::Body::empty(),
        req.remote,
    );
    r.path = req.path.clone();
    r.cookies = req.cookies.clone();
    r.params = req.params.clone();
    r.session = session;
    r.mode = req.mode;
    r.theme = req.theme.clone();
    r.fullscreen = req.fullscreen.clone();
    r.out = req.out.clone();
    r.csp_nonce = req.csp_nonce.clone();
    r
}

/// Booleans the item templates use (precomputed in the old route).
pub(crate) fn item_template_flags(cfg: &crate::config::Config, data: &mut Value, session: Option<&Session>, real_session: Option<&Session>) {
    let is_anon = session.map(|s| s.is_anon || s.user == "anonymous" || s.user.starts_with("anon_")).unwrap_or(false);
    let item_user = data["item"]["username"].as_str().unwrap_or("").to_string();
    let is_mod_or_admin = session.map(|s| s.admin || s.is_moderator).unwrap_or(false);
    let owns = session.map(|s| !s.user.is_empty() && !item_user.is_empty() && s.user.to_lowercase() == item_user.to_lowercase()).unwrap_or(false);
    let can_manage = !is_anon && (is_mod_or_admin || owns);
    let owner_name = s::session_owner_name(session);
    let can_rate = can_manage
        || (is_anon
            && s::can_anon_do(cfg, "rate_item")
            && owner_name.map(|n| !item_user.is_empty() && n.to_lowercase() == item_user.to_lowercase()).unwrap_or(false));
    let mime = data["item"]["mime"].as_str().unwrap_or("").to_string();
    let mimes = cfg.mimes();
    let ext = mimes.get(&mime).and_then(Value::as_str);
    let is_archive = mime.starts_with("application/") && ext.map(|e| !e.is_empty() && e != "swf" && e != "pdf").unwrap_or(false);
    data["is_mod_or_admin"] = json!(is_mod_or_admin);
    data["can_manage_item"] = json!(can_manage);
    data["can_rate_item"] = json!(can_rate);
    data["can_extract_meta"] = json!(!mime.is_empty() && !mime.contains("flash") && !is_archive);
    let favs = data["item"]["favorites"].as_array().cloned().unwrap_or_default();
    data["user_has_favorited"] = json!(crate::util::user_has_favorited(session, &favs));
    let slugs = |k: &str| data["item"][k].as_array().map(|a| a.iter().map(|h| crate::tpl::js_to_string(&minijinja::Value::from_serialize(&h["slug"]))).collect::<Vec<_>>().join(","));
    let (hs, uhs) = (slugs("halls").unwrap_or_default(), slugs("user_halls").unwrap_or_default());
    data["halls_slugs"] = json!(hs);
    data["user_halls_slugs"] = json!(uhs);

    if s::is_anonymize_session(cfg, real_session) {
        let item = &mut data["item"];
        if crate::config::truthy(&item["src"]) {
            item["src"] = Value::Null;
        }
        item["username"] = json!("anonymous");
        for k in [
            "author_banner_file",
            "author_banner_position",
            "author_banner_size",
            "author_avatar",
            "author_avatar_file",
            "author_color",
            "author_description",
            "author_display_name",
            "author_id",
        ] {
            item[k] = Value::Null;
        }
        if let Some(favs) = item["favorites"].as_array_mut() {
            for f in favs.iter_mut() {
                let is_self = session
                    .filter(|s| s.id != 0)
                    .map(|s| crate::config::as_int(&f["user_id"]).filter(|&u| u != 0) == Some(s.id))
                    .unwrap_or(false);
                if !is_self {
                    *f = json!({
                        "user_id": null, "user": "anonymous", "login": "anonymous", "display_name": "Anonymous",
                        "avatar": null, "avatar_file": null, "username_color": null,
                        "hide_fav_badge": f.get("hide_fav_badge").cloned().unwrap_or(Value::Null), "is_anon": true
                    });
                }
            }
        }
    }
    let item = &data["item"];
    let t = |k: &str| crate::config::truthy(&item[k]);
    let (class, label) = if t("is_nsfl") {
        ("is-nsfl", "NSFL")
    } else if t("is_nsfw") {
        ("is-nsfw", "NSFW")
    } else if t("is_sfw") {
        ("is-sfw", "SFW")
    } else {
        ("is-untagged", "?")
    };
    let username_lower = item["username"].as_str().unwrap_or("").to_lowercase();
    let is_flash = mime.contains("flash") || mime.contains("shockwave");
    let has_dims = t("width") && t("height");
    let show_repost = (session.is_some() || cfg.truthy("websrv.expose_repost_links_to_guests") || cfg.truthy("websrv.expose_repost_links"))
        && (t("is_repost") || item["reposts"].as_array().map(|r| !r.is_empty()).unwrap_or(false));
    let hall_slug = match &data["tmp"]["hall"] {
        Value::Object(m) => m.get("slug").cloned().unwrap_or(Value::Null),
        v if crate::config::truthy(v) => v.clone(),
        _ => json!(""),
    };
    let user_hall_slug = match &data["tmp"]["userHall"] {
        Value::Object(m) => m.get("slug").cloned().unwrap_or(Value::Null),
        v if crate::config::truthy(v) => v.clone(),
        _ => json!(""),
    };
    let owner = data["tmp"]["userHallOwner"].clone();
    data["item_rating_class"] = json!(class);
    data["item_rating_label"] = json!(label);
    data["item_username_lower"] = json!(username_lower);
    data["is_flash_item"] = json!(is_flash);
    data["is_archive_item"] = json!(is_archive);
    data["current_hall_slug"] = hall_slug;
    data["current_user_hall_slug"] = user_hall_slug;
    data["current_user_hall_owner"] = if crate::config::truthy(&owner) { owner } else { json!("") };
    data["item_has_dimensions"] = json!(has_dims);
    data["show_repost_row"] = json!(show_repost);
    data["item"]["show_repost_row"] = json!(show_repost);
}

/// Onara viewer: the item rendered into the feed grid page that contains it.
async fn onara_page(st: &AppState, req: &Req, tpl_req: &Req, opts: &FeedOpts<'_>, data: Value) -> anyhow::Result<Response> {
    let cfg = st.cfg();
    let item_html = crate::tpl::render(st, Some(tpl_req), "ajax-item", data.clone());
    let is_random = req.cookie("random_mode") == Some("1") || req.query("random") == Some("1");
    let mut item_page = 1;
    if let Some(pg) = req.param("page").filter(|p| !p.is_empty()) {
        item_page = pg.parse::<i64>().ok().filter(|&n| n != 0).unwrap_or(1);
    } else if !is_random && data["item"].is_object() {
        let o = FeedOpts {
            target_item_id: Some(crate::tpl::js_to_string(&minijinja::Value::from_serialize(&data["item"]["id"]))),
            target_item_pinned: Some(crate::config::truthy(&data["item"]["is_pinned"])),
            min_xd_score: min_xd(req, opts.session),
            itemid: None,
            ..opts.clone()
        };
        item_page = match f0cklib::get_item_page(st, &o).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("[ONARA] getItemPage error: {e}");
                1
            }
        };
    }
    let o = FeedOpts { page: Some(item_page), itemid: None, random: is_random, min_xd_score: min_xd(req, opts.session), ..opts.clone() };
    let mut grid = f0cklib::getf0cks(st, &o).await?;
    if !grid["success"].as_bool().unwrap_or(false) {
        grid = json!({
            "items": [],
            "pagination": { "start": 1, "end": 1, "current": 1, "page": 1, "cheat": [1], "prev": null, "next": null },
            "total": 0,
            "success": true,
            "link": { "main": "/", "path": "p/", "suffix": "" },
            "tmp": {}
        });
    }
    let item = &data["item"];
    let active_id = item["id"].clone();
    let active_slug = item["slug"].clone();
    let mut found = false;
    if let Some(items) = grid["items"].as_array_mut() {
        for it in items.iter_mut() {
            let hit = (crate::config::truthy(&active_slug) && it["slug"] == active_slug) || it["id"] == active_id;
            it["is_onara_active"] = json!(hit);
            found |= hit;
        }
        if !found && item.is_object() {
            let nsfl = cfg.nsfl_tag_id();
            let tag_id = if crate::config::truthy(&item["tag_id"]) {
                item["tag_id"].clone()
            } else if crate::config::truthy(&item["is_nsfl"]) {
                json!(nsfl)
            } else if crate::config::truthy(&item["is_nsfw"]) {
                json!(2)
            } else if crate::config::truthy(&item["is_sfw"]) {
                json!(1)
            } else {
                Value::Null
            };
            let first = |keys: &[&str]| keys.iter().map(|k| &item[*k]).find(|v| crate::config::truthy(v)).cloned();
            let thumb = first(&["thumb", "thumbnail"])
                .or_else(|| crate::config::truthy(&item["id"]).then(|| json!(format!("/t/{}.webp", crate::tpl::js_to_string(&minijinja::Value::from_serialize(&item["id"]))))))
                .unwrap_or(Value::Null);
            let dest = item["dest"].as_str().map(|d| d.strip_prefix("/b/").unwrap_or(d).to_string()).unwrap_or_default();
            let mut g = item.clone();
            g["tag_id"] = tag_id;
            g["thumb"] = thumb;
            g["dest"] = json!(dest);
            g["display_name"] = first(&["author_display_name", "display_name", "username"]).unwrap_or(Value::Null);
            g["mime"] = first(&["matching_sub_mime"]).unwrap_or_else(|| item["mime"].clone());
            g["thumb_size"] = first(&["thumb_size"]).unwrap_or(json!(1));
            g["is_onara_active"] = json!(true);
            if is_random && !items.is_empty() {
                use rand::RngExt;
                let idx = rand::rng().random_range(0..=items.len());
                items.insert(idx, g);
            } else {
                items.insert(0, g);
            }
        }
    }
    grid["session"] = data["session"].clone();
    grid["is_onara_item"] = json!(true);
    grid["onara"] = json!(true);
    grid["onara_item_html"] = json!(item_html);
    grid["item"] = data["item"].clone();
    grid["hidePagination"] = json!(false);
    Ok(html(crate::tpl::render(st, Some(tpl_req), "index", grid)))
}

/// `getSubscriptionStatus(userId, itemId)`
async fn subscription_status(st: &AppState, user_id: i64, item_id: i64) -> bool {
    if st.cfg().is_false("enable_comments") || user_id == 0 || item_id == 0 {
        return false;
    }
    sql!(
        "SELECT 1 FROM comment_subscriptions WHERE user_id = ",
        user_id as i32,
        " AND item_id = ",
        item_id as i32,
        " AND is_subscribed = true"
    )
    .fetch_optional(&st.db)
    .await
    .map(|r| r.is_some())
    .unwrap_or(false)
}

// ── static pages ────────────────────────────────────────────────────────────

fn member_session(req: &Req) -> Value {
    session_value(req.session().filter(|s| !s.user.is_empty()))
}

fn page_meta(cfg: &crate::config::Config, title: &str, description: &str, path: &str) -> Value {
    json!({ "title": title, "description": description, "url": format!("https://{}/{path}", cfg.str_or("main.url.domain", "")) })
}

async fn about(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let data = json!({ "tmp": null, "mail": cfg.get("main.mail"), "session": member_session(&req), "page_meta": page_meta(&cfg, "about", "About", "about") });
    Ok(html(crate::tpl::render(&st, Some(&req), "about", data)))
}

async fn terms(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let data = json!({ "tmp": null, "session": member_session(&req), "page_meta": page_meta(&cfg, "terms", "Terms of service", "terms") });
    Ok(html(crate::tpl::render(&st, Some(&req), "terms", data)))
}

async fn rules(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let data = json!({
        "tmp": null,
        "domain": cfg.str_or("main.url.domain", ""),
        "session": session_value(req.session()),
        "page_meta": page_meta(&cfg, "rules", "Rules and guidelines", "rules"),
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "rules", data)))
}

/// Retention periods in days (`getRetention()` of retention.mjs).
/// TODO(port): use the retention job's port once it exists.
fn retention_days(cfg: &crate::config::Config, key: &str, default: i64) -> i64 {
    let v = cfg.get(&format!("websrv.retention_{key}_days"));
    match v {
        Value::Null => default,
        Value::String(s) if s.is_empty() => default,
        _ => f0cklib_parse_int(v).filter(|&n| n > 0).unwrap_or(0),
    }
}

fn f0cklib_parse_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f.trunc() as i64),
        Value::String(s) => {
            let t = s.trim_start();
            let end = t.char_indices().find(|(i, c)| !(c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+')))).map(|(i, _)| i).unwrap_or(t.len());
            t[..end].parse().ok()
        }
        Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        Value::Null => None,
    }
}

async fn privacy(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let period = |n: i64| if n > 0 { format!("{n} day{}", if n == 1 { "" } else { "s" }) } else { "indefinitely".into() };
    let r = [
        ("ip", retention_days(&cfg, "ip", 30)),
        ("activity", retention_days(&cfg, "activity_log", 90)),
        ("login", retention_days(&cfg, "login_attempts", 30)),
        ("sessions", retention_days(&cfg, "sessions", 365)),
        ("fp", retention_days(&cfg, "fingerprint", 365)),
    ];
    let log_ips = s::log_user_ips(&cfg);
    let hash_ips = s::hash_user_ips(&cfg);
    let anon = s::enable_anonymous_access(&cfg);
    let mut ret = serde_json::Map::new();
    let mut ret_on = serde_json::Map::new();
    for (k, n) in r {
        ret.insert(k.into(), json!(period(n)));
        ret_on.insert(k.into(), json!(n > 0));
    }
    let pv = json!({
        "log_ips": log_ips,
        "hash_ips": hash_ips,
        "ip_mode": if !log_ips { "off" } else if hash_ips { "hashed" } else { "raw" },
        "anon": anon,
        "hw": anon && s::hw_fingerprint_enabled(&cfg),
        "https": cfg.str_or("main.url.full", "").starts_with("https"),
        "ip_secret_env": crate::security::ip_hash_secrets(&cfg).source.starts_with("env"),
        "domain": cfg.str_or("main.url.domain", ""),
        "ret": ret,
        "ret_on": ret_on,
    });
    let data = json!({
        "tmp": null,
        "mail": cfg.get("main.mail"),
        "pv": pv,
        "session": member_session(&req),
        "page_meta": page_meta(&cfg, "privacy", "Privacy: what is stored when you use the site", "privacy"),
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "privacy", data)))
}

// ── /mode/:n and /strict/:n ─────────────────────────────────────────────────

/// The old code changed the (cached) session object in place; drop the cached row instead so the
/// next request reads the new option.
fn invalidate_own_session(st: &AppState, req: &Req) {
    if let Some(c) = req.cookie("session") {
        st.invalidate_session_cache(&crate::util::sha256(c));
    }
}

async fn mode(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let mode: i64 = req.path.strip_prefix("/mode/").and_then(|r| r.chars().next()).and_then(|c| c.to_digit(10)).map(i64::from).unwrap_or(0);
    let session = req.session();
    let forbidden = |msg: &str| -> Response {
        if req.is_xhr() {
            reply(403, "application/json", json!({ "success": false, "msg": msg }).to_string())
        } else {
            redirect("/")
        }
    };
    if session.map(|s| s.user.is_empty()).unwrap_or(true) {
        if mode != 0 {
            return Ok(forbidden("Only SFW mode is allowed for guests"));
        }
    } else if s::is_anon_session(session) && (!s::can_anon_do(&cfg, "filter") || !s::can_anon_mode(&cfg, mode)) {
        return Ok(forbidden("Mode not permitted for anonymous users"));
    }

    let allowed = cfg.get("allowedModes").as_array().and_then(|a| a.get(mode as usize)).map(crate::config::truthy).unwrap_or(false);
    if allowed {
        if let Some(s) = session {
            let theme = if req.theme.is_empty() { cfg.str_or("websrv.theme", "f0ck").to_string() } else { req.theme.clone() };
            sql!(
                r#"insert into "user_options" (user_id, mode, theme) values ("#,
                s.id as i32,
                ", ",
                mode as i32,
                ", ",
                theme,
                r#") on conflict ("user_id") do update set mode = excluded.mode, theme = excluded.theme, user_id = excluded.user_id"#
            )
            .execute(&st.db)
            .await?;
            invalidate_own_session(&st, &req);
        }
    }
    let cookie = format!("mode={mode}; {}", cookie_options(&cfg, CookieExpiry::MaxAge(31536000), false));
    let mut r = if req.is_xhr() {
        reply(200, "application/json", json!({ "success": true, "mode": mode }).to_string())
    } else {
        let mut r = raw(302, "");
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
        r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/"));
        r
    };
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        r.headers_mut().insert(header::SET_COOKIE, v);
    }
    Ok(r)
}

async fn strict(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let on = req.path.split('/').nth(2) == Some("1");
    if let Some(s) = req.session() {
        let theme = s.theme.clone().filter(|t| !t.is_empty()).unwrap_or_else(|| cfg.str_or("websrv.theme", "f0ck").to_string());
        let fullscreen = crate::config::as_int(&s.fullscreen).unwrap_or(0) as i16;
        sql!(
            r#"insert into "user_options" (user_id, strict_mode, mode, theme, fullscreen) values ("#,
            s.id as i32,
            ", ",
            on,
            ", ",
            s.mode.unwrap_or(0) as i32,
            ", ",
            theme,
            ", ",
            fullscreen,
            r#") on conflict ("user_id") do update set strict_mode = excluded.strict_mode"#
        )
        .execute(&st.db)
        .await?;
        invalidate_own_session(&st, &req);
    }
    Ok(reply(200, "application/json", json!({ "success": true, "strict": on }).to_string()))
}
