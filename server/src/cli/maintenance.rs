//! Maintenance: `rehash` (debug/recreate_hashes.mjs + debug/backfill_phash.mjs), `find-duplicates`
//! (debug/find_duplicates.mjs), `autotag` (debug/autotagger.mjs), `clean` (debug/clean.mjs).

use super::{Args, confirm};
use crate::db::RowExt;
use crate::media::queue;
use crate::sql;
use crate::state::AppState;
use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where an item's file lives: live, pending approval or in the trash.
fn locate(st: &AppState, dest: &str) -> Option<PathBuf> {
    if !crate::media::is_plain_name(dest) {
        return None;
    }
    let cfg = st.cfg();
    [cfg.paths.b.join(dest), cfg.paths.pending.join("b").join(dest), cfg.paths.deleted.join("b").join(dest)].into_iter().find(|p| p.exists())
}

/// Recompute checksum and size of every item file and fix rows that differ; `--phash` instead
/// fills in missing perceptual hashes (marking unreadable files `MISSING`/`ERROR` like the old script).
pub async fn rehash(st: &AppState, args: &Args) -> Result<i32> {
    let dry = args.flag(&["--dry-run"]);
    if dry {
        println!("Dry run: nothing is written.");
    }
    if args.flag(&["--phash"]) {
        return backfill_phash(st, dry).await;
    }
    let items = sql!("SELECT id, dest, checksum, size FROM items WHERE mime != 'video/youtube' ORDER BY id ASC").fetch_all(&st.db).await?;
    println!("Found {} items. Processing...", items.len());
    let (mut updated, mut missing, mut errors, mut unchanged) = (0, 0, 0, 0);
    for (i, item) in items.iter().enumerate() {
        let Some(id) = item.i64("id") else { continue };
        let dest = item.string("dest");
        let Some(path) = locate(st, &dest) else {
            eprintln!("[MISSING] File not found for item {id}: {dest}");
            missing += 1;
            continue;
        };
        let (hash, size) = match (queue::sha256_file(&path).await, tokio::fs::metadata(&path).await) {
            (Ok(h), Ok(m)) => (h, m.len() as i64),
            (Err(e), _) => {
                eprintln!("[ERROR] Processing item {id}: {e:#}");
                errors += 1;
                continue;
            }
            (_, Err(e)) => {
                eprintln!("[ERROR] Processing item {id}: {e}");
                errors += 1;
                continue;
            }
        };
        // Bypass-mode uploads store `<sha256>_bypass_<ms>` so the duplicate check skips them: keep the suffix
        let old = item.string("checksum");
        let new_checksum = match old.split_once("_bypass_") {
            Some((_, suffix)) => format!("{hash}_bypass_{suffix}"),
            None => hash,
        };
        let old_size = item.i64("size").unwrap_or(-1);
        if new_checksum != old || size != old_size {
            println!("[UPDATE] Item {id} ({}/{})", i + 1, items.len());
            if new_checksum != old {
                println!("  - Hash: {old} -> {new_checksum}");
            }
            if size != old_size {
                println!("  - Size: {old_size} -> {size}");
            }
            if !dry {
                sql!("UPDATE items SET checksum = ", new_checksum, ", size = ", size.min(i32::MAX as i64) as i32, " WHERE id = ", id as i32).execute(&st.db).await?;
            }
            updated += 1;
        } else {
            unchanged += 1;
        }
        if (i + 1) % 100 == 0 {
            println!("Progress: {}/{} (Updated: {updated}, Missing: {missing}, Errors: {errors})", i + 1, items.len());
        }
    }
    println!("Done.\nTotal: {}\nUpdated: {updated}\nUnchanged: {unchanged}\nMissing files: {missing}\nErrors: {errors}", items.len());
    Ok(0)
}

