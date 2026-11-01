//! Passkeys (WebAuthn) on top of `webauthn-rs`.
//!
//! Replaces the hand-rolled ES256 verifier of the Node server (src/inc/webauthn.mjs). Credentials are
//! serialized `webauthn_rs::prelude::Passkey` values in `webauthn_credentials` (see
//! migrations/rust_webauthn.sql); the old `passkey_credentials` table is not used.
//!
//! Ceremony state stays on the server: `start_*` stores it in memory keyed by the challenge it sent
//! (5-minute expiry, single use, bounded), `take` removes it again. The HTTP API keeps the old flat
//! JSON shapes (`options.challenge`, `options.rp`, …; finish bodies with `challenge`, `credentialId`,
//! `clientDataJSON`, …); logins additionally send `userHandle`, which is checked when present.
//!
//! rpId: the configured domain (`websrv.passkey_rp_id`, else `main.url.domain`); requests to
//! localhost use `localhost` (any port) for development. The old code took the rpId from the Host
//! header for every request.

use crate::config::Config;
use crate::db::{Row, RowExt};
use crate::http::Req;
use crate::sql;
use crate::state::AppState;
use dashmap::DashMap;
use serde_json::Value;
use std::time::{Duration, Instant};
use uuid::Uuid;

const CEREMONY_TTL: Duration = Duration::from_secs(5 * 60);
/// Upper bound on outstanding ceremonies (the begin endpoints are unauthenticated).
const MAX_PENDING: usize = 50_000;

/// A started registration or authentication, waiting for the browser's answer.
pub struct Pending {
    /// `anon-register`, `anon-auth`, `anon-add`, `user-register`, `user-login`
    pub kind: String,
    pub user_id: Option<i64>,
    pub user_handle: Option<Uuid>,
    rp_id: String,
    state: backend::State,
}

static PENDING: once_cell::sync::Lazy<DashMap<String, (Instant, Pending)>> = once_cell::sync::Lazy::new(DashMap::new);

fn prune() {
    let now = Instant::now();
    PENDING.retain(|_, (exp, _)| *exp > now);
}

fn remember(challenge: String, p: Pending) -> anyhow::Result<()> {
    if PENDING.len() >= MAX_PENDING {
        prune();
        if PENDING.len() >= MAX_PENDING {
            anyhow::bail!("Too many pending passkey requests, try again later");
        }
    }
    PENDING.insert(challenge, (Instant::now() + CEREMONY_TTL, p));
    Ok(())
}

/// Remove and return the ceremony started with `challenge` (single use).
pub fn take(challenge: &str) -> Result<Pending, &'static str> {
    let Some((_, (exp, p))) = PENDING.remove(challenge) else { return Err("Challenge not found or expired") };
    if Instant::now() > exp {
        return Err("Challenge expired");
    }
    Ok(p)
}

/// Periodic cleanup of expired ceremonies.
pub fn spawn_cleanup() {
    tokio::spawn(async {
        let mut iv = tokio::time::interval(Duration::from_secs(60));
        loop {
            iv.tick().await;
            prune();
        }
    });
}

// ── Relying party ───────────────────────────────────────────────────────────

/// rpId and allowed origins for a request.
pub struct Rp {
    pub id: String,
    pub name: String,
    /// Primary origin first.
    pub origins: Vec<String>,
    /// Development on localhost: accept any port.
    pub any_port: bool,
}

fn is_local(host: &str) -> bool {
    host == "localhost" || host == "127.0.0.1" || host == "[::1]" || host == "::1"
}

pub fn rp_for(cfg: &Config, req: &Req) -> Rp {
    let name = cfg.str("main.sitename").filter(|s| !s.is_empty()).unwrap_or("f0ck.dev").to_string();
    let host = req.host_without_port();
    if is_local(&host) {
        return Rp { id: "localhost".into(), name, origins: vec!["http://localhost".into(), "https://localhost".into()], any_port: true };
    }
    let id = cfg
        .str("websrv.passkey_rp_id")
        .filter(|s| !s.is_empty())
        .or_else(|| cfg.str("main.url.domain").filter(|s| !s.is_empty()))
        .unwrap_or("localhost")
        .to_string();
    let mut origins = vec![format!("https://{id}")];
    if let Value::Array(extra) = cfg.get("websrv.passkey_extra_origins") {
        origins.extend(extra.iter().filter_map(Value::as_str).map(str::to_string));
    }
    Rp { id, name, origins, any_port: false }
}

