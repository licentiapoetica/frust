//! Permissions: what a user may do, action by action (comment, upload, fav, ...; moderators' powers
//! too).
//!
//! Who has a permission, first match wins:
//! 1. admins: every permission, always;
//! 2. a moderator power ([`Perm::mod_power`]) never for anyone who isn't a moderator;
//! 3. anonymous accounts: nothing while `enable_anonymous_access` is off;
//! 4. the user's own override (`"user".permissions`, admin panel → User Manager): allow or deny;
//! 5. the user's groups (`"user".groups`, their permissions in [`PermConfig::groups`]): a group
//!    that denies wins over one that allows;
//! 6. the default of the user's role ([`Role`]): admin panel → Permissions, else the built-in one
//!    ([`Perm::user`], ...; anonymous accounts: `anonymous_permissions` of the config for the keys
//!    it has, see [`ANON_CONFIG_KEYS`]).
//!
//! Visitors without an account have none of these. Role defaults and group permissions are one
//! site setting (`permissions`, [`PermConfig`]); routes check with [`can`] and answer [`denied`].

use crate::config::Config;
use crate::session::Session;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// One permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum P {
    Upload,
    UploadUrl,
    UploadLarge,
    UploadUnlimited,
    UploadSkipApproval,
    EditOwnItems,
    Comment,
    CommentFiles,
    CommentPoll,
    PollVote,
    CommentDelete,
    Fav,
    Tag,
    Rate,
    Report,
    Subscribe,
    Lists,
    Message,
    MessageFiles,
    Chat,
    ProfileMedia,
    ProfileText,
    Meme,
    Chan,
    ApiKeys,
    Invite,
    ModApprove,
    ModDeleteItems,
    ModEditItems,
    ModTags,
    ModComments,
    ModReports,
    ModWarn,
    ModBan,
    ModLists,
    ModChat,
    ModMotd,
    ModAudit,
}

/// A permission's description for the admin panel and its built-in defaults per role.
#[derive(Debug)]
pub struct Perm {
    pub p: P,
    /// Stored name (role defaults, groups, user overrides, `session.can.<key>`)
    pub key: &'static str,
    /// Section in the admin panel
    pub section: &'static str,
    pub label: &'static str,
    pub desc: &'static str,
    /// A moderator power: only moderators (and admins) can ever have it
    pub mod_power: bool,
    pub user: bool,
    pub anon: bool,
    pub moderator: bool,
}

const fn perm(p: P, key: &'static str, section: &'static str, label: &'static str, desc: &'static str, user: bool, anon: bool, moderator: bool) -> Perm {
    Perm { p, key, section, label, desc, mod_power: false, user, anon, moderator }
}

const fn mod_power(p: P, key: &'static str, label: &'static str, desc: &'static str) -> Perm {
    Perm { p, key, section: "Moderation", label, desc, mod_power: true, user: false, anon: false, moderator: true }
}

