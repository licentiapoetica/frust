//! Command-line tools of the `f0ckm` binary (`f0ckm <command> ...`), replacing the Node scripts in
//! `scripts/*.mjs` and `debug/*.mjs`. They reuse the server modules (media queue, settings, …) and
//! read `config.yaml` and the environment (`DB_HOST`, `STORAGE_DIR`, …) exactly like the server.

pub mod build_css;
mod import;
mod maintenance;
mod regen;
mod users;

pub use users::seed;

use crate::config::Config;
use crate::state::AppState;
use anyhow::{Result, bail};
use std::path::Path;
use std::sync::Arc;

const HELP: &str = "\
f0ckm — run without a command (or with `serve`) to start the web server.

Setup:
  seed                                      Default site settings and the rating tags (sfw, nsfw, nsfp, nsfl)
  create-admin <user> [<password>]          Create an admin (the first one gets id 1) or reset an existing one
  users list                                List accounts
  users create <user> [<password>] [--admin] [--mod]
  users passwd <user> [<password>]          Set a new password
  hash-password [<password>]                Print an Argon2id hash
  chat-upload-key create <name> [<max_mb>]  Upload-only API key for POST /api/chat/upload (shown once)
  chat-upload-key list
  chat-upload-key revoke <id>

Media:
  import [options]                          Bulk import files from f0ckm-data/import (`import --help`)
  regen <ids...> | --all | --audio | --pdf | --youtube
        [--blur] [--missing] [--from <id>] [--resume]
                                            Regenerate thumbnails, cover art and blurred thumbnails
  rehash [--phash] [--dry-run]              Recompute checksums/sizes (--phash: fill missing perceptual hashes)
  find-duplicates                           List visual duplicates by perceptual hash
  autotag <from_id> [<to_id>] [--dry-run]   Rate untagged images sfw/nsfw via the configured NSFW APIs
  clean [--dry-run] [--yes]                 Find missing media and delete unreferenced files

Build / development:
  build-css [--check]                       Build public/s/css/bundle.css (also done at server start)
  check-templates                           Compile every template in views/

Passwords left out on the command line are read from stdin. Environment: STORAGE_DIR, DB_HOST,
DB_PORT, DB_USER, DB_PASS, DB_NAME, F0CKM_BASE (project root, default: current directory).";

/// Runs a command; returns the process exit code.
pub async fn run(base: &Path, args: &[String]) -> i32 {
    let (cmd, rest) = (args[0].as_str(), &args[1..]);
    match dispatch(base, cmd, rest).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

async fn dispatch(base: &Path, cmd: &str, args: &[String]) -> Result<i32> {
    match cmd {
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            return Ok(0);
        }
        "build-css" => return build_css::run(base, &Args::parse(args, &["--check"], &[])?),
        "hash-password" => return users::hash_password(&Args::parse(args, &[], &[])?).await,
        "check-templates" => {
            let i18n = Arc::new(crate::i18n::I18n::new(crate::locales_dir(base)));
            let tpl = crate::tpl::Templates::new(base.join("views"), i18n);
            let errors = tpl.check_all(&base.join("views"));
            for (f, e) in &errors {
                println!("{f}: {e}");
            }
            println!("{} template(s) with errors", errors.len());
            return Ok(if errors.is_empty() { 0 } else { 1 });
        }
        _ => {}
    }

    // Everything else needs the config and the database.
    let known = [
        "seed", "create-admin", "users", "chat-upload-key", "import", "regen", "rehash", "find-duplicates", "autotag", "clean",
    ];
    if !known.contains(&cmd) {
        eprintln!("Unknown command: {cmd}\n\n{HELP}");
        return Ok(2);
    }
    let st = state(base).await?;
    // Say which database this runs against, so a missing DB_NAME doesn't go unnoticed
    if let Ok(Some(row)) = crate::sql!("select current_database() as db").fetch_optional(&st.db).await {
        use crate::db::RowExt;
        eprintln!("[database: {}]", row.string("db"));
    }
    if matches!(cmd, "seed" | "create-admin") {
        // Fresh setups: create the schema on an empty database first, like the server does at boot
        crate::prepare_db(&st).await;
    }
    crate::settings::load_from_db(&st).await;
    match cmd {
        "seed" => users::seed(&st).await,
        "create-admin" => users::create_admin(&st, &Args::parse(args, &[], &[])?).await,
        "users" => users::users(&st, args).await,
        "chat-upload-key" => users::chat_upload_key(&st, &Args::parse(args, &[], &[])?).await,
        "import" => import::run(&st, args).await,
        "regen" => regen::run(&st, &Args::parse(args, &["--all", "--audio", "--pdf", "--youtube", "--blur", "--missing", "--resume"], &["--from"])?).await,
        "rehash" => maintenance::rehash(&st, &Args::parse(args, &["--phash", "--dry-run"], &[])?).await,
        "find-duplicates" => maintenance::find_duplicates(&st).await,
        "autotag" => maintenance::autotag(&st, &Args::parse(args, &["--dry-run"], &[])?).await,
        "clean" => maintenance::clean(&st, &Args::parse(args, &["--dry-run", "--yes", "-y"], &[])?).await,
        _ => unreachable!(),
    }
}