// ── Ceremonies ──────────────────────────────────────────────────────────────

pub struct StartRegistration<'a> {
    pub kind: &'a str,
    pub user_id: Option<i64>,
    pub user_handle: Uuid,
    pub user_name: &'a str,
    pub display_name: &'a str,
    /// Credential ids (base64url) the authenticator must not register again.
    pub exclude: Vec<String>,
}

/// Begin a registration; returns the `publicKey` creation options for the browser.
pub fn start_registration(cfg: &Config, req: &Req, s: StartRegistration) -> anyhow::Result<Value> {
    let rp = rp_for(cfg, req);
    let (options, state) = backend::start_registration(&rp, s.user_handle, s.user_name, s.display_name, &s.exclude)?;
    let challenge = options["challenge"].as_str().ok_or_else(|| anyhow::anyhow!("options without challenge"))?.to_string();
    remember(challenge, Pending { kind: s.kind.into(), user_id: s.user_id, user_handle: Some(s.user_handle), rp_id: rp.id, state })?;
    Ok(options)
}

/// Begin a discoverable (username-less) login; returns the `publicKey` request options.
pub fn start_authentication(cfg: &Config, req: &Req, kind: &str) -> anyhow::Result<Value> {
    let rp = rp_for(cfg, req);
    let (options, state) = backend::start_authentication(&rp)?;
    let challenge = options["challenge"].as_str().ok_or_else(|| anyhow::anyhow!("options without challenge"))?.to_string();
    remember(challenge, Pending { kind: kind.into(), user_id: None, user_handle: None, rp_id: rp.id, state })?;
    Ok(options)
}

/// A verified new credential.
pub struct NewPasskey {
    pub cred_id: String,
    pub user_handle: Uuid,
    /// Serialized `Passkey`
    pub passkey: Value,
    pub aaguid: Option<String>,
}

fn field<'a>(body: &'a Value, k: &str) -> Result<&'a str, String> {
    body.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| format!("missing {k}"))
}

/// Finish a registration with the browser's answer (`credentialId`, `clientDataJSON`, `attestationObject`).
pub fn finish_registration(cfg: &Config, req: &Req, p: Pending, body: &Value) -> Result<NewPasskey, String> {
    let rp = rp_for(cfg, req);
    if rp.id != p.rp_id {
        return Err("Passkey request was started for a different site".into());
    }
    let handle = p.user_handle.ok_or("not a registration")?;
    let (cred_id, passkey, aaguid) = backend::finish_registration(
        &rp,
        p.state,
        field(body, "credentialId")?,
        field(body, "clientDataJSON")?,
        field(body, "attestationObject")?,
    )?;
    Ok(NewPasskey { cred_id, user_handle: handle, passkey, aaguid })
}

/// Stored credential row (`webauthn_credentials`) used to finish a login.
pub struct Stored<'a> {
    pub passkey: &'a Value,
    pub user_handle: Option<&'a str>,
}

/// Finish a discoverable login for the credential the browser picked. Returns the updated passkey
/// (counter / backup state) when it changed.
pub fn finish_authentication(cfg: &Config, req: &Req, p: Pending, body: &Value, stored: Stored) -> Result<Option<Value>, String> {
    let rp = rp_for(cfg, req);
    if rp.id != p.rp_id {
        return Err("Passkey request was started for a different site".into());
    }
    let user_handle = body.get("userHandle").and_then(Value::as_str).filter(|s| !s.is_empty());
    backend::finish_authentication(
        &rp,
        p.state,
        backend::Assertion {
            credential_id: field(body, "credentialId")?,
            client_data_json: field(body, "clientDataJSON")?,
            authenticator_data: field(body, "authenticatorData")?,
            signature: field(body, "signature")?,
            user_handle,
        },
        stored,
    )
}

