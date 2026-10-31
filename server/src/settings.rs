//! Live site settings (port of src/inc/settings.mjs).
//!
//! Values stored in `site_settings` are held in memory (loaded at boot, changed from the admin
//! panel); the rest are read from config.yaml on every call so a config reload applies.

use crate::config::{Config, as_int, truthy};
use crate::session::Session;
use parking_lot::RwLock;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone)]
pub struct SiteSettings {
    pub manual_approval: bool,
    pub min_tags: i64,
    pub registration_open: bool,
    pub trusted_uploads: i64,
    pub bypass_duplicate_check: bool,
    pub protect_files: bool,
    pub private_messages: bool,
    pub dm_attachments: bool,
    pub dm_unencrypted: bool,
    pub default_layout: String,
    pub enable_pdf: bool,
    pub enable_cleanup: bool,
    pub cleanup_start_date: String,
    pub cleanup_end_date: String,
    pub cleanup_include_engaged: bool,
    pub nsfp_ids: Vec<i64>,
    pub brand_image_url: String,
    pub motd: String,
    pub about_text: String,
    pub rules_text: String,
    pub terms_text: String,
}

pub struct Settings {
    inner: RwLock<SiteSettings>,
}

impl Settings {
    pub fn new(cfg: &Config) -> Settings {
        let brand = config_brand_image(cfg);
        Settings {
            inner: RwLock::new(SiteSettings {
                manual_approval: false,
                min_tags: 0,
                registration_open: false,
                trusted_uploads: 0,
                bypass_duplicate_check: false,
                protect_files: false,
                private_messages: true,
                dm_attachments: true,
                dm_unencrypted: false,
                default_layout: "modern".into(),
                enable_pdf: false,
                enable_cleanup: false,
                cleanup_start_date: String::new(),
                cleanup_end_date: String::new(),
                cleanup_include_engaged: false,
                nsfp_ids: cfg.nsfp(),
                brand_image_url: brand,
                motd: String::new(),
                about_text: String::new(),
                rules_text: String::new(),
                terms_text: String::new(),
            }),
        }
    }

    pub fn get(&self) -> SiteSettings {
        self.inner.read().clone()
    }

    pub fn update(&self, f: impl FnOnce(&mut SiteSettings)) {
        f(&mut self.inner.write());
    }

    pub fn manual_approval(&self) -> bool {
        self.inner.read().manual_approval
    }
    pub fn min_tags(&self) -> i64 {
        self.inner.read().min_tags
    }
    pub fn trusted_uploads(&self) -> i64 {
        self.inner.read().trusted_uploads
    }
    pub fn bypass_duplicate_check(&self) -> bool {
        self.inner.read().bypass_duplicate_check
    }
    pub fn protect_files(&self) -> bool {
        self.inner.read().protect_files
    }
    pub fn private_messages(&self) -> bool {
        self.inner.read().private_messages
    }
    pub fn dm_attachments(&self) -> bool {
        self.inner.read().dm_attachments
    }
    pub fn dm_unencrypted(&self) -> bool {
        self.inner.read().dm_unencrypted
    }
    pub fn default_layout(&self) -> String {
        self.inner.read().default_layout.clone()
    }
    pub fn enable_pdf(&self) -> bool {
        self.inner.read().enable_pdf
    }
    pub fn nsfp_ids(&self) -> Vec<i64> {
        self.inner.read().nsfp_ids.clone()
    }
    pub fn brand_image_url(&self) -> String {
        self.inner.read().brand_image_url.clone()
    }
    pub fn motd(&self) -> String {
        self.inner.read().motd.clone()
    }

