//! Outgoing mail (port of src/lib/smtp.mjs) via `lettre`. Same config keys under `smtp:` —
//! `host`, `port`, `secure` (implicit TLS), `user`, `password`, `from`.
//!
//! Without `secure`, STARTTLS is used when the server offers it (like the old client). lettre also
//! takes care of header encoding and dot-stuffing, which the hand-written client did not
//! (recipient/subject were written into the SMTP stream unescaped).

use crate::config::Config;
use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

pub struct Mail<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
}

/// Send a plain-text UTF-8 mail.
pub async fn send_mail(cfg: &Config, mail: Mail<'_>) -> anyhow::Result<()> {
    let host = cfg.str("smtp.host").filter(|h| !h.is_empty()).ok_or_else(|| anyhow::anyhow!("smtp.host is not set"))?;
    let secure = cfg.truthy("smtp.secure");
    let port = cfg.int("smtp.port").and_then(|p| u16::try_from(p).ok()).unwrap_or(if secure { 465 } else { 25 });

    let from: Mailbox = cfg.str_or("smtp.from", "").parse()?;
    let to: Mailbox = mail.to.parse()?;
    let msg = Message::builder()
        .from(from)
        .to(to)
        .subject(mail.subject)
        .header(ContentType::TEXT_PLAIN)
        .body(mail.body.to_string())?;

    let tls = TlsParameters::new(host.to_string())?;
    let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(port)
        .tls(if secure { Tls::Wrapper(tls) } else { Tls::Opportunistic(tls) })
        .timeout(Some(std::time::Duration::from_secs(30)));
    let user = cfg.str_or("smtp.user", "");
    let password = cfg.str_or("smtp.password", "");
    if !user.is_empty() && !password.is_empty() {
        builder = builder.credentials(Credentials::new(user.to_string(), password.to_string()));
    }
    builder.build().send(msg).await?;
    Ok(())
}
