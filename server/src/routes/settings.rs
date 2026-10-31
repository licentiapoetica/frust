//! Settings page and data export (port of src/inc/routes/settings.mjs).

use crate::db::RowExt;
use crate::http::{Group, Req, Res, Router, html, html_code, redirect, reply};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    // router.group(/^\/settings/, g => { g.get(/$/, …); g.get('/export-data', …) })
    let mut g = Group::new(r, r"^\/settings");
    g.get("$", page);
    g.get(r"\/export-data", export_data);
}

async fn page(st: AppState, req: Req) -> Res {
    let Some(s) = req.session().cloned() else { return Ok(redirect("/login")) };
    let cfg = st.cfg();
    let uid = s.id as i32;
    // The old route also loaded `select * from user_sessions` (session hashes and CSRF tokens of
    // all the user's devices) into the template data; the template never used it.
    let excluded = sql!(
        "select t.id, t.tag, t.normalized from unnest((select excluded_tags from user_options where user_id = ", uid,
        ")) as et(id) join tags t on t.id = et.id"
    )
    .fetch_all(&st.db)
    .await?;
    let opts = sql!("select avatar_file, banner_file, banner_position, banner_size from user_options where user_id = ", uid)
        .fetch_optional(&st.db)
        .await?
        .unwrap_or_default();
    let user = sql!(r#"select email, created_at, (password = '!') as password_disabled from "user" where id = "#, uid)
        .fetch_optional(&st.db)
        .await?
        .unwrap_or_default();
    let or_null = |k: &str| opts.str(k).filter(|v| !v.is_empty()).map(|v| json!(v)).unwrap_or(Value::Null);
    let or_default = |k: &str, d: &str| opts.str(k).filter(|v| !v.is_empty()).unwrap_or(d).to_string();
    let anon = s.is_anon;
    let session = if s.user.is_empty() { Value::Bool(false) } else { req.session_value() };
    let data = json!({
        "tmp": null,
        "excluded_tags": excluded.into_iter().map(Value::Object).collect::<Vec<_>>(),
        "avatar_file": or_null("avatar_file"),
        "banner_file": or_null("banner_file"),
        "banner_position": or_default("banner_position", "center"),
        "banner_size": or_default("banner_size", "cover"),
        "email": user.str("email").unwrap_or(""),
        "joined": if user.bool("created_at") { user.val("created_at").clone() } else { Value::Null },
        // Passkey-only account: password login switched off (password = '!')
        "password_disabled": user.bool("password_disabled"),
        "user_banner_enabled": cfg.enabled("websrv.user_banner_enabled"),
        "enable_swf": cfg.get("enable_swf"),
        "enable_data_export": !anon && cfg.truthy("websrv.enable_data_export"),
        "enable_user_api_keys": !anon && cfg.enabled("websrv.enable_user_api_keys"),
        "enable_user_invites": !anon && cfg.enabled("websrv.enable_user_invites"),
        "site_domain": cfg.get("main.url.domain"),
        "session": session,
        "page_meta": {
            "title": "settings",
            "description": "User settings",
            "url": format!("https://{}/settings", cfg.str_or("main.url.domain", "")),
        },
    });
    let body = crate::tpl::render(&st, Some(&req), "settings", data);
    let mut r = html(body);
    let h = r.headers_mut();
    h.insert("cache-control", axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
    h.insert("pragma", axum::http::HeaderValue::from_static("no-cache"));
    h.insert("expires", axum::http::HeaderValue::from_static("0"));
    h.insert("surrogate-control", axum::http::HeaderValue::from_static("no-store"));
    Ok(r)
}

async fn export_data(st: AppState, req: Req) -> Res {
    let Some(s) = req.session().cloned() else { return Ok(redirect("/login")) };
    if s.is_anon {
        return Ok(html_code("Export requires a registered account", 403));
    }
    if !st.cfg().truthy("websrv.enable_data_export") {
        return Ok(html_code("Export disabled", 403));
    }
    let uploads = sql!("select id, dest, mime from items where username = ", s.user.clone(), " and active = true order by id desc").fetch_all(&st.db).await?;
    let favorites = sql!(
        "select i.id, i.dest, i.mime from items i join favorites f on f.item_id = i.id where f.user_id = ", s.id as i32,
        " and i.active = true order by f.item_id desc"
    )
    .fetch_all(&st.db)
    .await?;
    let v = json!({
        "uploads": uploads.into_iter().map(Value::Object).collect::<Vec<_>>(),
        "favorites": favorites.into_iter().map(Value::Object).collect::<Vec<_>>(),
    });
    Ok(reply(200, "application/json", serde_json::to_string(&v)?))
}