async fn backfill_phash(st: &AppState, dry: bool) -> Result<i32> {
    let items = sql!("SELECT id, dest FROM items WHERE (phash IS NULL OR phash = '') AND active = true AND mime != 'video/youtube' AND mime NOT LIKE 'audio/%' ORDER BY id DESC")
        .fetch_all(&st.db)
        .await?;
    println!("Found {} items without a perceptual hash.", items.len());
    let (mut ok, mut failed) = (0, 0);
    for item in &items {
        let Some(id) = item.i64("id") else { continue };
        let dest = item.string("dest");
        let value = match locate(st, &dest) {
            None => {
                println!("[SKIP] File not found: {dest} (ID: {id})");
                "MISSING".to_string()
            }
            Some(path) => match queue::generate_phash(&path).await {
                Some(h) => {
                    println!("[SUCCESS] ID {id}: {h}");
                    ok += 1;
                    h
                }
                None => {
                    println!("[FAILED] ID {id}: Could not generate PHash.");
                    failed += 1;
                    "ERROR".to_string()
                }
            },
        };
        if !dry {
            sql!("UPDATE items SET phash = ", value, " WHERE id = ", id as i32).execute(&st.db).await?;
        }
    }
    println!("Backfill complete: {ok} hashed, {failed} failed, {} missing.", items.len() - ok - failed);
    Ok(0)
}

const THRESHOLD: u32 = 15;
const REQUIRED_MATCHES: usize = 2;

/// Bit distance of two hex hash segments (9999 when they can't be compared).
fn hamming(a: &str, b: &str) -> u32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 9999;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let mut d = 0;
    for i in (0..a.len()).step_by(2) {
        let byte = |s: &[u8]| std::str::from_utf8(&s[i..(i + 2).min(s.len())]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()).unwrap_or(0);
        d += (byte(a) ^ byte(b)).count_ones();
    }
    d
}

/// Match rule of the old script: frame-by-frame, 2 matching frames needed when 2+ are compared.
fn is_match(a: &[&str], b: &[&str]) -> Option<u32> {
    let n = a.len().min(b.len());
    let dists: Vec<u32> = (0..n).map(|f| hamming(a[f], b[f])).collect();
    let matches = dists.iter().filter(|d| **d <= THRESHOLD).count();
    let ok = (n >= 3 && matches >= REQUIRED_MATCHES) || (n == 2 && matches >= 2) || (n == 1 && matches == 1);
    ok.then(|| (dists.iter().sum::<u32>() as f64 / n as f64).round() as u32)
}

pub async fn find_duplicates(st: &AppState) -> Result<i32> {
    println!("Fetching items...");
    let items = sql!(
        "SELECT id, phash FROM items WHERE phash IS NOT NULL AND phash != '' AND phash != 'MISSING' AND phash != 'ERROR' AND phash NOT LIKE '00000000%' ORDER BY id ASC"
    )
    .fetch_all(&st.db)
    .await?;
    println!("Checking {} items for duplicates (Threshold: {THRESHOLD}, Required frame matches: {REQUIRED_MATCHES})...", items.len());
    let hashes: Vec<(i64, String)> = items.iter().filter_map(|r| Some((r.i64("id")?, r.string("phash")))).collect();
    let frames: Vec<Vec<&str>> = hashes.iter().map(|(_, h)| h.split('_').collect()).collect();
    let mut processed: HashSet<usize> = HashSet::new();
    let mut sets: Vec<(i64, Vec<(i64, u32)>)> = Vec::new();
    for i in 0..hashes.len() {
        if processed.contains(&i) {
            continue;
        }
        let mut found = Vec::new();
        for j in (i + 1)..hashes.len() {
            if processed.contains(&j) {
                continue;
            }
            if let Some(avg) = is_match(&frames[i], &frames[j]) {
                found.push((hashes[j].0, avg));
                processed.insert(j);
            }
        }
        if !found.is_empty() {
            processed.insert(i);
            sets.push((hashes[i].0, found));
        }
    }
    if sets.is_empty() {
        println!("No duplicates found.");
        return Ok(0);
    }
    println!("Found {} duplicate sets:\n---------------------------------------------------", sets.len());
    for (orig, list) in sets {
        let s: Vec<String> = list.iter().map(|(id, d)| format!("ID:{id} (avg-dist:{d})")).collect();
        println!("Original ID: {orig} matches with: {}", s.join(", "));
    }
    Ok(0)
}

