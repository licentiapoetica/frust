//! f0ckm server.
#![allow(dead_code)] // many helpers are used by routes that are still being ported

mod anon_auth;
mod app;
mod audit;
mod auth;
mod burn;
mod bot;
mod cap;
mod cap_image;
mod cli;
mod chan_http;
mod config;
mod config_write;
mod csp;
mod db;
mod f0cklib;
mod fonts;
mod gate;
mod hidden_items;
mod http;
mod i18n;
mod maintenance;
mod media;
mod media_gate;
mod migrations;
mod net_guard;
mod nojs;
mod perms;
mod private_items;
mod proc;
mod pwa;
mod remote;
mod routes;
mod scraper_guard;
mod security;
mod smtp;
mod session;
mod settings;
mod social;
mod state;
mod tor_listener;
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

/// Version shown in the footer: the short commit hash from `GIT_HASH` (baked into the Docker
/// image), else from the checkout. Only the hash, never the branch: older images carry
/// `f0ckm-<target>-<hash>`, so the last `-` part is taken. Empty when unknown, so the footer link
/// is left out.
fn git_hash() -> String {
    if let Ok(h) = std::env::var("GIT_HASH") {
        let h = h.trim().rsplit('-').next().unwrap_or("");
        if !h.is_empty() && h != "unknown" {
            return h.to_string();
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
    run(&["rev-parse", "--short", "HEAD"]).unwrap_or_default()
}

async fn wait_for_db(st: &state::AppState) {
    for attempt in 1..=30 {
        match sql!("SELECT 1 FROM information_schema.tables WHERE table_name = 'items' LIMIT 1").fetch_optional(&st.db).await {
            Ok(found) => {
                if found.is_none() {
                    tracing::info!("[BOOT] Database tables missing! Initializing schema from migrations/f0ckm_schema.sql...");
                    let schema = st.cfg().base.join("migrations/f0ckm_schema.sql");
                    match std::fs::read_to_string(&schema) {
                        // Schema file from the repository (trusted), run as one script
                        Ok(text) => match sqlx::raw_sql(sqlx::AssertSqlSafe(text.as_str())).execute(&st.db).await {
                            Ok(_) => {
                                tracing::info!("[BOOT] Database schema initialization complete!");
                                // Fresh database: default settings and rating tags, so `f0ckm seed` isn't a setup step.
                                // Only here, since seeding resets renamed rating tags.
                                if let Err(e) = cli::seed(st).await {
                                    tracing::error!("[BOOT] Seeding the new database failed: {e}");
                                }
                            }
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

pub(crate) async fn backfill_slugs(st: &state::AppState) {
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

    // Play time of videos and songs (timecodes on thumbnails), every 30 s
    media::duration::spawn_filler(st);

    // Postgres LISTEN → live event socket (/api/notifications/ws)
    routes::notifications::spawn_listener(st);

    // A remote f0ck mirrored as a data source (remote.interval, see remote.rs)
    remote::spawn_sync(st);

    // Expired DM attachments (every 6 hours)
    routes::messages::spawn_cleanup(st);

    // Unused invite tokens older than 7 days (hourly)
    routes::register::spawn_invite_expiry(st);

    // Private/unavailable item index, every 30 s
    let s = st.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            private_items::init(&s).await;
            burn::init(&s).await;
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
            // Reading the file ourselves raises access events: only react to changes
            if ev.kind.is_access() {
                return;
            }
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
                    // Site settings derived from the config (DB values still win)
                    settings::load_from_db(&s).await;
                    tracing::info!("[CONFIG] config.yaml reloaded");
                }
                Err(e) => tracing::error!("[CONFIG] config.yaml reload failed, keeping the running config: {e:#}"),
            }
        }
    });
    Some(w)
}

/// Translations directory.
pub fn locales_dir(base: &std::path::Path) -> std::path::PathBuf {
    base.join("locales")
}

/// Shared state for the server and the CLI commands.
pub fn new_state(cfg: config::Config, db: sqlx::PgPool, i18n: Arc<i18n::I18n>, tpl: tpl::Templates) -> Result<state::AppState> {
    Ok(state::AppState(Arc::new(state::Inner {
        settings: settings::Settings::new(&cfg),
        cfg: ArcSwap::from_pointee(cfg),
        db,
        tpl,
        i18n,
        caches: state::Caches::new(),
        git_hash: git_hash(),
        process_secret: util::random_hex(32),
        http: reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?,
        bot: bot::BotHandle::new(),
        router: std::sync::OnceLock::new(),
    })))
}

/// Wait for postgres, create the schema on an empty database and apply the startup migrations.
///
/// The migrations also run while the site is live (CLI commands like `remote-sync`, a second
/// container). `ALTER TABLE ... IF NOT EXISTS` takes its exclusive lock even when there is nothing
/// to do, and while it waits behind a running query every new query on that table queues behind
/// it: one slow query froze every item page for seconds. So each statement only waits briefly for
/// its lock and tries again later instead of holding the queue.
pub async fn prepare_db(st: &state::AppState) {
    const ATTEMPTS: u32 = 20;
    wait_for_db(st).await;
    let mut conn = match st.db.acquire().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("[BOOT] startup migrations skipped, no database connection: {e}");
            return;
        }
    };
    let _ = sqlx::raw_sql("SET lock_timeout = '500ms'").execute(&mut *conn).await;
    for m in migrations::STARTUP_MIGRATIONS {
        let first = m.lines().next().unwrap_or("").trim();
        for attempt in 1..=ATTEMPTS {
            match sqlx::raw_sql(*m).execute(&mut *conn).await {
                Ok(_) => break,
                // 55P03 lock_not_available: the table is busy, try again in a moment
                Err(e) if attempt < ATTEMPTS && e.as_database_error().and_then(|d| d.code()).as_deref() == Some("55P03") => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => {
                    tracing::warn!("[BOOT] startup migration failed ({first}): {e}");
                    break;
                }
            }
        }
    }
    let _ = sqlx::raw_sql("RESET lock_timeout").execute(&mut *conn).await;
}

#[tokio::main]
async fn main() -> Result<()> {
    // `f0ckm <command> ...` runs a CLI command (see `f0ckm help`); no argument or `serve` starts the server.
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().filter(|a| a.as_str() != "serve").cloned();
    // CLI commands log warnings to stderr (their output goes to stdout); the server logs info to stdout.
    let (default_filter, writer) = match command {
        Some(_) => ("warn", tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stderr)),
        None => ("info,sqlx=warn,tower_http=warn", tracing_subscriber::fmt::writer::BoxMakeWriter::new(std::io::stdout)),
    };
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| default_filter.into()))
        .with_target(false)
        .with_writer(writer)
        .init();
    let _ = rustls::crypto::ring::default_provider().install_default();

    let base = std::env::var("F0CKM_BASE").map(std::path::PathBuf::from).unwrap_or(std::env::current_dir()?);
    if command.is_some() {
        if let Err(e) = net_guard::start().await {
            eprintln!("outgoing request guard failed to start: {e}");
        }
        std::process::exit(cli::run(&base, &args).await);
    }
    let cfg = config::Config::load(&base)?;

    // Before anything can run ImageMagick on a user file (the tmp dir is created just below)
    let _ = std::fs::create_dir_all(&cfg.paths.tmp);
    proc::install_magick_policy(&cfg.paths.tmp);
    media::warn_if_socks_not_tor(&cfg);
    if security::ip_hash_secrets(&cfg).current == "YOUR_SECRET_HERE" || cfg.str("main.invite_secret") == Some("YOUR_SECRET_HERE") {
        // The example value is public: hashed IPs could be reversed by trying all addresses
        tracing::warn!("[BOOT] main.invite_secret is still YOUR_SECRET_HERE from config_example.yaml. Set a random secret (and IP_HASH_SECRET).");
    }

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
    let i18n = Arc::new(i18n::I18n::new(locales_dir(&base)));
    let tpl = tpl::Templates::new(base.join("views"), i18n.clone());
    cli::build_css::ensure_bundle(&base);

    let port = std::env::var("F0CKM_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(cfg.port());
    let st = new_state(cfg, pool, i18n, tpl)?;
    tracing::info!("Git hash: {}", if st.git_hash.is_empty() { "unknown" } else { &st.git_hash });

    net_guard::start().await.context("outgoing request guard")?;
    prepare_db(&st).await;
    ensure_nsfl_tag(&st).await;
    settings::load_from_db(&st).await;
    private_items::init(&st).await;
    burn::init(&st).await;
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
    {
        // PWA icons (192/512 px) from the favicon, so /manifest.json has them from the start
        let s = st.clone();
        tokio::spawn(async move { pwa::ensure_icons(&s.cfg(), &s.settings.favicon_url()).await });
    }

    let router = routes::build();
    tracing::info!("[BOOT] {} routes registered", router.len());
    let _ = st.router.set(Arc::new(router));

    spawn_background_tasks(&st);
    retention::start(&st);
    bot::start(&st);
    let _config_watcher = watch_config(&st);

    let app = axum::Router::new().fallback(app::handle).with_state(st.clone());
    // The .onion service's own port (crate::tor_listener): Tor tells f0ckm each visitor's circuit there
    if st.cfg().truthy("websrv.enable_tor_hs") {
        let tor_port = st.cfg().int("websrv.tor_port").and_then(|p| u16::try_from(p).ok()).unwrap_or(1338);
        let tor_addr = SocketAddr::from(([0, 0, 0, 0], tor_port));
        match tokio::net::TcpListener::bind(tor_addr).await {
            Ok(l) => {
                tracing::info!("[BOOT] Tor service port on {tor_addr}");
                tokio::spawn(tor_listener::serve(l, app.clone()));
            }
            Err(e) => tracing::error!("[BOOT] cannot listen on {tor_addr} for the Tor service: {e}"),
        }
    }
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await.with_context(|| format!("cannot listen on {addr}"))?;
    tracing::info!("[BOOT] listening on {addr}");
    serve(listener, app).await
}