// ── Storage (webauthn_credentials) ──────────────────────────────────────────

/// The user's WebAuthn user handle (shared by all of their passkeys), or a new random one.
pub async fn user_handle_for(st: &AppState, user_id: i64) -> anyhow::Result<Uuid> {
    let h = sql!("SELECT user_handle::text AS h FROM webauthn_credentials WHERE user_id = ", user_id as i32, " ORDER BY id LIMIT 1")
        .fetch_optional(&st.db)
        .await?
        .and_then(|r| r.str("h").and_then(|h| Uuid::parse_str(h).ok()));
    Ok(h.unwrap_or_else(Uuid::new_v4))
}

pub async fn credential_ids(st: &AppState, user_id: i64) -> anyhow::Result<Vec<String>> {
    Ok(sql!("SELECT cred_id FROM webauthn_credentials WHERE user_id = ", user_id as i32)
        .fetch_all(&st.db)
        .await?
        .iter()
        .map(|r| r.string("cred_id"))
        .collect())
}

pub async fn count(st: &AppState, user_id: i64) -> anyhow::Result<i64> {
    Ok(sql!("SELECT count(*)::int FROM webauthn_credentials WHERE user_id = ", user_id as i32).fetch_scalar::<i32, _>(&st.db).await?.unwrap_or(0) as i64)
}

/// Credential row by id: `user_id`, `passkey`, `user_handle`.
pub async fn find(st: &AppState, cred_id: &str) -> anyhow::Result<Option<Row>> {
    sql!("SELECT user_id, passkey, user_handle::text AS user_handle FROM webauthn_credentials WHERE cred_id = ", cred_id, " LIMIT 1")
        .fetch_optional(&st.db)
        .await
}

/// Whether a credential id is known (also as an anonymous identity key).
pub async fn known(st: &AppState, cred_id: &str) -> anyhow::Result<bool> {
    let n = sql!(
        "SELECT (SELECT count(*) FROM webauthn_credentials WHERE cred_id = ", cred_id,
        ") + (SELECT count(*) FROM anon_identities WHERE credential_id = ", cred_id, ")"
    )
    .fetch_scalar::<i64, _>(&st.db)
    .await?
    .unwrap_or(0);
    Ok(n > 0)
}

/// Store a new credential; false when the id already exists.
pub async fn store<'e, E: sqlx::PgExecutor<'e>>(db: E, user_id: i64, p: &NewPasskey, name: &str) -> anyhow::Result<bool> {
    let n = sql!(
        "INSERT INTO webauthn_credentials (user_id, cred_id, user_handle, passkey, name, aaguid) VALUES (",
        user_id as i32, ", ", p.cred_id.clone(), ", ", p.user_handle.to_string(), "::uuid, ", p.passkey.clone(), ", ", name, ", ", p.aaguid.clone(),
        ") ON CONFLICT (cred_id) DO NOTHING"
    )
    .execute(db)
    .await?;
    Ok(n > 0)
}

/// After a login: update last_used and, if it changed, the stored passkey.
pub async fn touch(st: &AppState, cred_id: &str, updated: Option<Value>) -> anyhow::Result<()> {
    let mut q = sql!("UPDATE webauthn_credentials SET last_used = NOW()");
    if let Some(p) = updated {
        q.push(", passkey = ").bind(p);
    }
    q.push(" WHERE cred_id = ").bind(cred_id);
    q.execute(&st.db).await?;
    Ok(())
}

/// Fingerprint of an anonymous identity: `SHA256:` + base64(sha256(credential id string)), unpadded.
pub fn credential_fingerprint(credential_id: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let d = Sha256::digest(credential_id.as_bytes());
    format!("SHA256:{}", base64::engine::general_purpose::STANDARD.encode(d).trim_end_matches('='))
}

// ── Authenticator names ─────────────────────────────────────────────────────

