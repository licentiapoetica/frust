//! `f0ckm import`: bulk import of media files from a directory (scripts/import.mjs).

use super::Args;
use crate::db::RowExt;
use crate::media::{self, mime, queue};
use crate::routes::uploads::{assign_tag, ensure_tag};
use crate::sql;
use crate::state::AppState;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const HELP: &str = "\
f0ckm Bulk Import
=================
Imports all media files in f0ckm-data/import (or a custom directory) into f0ckm.

Usage:
  f0ckm import [options]

Options:
  --user, -u <username>     User to attribute uploads to (defaults to the first admin)
  --rating, -r <sfw|nsfw|nsfl|untagged>  Default rating for imported items
  --tags, -t <tag1,tag2>    Tags for all imported items
  --dir, -d <path>          Directory to scan (default: f0ckm-data/import)
  --keep, -k                Keep source files after the import (default: delete them)
  --bypass, -b              Skip the duplicate checksum/pHash checks
  --check                   Check for duplicates even if the site setting bypasses the check
  --oc                      Mark all imported files as Original Content (OC)
  --title <title>           Title for imported files
  --help, -h                Show this help

Folders & sidecar JSON:
  - Rating subfolders: files in 'import/sfw/', 'import/nsfw/', 'import/nsfl/' or 'import/untagged/'
    get that rating.
  - Sidecar metadata: a file.jpg.json or file.json next to file.jpg:
    { \"title\": \"My Post\", \"rating\": \"sfw\", \"tags\": [\"funny\", \"cat\"], \"comment\": \"First comment\", \"is_oc\": true }";

struct Opts {
    user: Option<String>,
    rating: Option<String>,
    tags: Vec<String>,
    dir: PathBuf,
    keep: bool,
    bypass: Option<bool>,
    is_oc: bool,
    title: Option<String>,
}

fn rating_tag_id(st: &AppState, rating: &str) -> i64 {
    match rating {
        "sfw" => 1,
        "nsfw" => 2,
        _ => st.cfg().nsfl_tag_id(),
    }
}

/// All regular files below `dir` (following symlinks, skipping hidden names and `.json` sidecars).
fn collect_files(dir: &Path, visited: &mut HashSet<PathBuf>, out: &mut Vec<PathBuf>) {
    let canonical = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canonical) {
        return; // symlink loop
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(e) => return eprintln!("[IMPORT] Warning: unable to read directory '{}': {e}", dir.display()),
    };
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        match std::fs::metadata(&path) {
            Ok(m) if m.is_dir() => collect_files(&path, visited, out),
            Ok(m) if m.is_file() => {
                if !name.ends_with(".json") {
                    out.push(path);
                }
            }
            Ok(_) => {}
            Err(err) => eprintln!("[IMPORT] Warning: unable to stat '{}' (broken symlink?): {err}", path.display()),
        }
    }
}

/// `file.ext.json` or `file.json` next to the file.
fn read_sidecar(path: &Path) -> (Value, Option<PathBuf>) {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let candidates = [PathBuf::from(format!("{}.json", path.display())), path.with_file_name(format!("{stem}.json"))];
    for c in candidates {
        if !c.exists() {
            continue;
        }
        match std::fs::read_to_string(&c).map_err(anyhow::Error::from).and_then(|s| serde_json::from_str::<Value>(&s).map_err(Into::into)) {
            Ok(v) if v.is_object() => return (v, Some(c)),
            Ok(_) => eprintln!("[IMPORT] Warning: sidecar {} is not a JSON object", c.display()),
            Err(e) => eprintln!("[IMPORT] Warning: failed to parse sidecar JSON at {}: {e}", c.display()),
        }
    }
    (json!({}), None)
}

fn parse_opts(st: &AppState, raw: &[String]) -> Result<Option<Opts>> {
    let a = Args::parse(
        raw,
        &["--help", "-h", "--keep", "-k", "--bypass", "-b", "--check", "--oc"],
        &["--user", "-u", "--rating", "-r", "--tags", "-t", "--dir", "-d", "--title"],
    )?;
    if a.flag(&["--help", "-h"]) {
        return Ok(None);
    }
    if let Some(p) = a.pos.first() {
        bail!("unexpected argument {p} (see `f0ckm import --help`)");
    }
    let rating = a.value(&["--rating", "-r"]).map(str::to_lowercase);
    if let Some(r) = &rating {
        if !["sfw", "nsfw", "nsfl", "untagged"].contains(&r.as_str()) {
            bail!("--rating must be sfw, nsfw, nsfl or untagged");
        }
    }
    let dir = match a.value(&["--dir", "-d"]) {
        Some(d) => std::path::absolute(d)?,
        None => st.cfg().paths.import.clone(),
    };
    Ok(Some(Opts {
        user: a.value(&["--user", "-u"]).map(str::to_string),
        rating,
        tags: a.value(&["--tags", "-t"]).map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default(),
        dir,
        keep: a.flag(&["--keep", "-k"]),
        bypass: if a.flag(&["--bypass", "-b"]) {
            Some(true)
        } else if a.flag(&["--check"]) {
            Some(false)
        } else {
            None
        },
        is_oc: a.flag(&["--oc"]),
        title: a.value(&["--title"]).map(str::to_string),
    }))
}