/// Every permission, in the admin panel's order. The built-in defaults (users, anonymous
/// accounts, moderators) are what each role could do before permissions existed.
pub const PERMS: &[Perm] = &[
    perm(P::Upload, "upload", "Uploads", "Upload", "Upload files: upload page, drag and drop, API keys, 4chan rehosts.", true, false, true),
    perm(P::UploadUrl, "upload_url", "Uploads", "Upload from a link", "Let the site download a file from a link (when link uploads are on).", true, false, true),
    perm(P::UploadLarge, "upload_large", "Uploads", "Big uploads", "Files up to max upload size × big upload multiplier (admin panel → Settings → Uploads).", false, false, true),
    perm(P::UploadUnlimited, "upload_unlimited", "Uploads", "No upload limit", "No limit on the number of uploads every 12 hours (main.upload_limit) and of comment files.", false, false, true),
    perm(P::UploadSkipApproval, "upload_skip_approval", "Uploads", "Skip approval", "Uploads go live right away, even with manual approval or below the trusted upload threshold.", false, false, true),
    perm(P::EditOwnItems, "edit_own_items", "Uploads", "Edit own uploads", "Change the title, OC mark, visibility, expiry and thumbnail of their own uploads.", true, false, true),
    perm(P::Comment, "comment", "Comments", "Comment", "Write comments and replies.", true, true, true),
    perm(P::CommentFiles, "comment_files", "Comments", "Files in comments", "Attach files to comments (when comment uploads are on).", true, false, true),
    perm(P::CommentPoll, "comment_poll", "Comments", "Start polls", "Add polls to their comments (when comment polls are on).", true, true, true),
    perm(P::PollVote, "poll_vote", "Comments", "Vote in polls", "Vote in comment polls.", true, true, true),
    perm(P::CommentDelete, "comment_delete", "Comments", "Delete own comments", "Delete their own comments (when comment deletion is on).", true, true, true),
    perm(P::Fav, "fav", "Items", "Favourite", "Add items to their favourites.", true, true, true),
    perm(P::Tag, "tag", "Items", "Add tags", "Add tags to items.", true, true, true),
    perm(P::Rate, "rate", "Items", "Rate own uploads", "Change the rating (sfw / nsfw / nsfl) of their own uploads.", true, false, true),
    perm(P::Report, "report", "Items", "Report", "Report items, comments and users to the moderators.", true, true, true),
    perm(P::Subscribe, "subscribe", "Items", "Subscribe", "Subscribe to items' comments and to their own uploads.", true, true, true),
    perm(P::Lists, "lists", "Items", "Lists", "Make their own lists and add items to them (when lists are on).", true, false, true),
    perm(P::Message, "message", "Social", "Private messages", "Write private messages (when they are on).", true, true, true),
    perm(P::MessageFiles, "message_files", "Social", "Files in messages", "Attach files to private messages (when that is on).", true, true, true),
    perm(P::Chat, "chat", "Social", "Global chat", "Write in the site chat (when it is on).", true, true, true),
    perm(P::ProfileMedia, "profile_media", "Profile", "Avatar and banner", "Upload an avatar and a profile banner.", true, false, true),
    perm(P::ProfileText, "profile_text", "Profile", "Name and description", "Set a display name, a name colour and a profile description.", true, false, true),
    perm(P::Meme, "meme", "Profile", "Meme creator", "Use the meme creator (when it is on).", true, false, true),
    perm(P::Chan, "chan", "Profile", "4chan viewer", "Browse the 4chan viewer and rehost its files.", false, false, false),
    perm(P::ApiKeys, "api_keys", "Profile", "API keys", "Create API keys for uploading with scripts (when they are on).", true, false, true),
    perm(P::Invite, "invite", "Profile", "Invite", "Create invite tokens for new users (when user invites are on).", true, false, true),
    mod_power(P::ModApprove, "mod_approve", "Approve uploads", "Approve and deny uploads in the approval queue."),
    mod_power(P::ModDeleteItems, "mod_delete_items", "Delete items", "Delete anyone's items, restore them from the trash."),
    mod_power(P::ModEditItems, "mod_edit_items", "Edit items", "Change any item's rating, title, visibility, expiry, thumbnail and OC mark; pin items."),
    mod_power(P::ModTags, "mod_tags", "Edit tags", "Remove tags from any item, rename tags, bulk tag edits."),
    mod_power(P::ModComments, "mod_comments", "Moderate comments", "Edit, delete and pin anyone's comments, lock threads, delete polls."),
    mod_power(P::ModReports, "mod_reports", "Handle reports", "See and close user reports."),
    mod_power(P::ModWarn, "mod_warn", "Warn users", "Send users warnings."),
    mod_power(P::ModBan, "mod_ban", "Ban users", "Ban and unban users, IPs and devices (48 hours at most)."),
    mod_power(P::ModLists, "mod_lists", "Moderate lists", "Manage the site's lists."),
    mod_power(P::ModChat, "mod_chat", "Moderate the chat", "Delete chat messages, clear the chat, set its topic and background."),
    mod_power(P::ModMotd, "mod_motd", "Message of the day", "Change the message of the day."),
    mod_power(P::ModAudit, "mod_audit", "Audit log", "Read the log of moderation actions."),
];