const AAGUID_PROVIDERS: &[(&str, &str)] = &[
    ("d548826e79b4db40a3d811116f7e8349", "Bitwarden"),
    ("fbfc3007154e4ecc8c0b6e020557d7bd", "iCloud Keychain"),
    ("dd4ec289e01d41c9bb8970fa845d4bf2", "iCloud Keychain (Managed)"),
    ("ea9b8d664d011d213ce4b6b48cb575d4", "Google Password Manager"),
    ("adce000235bcc60a648b0b25f1f05503", "Chrome on Mac"),
    ("08987058cadc4b81b6e130de50dcbe96", "Windows Hello"),
    ("9ddd1817af5a4672a2b93e3dd95000a9", "Windows Hello"),
    ("6028b017b1d44c02b4b3afcdafc96bb2", "Windows Hello"),
    ("bada5566a7aa401fbd9645619a55120d", "1Password"),
    ("531126d6e717415c93203d9aa6981239", "Dashlane"),
    ("fdb141b25d84443e8a354698c205a502", "KeePassXC"),
    ("50726f746f6e5061737350726f746f6e", "Proton Pass"),
    ("53414d53554e47000000000000000000", "Samsung Pass"),
    ("cb69481e8ff7403993ec0a2729a154a8", "YubiKey"),
    ("ee882879721c491397753dfcce97072a", "YubiKey"),
    ("fa2b99dc9e3942578f924a30d23c4118", "YubiKey"),
    ("2fc0579f811347eab116bb5a8db9202a", "YubiKey"),
    ("c5ef55ffad9a4b9fb580adebafe026d0", "YubiKey"),
    ("73bb0cd4e50249b89c6fb59445bf720b", "YubiKey"),
    ("a4e9fc6d4cbe4758b8ba37598bb5bbaa", "Yubico Security Key"),
    ("b92c3f9ac0144056887f140a2501163b", "Yubico Security Key"),
    ("0bb43545fd2c418587ddfeb0b2916ace", "Yubico Security Key"),
];

/// Provider name for a stored AAGUID; `Undisclosed` for all zeros, `None` if unknown.
pub fn aaguid_provider(aaguid: Option<&str>) -> Option<&'static str> {
    let a = aaguid.filter(|a| !a.is_empty())?;
    let hex: String = a.to_lowercase().chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if !hex.is_empty() && hex.chars().all(|c| c == '0') {
        return Some("Undisclosed");
    }
    AAGUID_PROVIDERS.iter().find(|(k, _)| *k == hex).map(|(_, v)| *v)
}

/// Add `provider` and `current` to passkey list rows.
pub fn decorate_rows(rows: Vec<Row>, in_use: Option<&str>) -> Vec<Value> {
    rows.into_iter()
        .map(|mut r| {
            let provider = aaguid_provider(r.str("aaguid"));
            let current = in_use.is_some() && r.str("credential_id") == in_use;
            r.insert("provider".into(), provider.map(Value::from).unwrap_or(Value::Null));
            r.insert("current".into(), Value::Bool(current));
            Value::Object(r)
        })
        .collect()
}

// ── webauthn-rs backend ─────────────────────────────────────────────────────

mod backend {
    //! The webauthn-rs calls. Requests arrive as the frontend's flat JSON and are rebuilt into the
    //! `RegisterPublicKeyCredential` / `PublicKeyCredential` shapes webauthn-rs expects.

    use super::{Rp, Stored};
    use serde_json::{Value, json};
    use webauthn_rs::prelude::*;

    pub enum State {
        Register(Box<PasskeyRegistration>),
        Auth(Box<DiscoverableAuthentication>),
    }