pub async fn run(st: &AppState, raw: &[String]) -> Result<i32> {
    let Some(opts) = parse_opts(st, raw)? else {
        println!("{HELP}");
        return Ok(0);
    };
    let cfg = st.cfg();
    println!("--- f0ckm Bulk Import Starting ---");
    println!("Scan Directory: {}", opts.dir.display());
    if !opts.dir.is_dir() {
        eprintln!("[IMPORT ERROR] Directory does not exist: {}", opts.dir.display());
        return Ok(1);
    }

    // 1. Target user
    let user = match &opts.user {
        Some(u) => {
            let lu = u.to_lowercase();
            match sql!(r#"SELECT id, "user", login FROM "user" WHERE LOWER(login) = "#, lu.clone(), r#" OR LOWER("user") = "#, lu, " LIMIT 1").fetch_optional(&st.db).await? {
                Some(r) => r,
                None => {
                    eprintln!("[IMPORT ERROR] Specified user '{u}' not found in database.");
                    return Ok(1);
                }
            }
        }
        None => match sql!(r#"SELECT id, "user", login FROM "user" ORDER BY admin DESC, id ASC LIMIT 1"#).fetch_optional(&st.db).await? {
            Some(r) => r,
            None => {
                eprintln!("[IMPORT ERROR] No users found in database. Run `f0ckm create-admin` first.");
                return Ok(1);
            }
        },
    };
    let user_id = user.i64("id").unwrap_or(0);
    let username = user.string("user");
    println!("Attributing uploads to user: {username} (ID: {user_id})");
    for d in [&cfg.paths.b, &cfg.paths.t, &cfg.paths.ca, &cfg.paths.tmp] {
        tokio::fs::create_dir_all(d).await?;
    }

    // 2. Files
    let mut files = Vec::new();
    collect_files(&opts.dir, &mut HashSet::new(), &mut files);
    if files.is_empty() {
        println!("[IMPORT] No files found in {}. Place files there and re-run.", opts.dir.display());
        return Ok(0);
    }
    println!("Found {} file(s) to process.\n", files.len());

    let allowed = mime::allowed_mimes(&cfg);
    let bypass = opts.bypass.unwrap_or_else(|| st.settings.bypass_duplicate_check());
    let (mut ok, mut skipped, mut failed) = (0, 0, 0);
    for (i, path) in files.iter().enumerate() {
        let rel = path.strip_prefix(&opts.dir).unwrap_or(path).display().to_string();
        let prefix = format!("[{}/{}]", i + 1, files.len());
        match import_one(st, &opts, path, &allowed, bypass, user_id, &username).await {
            Ok(Outcome::Imported { id, rating, mime }) => {
                println!("{prefix} [SUCCESS] Imported '{rel}' -> Item #{id} (Rating: {}, MIME: {mime})", rating.as_deref().unwrap_or("untagged"));
                ok += 1;
            }
            Ok(Outcome::Skipped(why)) => {
                eprintln!("{prefix} [SKIP] {rel} - {why}");
                skipped += 1;
            }
            Err(e) => {
                eprintln!("{prefix} [ERROR] Failed to import '{rel}': {e:#}");
                failed += 1;
            }
        }
    }
    st.caches.feed_count.clear();
    println!("\n========================================");
    println!("Import Complete!");
    println!("Total Scanned : {}", files.len());
    println!("Successful    : {ok}");
    println!("Skipped       : {skipped}");
    println!("Failed        : {failed}");
    println!("========================================");
    Ok(if failed > 0 { 1 } else { 0 })
}

enum Outcome {
    Imported { id: i64, rating: Option<String>, mime: String },
    Skipped(String),
}