/// Rate images without a rating tag (sfw/nsfw/nsfl) via `apis.nsfw1`/`apis.nsfw2` (the old script called a
/// local classifier that was removed for a shell injection). The APIs fetch the file from the
/// public URL, so only live items are considered.
pub async fn autotag(st: &AppState, args: &Args) -> Result<i32> {
    let from: Option<i32> = args.pos.first().and_then(|a| a.parse().ok());
    let Some(from) = from else {
        eprintln!("Usage: f0ckm autotag <from_id> [<to_id>] [--dry-run]   (default range: 500 items)");
        return Ok(1);
    };
    let to: i32 = args.pos.get(1).and_then(|a| a.parse().ok()).unwrap_or(from.saturating_add(500));
    let dry = args.flag(&["--dry-run"]);
    let items = sql!(
        "SELECT id, dest, size FROM items WHERE active = true AND is_deleted = false AND mime LIKE 'image/%' AND id BETWEEN ", from, " AND ", to,
        " AND NOT EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = items.id AND ta.tag_id IN (1, 2, ", st.cfg().nsfl_tag_id() as i32, ")) ORDER BY id"
    )
    .fetch_all(&st.db)
    .await?;
    println!("{} untagged image(s) between {from} and {to}{}", items.len(), if dry { " (dry run)" } else { "" });
    let base = st.cfg().url_full().to_string();
    let mut failed = 0;
    for item in &items {
        let Some(id) = item.i64("id") else { continue };
        let size = item.i64("size").unwrap_or(0).max(0) as u64;
        match crate::media::autotagger::is_nsfw(st, &item.string("dest"), size).await {
            Ok(nsfw) => {
                println!("{base}/{id} {}", if nsfw { "nsfw" } else { "sfw" });
                if !dry {
                    if nsfw { queue::tag_nsfw(st, id).await? } else { queue::tag_sfw(st, id).await? };
                }
            }
            Err(e) => {
                eprintln!("{base}/{id} failed: {e:#}");
                failed += 1;
            }
        }
    }
    Ok(if failed > 0 { 1 } else { 0 })
}

