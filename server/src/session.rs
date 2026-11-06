//! The logged-in session (`req.session` in the old code), typed.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Columns selected for a session (user + user_options + session row + anon identity).
pub const SESSION_COLUMNS: &str = r#""user".id, "user".login, "user".user, "user".admin, "user".is_moderator, "user".groups, "user".banned, "user".ban_reason, "user".ban_expires, "user".force_password_change, "user_sessions".id as sess_id, "user_sessions".csrf_token, "user_options".mode, "user_options".theme, "user_options".fullscreen, "user_options".excluded_tags, "user_options".avatar, "user_options".avatar_file, "user_options".banner_file, "user_options".banner_position, "user_options".banner_size, "user_options".banner_repeat, "user_options".show_motd, "user_options".strict_mode, "user_options".show_background, "user_options".use_new_layout, "user_options".username_color, "user_options".font, "user_options".font_weight, "user_options".font_size, "user_options".disable_autoplay, "user_options".disable_swiping, "user_options".favorites_private, "user_options".hide_fav_badge, "user_options".default_upload_visibility, "user_options".description, "user_options".display_name, "user_options".ruffle_volume, "user_options".ruffle_background, "user_options".quote_emojis, "user_options".embed_youtube_in_comments, "user_options".hide_koepfe, "user_options".language, "user_options".use_alternative_infobox, "user_options".use_alternative_steuerung, "user_options".receive_system_notifications, "user_options".receive_user_notifications, "user_options".do_not_disturb, "user_options".comment_display_mode, "user_options".force_comment_display_mode, "anon_identities".fingerprint as anon_fingerprint"#;

/// Same user/user_options columns, without session-row fields (impersonation target).
pub const USER_COLUMNS: &str = r#""user".id, "user".login, "user".user, "user".admin, "user".is_moderator, "user".groups, "user".banned, "user".ban_reason, "user".ban_expires, "user".force_password_change, "user_options".mode, "user_options".theme, "user_options".fullscreen, "user_options".excluded_tags, "user_options".avatar, "user_options".avatar_file, "user_options".show_motd, "user_options".strict_mode, "user_options".show_background, "user_options".use_new_layout, "user_options".username_color, "user_options".font, "user_options".font_weight, "user_options".font_size, "user_options".disable_autoplay, "user_options".disable_swiping, "user_options".favorites_private, "user_options".hide_fav_badge, "user_options".default_upload_visibility, "user_options".description, "user_options".display_name, "user_options".ruffle_volume, "user_options".ruffle_background, "user_options".quote_emojis, "user_options".embed_youtube_in_comments, "user_options".hide_koepfe, "user_options".language, "user_options".use_alternative_infobox, "user_options".use_alternative_steuerung, "user_options".receive_system_notifications, "user_options".receive_user_notifications, "user_options".do_not_disturb, "user_options".comment_display_mode, "user_options".force_comment_display_mode"#;

fn de_i64_lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(crate::config::as_int(&v).unwrap_or(0))
}

fn de_opt_i64_lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(crate::config::as_int(&v))
}

fn de_bool_lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let v = Value::deserialize(d)?;
    Ok(crate::config::truthy(&v))
}

