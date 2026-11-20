//! Admin panel → Permissions: the defaults of each role and the permissions of user groups
//! (crate::perms). The users' own overrides and groups are set in the User Manager (users.rs).
//!
//! Every form is a plain post (no JavaScript: `back` turns the JSON reply into a redirect with a
//! flash message, crate::nojs); permissions.js saves them in place.

use super::{body_str, save_setting};
use crate::db::RowExt;
use crate::http::{Guard, Req, Res, Router, html, json_resp};
use crate::perms::{self, PERMS, PermConfig, Role};
use crate::sql;
use crate::state::AppState;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

pub fn register(r: &mut Router) {
    r.get_g(r"^\/admin\/permissions\/?$", Guard::Auth, page);
    r.post_g(r"^\/admin\/permissions\/roles\/?$", Guard::Auth, save_roles);
    r.post_g(r"^\/admin\/permissions\/group\/?$", Guard::Auth, save_group);
    r.post_g(r"^\/admin\/permissions\/group\/delete\/?$", Guard::Auth, delete_group);
}

/// Every group: the ones with permissions and the ones users are in, with their member counts.
pub(crate) async fn known_groups(st: &AppState) -> Vec<(String, i64)> {
    let mut groups: BTreeMap<String, i64> = st.settings.permissions().groups.keys().map(|g| (g.clone(), 0)).collect();
    let rows = sql!(r#"SELECT g, count(*)::bigint AS n FROM "user", unnest(groups) AS g GROUP BY g"#).fetch_all(&st.db).await.unwrap_or_default();
    for r in rows {
        let name = r.string("g");
        if perms::valid_group(&name) {
            groups.insert(name, r.i64("n").unwrap_or(0));
        }
    }
    groups.into_iter().collect()
}

async fn page(st: AppState, req: Req) -> Res {
    let cfg = st.cfg();
    let pc = st.settings.permissions();
    let groups: Vec<Value> = known_groups(&st)
        .await
        .into_iter()
        .map(|(name, members)| {
            let set = pc.groups.get(&name).cloned().unwrap_or_default();
            json!({ "name": name, "members": members, "perms": set, "configured": pc.groups.contains_key(&name) })
        })
        .collect();
    let data = json!({
        "session": req.session_value(),
        "catalog": perms::catalog_json(),
        "perm_off": perms::features_off_json(&st),
        "roles": Role::ALL.iter().map(|r| json!({ "key": r.key(), "label": r.label() })).collect::<Vec<_>>(),
        "role_values": perms::roles_json(&cfg, &pc),
        "groups": groups,
        "anon_access": crate::settings::enable_anonymous_access(&cfg),
        "tmp": null,
    });
    Ok(html(crate::tpl::render(&st, Some(&req), "admin/permissions", data)))
}

/// A select's value; repeated fields join with commas, the last one counts.
fn choice(body: &Value, key: &str) -> Option<String> {
    body_str(body, key).map(|v| v.rsplit(',').next().unwrap_or("").to_string())
}

async fn store(st: &AppState, pc: PermConfig) -> anyhow::Result<()> {
    save_setting(st, "permissions", &pc.to_json()).await?;
    st.settings.update(|s| s.permissions = pc);
    Ok(())
}

fn reply(r: anyhow::Result<Value>) -> Res {
    Ok(match r {
        Ok(v) => json_resp(&v, 200),
        Err(e) => json_resp(&json!({ "success": false, "msg": super::err_msg(&e) }), 400),
    })
}

/// `POST /admin/permissions/roles`: `<role>.<perm>` = `allow` / `deny` / `default` for every
/// select on the page; a role whose fields are missing keeps its settings.
async fn save_roles(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let mut pc = st.settings.permissions();
        let mut changed = 0;
        for role in Role::ALL {
            let mut set: BTreeMap<String, bool> = pc.roles.get(role.key()).cloned().unwrap_or_default();
            for perm in PERMS {
                if perm.mod_power && role != Role::Mod {
                    continue;
                }
                let Some(v) = choice(&body, &format!("{}.{}", role.key(), perm.key)) else { continue };
                let before = set.get(perm.key).copied();
                match v.as_str() {
                    "allow" => set.insert(perm.key.to_string(), true),
                    "deny" => set.insert(perm.key.to_string(), false),
                    _ => set.remove(perm.key),
                };
                if before != set.get(perm.key).copied() {
                    changed += 1;
                }
            }
            if set.is_empty() {
                pc.roles.remove(role.key());
            } else {
                pc.roles.insert(role.key().to_string(), set);
            }
        }
        store(&st, pc.clone()).await?;
        let uid = req.session().map(|s| s.id).unwrap_or(0);
        crate::audit::log(&st, uid, "admin_set_role_permissions", "settings", "permissions".to_string(), json!({ "roles": pc.roles, "changed": changed })).await;
        Ok(json!({ "success": true, "msg": "Role defaults saved", "role_values": perms::roles_json(&st.cfg(), &pc) }))
    }
    .await;
    reply(r)
}

/// A group name as typed: trimmed, lower case (like the User Manager stores it).
fn group_name(body: &Value) -> anyhow::Result<String> {
    let name = body_str(body, "name").unwrap_or_default().trim().to_lowercase();
    if !perms::valid_group(&name) {
        anyhow::bail!("A group name: up to 64 characters, no commas");
    }
    Ok(name)
}

/// `POST /admin/permissions/group`: `name` and `perm_<key>` = `allow` / `deny` / `inherit`.
/// Creates the group when it is new.
async fn save_group(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let name = group_name(&body)?;
        let mut set: BTreeMap<String, bool> = BTreeMap::new();
        for perm in PERMS {
            match choice(&body, &format!("perm_{}", perm.key)).as_deref() {
                Some("allow") => set.insert(perm.key.to_string(), true),
                Some("deny") => set.insert(perm.key.to_string(), false),
                _ => None,
            };
        }
        let mut pc = st.settings.permissions();
        let created = !pc.groups.contains_key(&name);
        pc.groups.insert(name.clone(), set.clone());
        store(&st, pc).await?;
        let uid = req.session().map(|s| s.id).unwrap_or(0);
        crate::audit::log(&st, uid, "admin_set_group_permissions", "group", name.clone(), json!({ "permissions": set })).await;
        let msg = if created { format!("Group \"{name}\" created") } else { format!("Group \"{name}\" saved") };
        Ok(json!({ "success": true, "msg": msg, "group": name, "created": created, "perms": Value::Object(set.into_iter().map(|(k, v)| (k, json!(v))).collect::<Map<_, _>>()) }))
    }
    .await;
    reply(r)
}

/// `POST /admin/permissions/group/delete`: drops the group's permissions. Its members keep the
/// group name (the User Manager removes it from them).
async fn delete_group(st: AppState, mut req: Req) -> Res {
    let body = req.post().await?.clone();
    let r = async {
        let name = group_name(&body)?;
        let mut pc = st.settings.permissions();
        if pc.groups.remove(&name).is_none() {
            anyhow::bail!("Group \"{name}\" has no permissions");
        }
        store(&st, pc).await?;
        let uid = req.session().map(|s| s.id).unwrap_or(0);
        crate::audit::log(&st, uid, "admin_delete_group_permissions", "group", name.clone(), json!({})).await;
        Ok(json!({ "success": true, "msg": format!("Permissions of group \"{name}\" removed"), "group": name }))
    }
    .await;
    reply(r)
}
