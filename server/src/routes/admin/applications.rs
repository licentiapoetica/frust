//! Account applications: guests who registered with a message instead of an invite token
//! (routes/register.rs). Approving creates the account, declining only marks the application; the
//! applicant sees the state when logging in, and gets a mail on approval when an address was given
//! (no mail when declined).
//!
//! The page works as plain forms (`back`, see crate::nojs), so it needs no script.

use super::{body_id, body_truthy, err_msg};
use crate::db::{Row, RowExt};
use crate::http::{Guard, Req, Res, Router, html, json_resp};
use crate::routes::register::{NewUser, insert_user};
use crate::sql;
use crate::state::AppState;
use serde_json::{Value, json};

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/applications\/?$", Guard::Auth, page);
    r.post_g(r"^\/api\/v2\/admin\/applications\/approve\/?$", Guard::Auth, approve);
    r.post_g(r"^\/api\/v2\/admin\/applications\/deny\/?$", Guard::Auth, deny);
}

fn fail(msg: &str) -> Res {
    Ok(json_resp(&json!({ "success": false, "msg": msg }), 200))
}

async fn page(st: AppState, req: Req) -> Res {
    // Never the password hash or the passkey
    let list = sql!(
        r#"SELECT a.id, a.username, a.email, a.message, a.status, a.created_at, a.reviewed_at, a.user_id,
                  (a.passkey IS NOT NULL) AS with_passkey, u_reviewer.user AS reviewed_by_name
           FROM account_applications a
           LEFT JOIN "user" u_reviewer ON u_reviewer.id = a.reviewed_by
           ORDER BY (a.status = 'pending') DESC, a.created_at DESC
           LIMIT 200"#
    )
    .fetch_all(&st.db)
    .await?;
    let pending = list.iter().filter(|a| a.str("status") == Some("pending")).count();
    let cfg = st.cfg();
    let data = json!({
        "session": req.session_value(),
        "applications": list.into_iter().map(Value::Object).collect::<Vec<_>>(),
        "pending_count": pending,
        "applications_on": st.settings.registration_applications(&cfg),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/applications", data)))
}

fn posted_id(body: &Value) -> Option<i32> {
    body_truthy(body, "id").then(|| body_id(body, "id")).flatten().map(|id| id as i32)
}

/// Tells an approved applicant that the account exists. Declined ones get no mail.
async fn mail_approved(st: &AppState, app: &Row) {
    let cfg = st.cfg();
    let Some(to) = app.str("email").filter(|e| !e.is_empty()) else { return };
    if !cfg.truthy("smtp.host") {
        return;
    }
    let name = app.string("username");
    let subject = "Your account application was approved";
    let with = if app.val("passkey").is_null() { "your username and password" } else { "your username and password or your passkey" };
    let body = format!("Hello {name},\n\nyour application was approved. You can now log in with {with}:\n\n{}/login\n", cfg.url_full());
    if let Err(e) = crate::smtp::send_mail(&cfg, crate::smtp::Mail { to, subject, body: &body }).await {
        tracing::error!("[SMTP] Send failed: {e}");
    }
}

enum Approved {
    Created(Row),
    Gone,
    NameTaken,
}

async fn approve(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = posted_id(&body) else { return fail("Missing application id") };
    let reviewer = req.session().map(|s| s.id).unwrap_or(0) as i32;

    let r: anyhow::Result<Approved> = async {
        let mut tx = st.db.begin().await?;
        let app = sql!("SELECT * FROM account_applications WHERE id = ", id, " AND status = 'pending' FOR UPDATE").fetch_optional(&mut *tx).await?;
        let Some(app) = app else { return Ok(Approved::Gone) };
        let (username, email) = (app.string("username"), app.str("email").filter(|e| !e.is_empty()).map(str::to_string));
        // Registration refuses names of pending applications, so this only hits accounts made
        // another way meanwhile (the CLI, a rename)
        let mut q = sql!(r#"SELECT 1 AS taken FROM "user" WHERE "login" = "#, username.to_lowercase(), r#" OR "user" = "#, username.clone());
        if let Some(e) = &email {
            q.push(r#" OR ("email" IS NOT NULL AND lower("email") = lower("#).bind(e.clone()).push("))");
        }
        if q.push(" LIMIT 1").fetch_optional(&mut *tx).await?.is_some() {
            return Ok(Approved::NameTaken);
        }
        // Applied with a passkey besides the password: the account gets it too
        let passkey = match app.val("passkey") {
            Value::Null => None,
            v => Some(serde_json::from_value::<crate::webauthn::NewPasskey>(v.clone())?),
        };
        let hash = app.string("password");
        let new = NewUser { username: &username, hash: &hash, email: email.as_deref(), activated: true, activation_token: None };
        let user_id = insert_user(&st, &mut tx, new).await?;
        if let Some(p) = &passkey {
            if !crate::webauthn::store(&mut *tx, user_id as i64, p, "Passkey").await? {
                anyhow::bail!("its passkey is registered to another account");
            }
        }
        // The account has the password hash (or the passkey) now
        sql!(
            "UPDATE account_applications SET status = 'approved', password = '', passkey = NULL, user_id = ", user_id, ", reviewed_by = ", reviewer,
            ", reviewed_at = NOW() WHERE id = ", id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Approved::Created(app))
    }
    .await;

    match r {
        Ok(Approved::Created(app)) => {
            let name = app.string("username");
            tracing::info!("[APPLICATIONS] #{id} approved, account {name} created");
            mail_approved(&st, &app).await;
            Ok(json_resp(&json!({ "success": true, "msg": format!("Approved: the account {name} exists now.") }), 200))
        }
        Ok(Approved::Gone) => fail("Application not found or already answered"),
        Ok(Approved::NameTaken) => fail("The username or email is used by an account by now. Decline the application."),
        Err(e) => {
            tracing::error!("[APPLICATIONS] Approving #{id} failed: {e:#}");
            fail(&format!("Failed to approve the application: {}", err_msg(&e)))
        }
    }
}

async fn deny(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let Some(id) = posted_id(&body) else { return fail("Missing application id") };
    let reviewer = req.session().map(|s| s.id).unwrap_or(0) as i32;
    let app = sql!(
        "UPDATE account_applications SET status = 'denied', reviewed_by = ", reviewer, ", reviewed_at = NOW() WHERE id = ", id,
        " AND status = 'pending' RETURNING username"
    )
    .fetch_optional(&st.db)
    .await;
    match app {
        Ok(Some(app)) => {
            tracing::info!("[APPLICATIONS] #{id} declined ({})", app.string("username"));
            Ok(json_resp(&json!({ "success": true, "msg": "Application declined." }), 200))
        }
        Ok(None) => fail("Application not found or already answered"),
        Err(e) => {
            tracing::error!("[APPLICATIONS] Declining #{id} failed: {e:#}");
            fail(&format!("Failed to decline the application: {}", err_msg(&e)))
        }
    }
}