#[allow(clippy::too_many_arguments)]
async fn import_one(st: &AppState, opts: &Opts, path: &Path, allowed: &[String], bypass: bool, user_id: i64, username: &str) -> Result<Outcome> {
    let cfg = st.cfg();
    let (meta, sidecar) = read_sidecar(path);
    let base_name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();

    // Rating: sidecar → --rating → folder name → sfw (unless shitpost mode allows untagged)
    let mut rating = meta["rating"].as_str().map(str::to_lowercase).or_else(|| opts.rating.clone());
    if rating.is_none() {
        let parent = path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ["sfw", "nsfw", "nsfl", "untagged"].contains(&parent.as_str()) {
            rating = Some(parent);
        }
    }
    if rating.as_deref() == Some("untagged") {
        rating = None;
    }
    if rating.is_none() && !cfg.truthy("websrv.shitpost_mode") {
        rating = Some("sfw".into());
    }
    if let Some(r) = &rating {
        if !["sfw", "nsfw", "nsfl"].contains(&r.as_str()) {
            return Ok(Outcome::Skipped(format!("unknown rating '{r}' in sidecar")));
        }
    }
    let title = meta["title"].as_str().filter(|t| !t.is_empty()).map(str::to_string).or_else(|| opts.title.clone());
    let comment = meta["comment"].as_str().map(str::trim).filter(|c| !c.is_empty()).map(str::to_string);
    let is_oc = if meta["is_oc"].is_null() { opts.is_oc } else { crate::config::truthy(&meta["is_oc"]) };
    let visibility = meta["visibility"].as_i64().filter(|v| (0..=3).contains(v)).unwrap_or(0) as i32;
    let mut tags = opts.tags.clone();
    match &meta["tags"] {
        Value::Array(a) => tags.extend(a.iter().filter_map(|t| t.as_str().map(str::to_string))),
        Value::String(s) => tags.extend(s.split(',').map(|t| t.trim().to_string())),
        _ => {}
    }
    tags.retain(|t| !t.is_empty() && !["sfw", "nsfw", "nsfl"].contains(&t.to_lowercase().as_str()));

    let size = tokio::fs::metadata(path).await?.len();
    if size == 0 {
        return Ok(Outcome::Skipped("0 bytes".into()));
    }

    // Work on a private copy in tmp/ (resolves symlinks; the source stays untouched until the end)
    let uuid = queue::genuuid();
    let tmp = media::TempPath::new(cfg.paths.tmp.join(format!("{uuid}.import")));
    tokio::fs::copy(path, &tmp.path).await.with_context(|| format!("copy {}", path.display()))?;
    let mut actual = mime::detect(&tmp.path).await?;
    if (actual.is_empty() || actual == "application/octet-stream") && base_name.to_lowercase().ends_with(".swf") {
        actual = "application/x-shockwave-flash".into();
    }
    if !allowed.contains(&actual) {
        return Ok(Outcome::Skipped(format!("Unsupported MIME type: '{actual}'")));
    }
    if mime::needs_transcode(&actual) {
        let converted = media::TempPath::new(cfg.paths.tmp.join(format!("{uuid}.mp4")));
        media::transcode::transcode_to_mp4(&tmp.path, &converted.path).await.context("transcode to mp4")?;
        tokio::fs::rename(&converted.path, &tmp.path).await?;
        actual = "video/mp4".into();
    }

    // Checksum, perceptual hash, dimensions, audio-only MP4
    let archive = mime::is_archive_mime(&cfg, &actual);
    let tp = tmp.path.clone();
    let (checksum, phash, dims) = tokio::join!(
        queue::sha256_file(&tp),
        async {
            if archive { None } else { queue::generate_phash(&tp).await }
        },
        async {
            if archive { None } else { queue::dimensions(&tp, &actual).await }
        },
    );
    let checksum = checksum?;
    if actual == "video/mp4" || actual == "video/quicktime" {
        let ext = base_name.rsplit('.').next().unwrap_or("").to_lowercase();
        let audio_only = ext == "m4a" || ext == "aac" || {
            let p = tp.to_string_lossy().into_owned();
            matches!(
                crate::proc::spawn_with("ffprobe", &["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=codec_type", "-of", "csv=p=0", &p], crate::proc::Opts { quiet: true, ..Default::default() }).await,
                Ok(o) if o.stdout_str().trim().is_empty()
            )
        };
        if audio_only {
            actual = "audio/mp4".into();
        }
    }

    if !bypass {
        if let Some(id) = queue::check_repost_sum(st, &checksum).await? {
            return Ok(Outcome::Skipped(format!("Duplicate file (matches existing Item #{id})")));
        }
        if let Some(id) = queue::check_repost_phash(st, phash.as_deref()).await? {
            return Ok(Outcome::Skipped(format!("Visual duplicate (matches existing Item #{id})")));
        }
    }

    let ext = mime::ext_for(&cfg, &actual).unwrap_or_else(|| "bin".into());
    let filename = format!("{uuid}.{ext}");
    let dest_path = cfg.paths.b.join(&filename);
    let size = tokio::fs::metadata(&tmp.path).await?.len() as i64;
    media::move_file(&tmp.path, &dest_path).await?;
    drop(tmp);
    media::make_public(&dest_path).await;
    let insert_checksum = if bypass { format!("{checksum}_bypass_{}", crate::util::now_ms()) } else { checksum };
    let slug = crate::util::generate_slug(11);

    let inserted = sql!(
        "INSERT INTO items (src, dest, mime, size, checksum, phash, username, userchannel, usernetwork, stamp, active, is_oc, original_filename, title, width, height, visibility, slug, expires_at) VALUES ('', ",
        filename.clone(), ", ", actual.clone(), ", ", size.min(i32::MAX as i64) as i32, ", ", insert_checksum, ", ", phash, ", ", username.to_string(),
        ", 'import_script', 'local', ", crate::util::now_secs() as i32, ", true, ", is_oc, ", ", multipart_name(&base_name), ", ", title, ", ",
        dims.map(|d| d.0), ", ", dims.map(|d| d.1), ", ", visibility, ", ", slug.clone(), ", NULL) RETURNING id"
    )
    .fetch_scalar::<i32, _>(&st.db)
    .await;
    let item_id = match inserted {
        Ok(Some(id)) => id as i64,
        other => {
            let _ = tokio::fs::remove_file(&dest_path).await;
            other?;
            bail!("insert returned no id");
        }
    };
    st.invalidate_upload_count_cache(username);
    if visibility == 2 {
        crate::private_items::add_private(st, item_id, &filename, username);
    }

    let _ = sql!("INSERT INTO comment_subscriptions (user_id, item_id) VALUES (", user_id as i32, ", ", item_id as i32, ") ON CONFLICT DO NOTHING").execute(&st.db).await;

    let r = queue::gen_thumbnail(&cfg, &filename, &actual, item_id, "", false, queue::thumb_size(0)).await;
    if actual.starts_with("audio/") && r.cover_extracted {
        sql!("UPDATE items SET has_coverart = TRUE WHERE id = ", item_id as i32).execute(&st.db).await?;
    }
    queue::gen_blurred_thumbnail(&cfg, item_id, false).await;

    if let Some(r) = &rating {
        assign_tag(st, item_id, rating_tag_id(st, r), user_id).await?;
    }
    let mut auto: Vec<&str> = Vec::new();
    if is_oc {
        auto.extend(["oc", "original content"]);
    }
    if mime::is_flash(&actual) {
        auto.extend(["Flash", "SWF"]);
    }
    for name in tags.iter().map(String::as_str).chain(auto) {
        if let Some(tag_id) = ensure_tag(st, name).await? {
            assign_tag(st, item_id, tag_id, user_id).await?;
        }
    }
    if let Some(c) = comment {
        sql!("INSERT INTO comments (item_id, user_id, content) VALUES (", item_id as i32, ", ", user_id as i32, ", ", c, ")").execute(&st.db).await?;
    }

    let display_name = sql!("SELECT display_name FROM user_options WHERE user_id = ", user_id as i32).fetch_optional(&st.db).await.ok().flatten().and_then(|r| r.str("display_name").map(str::to_string));
    let payload = json!({
        "id": item_id,
        "dest": filename,
        "mime": actual,
        "username": username,
        "display_name": display_name,
        "tag_id": rating.as_deref().map(|r| rating_tag_id(st, r)).unwrap_or(0),
        "is_oc": is_oc,
        "slug": slug,
        "visibility": visibility,
        "is_album": false,
        "album_count": 0,
    });
    let _ = sql!("SELECT pg_notify('new_item', ", payload.to_string(), ")").execute(&st.db).await;

    if !opts.keep {
        let _ = tokio::fs::remove_file(path).await;
        if let Some(s) = sidecar {
            let _ = tokio::fs::remove_file(s).await;
        }
    }
    Ok(Outcome::Imported { id: item_id, rating, mime: actual })
}

fn multipart_name(name: &str) -> Option<String> {
    Some(media::multipart::sanitize_filename(name)).filter(|n| !n.is_empty())
}