fn list_dir(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != ".empty" && n != ".gitkeep").collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// Report missing media and unreferenced files in b/ and t/ (and leftovers in tmp/), then offer
/// to delete the unreferenced ones. Fixes over debug/clean.mjs: files of pending/hidden items,
/// album sub-files, blurred thumbnails and targets of deduplication symlinks count as referenced
/// (the old script offered to delete them), and tmp/ entries younger than an hour are left alone
/// because uploads in progress use them.
pub async fn clean(st: &AppState, args: &Args) -> Result<i32> {
    let dry = args.flag(&["--dry-run"]);
    let yes = args.flag(&["--yes", "-y"]);
    println!("dry run? {dry}");
    let cfg = st.cfg();
    let (b_dir, t_dir, tmp_dir) = (cfg.paths.b.clone(), cfg.paths.t.clone(), cfg.paths.tmp.clone());

    let rows = sql!("SELECT id, dest, mime, active, is_deleted FROM items").fetch_all(&st.db).await?;
    let album = sql!("SELECT dest FROM album_items").fetch_all(&st.db).await?;

    let mut ref_b: HashSet<String> = HashSet::new();
    let mut ref_t: HashSet<String> = HashSet::new();
    let (mut missing_b, mut missing_t) = (Vec::new(), Vec::new());
    for r in &rows {
        let Some(id) = r.i64("id") else { continue };
        let dest = r.string("dest");
        ref_b.insert(dest.clone());
        ref_t.insert(format!("{id}.webp"));
        ref_t.insert(format!("{id}_blur.webp"));
        if r.bool("active") && !r.bool("is_deleted") {
            if r.str("mime") != Some("video/youtube") && !b_dir.join(&dest).exists() {
                missing_b.push(id);
            }
            if !t_dir.join(format!("{id}.webp")).exists() {
                missing_t.push(id);
            }
        }
    }
    for r in &album {
        let dest = r.string("dest");
        let thumb = crate::media::delete::album_thumb_name(&dest);
        ref_t.insert(format!("{}_blur.webp", thumb.trim_end_matches(".webp")));
        ref_t.insert(thumb);
        ref_b.insert(dest);
    }

    let files_b = list_dir(&b_dir);
    let files_t = list_dir(&t_dir);
    // Deduplicated uploads are symlinks to the first copy: their targets are in use too
    for f in &files_b {
        if let Ok(target) = std::fs::read_link(b_dir.join(f)) {
            if let Some(name) = target.file_name() {
                ref_b.insert(name.to_string_lossy().into_owned());
            }
        }
    }
    let mut extensions: Vec<String> = cfg.mimes().values().filter_map(|v| v.as_str().map(str::to_lowercase)).collect();
    extensions.push("mov".into());
    let invalid_b: Vec<&String> = files_b.iter().filter(|f| f.split('.').nth(1).map(|e| !extensions.contains(&e.to_lowercase())).unwrap_or(true)).collect();
    let invalid_t: Vec<&String> = files_t.iter().filter(|f| !f.ends_with(".webp")).collect();
    let spare_b: Vec<PathBuf> = files_b.iter().filter(|f| !ref_b.contains(*f)).map(|f| b_dir.join(f)).collect();
    let spare_t: Vec<PathBuf> = files_t.iter().filter(|f| !ref_t.contains(*f)).map(|f| t_dir.join(f)).collect();
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let old_tmp: Vec<PathBuf> = list_dir(&tmp_dir)
        .into_iter()
        .map(|f| tmp_dir.join(f))
        .filter(|p| std::fs::symlink_metadata(p).and_then(|m| m.modified()).map(|t| t < hour_ago).unwrap_or(false))
        .collect();

    let ids = |v: &[i64]| v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ");
    println!("missing in {}: {} {}", b_dir.display(), missing_b.len(), ids(&missing_b));
    println!("missing in {}: {} {}", t_dir.display(), missing_t.len(), ids(&missing_t));
    println!("invalid names in b: {} {:?}", invalid_b.len(), invalid_b);
    println!("invalid names in t: {} {:?}", invalid_t.len(), invalid_t);
    for (label, list) in [("unreferenced in b", &spare_b), ("unreferenced in t", &spare_t), ("tmp entries older than 1 h", &old_tmp)] {
        println!("{label}: {}", list.len());
        for p in list.iter() {
            println!("  {}", p.display());
        }
    }
    if dry {
        return Ok(0);
    }

    for (list, dir) in [(&spare_b, &b_dir), (&spare_t, &t_dir), (&old_tmp, &tmp_dir)] {
        if list.is_empty() {
            continue;
        }
        if !yes && !confirm(&format!("delete {} entries in {}?", list.len(), dir.display())) {
            println!("abort...");
            continue;
        }
        let mut n = 0;
        for p in list.iter() {
            let is_dir = std::fs::symlink_metadata(p).map(|m| m.is_dir()).unwrap_or(false);
            let r = if is_dir { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) };
            match r {
                Ok(()) => n += 1,
                Err(e) => eprintln!("cannot delete {}: {e}", p.display()),
            }
        }
        println!("deleted {n} entries");
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hamming_matches_the_old_helper() {
        assert_eq!(hamming("ff", "00"), 8);
        assert_eq!(hamming("0f0f", "0f0e"), 1);
        assert_eq!(hamming("ab", "abcd"), 9999);
        assert_eq!(hamming("", ""), 9999);
    }

    #[test]
    fn frame_rule() {
        let (a, b) = ("ffffffff", "00000000"); // 32 bits apart
        assert_eq!(is_match(&["ff"], &["fe"]), Some(1));
        assert_eq!(is_match(&[a, a], &[a, b]), None); // 2 frames: both must match
        assert_eq!(is_match(&[a, a, a], &[a, b, a]), Some(11)); // 3 frames: 2 of 3 suffice
        assert_eq!(is_match(&[a, a, a], &[a, b, b]), None);
    }
}
