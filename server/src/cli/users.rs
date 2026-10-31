//! Accounts and keys: `seed` (scripts/seed.mjs), `create-admin` (scripts/create-admin.mjs),
//! `users` + `hash-password` (debug/user-admin.mjs), `chat-upload-key` (scripts/chat-upload-key.mjs).

use super::{Args, read_line};
use crate::db::RowExt;
use crate::sql;
use crate::state::AppState;
use anyhow::{Result, bail};

/// Same minimum as passwords set from the admin panel.
const MIN_PASSWORD: usize = 20;

const SETTINGS: [(&str, &str); 8] = [
    ("motd", "Hello World!"),
    ("manual_approval", "false"),
    ("min_tags", "0"),
    // true: open registration with e-mail; false: registration by invite token
    ("registration_open", "true"),
    ("trusted_uploads", "0"),
    ("about_text", ""),
    ("rules_text", ""),
    ("terms_text", ""),
];

const TAGS: [(i32, &str); 4] = [(1, "sfw"), (2, "nsfw"), (3, "nsfp"), (4, "nsfl")];

pub async fn seed(st: &AppState) -> Result<i32> {
    println!("--- Starting Database Seed ---");
    println!("Seeding site_settings...");
    for (k, v) in SETTINGS {
        // Settings changed since (admin panel) are kept; the old script reset them
        let n = sql!("INSERT INTO site_settings (key, value) VALUES (", k, ", ", v, ") ON CONFLICT (key) DO NOTHING").execute(&st.db).await?;
        if n == 0 {
            println!("  Kept {k} (already set)");
            continue;
        }
        let shown: String = v.chars().take(30).collect();
        println!("  Set {k} = {shown}{}", if v.chars().count() > 30 { "..." } else { "" });
    }
    println!("Seeding tags...");
    for (id, tag) in TAGS {
        sql!("INSERT INTO tags (id, tag, normalized) VALUES (", id, ", ", tag, ", ", tag, ") ON CONFLICT (id) DO UPDATE SET tag = EXCLUDED.tag, normalized = EXCLUDED.normalized")
            .execute(&st.db)
            .await?;
        println!("  Tag: {tag}");
    }
    // Explicit ids don't advance the sequence
    sql!("SELECT setval('tags_id_seq', (SELECT MAX(id) FROM tags))").execute(&st.db).await?;
    println!("--- Seed Completed Successfully ---");
    Ok(0)
}

/// Password from the argument list, or from stdin when it was left out (keeps it out of the shell
/// history and the process list).
fn password_arg(args: &Args, idx: usize) -> Result<String> {
    let pw = match args.pos.get(idx) {
        Some(p) => p.clone(),
        None => read_line("Password: ")?,
    };
    if pw.chars().count() < MIN_PASSWORD {
        bail!("Password must be at least {MIN_PASSWORD} characters long.");
    }
    Ok(pw)
}

async fn insert_user_options(st: &AppState, user_id: i32, tx: &mut sqlx::PgConnection) -> Result<()> {
    let cfg = st.cfg();
    sql!(
        "insert into user_options (user_id, mode, theme, fullscreen, avatar, avatar_file, use_new_layout, disable_autoplay, disable_swiping, use_alternative_infobox) values (",
        user_id, ", 3, 'amoled', 0, null, 'default.png', ", st.settings.default_layout() == "modern", ", ", cfg.is_false("websrv.enable_autoplay"), ", ",
        cfg.is_false("websrv.enable_swiping"), ", ", cfg.enabled("websrv.user_alternative_infobox"), ")"
    )
    .execute(&mut *tx)
    .await?;
    Ok(())
}