    pub struct Assertion<'a> {
        pub credential_id: &'a str,
        pub client_data_json: &'a str,
        pub authenticator_data: &'a str,
        pub signature: &'a str,
        pub user_handle: Option<&'a str>,
    }

    fn webauthn(rp: &Rp) -> anyhow::Result<Webauthn> {
        let primary = Url::parse(rp.origins.first().map(String::as_str).unwrap_or("https://localhost"))?;
        let mut b = WebauthnBuilder::new(&rp.id, &primary)?.rp_name(&rp.name).allow_any_port(rp.any_port);
        for o in rp.origins.iter().skip(1) {
            match Url::parse(o) {
                Ok(u) => b = b.append_allowed_origin(&u),
                Err(e) => tracing::warn!("[PASSKEY] ignoring invalid origin {o}: {e}"),
            }
        }
        Ok(b.build()?)
    }

    fn b64url(b: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
    }

    fn from_b64url(s: &str) -> Option<Vec<u8>> {
        use base64::Engine;
        let norm: String = s.trim().trim_end_matches('=').chars().map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        }).collect();
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(norm).ok()
    }

    pub fn start_registration(rp: &Rp, handle: Uuid, name: &str, display: &str, exclude: &[String]) -> anyhow::Result<(Value, State)> {
        let wa = webauthn(rp)?;
        let exclude: Vec<CredentialID> = exclude.iter().filter_map(|c| from_b64url(c)).map(CredentialID::from).collect();
        let (ccr, state) = wa.start_passkey_registration(handle, name, display, (!exclude.is_empty()).then_some(exclude))?;
        let mut options = serde_json::to_value(&ccr)?["publicKey"].take();
        // Logins are username-less (discoverable), so the credential must be a resident key.
        if let Some(sel) = options.get_mut("authenticatorSelection").and_then(Value::as_object_mut) {
            sel.insert("residentKey".into(), json!("required"));
            sel.insert("requireResidentKey".into(), json!(true));
        }
        Ok((options, State::Register(Box::new(state))))
    }

    pub fn start_authentication(rp: &Rp) -> anyhow::Result<(Value, State)> {
        let wa = webauthn(rp)?;
        let (rcr, state) = wa.start_discoverable_authentication()?;
        let options = serde_json::to_value(&rcr)?["publicKey"].take();
        Ok((options, State::Auth(Box::new(state))))
    }

    /// AAGUID (hex) from the attested credential data; display only (attestation is `none`).
    fn aaguid(attestation_object: &[u8]) -> Option<String> {
        let v: ciborium::Value = ciborium::de::from_reader(attestation_object).ok()?;
        let auth_data = v.as_map()?.iter().find(|(k, _)| k.as_text() == Some("authData"))?.1.as_bytes()?.clone();
        if auth_data.len() < 53 || auth_data[32] & 0x40 == 0 {
            return None;
        }
        Some(hex::encode(&auth_data[37..53]))
    }

    pub fn finish_registration(rp: &Rp, state: State, id: &str, cdj: &str, att: &str) -> Result<(String, Value, Option<String>), String> {
        let State::Register(state) = state else { return Err("not a registration".into()) };
        let wa = webauthn(rp).map_err(|e| e.to_string())?;
        let reg: RegisterPublicKeyCredential = serde_json::from_value(json!({
            "id": id,
            "rawId": id,
            "type": "public-key",
            "response": { "attestationObject": att, "clientDataJSON": cdj }
        }))
        .map_err(|_| "Malformed credential".to_string())?;
        let passkey = wa.finish_passkey_registration(&reg, &state).map_err(|e| e.to_string())?;
        let cred_id = b64url(passkey.cred_id().as_ref());
        let aaguid = from_b64url(att).and_then(|a| aaguid(&a));
        let value = serde_json::to_value(&passkey).map_err(|e| e.to_string())?;
        Ok((cred_id, value, aaguid))
    }

    pub fn finish_authentication(rp: &Rp, state: State, a: Assertion, stored: Stored) -> Result<Option<Value>, String> {
        let State::Auth(state) = state else { return Err("not an authentication".into()) };
        // The user handle the authenticator returned must belong to the stored credential.
        if let (Some(sent), Some(expected)) = (a.user_handle, stored.user_handle.and_then(|h| Uuid::parse_str(h).ok())) {
            if from_b64url(sent).as_deref() != Some(expected.as_bytes().as_slice()) {
                return Err("User handle does not match this passkey".into());
            }
        }
        let wa = webauthn(rp).map_err(|e| e.to_string())?;
        let mut passkey: Passkey = serde_json::from_value(stored.passkey.clone()).map_err(|_| "Stored passkey is corrupt".to_string())?;
        let mut response = json!({
            "authenticatorData": a.authenticator_data,
            "clientDataJSON": a.client_data_json,
            "signature": a.signature,
        });
        if let Some(h) = a.user_handle {
            response["userHandle"] = json!(h);
        }
        let pkc: PublicKeyCredential = serde_json::from_value(json!({
            "id": a.credential_id,
            "rawId": a.credential_id,
            "type": "public-key",
            "response": response
        }))
        .map_err(|_| "Malformed credential".to_string())?;
        let res = wa
            .finish_discoverable_authentication(&pkc, *state, &[DiscoverableKey::from(&passkey)])
            .map_err(|e| e.to_string())?;
        Ok(match passkey.update_credential(&res) {
            Some(true) => serde_json::to_value(&passkey).ok(),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    //! Full ceremonies against webauthn-rs with a software P-256 authenticator.

    use super::backend::{self, Assertion};
    use super::{Rp, Stored};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use p256::ecdsa::{Signature, SigningKey, signature::Signer};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use uuid::Uuid;

    const RP: &str = "example.com";
    const ORIGIN: &str = "https://example.com";

    fn rp() -> Rp {
        Rp { id: RP.into(), name: "test".into(), origins: vec![ORIGIN.into()], any_port: false }
    }

    struct Authenticator {
        key: SigningKey,
        cred_id: Vec<u8>,
        counter: u32,
    }

    impl Authenticator {
        fn new() -> Self {
            // p256 0.14 takes a newer RNG interface than rand 0.8: build the key from 32 random bytes
            // (retried in the practically impossible case that they exceed the curve order)
            let key = loop {
                if let Ok(k) = SigningKey::from_slice(&crate::util::random_bytes(32)) {
                    break k;
                }
            };
            Authenticator { key, cred_id: crate::util::random_bytes(32), counter: 0 }
        }

        fn cose_key(&self) -> Vec<u8> {
            let point = self.key.verifying_key().to_sec1_point(false);
            use ciborium::Value as C;
            let m = C::Map(vec![
                (C::Integer(1.into()), C::Integer(2.into())),
                (C::Integer(3.into()), C::Integer((-7).into())),
                (C::Integer((-1).into()), C::Integer(1.into())),
                (C::Integer((-2).into()), C::Bytes(point.x().unwrap().to_vec())),
                (C::Integer((-3).into()), C::Bytes(point.y().unwrap().to_vec())),
            ]);
            let mut out = Vec::new();
            ciborium::ser::into_writer(&m, &mut out).unwrap();
            out
        }

        fn auth_data(&self, rp_id: &str, flags: u8, attested: bool) -> Vec<u8> {
            let mut ad = Sha256::digest(rp_id.as_bytes()).to_vec();
            ad.push(flags);
            ad.extend_from_slice(&self.counter.to_be_bytes());
            if attested {
                ad.extend_from_slice(&[0xab; 16]);
                ad.extend_from_slice(&(self.cred_id.len() as u16).to_be_bytes());
                ad.extend_from_slice(&self.cred_id);
                ad.extend_from_slice(&self.cose_key());
            }
            ad
        }

        fn id(&self) -> String {
            B64.encode(&self.cred_id)
        }

        /// Answer to `navigator.credentials.create` as the frontend posts it.
        fn create(&self, options: &Value, origin: &str, flags: u8) -> Value {
            let challenge = options["challenge"].as_str().unwrap();
            let cdj = json!({ "type": "webauthn.create", "challenge": challenge, "origin": origin, "crossOrigin": false }).to_string();
            use ciborium::Value as C;
            let att = C::Map(vec![
                (C::Text("fmt".into()), C::Text("none".into())),
                (C::Text("attStmt".into()), C::Map(vec![])),
                (C::Text("authData".into()), C::Bytes(self.auth_data(RP, flags, true))),
            ]);
            let mut att_bytes = Vec::new();
            ciborium::ser::into_writer(&att, &mut att_bytes).unwrap();
            json!({
                "challenge": challenge,
                "credentialId": self.id(),
                "clientDataJSON": B64.encode(cdj),
                "attestationObject": B64.encode(att_bytes),
            })
        }

        /// Answer to `navigator.credentials.get`.
        fn get(&mut self, options: &Value, origin: &str, flags: u8, handle: &Uuid) -> Value {
            self.counter += 1;
            let challenge = options["challenge"].as_str().unwrap();
            let cdj = json!({ "type": "webauthn.get", "challenge": challenge, "origin": origin, "crossOrigin": false }).to_string();
            let ad = self.auth_data(RP, flags, false);
            let mut msg = ad.clone();
            msg.extend_from_slice(&Sha256::digest(cdj.as_bytes()));
            let sig: Signature = self.key.sign(&msg);
            json!({
                "challenge": challenge,
                "credentialId": self.id(),
                "clientDataJSON": B64.encode(cdj),
                "authenticatorData": B64.encode(ad),
                "signature": B64.encode(sig.to_der().as_bytes()),
                "userHandle": B64.encode(handle.as_bytes()),
            })
        }
    }

    const UP_UV_AT: u8 = 0x01 | 0x04 | 0x40;
    const UP_UV: u8 = 0x01 | 0x04;

    fn register(a: &Authenticator, handle: Uuid) -> Value {
        let (opts, state) = backend::start_registration(&rp(), handle, "anon", "Anonymous", &[]).unwrap();
        assert_eq!(opts["authenticatorSelection"]["residentKey"], "required");
        assert_eq!(opts["rp"]["id"], RP);
        let body = a.create(&opts, ORIGIN, UP_UV_AT);
        let (cred_id, passkey, aaguid) = backend::finish_registration(
            &rp(),
            state,
            body["credentialId"].as_str().unwrap(),
            body["clientDataJSON"].as_str().unwrap(),
            body["attestationObject"].as_str().unwrap(),
        )
        .expect("registration");
        assert_eq!(cred_id, a.id());
        assert_eq!(aaguid.as_deref(), Some("abababababababababababababababab"));
        passkey
    }

    fn login(a: &mut Authenticator, passkey: &Value, handle: &Uuid, origin: &str, flags: u8, tamper: impl FnOnce(&mut Value)) -> Result<Option<Value>, String> {
        let (opts, state) = backend::start_authentication(&rp()).unwrap();
        assert_eq!(opts["userVerification"], "required");
        let mut body = a.get(&opts, origin, flags, handle);
        tamper(&mut body);
        let s = |k: &str| body[k].as_str().map(str::to_string);
        let (id, cdj, ad, sig, uh) = (s("credentialId").unwrap(), s("clientDataJSON").unwrap(), s("authenticatorData").unwrap(), s("signature").unwrap(), s("userHandle"));
        let stored_handle = handle.to_string();
        backend::finish_authentication(
            &rp(),
            state,
            Assertion { credential_id: &id, client_data_json: &cdj, authenticator_data: &ad, signature: &sig, user_handle: uh.as_deref() },
            Stored { passkey, user_handle: Some(&stored_handle) },
        )
    }

    #[test]
    fn register_and_login() {
        let mut a = Authenticator::new();
        let handle = Uuid::new_v4();
        let passkey = register(&a, handle);
        let updated = login(&mut a, &passkey, &handle, ORIGIN, UP_UV, |_| {}).expect("login");
        assert!(updated.is_some(), "counter update is reported");
        // and again with the updated credential
        login(&mut a, &updated.unwrap(), &handle, ORIGIN, UP_UV, |_| {}).expect("second login");
    }

    #[test]
    fn rejects_bad_assertions() {
        let mut a = Authenticator::new();
        let handle = Uuid::new_v4();
        let passkey = register(&a, handle);

        assert!(login(&mut a, &passkey, &handle, "https://evil.example", UP_UV, |_| {}).is_err(), "foreign origin");
        assert!(login(&mut a, &passkey, &handle, ORIGIN, 0x04, |_| {}).is_err(), "no user presence");
        assert!(login(&mut a, &passkey, &handle, ORIGIN, 0x01, |_| {}).is_err(), "no user verification");
        assert!(
            login(&mut a, &passkey, &handle, ORIGIN, UP_UV, |b| {
                let mut sig = B64.decode(b["signature"].as_str().unwrap()).unwrap();
                let n = sig.len();
                sig[n - 1] ^= 1;
                b["signature"] = json!(B64.encode(sig));
            })
            .is_err(),
            "bad signature"
        );
        assert!(
            login(&mut a, &passkey, &handle, ORIGIN, UP_UV, |b| b["userHandle"] = json!(B64.encode(Uuid::new_v4().as_bytes()))).is_err(),
            "foreign user handle"
        );
        // A different authenticator presenting this credential id
        let mut other = Authenticator::new();
        other.cred_id = a.cred_id.clone();
        assert!(login(&mut other, &passkey, &handle, ORIGIN, UP_UV, |_| {}).is_err(), "wrong key");
    }

    #[test]
    fn rejects_counter_regression() {
        let mut a = Authenticator::new();
        let handle = Uuid::new_v4();
        let passkey = register(&a, handle);
        a.counter = 10;
        let updated = login(&mut a, &passkey, &handle, ORIGIN, UP_UV, |_| {}).unwrap().unwrap();
        a.counter = 3; // clone with an older counter
        assert!(login(&mut a, &updated, &handle, ORIGIN, UP_UV, |_| {}).is_err());
    }

    #[test]
    fn rejects_registration_problems() {
        let a = Authenticator::new();
        let (opts, state) = backend::start_registration(&rp(), Uuid::new_v4(), "x", "x", &[]).unwrap();
        let body = a.create(&opts, "https://evil.example", UP_UV_AT);
        let r = backend::finish_registration(&rp(), state, &a.id(), body["clientDataJSON"].as_str().unwrap(), body["attestationObject"].as_str().unwrap());
        assert!(r.is_err(), "foreign origin");

        let (opts, state) = backend::start_registration(&rp(), Uuid::new_v4(), "x", "x", &[]).unwrap();
        let body = a.create(&opts, ORIGIN, 0x04 | 0x40);
        let r = backend::finish_registration(&rp(), state, &a.id(), body["clientDataJSON"].as_str().unwrap(), body["attestationObject"].as_str().unwrap());
        assert!(r.is_err(), "no user presence");

        // Answer for another challenge
        let (opts1, _s1) = backend::start_registration(&rp(), Uuid::new_v4(), "x", "x", &[]).unwrap();
        let (_opts2, s2) = backend::start_registration(&rp(), Uuid::new_v4(), "x", "x", &[]).unwrap();
        let body = a.create(&opts1, ORIGIN, UP_UV_AT);
        let r = backend::finish_registration(&rp(), s2, &a.id(), body["clientDataJSON"].as_str().unwrap(), body["attestationObject"].as_str().unwrap());
        assert!(r.is_err(), "challenge mismatch");
    }

    #[test]
    fn ceremony_state_is_single_use_and_expires() {
        let (opts, state) = backend::start_authentication(&rp()).unwrap();
        let ch = opts["challenge"].as_str().unwrap().to_string();
        super::remember(ch.clone(), super::Pending { kind: "x".into(), user_id: None, user_handle: None, rp_id: RP.into(), state }).unwrap();
        assert!(super::take(&ch).is_ok());
        assert!(super::take(&ch).is_err(), "second use");

        let (_, state) = backend::start_authentication(&rp()).unwrap();
        super::PENDING.insert(
            "old".into(),
            (std::time::Instant::now() - std::time::Duration::from_secs(1), super::Pending { kind: "x".into(), user_id: None, user_handle: None, rp_id: RP.into(), state }),
        );
        assert!(super::take("old").is_err(), "expired");
    }
}
