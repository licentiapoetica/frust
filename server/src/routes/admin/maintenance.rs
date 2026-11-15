//! Maintenance mode (`/admin/maintenance`): switch it on/off and set up its page (text, video,
//! glitch effect per element), see crate::maintenance.
//!
//! One plain multipart form that works without JavaScript: the save is a `pre` route (CSRF from
//! the `X-CSRF-Token` header or the form's `csrf_token` field) answering with a redirect to `back`.

use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, html};
use crate::maintenance::{self as mt, Glitch, Maintenance};
use crate::routes::admin_media::{admin_session, csrf_header_ok, read_form, send};
use crate::sql;
use crate::state::AppState;
use axum::http::Method;
use serde_json::{Value, json};
use std::path::Path;

/// Largest video upload.
const MAX_UPLOAD: u64 = 256 * 1024 * 1024;

/// Accepted video / image types (by content) and the extension they are stored with.
const TYPES: [(&str, &str); 6] =
    [("video/mp4", "mp4"), ("video/webm", "webm"), ("image/gif", "gif"), ("image/webp", "webp"), ("image/png", "png"), ("image/jpeg", "jpg")];

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/maintenance\/?$", Guard::Auth, page);
    r.get_g(r"^\/admin\/maintenance\/preview\/?$", Guard::Auth, preview);
    r.pre(Method::POST, r"^\/admin\/maintenance\/save$", None, save);
}

fn fail(code: u16, m: &str) -> Res {
    send(json!({ "success": false, "msg": m }), code)
}

