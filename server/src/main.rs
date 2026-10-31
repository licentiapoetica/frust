//! f0ckm server.
#![allow(dead_code)] // many helpers are used by routes that are still being ported

mod anon_auth;
mod app;
mod auth;
mod bot;
mod config;
mod db;
mod f0cklib;
mod gate;
mod hidden_items;
mod http;
mod i18n;
mod media;
mod migrations;
mod private_items;
mod proc;
mod routes;
mod security;
mod smtp;
mod session;
mod settings;
mod social;
mod state;
mod tpl;
mod util;
mod webauthn;
mod retention;
mod wordfilter;

use anyhow::{Context, Result};
use arc_swap::ArcSwap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

fn git_hash() -> String {
    if let Ok(h) = std::env::var("GIT_HASH") {
        if !h.is_empty() && h != "unknown" {
            return h;
        }
    }
    let run = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match (run(&["rev-parse", "--abbrev-ref", "HEAD"]), run(&["rev-parse", "--short", "HEAD"])) {
        (Some(b), Some(h)) => format!("{b}-{h}"),
        _ => "unknown".into(),
    }
}

async fn wait_for_db(st: &state::AppState) {
    for attempt in 1..=30 {
        match sql!("SELECT 1 FROM information_schema.tables WHERE table_name = 'items' LIMIT 1").fetch_optional(&st.db).await {
            Ok(found) => {
                if found.is_none() {
                    tracing::info!("[BOOT] Database tables missing! Initializing schema from migrations/f0ckm_schema.sql...");
                    let schema = st.cfg().base.join("migrations/f0ckm_schema.sql");
                    match std::fs::read_to_string(&schema) {
                        Ok(text) => match sqlx::raw_sql(&text).execute(&st.db).await {
                            Ok(_) => tracing::info!("[BOOT] Database schema initialization complete!"),
                            Err(e) => tracing::error!("[BOOT] Schema initialization failed: {e}"),
                        },
                        Err(e) => tracing::error!("[BOOT] Cannot read {}: {e}", schema.display()),
                    }
                }
                tracing::info!("[BOOT] Database schema ready (took {attempt} attempt(s))");
                return;
            }
            Err(e) => {
                if attempt == 1 {
                    tracing::info!("[BOOT] Waiting for database connection... ({e})");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    tracing::error!("[BOOT] Database schema check timed out after 30 attempts!");
}

async fn ensure_nsfl_tag(st: &state::AppState) {
    use db::RowExt;
    let cfg = st.cfg();
    if !cfg.enable_nsfl() {
        return;
    }
    let id = cfg.int("nsfl_tag_id").unwrap_or(0);
    if id <= 0 {
        return;
    }
    let id32 = id as i32;
    let by_id = sql!("SELECT id, normalized FROM tags WHERE id = ", id32, " LIMIT 1").fetch_optional(&st.db).await;
    let by_name = sql!("SELECT id FROM tags WHERE normalized = 'nsfl' LIMIT 1").fetch_optional(&st.db).await;
    match (by_id, by_name) {
        (Ok(Some(t)), _) if t.str("normalized") != Some("nsfl") => {
            tracing::warn!("[BOOT] nsfl_tag_id {id} points to tag \"{}\", not \"nsfl\" — check config", t.string("normalized"))
        }
        (Ok(None), Ok(Some(n))) => {
            tracing::warn!("[BOOT] NSFL tag exists as id {} but nsfl_tag_id is {id} — set nsfl_tag_id: {} in config", n.string("id"), n.string("id"))
        }
        (Ok(None), Ok(None)) => {
            if sql!("INSERT INTO tags (id, tag) VALUES (", id32, ", 'nsfl') ON CONFLICT DO NOTHING").execute(&st.db).await.is_ok() {
                tracing::info!("[BOOT] Created missing NSFL rating tag (id {id})");
            }
        }
        (Err(e), _) | (_, Err(e)) => tracing::error!("[BOOT] NSFL tag check failed: {e}"),
        _ => {}
    }
}

async fn backfill_slugs(st: &state::AppState) {
    for table in ["items", "album_items"] {
        let ids: Vec<i32> = match sql!("SELECT id FROM ").push(table).push(" WHERE slug IS NULL OR slug = ''").fetch_scalar_list(&st.db).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[SLUG_BACKFILL] {table}: {e}");
                continue;
            }
        };
        if ids.is_empty() {
            continue;
        }
        tracing::info!("[SLUG_BACKFILL] Found {} {table} row(s) missing slugs. Backfilling...", ids.len());
        for id in ids {
            let mut q = sql!("UPDATE ");
            q.push(table).push(" SET slug = ").bind(util::generate_slug(11)).push(" WHERE id = ").bind(id).push(" AND (slug IS NULL OR slug = '')");
            let _ = q.execute(&st.db).await;
        }
    }
}

fn spawn_background_tasks(st: &state::AppState) {
    webauthn::spawn_cleanup();
    let cfg = st.cfg();

    // Stale sessions (unused for 30 days), every 6 hours
    let s = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let cutoff = util::now_secs() - 30 * 24 * 3600;
            match sql!("DELETE FROM user_sessions WHERE last_used <= ", cutoff as i32).execute(&s.db).await {
                Ok(n) if n > 0 => tracing::info!("[SESSION CLEANUP] Purged {n} stale sessions (unused >30 days)"),
                Ok(_) => {}
                Err(e) => tracing::error!("[SESSION CLEANUP] Failed: {e}"),
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    });

    // Tag preview cache (rebuilt on tag changes) and the ranking page's disk usage figure
    routes::tag_image::spawn_listener(st);
    routes::ranking::spawn_disk_size_task(st);

    // Expired uploads, 10 s after boot then every 30 s
    media::delete::spawn_expiry_purger(st);

    // Postgres LISTEN → live event stream (/api/notifications/stream)
    routes::notifications::spawn_listener(st);

    // Expired DM attachments, every 6 hours
    routes::messages::spawn_cleanup(st);

    // Private/unavailable item index, every 30 s
    let s = st.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            private_items::init(&s).await;
        }
    });

    // Inactivity ban
    let days = cfg.int("websrv.inactivity_ban_days").unwrap_or(0);
    if days <= 0 {
        tracing::info!("[BOOT] Inactivity ban: DISABLED (set websrv.inactivity_ban_days > 0 to enable)");
    } else {
        tracing::info!("[BOOT] Inactivity ban: enabled — threshold {days} days (config: websrv.inactivity_ban_days)");
        let s = st.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            loop {
                ban_inactive_users(&s, days).await;
                tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
            }
        });
    }
}

