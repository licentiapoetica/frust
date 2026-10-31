//! `f0ckm regen`: regenerate thumbnails, audio cover art and blurred thumbnails (scripts/regen.mjs).
//! Also covers debug/thumbnailer.mjs (`--all`), debug/blur_existing_thumbnails.mjs
//! (`--blur --missing`) and scripts/recreate_covers.mjs (`--audio`).
//!
//! Flash items are always skipped: their thumbnails come from the Ruffle snapshot.

use super::Args;
use crate::db::{Row, RowExt, Sql};
use crate::media::{mime, queue};
use crate::sql;
use crate::state::AppState;
use anyhow::Result;
use std::path::PathBuf;

const USAGE: &str = "\
Usage:
  f0ckm regen <id> [<id> ...]   Regenerate the given items
  f0ckm regen --all             All live items
  f0ckm regen --audio           Audio items (cover art)
  f0ckm regen --pdf             PDF items
  f0ckm regen --youtube         YouTube items
Options:
  --blur                        Only the blurred thumbnails (all live items unless a selection is given)
  --missing                     Only items whose thumbnail (with --blur: blurred thumbnail) is missing
  --from <id>                   Start at this item id (inclusive)
  --resume                      Continue after the last item of an interrupted run";

const FLASH: [&str; 2] = ["application/x-shockwave-flash", "application/vnd.adobe.flash.movie"];

fn state_file(st: &AppState) -> PathBuf {
    st.cfg().paths.logs.join("regen_state.json")
}

pub async fn run(st: &AppState, args: &Args) -> Result<i32> {
    let cfg = st.cfg();
    let blur_only = args.flag(&["--blur"]);
    let missing_only = args.flag(&["--missing"]);
    let selection = ["--all", "--audio", "--pdf", "--youtube"].into_iter().find(|f| args.flag(&[f]));
    if selection.is_none() && args.pos.is_empty() && !blur_only {
        println!("{USAGE}");
        return Ok(0);
    }

    let mut from: Option<i32> = match args.value(&["--from"]) {
        Some(v) => match v.parse() {
            Ok(n) => Some(n),
            Err(_) => {
                eprintln!("Invalid ID provided for --from");
                return Ok(1);
            }
        },
        None => None,
    };
    if from.is_none() && args.flag(&["--resume"]) {
        let saved = std::fs::read_to_string(state_file(st)).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
        match saved.as_ref().and_then(|v| v["lastId"].as_i64()) {
            Some(id) => {
                from = Some(id as i32);
                println!("[regen] Resuming from checkpoint at item ID: {id}\n");
            }
            None => println!("[regen] No previous state checkpoint found to resume from.\n"),
        }
    }
    if let Some(f) = from {
        println!("[regen] Starting from item ID >= {f}\n");
    }

    let live = "SELECT id, dest, mime, src FROM items WHERE active = true AND is_deleted = false";
    let mut q = match selection {
        Some("--all") => {
            let mut q = Sql::new(live);
            q.push(" AND NOT (mime = ANY(").bind(FLASH.map(String::from).to_vec()).push("))");
            q
        }
        Some("--audio") => sql!("SELECT id, dest, mime, src FROM items WHERE active = true AND is_deleted = false AND mime ILIKE 'audio/%'"),
        Some("--pdf") => sql!("SELECT id, dest, mime, src FROM items WHERE active = true AND is_deleted = false AND mime = 'application/pdf'"),
        Some("--youtube") => sql!("SELECT id, dest, mime, src FROM items WHERE active = true AND is_deleted = false AND mime = 'video/youtube'"),
        _ if args.pos.is_empty() => {
            // --blur alone: every live non-Flash item
            let mut q = Sql::new(live);
            q.push(" AND NOT (mime = ANY(").bind(FLASH.map(String::from).to_vec()).push("))");
            q
        }
        _ => {
            let ids: Vec<i32> = args.pos.iter().filter_map(|a| a.parse().ok()).filter(|n| *n > 0).collect();
            if ids.is_empty() {
                eprintln!("No valid item IDs provided.");
                return Ok(1);
            }
            sql!("SELECT id, dest, mime, src FROM items WHERE id = ANY(", ids, ")")
        }
    };
    if let Some(f) = from {
        q.push(" AND id >= ").bind(f);
    }
    q.push(" ORDER BY id");
    let items: Vec<Row> = q.fetch_all(&st.db).await?;

    if selection.is_none() && !args.pos.is_empty() {
        let found: Vec<i64> = items.iter().filter_map(|r| r.i64("id")).collect();
        let missing: Vec<String> = args.pos.iter().filter_map(|a| a.parse::<i64>().ok()).filter(|id| *id > 0 && !found.contains(id)).map(|i| i.to_string()).collect();
        if !missing.is_empty() && from.is_none() {
            eprintln!("Items not found: {}\n", missing.join(", "));
        }
    }
    let what = if blur_only { "blurred thumbnails of" } else { "thumbnails of" };
    println!("Regenerating {what} {} item(s)...\n", items.len());

    let t_dir = cfg.paths.t.clone();
    let mut processed = 0usize;
    let mut failed = 0usize;
    for item in &items {
        let Some(id) = item.i64("id") else { continue };
        let (dest, mime_type, src) = (item.string("dest"), item.string("mime"), item.string("src"));
        if mime::is_flash(&mime_type.to_lowercase()) {
            println!("[{id}] Skipped (Flash/SWF — thumbnail managed by Ruffle snapshot)");
            continue;
        }
        if missing_only {
            let name = if blur_only { format!("{id}_blur.webp") } else { format!("{id}.webp") };
            if t_dir.join(name).exists() {
                continue;
            }
        }
        processed += 1;
        if blur_only {
            if queue::gen_blurred_thumbnail(&cfg, id, false).await {
                println!("[{id}] ✓ Blurred thumbnail regenerated");
            } else {
                println!("[{id}] ✗ FAILED (see log)");
                failed += 1;
            }
        } else {
            println!("[{id}] Regenerating: {dest} ({mime_type})");
            let r = queue::gen_thumbnail(&cfg, &dest, &mime_type, id, &src, false, queue::thumb_size(0)).await;
            if mime_type.starts_with("audio/") {
                sql!("UPDATE items SET has_coverart = ", r.cover_extracted, " WHERE id = ", id as i32).execute(&st.db).await?;
                println!("[{id}] ✓ {}", if r.cover_extracted { "Cover art extracted and saved" } else { "No cover art found, placeholder generated" });
            } else if r.ok {
                println!("[{id}] ✓ Thumbnail regenerated");
            } else {
                println!("[{id}] ✗ Thumbnail failed, placeholder written");
                failed += 1;
            }
            if queue::gen_blurred_thumbnail(&cfg, id, false).await {
                println!("[{id}] ✓ Blurred thumbnail regenerated");
            }
        }
        // Checkpoint for --resume
        let _ = std::fs::write(state_file(st), serde_json::json!({ "lastId": id, "timestamp": chrono::Utc::now().to_rfc3339() }).to_string());
    }
    let _ = std::fs::remove_file(state_file(st));
    println!("\nDone. {processed} item(s) processed, {failed} failed.");
    Ok(if failed > 0 { 1 } else { 0 })
}