/// The description of a permission.
pub fn info(p: P) -> &'static Perm {
    PERMS.iter().find(|x| x.p == p).expect("every permission is in PERMS")
}

/// The permission with this stored name.
pub fn by_key(key: &str) -> Option<&'static Perm> {
    PERMS.iter().find(|x| x.key == key)
}

/// Permission → key of the config's `anonymous_permissions` that is its default for anonymous
/// accounts (before the admin panel sets one).
pub const ANON_CONFIG_KEYS: &[(P, &str)] = &[
    (P::Upload, "upload"),
    (P::UploadUrl, "upload"),
    (P::Comment, "comment"),
    (P::CommentFiles, "comment_attachments"),
    (P::PollVote, "poll_vote"),
    (P::Tag, "tag"),
    (P::Rate, "rate_item"),
    (P::Fav, "favorite"),
    (P::Chan, "chan"),
];

/// Whose defaults apply to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Anon,
    User,
    Mod,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::User, Role::Anon, Role::Mod];

    pub fn key(self) -> &'static str {
        match self {
            Role::Anon => "anon",
            Role::User => "user",
            Role::Mod => "mod",
        }
    }

    pub fn from_key(k: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.key() == k)
    }

    pub fn label(self) -> &'static str {
        match self {
            Role::Anon => "Anonymous accounts",
            Role::User => "Users",
            Role::Mod => "Moderators",
        }
    }

    pub fn of(s: &Session) -> Role {
        if s.is_moderator {
            Role::Mod
        } else if s.is_anonymous_user() {
            Role::Anon
        } else {
            Role::User
        }
    }
}

/// Role defaults and group permissions set in the admin panel (site setting `permissions`).
/// Only what was set is stored: a missing entry means "the built-in default" (roles) or "no say"
/// (groups).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermConfig {
    #[serde(default)]
    pub roles: BTreeMap<String, BTreeMap<String, bool>>,
    #[serde(default)]
    pub groups: BTreeMap<String, BTreeMap<String, bool>>,
}

impl Default for PermConfig {
    /// Before anything is saved: the `4chan` group has the 4chan viewer (as it had before
    /// permissions existed).
    fn default() -> Self {
        let mut groups = BTreeMap::new();
        groups.insert("4chan".to_string(), BTreeMap::from([("chan".to_string(), true)]));
        PermConfig { roles: BTreeMap::new(), groups }
    }
}