async fn ban_inactive_users(st: &state::AppState, days: i64) {
    use db::RowExt;
    let cutoff = (util::now_secs() - days * 24 * 3600) as i32;
    let targets = match sql!(
        r#"SELECT id, login FROM "user" WHERE banned = false AND activated = true AND admin = false AND is_moderator = false AND login != 'deleted_user' AND last_seen <= "#,
        cutoff
    )
    .fetch_all(&st.db)
    .await
    {
        Ok(t) => t,
        Err(e) => return tracing::error!("[INACTIVITY BAN] Failed: {e}"),
    };
    if targets.is_empty() {
        return;
    }
    let ids: Vec<i32> = targets.iter().filter_map(|r| r.i64("id")).map(|i| i as i32).collect();
    let reason = format!("Inactivity (no login for {days}+ days)");
    let res = async {
        sql!(r#"UPDATE "user" SET banned = true, ban_reason = "#, reason, ", ban_expires = NULL WHERE id = ANY(", ids.clone(), ")").execute(&st.db).await?;
        sql!("DELETE FROM user_sessions WHERE user_id = ANY(", ids, ")").execute(&st.db).await?;
        anyhow::Ok(())
    }
    .await;
    match res {
        Ok(()) => tracing::info!(
            "[INACTIVITY BAN] Banned {} inactive account(s) (threshold: {days} days): {}",
            targets.len(),
            targets.iter().map(|r| r.string("login")).collect::<Vec<_>>().join(", ")
        ),
        Err(e) => tracing::error!("[INACTIVITY BAN] Failed: {e}"),
    }
}

/// Reload config.yaml when it changes (a file that fails to parse is ignored).
fn watch_config(st: &state::AppState) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let s = st.clone();
    let base = st.cfg().base.clone();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if ev.paths.iter().any(|p| p.file_name().map(|n| n == "config.yaml").unwrap_or(false)) {
                let _ = tx.send(());
            }
        }
    })
    .ok()?;
    w.watch(&base, notify::RecursiveMode::NonRecursive).ok()?;
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            while rx.try_recv().is_ok() {}
            match s.cfg().reload() {
                Ok(c) => {
                    s.cfg.store(Arc::new(c));
                    tracing::info!("[CONFIG] config.yaml reloaded");
                }
                Err(e) => tracing::error!("[CONFIG] config.yaml reload failed, keeping the running config: {e:#}"),
            }
        }
    });
    Some(w)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info,sqlx=warn,tower_http=warn".into()))
        .with_target(false)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let base = std::env::var("F0CKM_BASE").map(std::path::PathBuf::from).unwrap_or(std::env::current_dir()?);
    let cfg = config::Config::load(&base)?;

    for dir in [
        &cfg.paths.a,
        &cfg.paths.b,
        &cfg.paths.c,
        &cfg.paths.t,
        &cfg.paths.ca,
        &cfg.paths.emojis,
        &cfg.paths.memes,
        &cfg.paths.tmp,
        &cfg.paths.logs,
        &cfg.paths.pending.join("b"),
        &cfg.paths.pending.join("t"),
        &cfg.paths.pending.join("ca"),
        &cfg.paths.deleted.join("b"),
        &cfg.paths.deleted.join("t"),
        &cfg.paths.deleted.join("ca"),
    ] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            tracing::warn!("[BOOT] Could not create directory {}: {e}", dir.display());
        }
    }

    let pool = db::connect(&cfg).await?;
    let i18n = Arc::new(i18n::I18n::new(base.join("src/inc/locales")));
    let tpl = tpl::Templates::new(base.join("views"), i18n.clone());

    if std::env::args().nth(1).as_deref() == Some("check-templates") {
        let errors = tpl.check_all(&base.join("views"));
        for (f, e) in &errors {
            println!("{f}: {e}");
        }
        println!("{} template(s) with errors", errors.len());
        std::process::exit(if errors.is_empty() { 0 } else { 1 });
    }
    let port = std::env::var("F0CKM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(cfg.port());
    let st = state::AppState(Arc::new(state::Inner {
        settings: settings::Settings::new(&cfg),
        cfg: ArcSwap::from_pointee(cfg),
        db: pool,
        tpl,
        i18n,
        caches: state::Caches::new(),
        git_hash: git_hash(),
        process_secret: util::random_hex(32),
        http: reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?,
        bot: bot::BotHandle::new(),
        router: std::sync::OnceLock::new(),
    }));
    tracing::info!("Git hash: {}", st.git_hash);

    wait_for_db(&st).await;
    for m in migrations::STARTUP_MIGRATIONS {
        let _ = sqlx::raw_sql(m).execute(&st.db).await;
    }
    ensure_nsfl_tag(&st).await;
    settings::load_from_db(&st).await;
    private_items::init(&st).await;
    if st.cfg().enabled("websrv.halls_enabled") {
        st.update_halls_cache().await;
    }
    {
        let c = st.cfg();
        tracing::info!("[BOOT] Halls: {}", if c.enabled("websrv.halls_enabled") { "ENABLED" } else { "DISABLED" });
        tracing::info!("[BOOT] UserHalls: {}", if c.enabled("websrv.userhalls_enabled") { "ENABLED" } else { "DISABLED" });
        tracing::info!("[BOOT] Abyss: {}", if c.enabled("websrv.abyss_enabled") { "ENABLED" } else { "DISABLED" });
        if c.truthy("websrv.hash_user_ips") {
            tracing::info!("[BOOT] IP hash secret: {}", security::ip_hash_secrets(&c).source);
        }
    }
    {
        let s = st.clone();
        tokio::spawn(async move { backfill_slugs(&s).await });
    }

    let router = routes::build();
    tracing::info!("[BOOT] {} routes registered", router.len());
    let _ = st.router.set(Arc::new(router));

    spawn_background_tasks(&st);
    retention::start(&st);
    bot::start(&st);
    let _config_watcher = watch_config(&st);

    let app = axum::Router::new().fallback(app::handle).with_state(st.clone());
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("cannot listen on {addr}"))?;
    tracing::info!("[BOOT] listening on {addr}");
    axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await?;
    Ok(())
}