async fn page(st: AppState, req: Req) -> Res {
    let m = st.settings.get().maintenance;
    let elements: Vec<Value> = mt::ELEMENTS
        .iter()
        .map(|(key, label)| {
            let g = m.glitch(key);
            json!({ "key": key, "label": label, "on": g.on, "strength": g.strength, "mode": g.mode })
        })
        .collect();
    let modes: Vec<Value> = mt::MODES.iter().map(|(key, label)| json!({ "key": key, "label": label })).collect();
    let layouts: Vec<Value> = mt::LAYOUTS.iter().map(|(key, label, about)| json!({ "key": key, "label": label, "about": about })).collect();
    let data = json!({
        "session": req.session_value(),
        "mt": {
            "enabled": m.enabled,
            "layout": m.layout(),
            "layouts": layouts,
            "title": m.title,
            "text": m.text,
            "eta": m.eta,
            "video_url": m.video_url,
            "video_from": m.video_from,
            "video_is_image": m.video_is_image(),
            "video_controls": m.video_controls,
            "elements": elements,
            "strengths": mt::STRENGTHS,
            "modes": modes,
            "max_title": mt::MAX_TITLE,
            "max_text": mt::MAX_TEXT,
            "max_lines": mt::MAX_LINES,
            "max_upload_mb": MAX_UPLOAD / (1024 * 1024),
        },
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/maintenance", data)))
}

/// The page as visitors get it (with the saved setup, whether maintenance is on or not);
/// `?layout=<key>` shows it in another layout, to compare them.
async fn preview(st: AppState, req: Req) -> Res {
    let mut m = st.settings.get().maintenance;
    if let Some(l) = req.query("layout") {
        m.layout = mt::layout_key(l).to_string();
    }
    Ok(html(mt::render_page(&st, &req, &m, true)))
}

async fn save(st: AppState, mut req: Req) -> Res {
    let Some(session) = admin_session(&req) else { return fail(401, "Unauthorized") };
    let ajax = crate::auth::is_ajax(&req);
    let header_ok = csrf_header_ok(&req, &session);
    let cfg = st.cfg();
    let form = match read_form(&mut req, &cfg, MAX_UPLOAD, "msg").await {
        Ok(f) => f,
        Err(r) => return Ok(r),
    };
    let field_ok = match (form.text("csrf_token"), session.csrf_token.as_deref().filter(|t| !t.is_empty())) {
        (_, None) => true,
        (Some(t), Some(expected)) => crate::util::ct_eq(t, expected),
        _ => false,
    };
    if !header_ok && !field_ok {
        tracing::warn!("[CSRF] Blocked maintenance save for user {}. Invalid token.", session.user);
        return fail(403, "Invalid CSRF token");
    }
    let back = form.text("back").and_then(crate::nojs::safe_back);
    let resp = apply(&st, &session, &form).await;
    drop(form);
    crate::routes::profile_media::finish_form(&st, ajax, back, resp).await
}

/// At most `max` characters of a form field, trimmed.
fn field(form: &crate::media::multipart::Form, name: &str, max: usize) -> String {
    form.text(name).unwrap_or("").trim().chars().take(max).collect()
}

async fn apply(st: &AppState, s: &crate::session::Session, form: &crate::media::multipart::Form) -> Res {
    let old = st.settings.get().maintenance;
    let mut m = Maintenance {
        enabled: form.has("enabled"),
        layout: mt::layout_key(form.text("layout").unwrap_or("")).to_string(),
        title: field(form, "title", mt::MAX_TITLE),
        // line ends as the browser sent them (CRLF) count as one
        text: form.text("text").unwrap_or("").replace("\r\n", "\n").trim_end().chars().take(mt::MAX_TEXT).collect(),
        eta: field(form, "eta", mt::MAX_TITLE),
        video_controls: form.has("video_controls"),
        ..old.clone()
    };
    if m.enabled && !old.enabled {
        m.since = chrono::Utc::now().timestamp();
    }
    m.glitch = mt::ELEMENTS
        .iter()
        .map(|(key, _)| {
            let g = Glitch { on: form.has(&format!("glitch_{key}")), strength: field(form, &format!("strength_{key}"), 16), mode: field(form, &format!("mode_{key}"), 16) };
            (key.to_string(), g.clean())
        })
        .collect();

    // Video: a new upload wins over an item id, which wins over "remove"
    let upload = form.files("file").into_iter().find(|f| f.size > 0);
    let item_id = form.text("item_id").map(str::trim).filter(|v| !v.is_empty());
    let new_video = if let Some(f) = upload {
        let name: String = f.filename.chars().take(80).collect();
        match store_video(st, &f.path).await {
            Ok(url) => Some((url, format!("upload: {name}"))),
            Err(why) => return fail(400, &why),
        }
    } else if let Some(raw_id) = item_id {
        let Some(id) = raw_id.parse::<i32>().ok().filter(|i| *i > 0) else { return fail(400, "Item ID: not a number") };
        let row = sql!("SELECT dest FROM items WHERE id = ", id, " AND NOT COALESCE(is_deleted, false) AND NOT COALESCE(is_purged, false) LIMIT 1")
            .fetch_optional(&st.db)
            .await?;
        let Some(dest) = row.map(|r| r.string("dest")).filter(|d| crate::media::is_plain_name(d)) else {
            return fail(400, &format!("Item {id} not found"));
        };
        match store_video(st, &st.cfg().paths.b.join(&dest)).await {
            Ok(url) => Some((url, format!("item {id}"))),
            Err(why) => return fail(400, &format!("Item {id}: {why}")),
        }
    } else if form.has("video_remove") {
        Some((String::new(), String::new()))
    } else {
        None
    };
    if let Some((url, from)) = new_video.clone() {
        m.video_url = url;
        m.video_from = from;
    }

    let stored = match serde_json::to_string(&m) {
        Ok(v) => v,
        Err(e) => return fail(500, &format!("Save failed: {e}")),
    };
    if let Err(e) = crate::settings::save(st, mt::KEY, &stored).await {
        tracing::error!("[MAINTENANCE] save failed: {e:#}");
        // keep the old video, drop the one just stored
        if let Some((url, _)) = &new_video {
            delete_video(st, url).await;
        }
        return fail(500, "Save failed");
    }
    if new_video.is_some() && old.video_url != m.video_url {
        delete_video(st, &old.video_url).await;
    }
    let (enabled, changed) = (m.enabled, m.enabled != old.enabled);
    st.settings.update(|set| set.maintenance = m.clone());
    crate::social::helpers::audit_log(
        st,
        s.id,
        if !changed { "update_maintenance" } else if enabled { "maintenance_on" } else { "maintenance_off" },
        "system",
        "0",
        Some(json!({ "enabled": enabled, "title": m.title, "video": m.video_from })),
    )
    .await;
    // Other processes reload the setup; open tabs of non-admins reload when it was switched on
    if let Err(e) = sql!("SELECT pg_notify('maintenance', ", if enabled { "1" } else { "0" }, ")").execute(&st.db).await {
        tracing::error!("[MAINTENANCE] notify failed: {e}");
    }
    let msg = match (changed, enabled) {
        (true, true) => "Maintenance mode is ON: only admins can use the site",
        (true, false) => "Maintenance mode is off",
        (false, true) => "Saved (maintenance mode is on)",
        (false, false) => "Saved",
    };
    send(json!({ "success": true, "enabled": enabled, "video_url": m.video_url, "msg": msg }), 200)
}

/// Copy a video / image into `public/s/maintenance/` under a fresh name; `Err` is the message
/// for the admin.
async fn store_video(st: &AppState, src: &Path) -> Result<String, String> {
    let mime = crate::media::mime::detect(src).await.map_err(|e| {
        tracing::warn!("[MAINTENANCE] {}: {e}", src.display());
        "file not found or unreadable".to_string()
    })?;
    let Some((_, ext)) = TYPES.iter().find(|(m, _)| *m == mime) else {
        return Err(format!("unsupported type {mime} (mp4, webm, gif, webp, png or jpg)"));
    };
    let dir = st.cfg().paths.s.join("maintenance");
    let name = format!("video-{}.{ext}", crate::util::random_hex(6));
    let dest = dir.join(&name);
    let copied = async {
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::copy(src, &dest).await
    }
    .await;
    if let Err(e) = copied {
        tracing::error!("[MAINTENANCE] store {name}: {e}");
        let _ = tokio::fs::remove_file(&dest).await;
        return Err("could not store the file".into());
    }
    crate::media::make_public(&dest).await;
    Ok(format!("/s/maintenance/{name}"))
}

/// Delete one of our stored videos (`/s/maintenance/<name>`), anything else is left alone.
async fn delete_video(st: &AppState, url: &str) {
    if !mt::own_file(url) {
        return;
    }
    let name = url.trim_start_matches("/s/maintenance/");
    if let Err(e) = tokio::fs::remove_file(st.cfg().paths.s.join("maintenance").join(name)).await {
        tracing::warn!("[MAINTENANCE] delete {name}: {e}");
    }
}