fn de_vec_or_empty<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    #[serde(deserialize_with = "de_i64_lenient")]
    pub id: i64,
    pub login: String,
    pub user: String,
    #[serde(deserialize_with = "de_bool_lenient")]
    pub admin: bool,
    #[serde(deserialize_with = "de_bool_lenient")]
    pub is_moderator: bool,
    #[serde(deserialize_with = "de_vec_or_empty")]
    pub groups: Vec<String>,
    #[serde(deserialize_with = "de_bool_lenient")]
    pub banned: bool,
    pub ban_reason: Option<String>,
    pub ban_expires: Option<String>,
    #[serde(deserialize_with = "de_bool_lenient")]
    pub force_password_change: bool,
    #[serde(deserialize_with = "de_i64_lenient")]
    pub sess_id: i64,
    pub csrf_token: Option<String>,

    // user_options
    #[serde(deserialize_with = "de_opt_i64_lenient")]
    pub mode: Option<i64>,
    pub theme: Option<String>,
    /// Number from the DB, replaced by the `fullscreen` cookie string per request.
    pub fullscreen: Value,
    #[serde(deserialize_with = "de_vec_or_empty")]
    pub excluded_tags: Vec<i64>,
    #[serde(deserialize_with = "de_opt_i64_lenient")]
    pub avatar: Option<i64>,
    pub avatar_file: Option<String>,
    pub banner_file: Option<String>,
    pub banner_position: Option<String>,
    pub banner_size: Option<String>,
    pub banner_repeat: Option<String>,
    pub show_motd: Option<bool>,
    pub strict_mode: Option<bool>,
    pub show_background: Option<bool>,
    pub use_new_layout: Option<bool>,
    pub username_color: Option<String>,
    pub font: Option<String>,
    /// Website font weight (100-900), None: the font's own (crate::fonts)
    #[serde(default, deserialize_with = "de_opt_i64_lenient")]
    pub font_weight: Option<i64>,
    /// Text size in percent (crate::fonts::SIZES), None: 100
    #[serde(default, deserialize_with = "de_opt_i64_lenient")]
    pub font_size: Option<i64>,
    pub disable_autoplay: Option<bool>,
    pub disable_swiping: Option<bool>,
    pub favorites_private: Option<bool>,
    pub hide_fav_badge: Option<bool>,
    #[serde(deserialize_with = "de_opt_i64_lenient")]
    pub default_upload_visibility: Option<i64>,
    pub description: Option<String>,
    pub display_name: Option<String>,

    /// numeric(3,2) — a decimal string like postgres.js returns.
    pub ruffle_volume: Option<String>,
    pub ruffle_background: Option<bool>,
    pub quote_emojis: Option<bool>,
    pub embed_youtube_in_comments: Option<bool>,
    pub hide_koepfe: Option<bool>,
    pub language: Option<String>,
    pub use_alternative_infobox: Option<bool>,
    pub use_alternative_steuerung: Option<bool>,
    pub receive_system_notifications: Option<bool>,
    pub receive_user_notifications: Option<bool>,
    pub do_not_disturb: Option<bool>,
    #[serde(deserialize_with = "de_opt_i64_lenient")]
    pub comment_display_mode: Option<i64>,
    #[serde(deserialize_with = "de_opt_i64_lenient")]
    pub force_comment_display_mode: Option<i64>,
    pub anon_fingerprint: Option<String>,

    // ── Computed per request ────────────────────────────────────────────────
    pub is_anon: bool,
    pub fingerprint: Option<String>,
    pub anon_login: Option<String>,
    pub can_chan: bool,
    /// Pending items (admins only).
    pub pending_count: Option<i64>,
    pub own_pending_count: Option<i64>,
    pub uploads_remaining: Option<i64>,
    pub min_tags: i64,
    pub mimes_json: String,
    pub allowed_mimes: String,
    pub max_file_size: String,
    pub max_file_size_bytes: i64,
    pub onara: Option<bool>,
    pub api_key_auth: bool,
    #[serde(rename = "_is_impersonating")]
    pub is_impersonating: bool,
    #[serde(rename = "_impersonated_by")]
    pub impersonated_by: Option<String>,
    #[serde(rename = "_impersonated_by_display")]
    pub impersonated_by_display: Option<String>,
    #[serde(rename = "_preview_role")]
    pub preview_role: Option<String>,
}

impl Session {
    pub fn from_row(row: crate::db::Row) -> anyhow::Result<Session> {
        Ok(serde_json::from_value(Value::Object(row))?)
    }

    pub fn is_admin_or_mod(&self) -> bool {
        self.admin || self.is_moderator
    }

    /// Anonymous (passkey shadow) user or legacy `anon_` account.
    pub fn is_anonymous_user(&self) -> bool {
        self.is_anon || self.user == "anonymous" || self.user.starts_with("anon_")
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Bool(false))
    }
}