async fn state(base: &Path) -> Result<AppState> {
    let cfg = Config::load(base)?;
    let pool = crate::db::connect(&cfg).await?;
    let i18n = Arc::new(crate::i18n::I18n::new(crate::locales_dir(base)));
    let tpl = crate::tpl::Templates::new(base.join("views"), i18n.clone());
    crate::new_state(cfg, pool, i18n, tpl)
}

/// Minimal argument parser: positionals, boolean flags and `--opt value` / `--opt=value`.
#[derive(Debug, Default)]
pub struct Args {
    pub pos: Vec<String>,
    flags: Vec<String>,
    values: Vec<(String, String)>,
}

impl Args {
    /// `flags`: options without a value, `valued`: options that take one. Anything else starting
    /// with `-` is an error (so typos don't silently run a different operation).
    pub fn parse(raw: &[String], flags: &[&str], valued: &[&str]) -> Result<Args> {
        let mut a = Args::default();
        let mut it = raw.iter();
        while let Some(arg) = it.next() {
            if arg == "--" {
                a.pos.extend(it.by_ref().cloned());
                break;
            }
            if !arg.starts_with('-') || arg == "-" || arg.parse::<f64>().is_ok() {
                a.pos.push(arg.clone());
                continue;
            }
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (arg.as_str(), None),
            };
            if valued.contains(&name) {
                let v = match inline {
                    Some(v) => v,
                    None => it.next().cloned().ok_or_else(|| anyhow::anyhow!("{name} needs a value"))?,
                };
                a.values.push((name.to_string(), v));
            } else if flags.contains(&name) && inline.is_none() {
                a.flags.push(name.to_string());
            } else {
                bail!("unknown option {arg}");
            }
        }
        Ok(a)
    }

    pub fn flag(&self, names: &[&str]) -> bool {
        self.flags.iter().any(|f| names.contains(&f.as_str()))
    }

    pub fn value(&self, names: &[&str]) -> Option<&str> {
        self.values.iter().rev().find(|(n, _)| names.contains(&n.as_str())).map(|(_, v)| v.as_str())
    }
}

/// One line from stdin (passwords left out on the command line), prompting on stderr.
pub fn read_line(prompt: &str) -> Result<String> {
    use std::io::{BufRead, Write};
    eprint!("{prompt}");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

/// `[y/N]` question; anything but `y`/`yes` (or a closed stdin) is a no.
pub fn confirm(question: &str) -> bool {
    read_line(&format!("{question} [y/N] ")).map(|a| matches!(a.trim().to_lowercase().as_str(), "y" | "yes")).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_flags_values_and_positionals() {
        let a = Args::parse(&v(&["1", "--from", "5", "--blur", "2", "--x=y"]), &["--blur"], &["--from", "--x"]).unwrap();
        assert_eq!(a.pos, v(&["1", "2"]));
        assert!(a.flag(&["--blur"]));
        assert_eq!(a.value(&["--from"]), Some("5"));
        assert_eq!(a.value(&["--x"]), Some("y"));
        assert!(Args::parse(&v(&["--nope"]), &[], &[]).is_err());
        assert!(Args::parse(&v(&["--from"]), &[], &["--from"]).is_err());
        // negative numbers and `--` are positionals
        let a = Args::parse(&v(&["-5", "--", "--blur"]), &["--blur"], &[]).unwrap();
        assert_eq!(a.pos, v(&["-5", "--blur"]));
        assert!(!a.flag(&["--blur"]));
    }
}
