//! Outgoing mail (port of src/lib/smtp.mjs). Same config keys under `smtp:` —
//! `host`, `port`, `secure` (implicit TLS), `user`, `password`, `from`.

use crate::config::Config;

pub struct Mail<'a> {
    pub to: &'a str,
    pub subject: &'a str,
    pub body: &'a str,
}

/// Send a plain-text UTF-8 mail.
///
/// TODO(port): `lettre` is not in the offline cargo cache yet; until it is, sending fails.
pub async fn send_mail(_cfg: &Config, _mail: Mail<'_>) -> anyhow::Result<()> {
    anyhow::bail!("SMTP support is not available on this server build")
}
