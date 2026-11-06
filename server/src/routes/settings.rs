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
    g.post("$", save);
    g.get(r"\/export-data", export_data);
}

async fn page(st: AppState, req: Req) -> Res {
    let Some(s) = req.session().cloned() else { return Ok(guest_page(&st, &req)) };
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
    Ok(no_store(html(crate::tpl::render(&st, Some(&req), "settings", data))))
}

/// Visitors without a session get the preferences that live in this browser (theme and language
/// cookies, localStorage toggles); everything stored per account stays hidden (`guest` in settings.html).
fn guest_page(st: &AppState, req: &Req) -> axum::response::Response {
    let cfg = st.cfg();
    let data = json!({
        "guest": true,
        "session": false,
        "excluded_tags": [],
        "enable_swf": cfg.get("enable_swf"),
        "enable_data_export": false,
        "enable_user_api_keys": false,
        "enable_user_invites": false,
        "site_domain": cfg.get("main.url.domain"),
        "page_meta": {
            "title": "settings",
            "description": "Settings",
            "url": format!("https://{}/settings", cfg.str_or("main.url.domain", "")),
        },
    });
    no_store(html(crate::tpl::render(st, Some(req), "settings", data)))
}

// ── Saving without JavaScript ───────────────────────────────────────────────
// With JavaScript every control saves itself (settings.js → /api/v2/settings/*, cookies,
// localStorage). Without it the preferences are one form (settings.html) posted here: only the
// fields it carried are saved, with the same rules as those endpoints. Checkboxes come as a hidden
// "0" plus the box's "1", joined to "0,1" when ticked (http::parse_body), so an unticked box still
// says it was there.

/// Account preferences that are plain booleans in user_options (column = form field)
const BOOL_PREFS: [&str; 16] = [
    "show_background",
    "show_motd",
    "use_new_layout",
    "use_alternative_infobox",
    "use_alternative_steuerung",
    "hide_koepfe",
    "disable_autoplay",
    "disable_swiping",
    "quote_emojis",
    "embed_youtube_in_comments",
    "favorites_private",
    "hide_fav_badge",
    "receive_system_notifications",
    "receive_user_notifications",
    "do_not_disturb",
    "ruffle_background",
];

fn form_bool(v: &str) -> bool {
    v.split(',').any(|p| p == "1")
}

async fn save(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    let form = req.post_value().await;
    let field = |k: &str| form.get(k).and_then(Value::as_str).map(str::to_string);
    let long = || crate::http::CookieExpiry::MaxAge(360 * 24 * 3600);

    // Theme: a cookie for everyone (theme.js does the same)
    if let Some(theme) = field("theme") {
        let themes = cfg.get("websrv.themes");
        let known = match themes {
            Value::Array(a) => a.iter().any(|t| t.as_str() == Some(theme.as_str())),
            _ => ["amoled", "atmos", "f0ck", "f0ck95d", "iced", "orange", "p1nk", "4d"].contains(&theme.as_str()),
        };
        if known {
            req.set_cookie(format!("theme={}; {}", crate::util::encode_uri_component(&theme), crate::http::cookie_options(&cfg, long(), false)));
        }
    }

    // Language: cookie for everyone, and the account's choice
    let language = if cfg.is_false("websrv.allow_language_change") {
        None
    } else {
        field("language").map(|l| if crate::i18n::is_selectable(&cfg, &l) { l } else { String::new() })
    };
    if let Some(l) = &language {
        let cookie = if l.is_empty() {
            format!("language=; {}", crate::http::cookie_options(&cfg, crate::http::CookieExpiry::Expires(crate::http::EXPIRED), false))
        } else {
            format!("language={l}; {}", crate::http::cookie_options(&cfg, long(), false))
        };
        req.set_cookie(cookie);
    }

    // Browser-side preferences (blur, ...): only the ones on the form change
    let mut prefs: Vec<&str> = crate::nojs::prefs(&req);
    for (key, _) in crate::nojs::PREF_CLASSES {
        if let Some(v) = field(key) {
            prefs.retain(|p| *p != key);
            if form_bool(&v) {
                prefs.push(key);
            }
        }
    }
    req.set_cookie(format!("{}={}; {}", crate::nojs::PREFS_COOKIE, prefs.join("."), crate::http::cookie_options(&cfg, long(), true)));

    if let Some(s) = req.session().cloned() {
        let uid = s.id as i32;
        let mut q = sql!("update user_options set user_id = user_id");
        for col in BOOL_PREFS {
            if let Some(v) = field(col) {
                // `col` is one of the literals above
                q.push(", ").push(col).push(" = ").bind(form_bool(&v));
            }
        }
        if let Some(font) = field("font") {
            let font = Some(font).filter(|f| !f.is_empty() && crate::fonts::exists(&cfg, f));
            // The weight select (families with several weights): kept when the family has it
            let weight = font.as_deref().and_then(|f| crate::fonts::weight_for(&cfg, f, field("font_weight").and_then(|w| w.trim().parse().ok())));
            q.push(", font = ").bind(font);
            q.push(", font_weight = ").bind(weight.map(i32::from));
        }
        if let Some(size) = field("font_size") {
            let size = crate::fonts::size_for(size.trim().parse().ok());
            q.push(", font_size = ").bind(size.map(i32::from));
        }
        if let Some(l) = &language {
            q.push(", language = ").bind(Some(l.clone()).filter(|l| !l.is_empty()));
        }
        if let Some(mode) = field("comment_display_mode").and_then(|m| m.parse::<i32>().ok()).filter(|m| *m == 0 || *m == 1) {
            if s.force_comment_display_mode.unwrap_or(0) == 0 {
                q.push(", comment_display_mode = ").bind(mode);
            }
        }
        let vis_allowed = cfg.get("allow_user_upload_visibility") != &Value::Bool(false) && cfg.get("websrv.allow_user_upload_visibility") != &Value::Bool(false);
        if let Some(vis) = field("default_upload_visibility").and_then(|v| v.parse::<i16>().ok()).filter(|v| (0..=2).contains(v)) {
            if vis_allowed && !s.is_anon {
                q.push(", default_upload_visibility = ").bind(vis);
            }
        }
        q.push(" where user_id = ").bind(uid);
        q.execute(&st.db).await?;
        if let Some(c) = req.cookie("session") {
            st.invalidate_session_cache(&crate::util::sha256(c));
        }
    }

    let mut r = axum::response::Response::new(axum::body::Body::empty());
    *r.status_mut() = axum::http::StatusCode::SEE_OTHER;
    r.headers_mut().insert(axum::http::header::LOCATION, axum::http::HeaderValue::from_static("/settings?saved=1#preferences"));
    Ok(r)
}

fn no_store(mut r: axum::response::Response) -> axum::response::Response {
    let h = r.headers_mut();
    h.insert("cache-control", axum::http::HeaderValue::from_static("no-store, no-cache, must-revalidate, proxy-revalidate"));
    h.insert("pragma", axum::http::HeaderValue::from_static("no-cache"));
    h.insert("expires", axum::http::HeaderValue::from_static("0"));
    h.insert("surrogate-control", axum::http::HeaderValue::from_static("no-store"));
    r
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

#[cfg(test)]
mod tests {
    #[test]
    fn checkbox_values() {
        assert!(super::form_bool("0,1"));
        assert!(!super::form_bool("0"));
        assert!(super::form_bool("1"));
    }
}