impl PermConfig {
    /// The stored setting; unknown permissions, roles and malformed parts are dropped.
    pub fn parse(text: &str) -> PermConfig {
        let Ok(v) = serde_json::from_str::<Value>(text) else { return PermConfig::default() };
        let map = |v: Option<&Value>| -> BTreeMap<String, BTreeMap<String, bool>> {
            v.and_then(Value::as_object)
                .map(|m| m.iter().map(|(k, v)| (k.clone(), parse_overrides(v))).collect())
                .unwrap_or_default()
        };
        let mut roles = map(v.get("roles"));
        roles.retain(|k, _| Role::from_key(k).is_some());
        let mut groups = map(v.get("groups"));
        groups.retain(|k, _| valid_group(k));
        PermConfig { roles, groups }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// The default of a role for a permission: admin panel, else built in.
    pub fn role_default(&self, cfg: &Config, role: Role, perm: &Perm) -> bool {
        if let Some(v) = self.roles.get(role.key()).and_then(|m| m.get(perm.key)) {
            return *v && !(perm.mod_power && role != Role::Mod);
        }
        builtin_default(cfg, role, perm)
    }
}

/// The built-in default (no admin panel setting) of a role for a permission.
pub fn builtin_default(cfg: &Config, role: Role, perm: &Perm) -> bool {
    match role {
        Role::User => perm.user,
        Role::Mod => perm.moderator,
        Role::Anon => match ANON_CONFIG_KEYS.iter().find(|(p, _)| *p == perm.p) {
            Some((_, key)) => anon_config(cfg, key, perm.anon),
            None => perm.anon,
        },
    }
}

/// `anonymous_permissions.<key>` of the config (also under `websrv.`), else `fallback`.
fn anon_config(cfg: &Config, key: &str, fallback: bool) -> bool {
    let from_cfg = match cfg.get("anonymous_permissions") {
        Value::Object(m) => Some(m),
        _ => cfg.get("websrv.anonymous_permissions").as_object(),
    };
    match from_cfg.and_then(|m| m.get(key)) {
        Some(v) => crate::config::truthy(v),
        None => crate::settings::default_anon_permission(key).unwrap_or(fallback),
    }
}

/// A group name as stored on users (routes/admin/users.rs `parse_groups`): lower case, no commas.
pub fn valid_group(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name == name.trim() && name == name.to_lowercase() && !name.contains(',')
}

/// `{"comment": false, ...}` → known permissions only (bools, or "allow"/"deny").
pub fn parse_overrides(v: &Value) -> BTreeMap<String, bool> {
    let Some(m) = v.as_object() else { return BTreeMap::new() };
    m.iter()
        .filter(|(k, _)| by_key(k).is_some())
        .filter_map(|(k, v)| {
            let b = match v {
                Value::Bool(b) => *b,
                Value::String(s) if s == "allow" => true,
                Value::String(s) if s == "deny" => false,
                _ => return None,
            };
            Some((k.clone(), b))
        })
        .collect()
}

/// Does this session have the permission? (See the module docs for the order.)
pub fn resolve(cfg: &Config, pc: &PermConfig, s: &Session, p: P) -> bool {
    resolve_perm(cfg, pc, s, info(p))
}

fn resolve_perm(cfg: &Config, pc: &PermConfig, s: &Session, perm: &Perm) -> bool {
    if s.admin {
        return true;
    }
    if s.user.is_empty() {
        return false;
    }
    if perm.mod_power && !s.is_moderator {
        return false;
    }
    let role = Role::of(s);
    if role == Role::Anon && !crate::settings::enable_anonymous_access(cfg) {
        return false;
    }
    if let Some(v) = s.permissions.get(perm.key) {
        return *v;
    }
    let mut allowed = false;
    for g in &s.groups {
        match pc.groups.get(g).and_then(|m| m.get(perm.key)) {
            Some(false) => return false,
            Some(true) => allowed = true,
            None => {}
        }
    }
    allowed || pc.role_default(cfg, role, perm)
}

/// [`resolve`] with the site's settings; `None` (no session) has no permissions.
pub fn can(st: &crate::state::AppState, s: Option<&Session>, p: P) -> bool {
    match s {
        Some(s) => resolve(&st.cfg(), &st.settings.permissions(), s, p),
        None => false,
    }
}

/// May change an item's details (title, OC mark, visibility, expiry, thumbnail): its uploader
/// with `edit_own_items`, anyone with `mod_edit_items`.
pub fn may_edit_item(st: &crate::state::AppState, s: Option<&Session>, is_owner: bool) -> bool {
    (is_owner && can(st, s, P::EditOwnItems)) || can(st, s, P::ModEditItems)
}

/// May change an item's rating: its uploader with `rate`, anyone with `mod_edit_items`.
pub fn may_rate_item(st: &crate::state::AppState, s: Option<&Session>, is_owner: bool) -> bool {
    (is_owner && can(st, s, P::Rate)) || can(st, s, P::ModEditItems)
}

/// Every permission of the session (`session.can`).
pub fn all(cfg: &Config, pc: &PermConfig, s: &Session) -> BTreeMap<String, bool> {
    PERMS.iter().map(|perm| (perm.key.to_string(), resolve_perm(cfg, pc, s, perm))).collect()
}

/// The message when a permission is missing.
pub fn denied_msg(p: P) -> String {
    format!("You don't have permission for this ({}).", info(p).label)
}

/// `403 { success: false, msg }` for a missing permission.
pub fn denied(p: P) -> axum::response::Response {
    crate::http::json_resp(&json!({ "success": false, "msg": denied_msg(p), "permission": info(p).key }), 403)
}

/// The site switch that has the feature behind a permission turned off (config or admin
/// settings), if it is off: nobody can use it then, whatever the permission says. The admin pages
/// grey such permissions out.
pub fn feature_off(st: &crate::state::AppState, p: P) -> Option<&'static str> {
    let cfg = st.cfg();
    let comments_off = cfg.is_false("enable_comments");
    let off = |cond: bool, why: &'static str| cond.then_some(why);
    match p {
        P::UploadUrl => off(!cfg.truthy("websrv.web_url_upload"), "websrv.web_url_upload"),
        P::Comment | P::CommentDelete => off(comments_off, "enable_comments"),
        P::CommentFiles => off(comments_off, "enable_comments").or(off(!cfg.truthy("websrv.allow_fileupload_comments"), "websrv.allow_fileupload_comments")),
        P::CommentPoll | P::PollVote => off(comments_off, "enable_comments").or(off(!cfg.truthy("websrv.enable_comment_polls"), "websrv.enable_comment_polls")),
        P::Lists => off(cfg.is_false("websrv.userhalls_enabled"), "websrv.userhalls_enabled"),
        P::ModLists => off(cfg.is_false("websrv.halls_enabled") && cfg.is_false("websrv.userhalls_enabled"), "websrv.halls_enabled"),
        P::Message => off(!st.settings.private_messages(), "websrv.private_messages"),
        P::MessageFiles => off(!st.settings.private_messages(), "websrv.private_messages").or(off(!st.settings.dm_attachments(), "websrv.dm_attachments")),
        P::Chat | P::ModChat => off(!cfg.truthy("websrv.enable_global_chat"), "websrv.enable_global_chat"),
        P::Meme => off(!cfg.truthy("websrv.meme_creator"), "websrv.meme_creator"),
        P::ApiKeys => off(cfg.is_false("websrv.enable_user_api_keys"), "websrv.enable_user_api_keys"),
        P::Invite => off(cfg.is_false("websrv.enable_user_invites"), "websrv.enable_user_invites"),
        _ => None,
    }
}