/// Seconds a client gets to send its request headers. axum::serve builds hyper without a timer, so
/// hyper's header timeout never applied: a connection that never finished its headers stayed
/// open forever (slowloris) when f0ckm is reachable without nginx in front.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// `axum::serve` with [`HEADER_READ_TIMEOUT`]; requests carry `ConnectInfo<SocketAddr>` as before.
async fn serve(listener: tokio::net::TcpListener, app: axum::Router) -> Result<()> {
    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(c) => c,
            Err(e) => {
                // Out of file descriptors and the like: back off instead of spinning
                tracing::warn!("[HTTP] accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        tokio::spawn(serve_connection(stream, remote, None, app.clone()));
    }
}

/// One connection: HTTP/1 with [`HEADER_READ_TIMEOUT`]. Requests carry `ConnectInfo(remote)` and,
/// from the Tor port, the visitor's circuit (tor_listener::Circuit).
async fn serve_connection(stream: tokio::net::TcpStream, remote: SocketAddr, circuit: Option<tor_listener::Circuit>, app: axum::Router) {
    use hyper_util::rt::{TokioIo, TokioTimer};
    use hyper_util::service::TowerToHyperService;
    use tower::ServiceExt;
    let _ = stream.set_nodelay(true);
    let svc = app.map_request(move |mut req: axum::http::Request<hyper::body::Incoming>| {
        req.extensions_mut().insert(axum::extract::ConnectInfo(remote));
        if let Some(c) = circuit {
            req.extensions_mut().insert(c);
        }
        req.map(axum::body::Body::new)
    });
    // HTTP/1 only, like axum::serve was built here (no http2 feature)
    let mut builder = hyper::server::conn::http1::Builder::new();
    builder.timer(TokioTimer::new()).header_read_timeout(HEADER_READ_TIMEOUT);
    let conn = builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(svc)).with_upgrades();
    if let Err(e) = conn.await {
        tracing::trace!("[HTTP] connection {remote}: {e:#}");
    }
}