pub async fn create_admin(st: &AppState, args: &Args) -> Result<i32> {
    let Some(username) = args.pos.first().cloned() else {
        eprintln!("Usage: f0ckm create-admin <username> ['<password>']");
        eprintln!("Without a password argument it is read from stdin. Wrap it in single quotes on the command line so the shell leaves characters like ! alone.");
        return Ok(1);
    };
    let password = password_arg(args, 1)?;
    println!("--- Creating Admin User: {username} ---");
    let login = username.to_lowercase();
    let hash = crate::util::hash_password(password).await;
    if hash.is_empty() {
        bail!("password hashing failed");
    }

    let existing = sql!(r#"select id from "user" where "login" = "#, login.clone(), r#" or "user" = "#, username.clone()).fetch_optional(&st.db).await?;
    if let Some(row) = existing {
        let id = row.i64("id").unwrap_or(0) as i32;
        println!("User already exists. Updating password and ensuring admin status for ID: {id}...");
        sql!(r#"update "user" set password = "#, hash, ", admin = true, is_moderator = true, activated = true where id = ", id).execute(&st.db).await?;
        println!("--- Admin User {username} Updated Successfully ---");
        return Ok(0);
    }

    // The first admin must be user id 1: claim it when it is free, refuse if a non-admin has it.
    let id_one = sql!(r#"select login, admin from "user" where id = 1"#).fetch_optional(&st.db).await?;
    if let Some(r) = &id_one {
        if !r.bool("admin") {
            eprintln!("Error: user id 1 belongs to the non-admin account \"{}\". The first admin must be id 1.", r.string("login"));
            eprintln!("Create the admin on a fresh database (right after loading the schema), or move that account off id 1 first.");
            return Ok(1);
        }
    }

    let mut tx = st.db.begin().await?;
    let now = crate::util::now_secs() as f64;
    let id = if id_one.is_some() {
        sql!(
            r#"insert into "user" ("login", "password", "user", "created_at", "admin", "is_moderator", "activated") values ("#,
            login, ", ", hash, ", ", username.clone(), ", to_timestamp(", now, "), true, true, true) returning id"
        )
        .fetch_scalar::<i32, _>(&mut *tx)
        .await?
    } else {
        let id = sql!(
            r#"insert into "user" ("id", "login", "password", "user", "created_at", "admin", "is_moderator", "activated") values (1, "#,
            login, ", ", hash, ", ", username.clone(), ", to_timestamp(", now, "), true, true, true) returning id"
        )
        .fetch_scalar::<i32, _>(&mut *tx)
        .await?;
        // An explicit id doesn't advance the sequence: move it past every existing id
        sql!(r#"select setval('public.user_id_seq', (select max(id) from "user"))"#).execute(&mut *tx).await?;
        id
    };
    let id = id.ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
    insert_user_options(st, id, &mut tx).await?;
    tx.commit().await?;
    println!("--- Admin User {username} Created Successfully ---");
    Ok(0)
}

pub async fn hash_password(args: &Args) -> Result<i32> {
    let pw = match args.pos.first() {
        Some(p) => p.clone(),
        None => read_line("Password: ")?,
    };
    println!("{}", crate::util::hash_password(pw).await);
    Ok(0)
}

const USERS_USAGE: &str = "\
Usage:
  f0ckm users list
  f0ckm users create <user> ['<password>'] [--admin] [--mod]
  f0ckm users passwd <user> ['<password>']
Passwords left out are read from stdin. Delete accounts from the admin panel (it reassigns their
content to deleted_user).";

pub async fn users(st: &AppState, raw: &[String]) -> Result<i32> {
    let Some(sub) = raw.first() else {
        eprintln!("{USERS_USAGE}");
        return Ok(1);
    };
    match sub.as_str() {
        "list" => {
            Args::parse(&raw[1..], &[], &[])?;
            let rows = sql!(r#"select id, login, "user", admin, is_moderator, activated, banned, created_at from "user" order by id asc"#).fetch_all(&st.db).await?;
            println!("{:>6}  {:<24} {:<24} {:<5} {:<5} {:<9} {:<6} created", "id", "login", "user", "admin", "mod", "activated", "banned");
            for r in rows {
                println!(
                    "{:>6}  {:<24} {:<24} {:<5} {:<5} {:<9} {:<6} {}",
                    r.string("id"),
                    r.string("login"),
                    r.string("user"),
                    r.bool("admin"),
                    r.bool("is_moderator"),
                    r.bool("activated"),
                    r.bool("banned"),
                    r.string("created_at")
                );
            }
            Ok(0)
        }
        "create" => {
            let args = Args::parse(&raw[1..], &["--admin", "--mod"], &[])?;
            let Some(username) = args.pos.first().cloned() else {
                eprintln!("{USERS_USAGE}");
                return Ok(1);
            };
            let password = password_arg(&args, 1)?;
            let login = username.to_lowercase();
            if sql!(r#"select id from "user" where login = "#, login.clone()).fetch_optional(&st.db).await?.is_some() {
                eprintln!("Error: User '{username}' already exists.");
                return Ok(1);
            }
            let hash = crate::util::hash_password(password).await;
            if hash.is_empty() {
                bail!("password hashing failed");
            }
            let mut tx = st.db.begin().await?;
            let id = sql!(
                r#"insert into "user" ("login", "user", "password", "admin", "is_moderator", "activated", "created_at") values ("#,
                login, ", ", username.clone(), ", ", hash, ", ", args.flag(&["--admin"]), ", ", args.flag(&["--mod"]), ", true, to_timestamp(",
                crate::util::now_secs() as f64, ")) returning id"
            )
            .fetch_scalar::<i32, _>(&mut *tx)
            .await?
            .ok_or_else(|| anyhow::anyhow!("insert returned no id"))?;
            insert_user_options(st, id, &mut tx).await?;
            tx.commit().await?;
            println!("Successfully created user '{username}' (ID: {id})");
            Ok(0)
        }
        "passwd" => {
            let args = Args::parse(&raw[1..], &[], &[])?;
            let Some(username) = args.pos.first().map(|u| u.to_lowercase()) else {
                eprintln!("{USERS_USAGE}");
                return Ok(1);
            };
            if username == "deleted_user" {
                bail!("The deleted_user account is protected.");
            }
            let password = password_arg(&args, 1)?;
            let hash = crate::util::hash_password(password).await;
            if hash.is_empty() {
                bail!("password hashing failed");
            }
            let n = sql!(r#"update "user" set password = "#, hash, " where login = ", username.clone()).execute(&st.db).await?;
            if n == 0 {
                eprintln!("Error: User '{username}' not found.");
                return Ok(1);
            }
            // Log out everywhere
            sql!(r#"delete from user_sessions where user_id = (select id from "user" where login = "#, username.clone(), ")").execute(&st.db).await?;
            println!("Successfully updated password for user '{username}'");
            Ok(0)
        }
        _ => {
            eprintln!("{USERS_USAGE}");
            Ok(1)
        }
    }
}

const KEY_USAGE: &str = "\
Usage:
  f0ckm chat-upload-key create <name> [max_mb]
  f0ckm chat-upload-key list
  f0ckm chat-upload-key revoke <id>";

/// Upload-only API keys for chat uploads (POST /api/chat/upload). The plain key is shown once;
/// only its SHA-256 is stored.
pub async fn chat_upload_key(st: &AppState, args: &Args) -> Result<i32> {
    let (cmd, arg, max_mb) = (args.pos.first().map(String::as_str), args.pos.get(1), args.pos.get(2));
    match (cmd, arg) {
        (Some("create"), Some(name)) => {
            use base64::Engine;
            let key = format!("cu_{}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::util::random_bytes(24)));
            let max_mb: Option<i64> = match max_mb {
                Some(m) => Some(m.parse().ok().filter(|n: &i64| *n > 0).ok_or_else(|| anyhow::anyhow!("max_mb must be a positive number"))?),
                None => None,
            };
            let max_bytes = max_mb.map(|m| m * 1024 * 1024);
            let id = sql!("INSERT INTO upload_api_keys (name, key_hash, max_bytes) VALUES (", name.clone(), ", ", crate::util::sha256(&key), ", ", max_bytes, ") RETURNING id")
                .fetch_scalar::<i32, _>(&st.db)
                .await?
                .unwrap_or(0);
            let limit = max_mb.map(|m| format!(" (max {m} MB)")).unwrap_or_default();
            println!("Created key #{id} \"{name}\"{limit}");
            println!("Key (shown once): {key}");
            Ok(0)
        }
        (Some("list"), None) => {
            let rows = sql!(
                "SELECT k.id, k.name, k.max_bytes, k.revoked, k.last_used_at, count(c.id)::int AS files, coalesce(sum(c.size_bytes), 0)::bigint AS bytes \
                 FROM upload_api_keys k LEFT JOIN chat_uploads c ON c.key_id = k.id GROUP BY k.id ORDER BY k.id"
            )
            .fetch_all(&st.db)
            .await?;
            println!("{:>4}  {:<24} {:<7} {:>7} {:>6} {:>9}  last used", "id", "name", "revoked", "max_mb", "files", "used_mb");
            for r in rows {
                let mb = |v: Option<f64>| v.map(|b| b / 1048576.0);
                println!(
                    "{:>4}  {:<24} {:<7} {:>7} {:>6} {:>9.1}  {}",
                    r.string("id"),
                    r.string("name"),
                    r.bool("revoked"),
                    mb(r.f64("max_bytes")).map(|m| format!("{m}")).unwrap_or_else(|| "-".into()),
                    r.string("files"),
                    mb(r.f64("bytes")).unwrap_or(0.0),
                    r.str("last_used_at").unwrap_or("-")
                );
            }
            Ok(0)
        }
        (Some("revoke"), Some(id)) => {
            let Ok(id) = id.parse::<i32>() else {
                eprintln!("{KEY_USAGE}");
                return Ok(1);
            };
            let n = sql!("UPDATE upload_api_keys SET revoked = true WHERE id = ", id).execute(&st.db).await?;
            println!("{}", if n > 0 { format!("Revoked key #{id}") } else { format!("No key #{id}") });
            Ok(0)
        }
        _ => {
            eprintln!("{KEY_USAGE}");
            Ok(1)
        }
    }
}