/// `{ perm key: switch }` for every permission whose feature is off on this site.
pub fn features_off_json(st: &crate::state::AppState) -> Value {
    Value::Object(PERMS.iter().filter_map(|perm| feature_off(st, perm.p).map(|why| (perm.key.to_string(), json!(why)))).collect::<Map<_, _>>())
}

/// The catalog for the admin pages: sections with their permissions.
pub fn catalog_json() -> Value {
    let mut sections: Vec<(&str, Vec<Value>)> = Vec::new();
    for perm in PERMS {
        let entry = json!({ "key": perm.key, "label": perm.label, "desc": perm.desc, "mod_power": perm.mod_power });
        match sections.iter_mut().find(|(name, _)| *name == perm.section) {
            Some((_, list)) => list.push(entry),
            None => sections.push((perm.section, vec![entry])),
        }
    }
    Value::Array(sections.into_iter().map(|(name, perms)| json!({ "name": name, "perms": perms })).collect())
}

/// Role defaults for the admin page: `{ role: { perm: { value, builtin, set } } }`.
pub fn roles_json(cfg: &Config, pc: &PermConfig) -> Value {
    let mut out = Map::new();
    for role in Role::ALL {
        let mut m = Map::new();
        for perm in PERMS {
            if perm.mod_power && role != Role::Mod {
                continue;
            }
            let set = pc.roles.get(role.key()).and_then(|r| r.get(perm.key)).copied();
            m.insert(
                perm.key.to_string(),
                json!({ "value": pc.role_default(cfg, role, perm), "builtin": builtin_default(cfg, role, perm), "set": set }),
            );
        }
        out.insert(role.key().to_string(), Value::Object(m));
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(v: Value) -> Config {
        crate::media::test_cfg(&serde_json::to_string(&v).unwrap())
    }

    fn user(name: &str) -> Session {
        Session { user: name.into(), login: name.into(), ..Default::default() }
    }

    #[test]
    fn keys_are_unique_and_known() {
        let mut keys: Vec<&str> = PERMS.iter().map(|p| p.key).collect();
        keys.sort();
        let n = keys.len();
        keys.dedup();
        assert_eq!(n, keys.len(), "duplicate permission key");
        for p in PERMS {
            assert_eq!(info(p.p).key, p.key);
            assert_eq!(p.mod_power, p.section == "Moderation");
        }
    }

    #[test]
    fn resolution_order() {
        let c = cfg(json!({}));
        let mut pc = PermConfig::default();
        let mut s = user("bob");
        // built-in defaults
        assert!(resolve(&c, &pc, &s, P::Comment));
        assert!(!resolve(&c, &pc, &s, P::UploadLarge));
        assert!(!resolve(&c, &pc, &s, P::ModBan));
        // role default from the admin panel
        pc.roles.insert("user".into(), BTreeMap::from([("comment".into(), false)]));
        assert!(!resolve(&c, &pc, &s, P::Comment));
        // a group allows, another denies: deny wins
        pc.groups.insert("trusted".into(), BTreeMap::from([("comment".into(), true), ("upload_large".into(), true)]));
        s.groups = vec!["trusted".into()];
        assert!(resolve(&c, &pc, &s, P::Comment));
        assert!(resolve(&c, &pc, &s, P::UploadLarge));
        pc.groups.insert("muted".into(), BTreeMap::from([("comment".into(), false)]));
        s.groups.push("muted".into());
        assert!(!resolve(&c, &pc, &s, P::Comment));
        // the user's own override beats the groups
        s.permissions.insert("comment".into(), true);
        assert!(resolve(&c, &pc, &s, P::Comment));
        // mod powers only for moderators, even when granted
        s.permissions.insert("mod_ban".into(), true);
        assert!(!resolve(&c, &pc, &s, P::ModBan));
        s.is_moderator = true;
        assert!(resolve(&c, &pc, &s, P::ModBan));
        s.permissions.insert("mod_ban".into(), false);
        assert!(!resolve(&c, &pc, &s, P::ModBan));
        // admins have everything
        s.admin = true;
        assert!(resolve(&c, &pc, &s, P::ModBan));
        // no account: nothing
        assert!(!resolve(&c, &pc, &Session::default(), P::Comment));
    }

    #[test]
    fn four_chan_group_and_anon_config() {
        let c = cfg(json!({ "enable_anonymous_access": true, "anonymous_permissions": { "comment": false } }));
        let pc = PermConfig::default();
        let mut s = user("bob");
        assert!(!resolve(&c, &pc, &s, P::Chan));
        s.groups = vec!["4chan".into()];
        assert!(resolve(&c, &pc, &s, P::Chan));
        let mut anon = user("anonymous");
        anon.is_anon = true;
        assert!(!resolve(&c, &pc, &anon, P::Comment));
        assert!(resolve(&c, &pc, &anon, P::Fav));
        // anonymous access off: nothing for anonymous accounts
        let off = cfg(json!({ "enable_anonymous_access": false }));
        assert!(!resolve(&off, &pc, &anon, P::Fav));
    }

    #[test]
    fn parsing_drops_unknown() {
        let pc = PermConfig::parse(r#"{"roles":{"user":{"comment":false,"nope":true},"guest":{"fav":true}},"groups":{"VIP":{"fav":true},"vip":{"fav":"deny","x":1}}}"#);
        assert_eq!(pc.roles.len(), 1);
        assert_eq!(pc.roles["user"].len(), 1);
        assert_eq!(pc.groups.len(), 1);
        assert_eq!(pc.groups["vip"], BTreeMap::from([("fav".to_string(), false)]));
        assert_eq!(PermConfig::parse("garbage"), PermConfig::default());
    }
}