    pub fn set_min_tags(&self, v: &Value) {
        self.inner.write().min_tags = as_int(v).unwrap_or(0).max(0);
    }
    pub fn set_trusted_uploads(&self, v: &Value) {
        self.inner.write().trusted_uploads = as_int(v).unwrap_or(0).max(0);
    }
    pub fn set_default_layout(&self, v: &str) {
        self.inner.write().default_layout = if v == "legacy" { "legacy".into() } else { "modern".into() };
    }
    pub fn set_nsfp_ids(&self, ids: &[i64]) {
        self.inner.write().nsfp_ids = ids.iter().copied().filter(|n| *n > 0).collect();
    }

    pub fn enable_cleanup(&self, cfg: &Config) -> bool {
        if cfg.is_false("websrv.enable_cleanup") {
            return false;
        }
        self.inner.read().enable_cleanup
    }

    pub fn registration_open(&self, cfg: &Config) -> bool {
        if cfg.is_false("websrv.open_registration_web_toggle") {
            return cfg.truthy("websrv.open_registration");
        }
        self.inner.read().registration_open
    }
}

// ── Config-based settings ─────────────────────────────────────────────────────

pub fn shitpost_mode(cfg: &Config) -> bool {
    cfg.truthy("websrv.shitpost_mode")
}

pub fn enable_expiring_uploads(cfg: &Config) -> bool {
    !(cfg.is_false("enable_expiring_uploads") || cfg.is_false("websrv.enable_expiring_uploads"))
}

pub fn enable_item_slugs(cfg: &Config) -> bool {
    !(cfg.is_false("enable_item_slugs") || cfg.is_false("websrv.enable_item_slugs"))
}

pub fn enable_anonymous_access(cfg: &Config) -> bool {
    !(cfg.is_false("enable_anonymous_access")
        || cfg.is_false("anonymous_access")
        || cfg.is_false("websrv.enable_anonymous_access")
        || cfg.is_false("websrv.anonymous_access"))
}

pub fn registration_require_mail_andor_token(cfg: &Config) -> bool {
    cfg.truthy("websrv.open_registration_require_mail_andor_token")
}

pub fn log_user_ips(cfg: &Config) -> bool {
    cfg.truthy("websrv.log_user_ips")
}

pub fn hash_user_ips(cfg: &Config) -> bool {
    cfg.truthy("websrv.hash_user_ips")
}

pub fn hw_fingerprint_enabled(cfg: &Config) -> bool {
    cfg.get("websrv.anon_hw_fingerprint") == &Value::Bool(true)
}

pub fn allow_comment_deletion(cfg: &Config) -> bool {
    cfg.truthy("websrv.allow_comment_deletion")
}

pub fn dm_attachment_expiry_days(cfg: &Config) -> i64 {
    match cfg.int("websrv.dm_attachment_expiry_days") {
        Some(v) if v > 0 => v,
        _ => 90,
    }
}

