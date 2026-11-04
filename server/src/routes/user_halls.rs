//! User halls: browse pages, thumbnails and the `/api/v2/me/halls` API
//! (port of src/inc/routes/user_halls.mjs and src/user_hall_image_handler.mjs).

use super::halls::{basename, build_mosaic, cache_dir, custom_dir, error_page, mode_param, mosaic_mode_join, placeholder, serve_cached, serve_custom};
use crate::db::{Row, RowExt};
use crate::http::{Req, Res, Router, html, json_raw, raw, redirect};
use crate::session::Session;
use crate::settings::{can_anon_do, is_anonymize_session, session_owner_name};
use crate::social::helpers::{js_number, js_str};
use crate::social::{comments as sc, feed_bridge, halls};
use crate::sql;
use crate::state::AppState;
use axum::http::{HeaderValue, Method, header};
use axum::response::Response;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/user\/(?<owner>[^/]+)\/halls\/?$", list_halls);
    r.get(r"^\/user\/(?<owner>[^/]+)\/hall\/(?<slug>[^/]+)(?:\/p\/(?<page>\d+))?\/?$", hall_grid);
    r.get(r"^\/user\/(?<owner>[^/]+)\/hall\/(?<slug>[^/]+)\/(?<itemid>\d+)\/?$", hall_item);
    r.get(r"^\/user_hall_image\/(?<userId>\d+)\/(?<slug>.+)$", user_hall_image);
    r.get(r"^\/api\/v2\/me\/halls\/?$", api_list);
    r.post(r"^\/api\/v2\/me\/halls\/?$", api_create);
    r.patch(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/?$", api_update);
    r.delete(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/?$", api_delete);
    r.post(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/items\/?$", api_add_item);
    r.delete(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/items\/(?<itemid>\d+)\/?$", api_remove_item);
    // One form for the halls dialog of an item (works as a plain form without JavaScript)
    r.post(r"^\/api\/v2\/me\/hall-items\/?$", api_item_form);
    r.post(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/image\/?$", api_image_stub);
    r.delete(r"^\/api\/v2\/me\/halls\/(?<slug>[^/]+)\/image\/?$", api_image_delete);
    // index.mjs bypass middleware (multipart upload, needs the raw body)
    r.pre(Method::POST, r"^\/api\/v2\/me\/halls\/([^/]+)\/image$", None, image_upload);
}

/// `slugify` of user_halls.mjs: lowercase, runs of non-alphanumerics → `-`, trimmed dashes.
fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn userhalls_enabled(st: &AppState) -> bool {
    !st.cfg().is_false("websrv.userhalls_enabled")
}

fn decode(s: Option<&str>) -> String {
    crate::util::decode_lossy(s.unwrap_or(""))
}

/// `requireLogin(req, res)` — `Err(response)` when logged out.
fn require_login(req: &Req) -> Result<&Session, Response> {
    if let Some(s) = req.session() {
        return Ok(s);
    }
    if req.path.starts_with("/api/") {
        return Err(json_raw(&json!({ "success": false, "msg": "Login required" }), 401));
    }
    let mut r = raw(302, "");
    r.headers_mut().insert(header::LOCATION, HeaderValue::from_static("/login"));
    Err(r)
}

/// Private halls only resolve for their owner or an admin.
async fn resolve_hall(st: &AppState, owner: &str, slug: &str, viewer: Option<&Session>) -> Option<Row> {
    let hall = halls::get_user_hall_by_owner_name(st, owner, slug).await?;
    if hall.bool("is_private") {
        let is_owner = viewer.map(|s| s.user.to_lowercase() == owner.to_lowercase()).unwrap_or(false);
        let is_admin = viewer.map(|s| s.admin).unwrap_or(false);
        if !is_owner && !is_admin {
            return None;
        }
    }
    Some(hall)
}

fn anonymize_redirect(st: &AppState, req: &Req) -> Option<Response> {
    is_anonymize_session(&st.cfg(), req.session()).then(|| redirect(if req.session().is_some() { "/" } else { "/login" }))
}

// ── Browse ──────────────────────────────────────────────────────────────────

async fn list_halls(st: AppState, req: Req) -> Res {
    if let Some(r) = anonymize_redirect(&st, &req) {
        return Ok(r);
    }
    if !userhalls_enabled(&st) {
        return Ok(error_page(&st, &req, "Not found", 404));
    }
    let owner_name = decode(req.param("owner"));
    let excluded: Vec<i64> = req.session().map(|s| s.excluded_tags.clone()).unwrap_or_default();
    let owner = sql!(r#"SELECT id, "user", admin FROM "user" WHERE "user" ILIKE "#, crate::util::escape_like(&owner_name), " LIMIT 1")
        .fetch_optional(&st.db)
        .await?;
    let Some(owner) = owner else { return Ok(error_page(&st, &req, "User not found", 404)) };
    let owner_id = owner.i64("id").unwrap_or(0);
    let viewer = req.session().map(|s| s.id);
    let list = halls::get_user_halls(&st, owner_id, req.mode, &excluded, viewer).await;
    let owner_user = owner.string("user");
    let cfg = st.cfg();
    let data = json!({
        "hallsList": list,
        "ownerUser": owner,
        "isOwner": viewer == Some(owner_id),
        "tmp": null,
        "hidePagination": true,
        "session": req.session_value(),
        "page_meta": {
            "title": format!("{owner_user}'s Halls"),
            "description": format!("Browse {owner_user}'s personal collections"),
            "url": format!("https://{}/user/{}/halls", cfg.domain(), crate::util::encode_uri_component(&owner_user)),
        }
    });
    let view = if req.is_xhr() { "user-halls-partial" } else { "user-halls" };
    Ok(html(crate::tpl::render(&st, Some(&req), view, data)))
}

/// Options object of the old `getf0cks` / `getf0ck` calls.
fn feed_opts(req: &Req, slug: &str, owner: &str) -> serde_json::Map<String, Value> {
    let s = req.session();
    let mut m = serde_json::Map::new();
    m.insert("mode".into(), json!(req.mode));
    m.insert("session".into(), req.session_value());
    m.insert("exclude".into(), json!(s.map(|s| s.excluded_tags.clone()).unwrap_or_default()));
    m.insert("user_id".into(), s.map(|s| json!(s.id)).unwrap_or(Value::Null));
    m.insert("userHall".into(), json!(slug));
    m.insert("userHallOwner".into(), json!(owner));
    m.insert("mime".into(), req.cookie("mime").filter(|m| !m.is_empty()).map(|m| json!(m)).unwrap_or(Value::Null));
    m.insert("random".into(), json!(req.cookie("random_mode") == Some("1")));
    m
}

async fn hall_grid(st: AppState, req: Req) -> Res {
    if let Some(r) = anonymize_redirect(&st, &req) {
        return Ok(r);
    }
    if !userhalls_enabled(&st) {
        return Ok(error_page(&st, &req, "Not found", 404));
    }
    let owner_name = decode(req.param("owner"));
    let slug = decode(req.param("slug"));
    let Some(hall) = resolve_hall(&st, &owner_name, &slug, req.session()).await else {
        return Ok(error_page(&st, &req, "Hall not found", 404));
    };
    let mut opts = feed_opts(&req, &slug, &owner_name);
    opts.insert("page".into(), req.param("page").map(|p| json!(p)).unwrap_or(Value::Null));
    let mut data = match feed_bridge::get_f0cks(&st, req.session(), Value::Object(opts)).await {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let hall_owner = hall.string("owner_name");
    let hall_slug = hall.string("slug");
    if !data.get("success").map(crate::config::truthy).unwrap_or(false) {
        data.insert("items".into(), json!([]));
        data.insert("pagination".into(), json!({ "start": 1, "end": 1, "current": 1, "page": 1, "cheat": [1], "prev": null, "next": null }));
        data.insert("total".into(), json!(0));
        data.insert("success".into(), json!(true));
        data.insert(
            "link".into(),
            json!({
                "main": format!("/user/{}/hall/{}/", crate::util::encode_uri_component(&hall_owner), crate::util::encode_uri_component(&hall_slug)),
                "path": "p/",
                "suffix": ""
            }),
        );
        data.insert("tmp".into(), json!({ "userHall": hall, "userHallOwner": hall_owner }));
    }
    let cfg = st.cfg();
    data.insert("session".into(), req.session_value());
    data.insert("isOwner".into(), json!(req.session().map(|s| Some(s.id) == hall.i64("user_id")).unwrap_or(false)));
    let description = hall.str("description").filter(|d| !d.is_empty()).map(str::to_string).unwrap_or_else(|| format!("{hall_owner}'s collection"));
    data.insert(
        "page_meta".into(),
        json!({
            "title": format!("{} — {hall_owner}'s Hall", hall.string("name")),
            "description": description,
            "url": format!("https://{}/user/{}/hall/{}", cfg.domain(), crate::util::encode_uri_component(&hall_owner), crate::util::encode_uri_component(&hall_slug)),
        }),
    );
    let view = if req.is_xhr() { "index-partial" } else { "index" };
    Ok(html(crate::tpl::render(&st, Some(&req), view, Value::Object(data))))
}

/// `lib.userHasFavorited(session, favorites)`
fn user_has_favorited(session: Option<&Session>, favorites: &Value) -> bool {
    let (Some(s), Some(favs)) = (session, favorites.as_array()) else { return false };
    let sess_user = s.user.to_lowercase();
    let sess_login = s.login.to_lowercase();
    let sess_anon = s.anon_login.clone().unwrap_or_default().to_lowercase();
    favs.iter().any(|f| {
        let fid = f.get("user_id").filter(|v| crate::config::truthy(v)).or_else(|| f.get("id").filter(|v| crate::config::truthy(v))).and_then(crate::config::as_int);
        if s.id != 0 && fid == Some(s.id) {
            return true;
        }
        let low = |k: &str| f.get(k).and_then(Value::as_str).unwrap_or("").to_lowercase();
        let (fu, fl) = (low("user"), low("login"));
        (!sess_user.is_empty() && sess_user != "anonymous" && (fu == sess_user || fl == sess_user))
            || (!sess_login.is_empty() && (fu == sess_login || fl == sess_login))
            || (!sess_anon.is_empty() && (fu == sess_anon || fl == sess_anon))
    })
}

async fn hall_item(st: AppState, req: Req) -> Res {
    if let Some(r) = anonymize_redirect(&st, &req) {
        return Ok(r);
    }
    if !userhalls_enabled(&st) {
        return Ok(error_page(&st, &req, "Not found", 404));
    }
    let cfg = st.cfg();
    let owner_name = decode(req.param("owner"));
    let slug = decode(req.param("slug"));
    let item_id = req.param("itemid").unwrap_or("").to_string();
    if resolve_hall(&st, &owner_name, &slug, req.session()).await.is_none() {
        return Ok(error_page(&st, &req, "Hall not found", 404));
    }
    let mut opts = feed_opts(&req, &slug, &owner_name);
    opts.insert("itemid".into(), json!(item_id));
    let mut data = match feed_bridge::get_f0ck(&st, req.session(), Value::Object(opts)).await {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    if !data.get("success").map(crate::config::truthy).unwrap_or(false) {
        let has_item = data.get("item").map(crate::config::truthy).unwrap_or(false);
        let body = crate::tpl::render(
            &st,
            Some(&req),
            "error",
            json!({ "message": data.get("message").cloned().unwrap_or(Value::Null), "item": data.get("item").cloned().unwrap_or(Value::Null), "tmp": null }),
        );
        return Ok(crate::http::html_code(body, if has_item { 200 } else { 404 }));
    }

    let session = req.session();
    data.insert("hidePagination".into(), json!(true));
    data.insert("session".into(), req.session_value());

    if let Some(Value::Object(mut item)) = data.get("item").cloned().filter(crate::config::truthy) {
        // The old code referenced an undeclared `item` here (ReferenceError on every request);
        // it clearly meant `data.item`.
        let is_anon = session.map(Session::is_anonymous_user).unwrap_or(false);
        let item_user = item.get("username").and_then(Value::as_str).unwrap_or("").to_string();
        let is_mod_or_admin = session.map(|s| s.admin || s.is_moderator).unwrap_or(false);
        let can_manage = !is_anon
            && session.map(|s| s.admin || s.is_moderator || (!s.user.is_empty() && !item_user.is_empty() && s.user.to_lowercase() == item_user.to_lowercase())).unwrap_or(false);
        let owner = session_owner_name(session);
        let can_rate = can_manage
            || (is_anon && can_anon_do(&cfg, "rate_item") && owner.map(|o| !item_user.is_empty() && o.to_lowercase() == item_user.to_lowercase()).unwrap_or(false));
        let mime = item.get("mime").and_then(Value::as_str).unwrap_or("").to_string();
        let mimes = cfg.mimes();
        let ext = mimes.get(&mime).and_then(Value::as_str);
        let is_archive = mime.starts_with("application/") && ext.is_some() && !matches!(ext, Some("swf" | "pdf"));
        let can_extract = !mime.is_empty() && !mime.contains("flash") && !is_archive;
        let slugs = |k: &str| match item.get(k) {
            Some(Value::Array(a)) => a.iter().map(|h| h.get("slug").map(jsv).unwrap_or_default()).collect::<Vec<_>>().join(","),
            _ => String::new(),
        };
        let flag = |k: &str| item.get(k).map(crate::config::truthy).unwrap_or(false);
        let (rating_class, rating_label) = if flag("is_nsfl") {
            ("is-nsfl", "NSFL")
        } else if flag("is_nsfw") {
            ("is-nsfw", "NSFW")
        } else if flag("is_sfw") {
            ("is-sfw", "SFW")
        } else {
            ("is-untagged", "?")
        };
        let tmp = data.get("tmp").cloned().unwrap_or(Value::Null);
        let tmp_slug = |k: &str| match tmp.get(k) {
            Some(Value::Object(o)) => o.get("slug").map(jsv).unwrap_or_default(),
            Some(v) if crate::config::truthy(v) => jsv(v),
            _ => String::new(),
        };
        let user_owner = tmp.get("userHallOwner").filter(|v| crate::config::truthy(v)).map(jsv).unwrap_or_default();

        let mut put = |k: &str, v: Value| {
            data.insert(k.into(), v);
        };
        put("is_mod_or_admin", json!(is_mod_or_admin));
        put("can_manage_item", json!(can_manage));
        put("can_rate_item", json!(can_rate));
        put("can_extract_meta", json!(can_extract));
        put("user_has_favorited", json!(user_has_favorited(session, item.get("favorites").unwrap_or(&Value::Null))));
        put("halls_slugs", json!(slugs("halls")));
        put("user_halls_slugs", json!(slugs("user_halls")));
        put("item_rating_class", json!(rating_class));
        put("item_rating_label", json!(rating_label));
        put("item_username_lower", json!(item_user.to_lowercase()));
        put("is_flash_item", json!(!mime.is_empty() && (mime.contains("flash") || mime.contains("shockwave"))));
        put("is_archive_item", json!(is_archive));
        put("current_hall_slug", json!(tmp_slug("hall")));
        put("current_user_hall_slug", json!(tmp_slug("userHall")));
        put("current_user_hall_owner", json!(user_owner));
        put("item_has_dimensions", json!(flag("width") && flag("height")));

        if is_anonymize_session(&cfg, session) {
            if flag("src") {
                item.insert("src".into(), Value::Null);
            }
            item.insert("username".into(), json!("anonymous"));
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
                item.insert(k.into(), Value::Null);
            }
            if let Some(Value::Object(up)) = data.get_mut("uploader") {
                up.insert("name".into(), json!("anonymous"));
                up.insert("id".into(), Value::Null);
                up.insert("color".into(), Value::Null);
            }
            if let Some(Value::Array(favs)) = item.get_mut("favorites") {
                for f in favs.iter_mut() {
                    let is_self = session.map(|s| s.id != 0 && f.get("user_id").and_then(crate::config::as_int) == Some(s.id)).unwrap_or(false);
                    if !is_self {
                        let hide = f.get("hide_fav_badge").cloned().unwrap_or(Value::Null);
                        *f = json!({
                            "user_id": null, "user": "anonymous", "login": "anonymous", "display_name": "Anonymous",
                            "avatar": null, "avatar_file": null, "username_color": null, "hide_fav_badge": hide, "is_anon": true
                        });
                    }
                }
            }
        }

        match item.get("halls").cloned() {
            Some(Value::Array(h)) if !h.is_empty() => {
                item.insert("primaryHall".into(), h[0].clone());
                item.insert("otherHalls".into(), Value::Array(h[1..].to_vec()));
            }
            _ => {
                item.insert("primaryHall".into(), Value::Null);
                item.insert("otherHalls".into(), json!([]));
            }
        }

        if session.is_some() || !cfg.truthy("main.hide_comments_from_public") {
            let prefetch = [("x-purpose", "prefetch"), ("purpose", "prefetch"), ("sec-purpose", "prefetch")].iter().any(|(h, v)| req.header(h) == Some(*v));
            if let Some(s) = session.filter(|s| s.id != 0 && !prefetch) {
                let (st2, uid, iid) = (st.clone(), s.id, item_id.clone());
                tokio::spawn(async move { sc::mark_notifications_read(&st2, uid, &iid).await });
            }
            let subscribed = match session {
                Some(s) => sc::get_subscription_status(&st, s.id, &item_id).await,
                None => false,
            };
            data.insert("isSubscribed".into(), json!(subscribed));
        } else {
            data.insert("isSubscribed".into(), json!(false));
        }
        data.insert("commentsJSON".into(), Value::Null);
        data.insert("comments".into(), json!([]));
        data.insert("item".into(), Value::Object(item));
    }

    Ok(html(crate::tpl::render(&st, Some(&req), "item", Value::Object(data))))
}

// ── Thumbnail ───────────────────────────────────────────────────────────────

async fn user_hall_image(st: AppState, req: Req) -> Res {
    let user_id: i64 = req.param("userId").and_then(|u| u.parse().ok()).unwrap_or(0);
    let slug = decode(req.param("slug"));
    let safe = basename(&slug);
    let (mode, mode_str) = mode_param(&req);
    let r: anyhow::Result<Option<Response>> = async {
        if let Some(r) = serve_custom(&req, &custom_dir(&st).join(format!("u_{user_id}_{safe}.webp"))).await {
            return Ok(Some(r));
        }
        let hash = crate::util::md5(&format!("uh_{user_id}_{safe}_{mode_str}"));
        let cache_path = cache_dir(&st).join(format!("{hash}.webp"));
        if let Some(r) = serve_cached(&cache_path).await {
            return Ok(Some(r));
        }
        let Some(hall) = halls::get_user_hall(&st, user_id, &slug).await else { return Ok(None) };
        let mut q = crate::db::Sql::new("SELECT i.id FROM items i JOIN user_halls_assign uha ON uha.item_id = i.id ");
        q.push(mosaic_mode_join(mode))
            .push(" WHERE uha.hall_id = ")
            .bind(hall.i64("id").unwrap_or(0) as i32)
            .push(" AND i.active = true AND COALESCE(i.visibility, 0) = 0 ORDER BY RANDOM() LIMIT 3");
        let ids: Vec<i64> = q.fetch_all(&st.db).await?.iter().filter_map(|r| r.i64("id")).collect();
        if ids.is_empty() {
            return Ok(None);
        }
        build_mosaic(&st, &ids, &cache_path).await
    }
    .await;
    match r {
        Ok(Some(resp)) => Ok(resp),
        Ok(None) => Ok(placeholder()),
        Err(e) => {
            tracing::error!("[USER_HALL_IMAGE] {e:#}");
            Ok(placeholder())
        }
    }
}

// ── API ─────────────────────────────────────────────────────────────────────

fn jr(v: Value, code: u16) -> Res {
    Ok(json_raw(&v, code))
}

fn disabled() -> Res {
    jr(json!({ "success": false }), 404)
}

fn ok_status(v: &Value, ok: u16, err: u16) -> u16 {
    if v.get("success").map(crate::config::truthy).unwrap_or(false) { ok } else { err }
}

async fn api_list(st: AppState, req: Req) -> Res {
    if !userhalls_enabled(&st) {
        return disabled();
    }
    let s = match require_login(&req) {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    let list = halls::get_user_halls(&st, s.id, 3, &[], Some(s.id)).await;
    jr(json!({ "success": true, "halls": list }), 200)
}

async fn api_create(st: AppState, mut req: Req) -> Res {
    if !userhalls_enabled(&st) {
        return disabled();
    }
    let uid = match require_login(&req) {
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let body = req.post_value().await;
    let field = |k: &str| js_str(body.get(k)).filter(|v| !v.is_empty());
    let name = field("name").unwrap_or_default().trim().to_string();
    let slug = slugify(&field("slug").unwrap_or_else(|| name.clone()));
    let description = field("description").map(|d| d.trim().to_string()).filter(|d| !d.is_empty());
    if name.is_empty() || slug.is_empty() {
        return jr(json!({ "success": false, "msg": "Name is required" }), 400);
    }
    let result = halls::create_user_hall(&st, uid, &name, &slug, description.as_deref()).await;
    let code = ok_status(&result, 200, 409);
    jr(result, code)
}

async fn api_update(st: AppState, mut req: Req) -> Res {
    if !userhalls_enabled(&st) {
        return disabled();
    }
    let uid = match require_login(&req) {
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let slug = decode(req.param("slug"));
    let body = req.post_value().await;
    let new_slug = if crate::social::helpers::js_truthy(body.get("slug")) {
        // An input that slugifies to nothing used to blank the slug (making the hall unreachable).
        Some(slugify(&js_str(body.get("slug")).unwrap_or_default())).filter(|s| !s.is_empty())
    } else {
        None
    };
    let description = match body.get("description") {
        None => None,
        Some(v) => Some(if crate::config::truthy(v) { js_str(Some(v)) } else { None }),
    };
    let is_private = body.get("is_private").map(|v| matches!(v, Value::Bool(true)) || v.as_str() == Some("true") || v.as_f64() == Some(1.0));
    let update = halls::UserHallUpdate { name: js_str(body.get("name")), new_slug, description, is_private };
    let result = halls::update_user_hall(&st, uid, &slug, update).await;
    let code = ok_status(&result, 200, 400);
    jr(result, code)
}

async fn api_delete(st: AppState, req: Req) -> Res {
    if !userhalls_enabled(&st) {
        return disabled();
    }
    let s = match require_login(&req) {
        Ok(s) => s,
        Err(r) => return Ok(r),
    };
    let slug = decode(req.param("slug"));
    let mut target = Some(s.id);
    if s.admin {
        if let Some(q) = req.query("user_id").filter(|q| !q.is_empty()) {
            target = js_number(q).filter(|f| f.fract() == 0.0 && f.is_finite()).map(|f| f as i64);
        }
    }
    let Some(target) = target else {
        // `+qs.user_id` was NaN: the old delete query failed.
        return jr(json!({ "success": false, "message": "invalid user_id" }), 404);
    };
    let result = halls::delete_user_hall(&st, target, &slug).await;
    let _ = tokio::fs::remove_file(custom_dir(&st).join(format!("u_{target}_{}.webp", basename(&slug)))).await;
    let code = ok_status(&result, 200, 404);
    jr(result, code)
}

async fn api_add_item(st: AppState, mut req: Req) -> Res {
    let uid = match require_login(&req) {
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let slug = decode(req.param("slug"));
    let body = req.post_value().await;
    let item_id = match body.get("item_id") {
        None | Some(Value::Null) => None,
        Some(v) => js_number(&js_str(Some(v)).unwrap_or_default()),
    }
    .filter(|n| *n != 0.0 && n.is_finite());
    let Some(item_id) = item_id else { return jr(json!({ "success": false, "msg": "Missing item_id" }), 400) };
    let Some(hall) = halls::get_user_hall(&st, uid, &slug).await else {
        return jr(json!({ "success": false, "msg": "Hall not found" }), 404);
    };
    let result = if item_id.fract() != 0.0 || item_id.abs() > i32::MAX as f64 {
        // the old query failed on non-integer ids
        json!({ "success": false, "message": "invalid input syntax for type integer" })
    } else {
        halls::add_item_to_user_hall(&st, hall.i64("id").unwrap_or(0), item_id as i64, uid).await
    };
    let code = ok_status(&result, 200, 409);
    jr(result, code)
}

/// `POST /api/v2/me/hall-items`: the item halls dialog as one plain form (no JavaScript needed).
/// Fields: `item_id`, `hall` (slug of one of the visitor's halls), `action` ("add" or "remove"),
/// and `new_name`: with no `hall` picked, "add" creates a hall of that name first.
async fn api_item_form(st: AppState, mut req: Req) -> Res {
    if !userhalls_enabled(&st) {
        return disabled();
    }
    let uid = match require_login(&req) {
        Ok(s) if s.is_anonymous_user() => return jr(json!({ "success": false, "msg": "Login required" }), 401),
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let body = req.post_value().await;
    let field = |k: &str| js_str(body.get(k)).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
    let Some(item_id) = field("item_id").and_then(|v| v.parse::<i64>().ok()).filter(|n| *n > 0 && *n <= i32::MAX as i64) else {
        return jr(json!({ "success": false, "msg": "Missing item_id" }), 400);
    };
    let remove = field("action").as_deref() == Some("remove");
    let mut slug = field("hall").unwrap_or_default();
    let mut created = None;
    if slug.is_empty() && !remove {
        if let Some(name) = field("new_name") {
            let new_slug = slugify(&name);
            if new_slug.is_empty() {
                return jr(json!({ "success": false, "msg": "Name is required" }), 400);
            }
            let r = halls::create_user_hall(&st, uid, &name, &new_slug, None).await;
            if !crate::config::truthy(&r["success"]) {
                return jr(r, 409);
            }
            created = Some(json!({ "slug": new_slug, "name": name }));
            slug = new_slug;
        }
    }
    if slug.is_empty() {
        return jr(json!({ "success": false, "msg": "Choose a hall or enter a new name." }), 400);
    }
    let Some(hall) = halls::get_user_hall(&st, uid, &slug).await else {
        return jr(json!({ "success": false, "msg": "Hall not found" }), 404);
    };
    let hall_id = hall.i64("id").unwrap_or(0);
    let mut result = if remove {
        halls::remove_item_from_user_hall(&st, hall_id, item_id).await
    } else {
        halls::add_item_to_user_hall(&st, hall_id, item_id, uid).await
    };
    if crate::config::truthy(&result["success"]) {
        result["msg"] = json!(if remove { "Removed from hall" } else { "Added to hall" });
        result["hall"] = json!({ "slug": slug, "name": hall.get("name").cloned().unwrap_or(Value::Null) });
        if let Some(c) = created {
            result["created"] = c;
        }
    }
    let code = ok_status(&result, 200, 409);
    jr(result, code)
}

async fn api_remove_item(st: AppState, req: Req) -> Res {
    let uid = match require_login(&req) {
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let slug = decode(req.param("slug"));
    let item_id: i64 = req.param("itemid").and_then(|i| i.parse().ok()).unwrap_or(0);
    let Some(hall) = halls::get_user_hall(&st, uid, &slug).await else {
        return jr(json!({ "success": false, "msg": "Hall not found" }), 404);
    };
    let result = halls::remove_item_from_user_hall(&st, hall.i64("id").unwrap_or(0), item_id).await;
    let code = ok_status(&result, 200, 500);
    jr(result, code)
}

async fn api_image_stub(_st: AppState, req: Req) -> Res {
    if let Err(r) = require_login(&req) {
        return Ok(r);
    }
    jr(json!({ "success": false, "msg": "Send as multipart/form-data" }), 400)
}

/// Clear the mosaic cache of a user hall (modes 0–2).
async fn clear_mosaic_cache(st: &AppState, user_id: i64, safe_slug: &str) {
    for m in 0..=2 {
        let h = crate::util::md5(&format!("uh_{user_id}_{safe_slug}_{m}"));
        let _ = tokio::fs::remove_file(cache_dir(st).join(format!("{h}.webp"))).await;
    }
}

async fn api_image_delete(st: AppState, req: Req) -> Res {
    let uid = match require_login(&req) {
        Ok(s) => s.id,
        Err(r) => return Ok(r),
    };
    let slug = decode(req.param("slug"));
    let Some(hall) = halls::get_user_hall(&st, uid, &slug).await else {
        return jr(json!({ "success": false, "msg": "Hall not found" }), 404);
    };
    let safe = basename(&slug);
    let _ = tokio::fs::remove_file(custom_dir(&st).join(format!("u_{uid}_{safe}.webp"))).await;
    clear_mosaic_cache(&st, uid, &safe).await;
    sql!("UPDATE user_halls SET custom_image = false WHERE id = ", hall.i64("id").unwrap_or(0) as i32).execute(&st.db).await?;
    jr(json!({ "success": true }), 200)
}

// ── Custom image upload (user_hall_image_handler.mjs) ───────────────────────

async fn image_upload(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    if !userhalls_enabled(&st) || cfg.is_false("websrv.enable_userhall_image_upload") {
        // bypass inactive → the normal stub route
        return api_image_stub(st, req).await;
    }
    let slug = basename(&decode(req.path.strip_prefix("/api/v2/me/halls/").and_then(|p| p.strip_suffix("/image"))));
    let Some(session) = req.session().cloned() else { return jr(json!({ "success": false, "msg": "Unauthorized" }), 403) };
    let token = req.header("x-csrf-token").or_else(|| req.query("csrf_token")).map(str::to_string);
    let expected = session.csrf_token.clone().unwrap_or_default();
    if token.as_deref().map(|t| expected.is_empty() || !crate::util::ct_eq(t, &expected)).unwrap_or(true) {
        return jr(json!({ "success": false, "msg": "Invalid CSRF token" }), 403);
    }
    let Some(hall) = sql!("SELECT id FROM user_halls WHERE user_id = ", session.id as i32, " AND slug = ", &slug, " LIMIT 1").fetch_optional(&st.db).await? else {
        return jr(json!({ "success": false, "msg": "Hall not found" }), 404);
    };
    match upload_inner(&st, &mut req, &session, &slug, hall.i64("id").unwrap_or(0)).await {
        Ok(r) => Ok(r),
        Err(e) => {
            tracing::error!("[USER_HALL_IMG] Upload error: {e:#}");
            jr(json!({ "success": false, "msg": e.to_string() }), 500)
        }
    }
}

const MAX_IMAGE: usize = 10 * 1024 * 1024;

async fn upload_inner(st: &AppState, req: &mut Req, session: &Session, slug: &str, hall_id: i64) -> anyhow::Result<Response> {
    let cfg = st.cfg();
    let ct = req.content_type().to_string();
    let Some(boundary) = boundary_of(&ct) else {
        return Ok(json_raw(&json!({ "success": false, "msg": "Invalid content type" }), 400));
    };
    let body = match req.body_bytes(MAX_IMAGE + 64 * 1024).await {
        Ok(b) => b,
        Err(_) => return Ok(json_raw(&json!({ "success": false, "msg": "File too large (10 MB max)" }), 400)),
    };
    let file = multipart_file(&body, &boundary, "file");
    if file.as_ref().map(|(_, d)| d.len() > MAX_IMAGE).unwrap_or(false) {
        return Ok(json_raw(&json!({ "success": false, "msg": "File too large (10 MB max)" }), 400));
    }
    let Some((mime, data)) = file.filter(|(_, d)| !d.is_empty()) else {
        return Ok(json_raw(&json!({ "success": false, "msg": "No file provided" }), 400));
    };
    if !["image/gif", "image/jpeg", "image/jpg", "image/png", "image/webp"].contains(&mime.as_str()) {
        return Ok(json_raw(&json!({ "success": false, "msg": format!("Invalid file type: {mime}") }), 400));
    }
    let custom = custom_dir(st);
    tokio::fs::create_dir_all(&custom).await?;
    let tmp_dir = if cfg.paths.tmp.as_os_str().is_empty() { std::path::PathBuf::from("/tmp") } else { cfg.paths.tmp.clone() };
    let tmp = tmp_dir.join(format!("uhall_{}_{slug}_tmp", session.id));
    let final_path = custom.join(format!("u_{}_{slug}.webp", session.id));
    tokio::fs::write(&tmp, &data).await?;
    let tmp_s = tmp.display().to_string();
    let actual = crate::proc::spawn("file", &["--mime-type", "-b", &tmp_s]).await.map(|o| o.stdout_str().trim().to_string());
    let actual = match actual {
        Ok(a) => a,
        Err(e) => {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(e);
        }
    };
    if !["image/gif", "image/jpeg", "image/png", "image/webp"].contains(&actual.as_str()) {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Ok(json_raw(&json!({ "success": false, "msg": format!("Invalid file type: {actual}") }), 400));
    }
    let out = final_path.display().to_string();
    let magick = crate::proc::spawn(
        "magick",
        &[&tmp_s, "-coalesce", "-resize", "600x300^", "-gravity", "center", "-background", "none", "-extent", "600x300", "-quality", "85", &out],
    )
    .await;
    let _ = tokio::fs::remove_file(&tmp).await;
    if let Err(e) = magick {
        tracing::error!("[USER_HALL_IMG] Magick error: {e}");
        return Ok(json_raw(&json!({ "success": false, "msg": "Failed to process image" }), 500));
    }
    clear_mosaic_cache(st, session.id, slug).await;
    sql!("UPDATE user_halls SET custom_image = true WHERE id = ", hall_id as i32).execute(&st.db).await?;
    tracing::info!("[USER_HALL_IMG] Uploaded custom image for: {slug} by user: {}", session.id);
    Ok(json_raw(&json!({ "success": true }), 200))
}

/// `boundary=...` of a multipart content type (quotes removed).
fn boundary_of(ct: &str) -> Option<String> {
    let raw = ct.split("boundary=").nth(1)?.split(';').next()?.trim();
    let b = raw.strip_prefix('"').and_then(|r| r.strip_suffix('"')).unwrap_or(raw);
    (!b.is_empty()).then(|| b.to_string())
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > hay.len() {
        return None;
    }
    hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

/// First file part named `name` → (lower-cased content type, data), like `parseMultipart(...)[name]`.
fn multipart_file(body: &[u8], boundary: &str, name: &str) -> Option<(String, Vec<u8>)> {
    let delim = format!("--{boundary}");
    let delim = delim.as_bytes();
    let mut start = find(body, delim, 0)? + delim.len();
    while let Some(next) = find(body, delim, start) {
        let seg_start = (start + 2).min(next);
        let seg_end = next.saturating_sub(2).max(seg_start);
        let seg = &body[seg_start..seg_end];
        start = next + delim.len();
        let Some(hend) = find(seg, b"\r\n\r\n", 0) else { continue };
        let headers = String::from_utf8_lossy(&seg[..hend]);
        let field = headers.split("name=\"").nth(1).and_then(|r| r.split('"').next()).unwrap_or("");
        if field.trim_end_matches("[]") != name || !headers.contains("filename") {
            continue;
        }
        let ctype = headers
            .lines()
            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-type:").map(|v| v.trim().to_string()))
            .unwrap_or_else(|| "application/octet-stream".into());
        return Some((ctype, seg[hend + 4..].to_vec()));
    }
    None
}

/// `String(v)` for template-bound values.
fn jsv(v: &Value) -> String {
    js_str(Some(v)).unwrap_or_default()
}
