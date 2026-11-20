//! The upload page (src/inc/routes/upload.mjs).

use crate::perms::{self, P};
use crate::http::{Guard, Req, Res, Router, check_guard, html, redirect};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get(r"^\/upload$", upload_page);
}

async fn upload_page(st: AppState, mut req: Req) -> Res {
    let cfg = st.cfg();
    // uploadAuth: anonymous sessions only with the upload permission, others need lib.userauth
    let Some(session) = req.session().cloned() else { return Ok(redirect("/login")) };
    let anon = crate::settings::is_anon_session(Some(&session));
    if !anon {
        if let Some(r) = check_guard(&st, &mut req, Guard::UserAuth).await {
            return Ok(r);
        }
    }
    if !perms::can(&st, Some(&session), P::Upload) {
        if anon {
            return Ok(redirect("/login"));
        }
        return Ok(crate::routes::halls::error_page(&st, &req, &perms::denied_msg(P::Upload), 403));
    }

    let max = st.settings.upload_limit(&cfg, perms::can(&st, Some(&session), P::UploadLarge));
    // Uploads left in the 12 h window (admins/mods are exempt)
    let uploads_remaining = if perms::can(&st, Some(&session), P::UploadUnlimited) {
        Value::Null
    } else {
        let since = crate::util::now_secs() - 12 * 3600;
        let count: i64 = sql!("SELECT count(*) FROM items WHERE username = ", session.user.clone(), " AND stamp > ", since, " AND is_deleted = false")
            .fetch_scalar(&st.db)
            .await?
            .unwrap_or(0);
        json!((cfg.int_or("main.upload_limit", 0) - count).max(0))
    };

    req.set_header("cache-control", "no-store, no-cache, must-revalidate, proxy-revalidate");
    req.set_header("pragma", "no-cache");
    req.set_header("expires", "0");
    req.set_header("surrogate-control", "no-store");

    let mimes = cfg.mimes();
    let data = json!({
        "tmp": null,
        "session": if session.user.is_empty() { Value::Bool(false) } else { req.session_value() },
        "max_file_size": crate::util::format_size_f(max),
        "min_tags": st.settings.min_tags(),
        "uploads_remaining": uploads_remaining,
        // `allowed_mimes` comes from the template globals (MIME types plus `.ext`): in the old
        // renderer the global won over the route's extension-less list.
        "mimes_json": serde_json::to_string(&mimes).unwrap_or_default(),
        "web_url_upload": cfg.truthy("websrv.web_url_upload") && perms::can(&st, Some(&session), P::UploadUrl),
        "page_meta": {
            "title": "upload",
            "description": "Upload content",
            "url": format!("https://{}/upload", cfg.domain()),
        },
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "upload", data)))
}