fn default_anon_permissions() -> Map<String, Value> {
    match json!({
        "upload": false,
        "comment": true,
        "comment_attachments": false,
        "comment_vote": true,
        "poll_vote": true,
        "tag": true,
        "tag_vote": true,
        "favorite": true,
        "rate_item": false,
        "filter": true,
        "exclude_tags": true,
        "chan": false,
        "anonymize_users": false,
        "allowed_modes": ["sfw", "nsfw", "untagged", "all", "nsfl"],
        "allowed_mimes": ["image", "video", "audio", "flash", "pdf"]
    }) {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

pub fn anon_permissions(cfg: &Config) -> Map<String, Value> {
    let mut perms = default_anon_permissions();
    let from_cfg = match cfg.get("anonymous_permissions") {
        Value::Object(m) => Some(m),
        _ => cfg.get("websrv.anonymous_permissions").as_object(),
    };
    if let Some(m) = from_cfg {
        for (k, v) in m {
            perms.insert(k.clone(), v.clone());
        }
    }
    perms
}

pub fn anon_anonymize(cfg: &Config) -> bool {
    let perms = anon_permissions(cfg);
    for key in ["anonymize_users", "anon_anonymize"] {
        if let Some(Value::Bool(b)) = perms.get(key) {
            return *b;
        }
    }
    for path in ["main.anon_anonymize", "main.anonymous_anonymize", "anon_anonymize", "anonymous_anonymize"] {
        if let Value::Bool(b) = cfg.get(path) {
            return *b;
        }
    }
    false
}

pub fn is_anonymize_session(cfg: &Config, session: Option<&Session>) -> bool {
    match session {
        Some(s) if !s.user.is_empty() => {
            if s.is_anonymous_user() {
                anon_anonymize(cfg)
            } else {
                false
            }
        }
        _ => {
            let v = cfg.get("main.guest_anonymize");
            if v.is_null() { cfg.truthy("guest_anonymize") } else { truthy(v) }
        }
    }
}

pub fn can_anon_do(cfg: &Config, action: &str) -> bool {
    if !enable_anonymous_access(cfg) {
        return false;
    }
    let perms = anon_permissions(cfg);
    if matches!(action, "exclude_tags" | "exclude_tag" | "tag_exclude") {
        for k in ["exclude_tags", "exclude_tag", "tag_exclude"] {
            if let Some(v) = perms.get(k) {
                return truthy(v);
            }
        }
        return perms.get("filter").map(truthy).unwrap_or(true);
    }
    match perms.get(action) {
        Some(v) => truthy(v),
        None => default_anon_permissions().get(action).map(truthy).unwrap_or(false),
    }
}

fn lower_list(v: Option<&Value>) -> Option<Vec<String>> {
    v.and_then(Value::as_array).map(|a| {
        a.iter()
            .map(|x| match x {
                Value::String(s) => s.to_lowercase(),
                other => other.to_string().to_lowercase(),
            })
            .collect()
    })
}

pub fn anon_allowed_modes(cfg: &Config) -> Vec<String> {
    lower_list(anon_permissions(cfg).get("allowed_modes"))
        .unwrap_or_else(|| ["sfw", "nsfw", "untagged", "all", "nsfl"].map(String::from).to_vec())
}

pub fn anon_allowed_mimes(cfg: &Config) -> Vec<String> {
    lower_list(anon_permissions(cfg).get("allowed_mimes"))
        .unwrap_or_else(|| ["image", "video", "audio", "flash", "pdf"].map(String::from).to_vec())
}

pub const MODE_NAMES: [&str; 5] = ["sfw", "nsfw", "untagged", "all", "nsfl"];

pub fn can_anon_mode(cfg: &Config, mode: i64) -> bool {
    if !can_anon_do(cfg, "filter") {
        return false;
    }
    let name = MODE_NAMES.get(mode as usize).copied().unwrap_or("");
    anon_allowed_modes(cfg).iter().any(|m| m == name)
}

pub fn can_anon_mime(cfg: &Config, mime: &str) -> bool {
    if !can_anon_do(cfg, "filter") {
        return false;
    }
    let mime = mime.to_lowercase();
    anon_allowed_mimes(cfg).contains(&mime)
}

/// No session, or an anonymous one.
pub fn is_anon_session(session: Option<&Session>) -> bool {
    match session {
        None => true,
        Some(s) if s.user.is_empty() => true,
        Some(s) => s.is_anonymous_user(),
    }
}

/// Onara viewer preference: cookie, then session, then config default.
pub fn is_onara_enabled_for(cfg: &Config, cookies: &std::collections::HashMap<String, String>, session: Option<&Session>, force_on: bool) -> bool {
    if force_on {
        return true;
    }
    if let Some(raw) = cookies.get("f0ck_onara").or_else(|| cookies.get("onara")) {
        return raw == "1" || raw == "true";
    }
    if let Some(v) = session.and_then(|s| s.onara) {
        return v;
    }
    let c = if cfg.get("onara").is_null() { cfg.get("websrv.onara") } else { cfg.get("onara") };
    c == &Value::Bool(true)
}

/// The name `items.username` holds for this session's uploads.
pub fn session_owner_name(session: Option<&Session>) -> Option<String> {
    let s = session?;
    if s.is_anon {
        return s.anon_login.clone().or_else(|| Some(s.login.clone()).filter(|l| !l.is_empty()));
    }
    Some(s.user.clone()).filter(|u| !u.is_empty())
}

/// 4chan viewer & rehost access.
pub fn can_use_chan(cfg: &Config, session: Option<&Session>) -> bool {
    let Some(s) = session else { return false };
    if s.admin {
        return true;
    }
    if s.groups.iter().any(|g| g == "4chan") {
        return true;
    }
    s.is_anon && can_anon_do(cfg, "chan")
}

pub fn check_anon_permission(cfg: &Config, session: Option<&Session>, action: &str) -> bool {
    if !is_anon_session(session) {
        return true;
    }
    can_anon_do(cfg, action)
}

/// Load the DB-backed settings at boot.
pub async fn load_from_db(st: &crate::state::AppState) {
    let rows = match crate::sql!("SELECT key, value FROM site_settings").fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[BOOT] site_settings fetch failed: {e}");
            return;
        }
    };
    use crate::db::RowExt;
    let cfg = st.cfg();
    st.settings.update(|s| {
        // Static (config.yaml) settings
        s.enable_pdf = cfg.truthy("enable_pdf");
        s.enable_cleanup = cfg.truthy("websrv.enable_cleanup");
        s.bypass_duplicate_check = cfg.get("websrv.bypass_duplicate_check") == &Value::Bool(true);
        s.protect_files = cfg.get("websrv.protect_files") == &Value::Bool(true);
        s.nsfp_ids = cfg.nsfp();
        s.brand_image_url = config_brand_image(&cfg);
        s.private_messages = cfg.enabled("websrv.private_messages");
        s.dm_attachments = cfg.enabled("websrv.dm_attachments");
        s.dm_unencrypted = cfg.truthy("websrv.dm_unencrypted");
        if let Some(l) = cfg.str("websrv.default_layout").filter(|l| !l.is_empty()) {
            s.default_layout = if l == "legacy" { "legacy".into() } else { "modern".into() };
        }

        // DB-backed settings
        for r in &rows {
            let key = r.string("key");
            let val = r.string("value");
            let b = val == "true";
            match key.as_str() {
                "motd" => s.motd = val,
                "manual_approval" => s.manual_approval = b,
                "min_tags" => s.min_tags = as_int(&Value::String(val)).unwrap_or(0).max(0),
                "registration_open" => s.registration_open = b,
                "trusted_uploads" => s.trusted_uploads = as_int(&Value::String(val)).unwrap_or(0).max(0),
                "enable_cleanup" => s.enable_cleanup = b,
                "cleanup_start_date" => s.cleanup_start_date = val,
                "cleanup_end_date" => s.cleanup_end_date = val,
                "cleanup_include_engaged" => s.cleanup_include_engaged = b,
                "brand_image_url" => s.brand_image_url = val,
                "about_text" => s.about_text = val,
                "rules_text" => s.rules_text = val,
                "terms_text" => s.terms_text = val,
                "nsfp" => {
                    if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&val) {
                        s.nsfp_ids = a.iter().filter_map(as_int).filter(|n| *n > 0).collect();
                    }
                }
                _ => {}
            }
        }
    });
}

/// `websrv.custom_brand_image`: a string or a list (first entry wins).
fn config_brand_image(cfg: &Config) -> String {
    match cfg.get("websrv.custom_brand_image") {
        Value::Array(a) => a.first().and_then(Value::as_str).unwrap_or("").to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Persist one site setting.
pub async fn save(st: &crate::state::AppState, key: &str, value: &str) -> anyhow::Result<()> {
    crate::sql!(
        "INSERT INTO site_settings (key, value) VALUES (", key, ", ", value,
        ") ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value"
    )
    .execute(&st.db)
    .await?;
    Ok(())
}
