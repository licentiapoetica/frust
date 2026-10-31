//! Upload-pipeline functions the bot triggers need (`queue.mjs`, `lib_delete.mjs`, `transcode.mjs`,
//! parts of `lib.mjs`).
//!
//! The simple ones are implemented here. `generate_phash` and `gen_thumbnail` are `TODO(port)`
//! stubs: they belong to the upload pipeline (`port/uploads`), and once that lands these should
//! become thin wrappers around the shared implementation (same signatures).

use crate::db::RowExt;
use crate::sql;
use crate::state::AppState;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// `queue.genuuid()` — 48 hex chars.
pub fn gen_uuid() -> String {
    let mut s = crate::util::random_hex(24);
    s.truncate(48);
    s
}

/// `queue.checkrepostlink(link)`
pub async fn check_repost_link(st: &AppState, link: &str) -> Result<Option<i64>> {
    Ok(sql!("SELECT id FROM items WHERE src = ", link, " LIMIT 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

/// `queue.checkrepostsum(checksum)`
pub async fn check_repost_sum(st: &AppState, checksum: &str) -> Result<Option<i64>> {
    Ok(sql!("SELECT id FROM items WHERE checksum = ", checksum, " LIMIT 1").fetch_optional(&st.db).await?.and_then(|r| r.i64("id")))
}

/// `queue.generatePHash(source)` — temporal dHash (`h1_h2_h3`).
/// TODO(port): provided by the upload pipeline (`port/uploads`). Until then no perceptual hash is
/// stored and only exact checksum reposts are detected.
pub async fn generate_phash(_st: &AppState, _source: &Path) -> Option<String> {
    None
}

/// `queue.checkrepostphash(hash)` — visual duplicate (≥ 2 of 3 frame hashes within distance 15).
pub async fn check_repost_phash(st: &AppState, phash: &str) -> Result<Option<i64>> {
    let parts: Vec<&str> = phash.split('_').filter(|s| !s.is_empty() && !s.starts_with("00000000")).collect();
    if parts.is_empty() {
        return Ok(None);
    }
    let h = |i: usize| parts.get(i).copied().unwrap_or("").to_string();
    let (h1, h2, h3) = (h(0), h(1), h(2));
    let row = sql!(
        "SELECT id FROM items
         WHERE is_deleted = false
           AND phash IS NOT NULL AND phash != '' AND phash != 'ERROR' AND phash != 'MISSING' AND phash NOT LIKE '00000000%'
           AND (
             (CASE WHEN split_part(phash, '_', 1) != '' AND ", h1.clone(), "::text != '' THEN
                bit_count(('x' || split_part(phash, '_', 1))::bit(1024) # ('x' || ", h1, "::text)::bit(1024)) <= 15 ELSE false END::int
              + CASE WHEN split_part(phash, '_', 2) != '' AND ", h2.clone(), "::text != '' THEN
                bit_count(('x' || split_part(phash, '_', 2))::bit(1024) # ('x' || ", h2.clone(), "::text)::bit(1024)) <= 15 ELSE false END::int
              + CASE WHEN split_part(phash, '_', 3) != '' AND ", h3.clone(), "::text != '' THEN
                bit_count(('x' || split_part(phash, '_', 3))::bit(1024) # ('x' || ", h3.clone(), "::text)::bit(1024)) <= 15 ELSE false END::int
             ) >= (CASE WHEN split_part(phash, '_', 3) != '' AND ", h3, "::text != '' THEN 2
                        WHEN split_part(phash, '_', 2) != '' AND ", h2, "::text != '' THEN 2
                        ELSE 1 END)
           )
         LIMIT 1"
    )
    .fetch_optional(&st.db)
    .await?;
    Ok(row.and_then(|r| r.i64("id")))
}

fn media_dirs(st: &AppState, pending: bool) -> (PathBuf, PathBuf) {
    let p = &st.cfg().paths;
    if pending { (p.pending.join("b"), p.pending.join("t")) } else { (p.b.clone(), p.t.clone()) }
}

/// `queue.genThumbnail(filename, mime, itemid, link, pending)`.
/// TODO(port): provided by the upload pipeline (`port/uploads`). Returning `Err` makes the caller
/// fall back to the placeholder thumbnail (`mugge.png`), exactly like a failed thumbnail before.
pub async fn gen_thumbnail(_st: &AppState, _filename: &str, _mime: &str, _item_id: i64, _link: &str, _pending: bool) -> Result<()> {
    bail!("thumbnail generation not ported yet")
}

/// Placeholder thumbnail used when thumbnail generation fails.
pub async fn placeholder_thumbnail(st: &AppState, item_id: i64, pending: bool) -> Result<()> {
    let (_, t) = media_dirs(st, pending);
    let src = st.cfg().base.join("mugge.png");
    let dst = t.join(format!("{item_id}.webp"));
    crate::proc::spawn("magick", &[&src.to_string_lossy(), &dst.to_string_lossy()]).await?;
    Ok(())
}

/// `queue.genBlurredThumbnail(itemid, pending)`
pub async fn gen_blurred_thumbnail(st: &AppState, item_id: i64, pending: bool) -> Result<()> {
    let (_, t) = media_dirs(st, pending);
    let t = tokio::fs::canonicalize(&t).await.unwrap_or(t);
    let src = t.join(format!("{item_id}.webp"));
    let dst = t.join(format!("{item_id}_blur.webp"));
    if tokio::fs::symlink_metadata(&dst).await.map(|m| m.file_type().is_symlink()).unwrap_or(false) {
        let _ = tokio::fs::remove_file(&dst).await;
    }
    crate::proc::spawn("magick", &[&src.to_string_lossy(), "-blur", "0x48", &dst.to_string_lossy()]).await?;
    Ok(())
}

/// `queue.notifyAdmins(itemid)` — pending-approval notification for admins and moderators.
pub async fn notify_admins(st: &AppState, item_id: i64) {
    let res = sql!(
        r#"INSERT INTO notifications (user_id, type, reference_id, item_id)
           SELECT id, 'admin_pending', 0, "#,
        item_id as i32,
        r#" FROM "user" WHERE admin = true OR is_moderator = true
           ON CONFLICT DO NOTHING"#
    )
    .execute(&st.db)
    .await;
    if let Err(e) = res {
        tracing::error!("[QUEUE] Failed to notify admins/mods: {e}");
    }
}

/// `transcode.mjs`: mimes browsers can't play that are re-encoded to MP4.
pub fn needs_transcode(mime: &str) -> bool {
    mime == "video/mpeg"
}

/// `transcodeToMp4(queue, src, dest)`
pub async fn transcode_to_mp4(src: &Path, dest: &Path) -> Result<()> {
    let (s, d) = (src.to_string_lossy(), dest.to_string_lossy());
    crate::proc::spawn(
        "ffmpeg",
        &[
            "-y", "-hide_banner", "-loglevel", "error", "-i", &s, "-map", "0:v:0", "-map", "0:a?", "-vf",
            "yadif=deint=interlaced,scale=trunc(iw/2)*2:trunc(ih/2)*2", "-c:v", "libx264", "-preset", "veryfast", "-crf", "22",
            "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart", &d,
        ],
    )
    .await?;
    Ok(())
}

/// sha256 of a file (replaces spawning `sha256sum`).
pub async fn sha256_file(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// `lib_delete.moveToDeleted(dest, id)`: move `b/<dest>` to `deleted/b/`. Reposts can be stored as
/// symlinks to the original file; if the real file is still referenced by other live items, one of
/// their symlinks is promoted to hold the file and the others are re-pointed.
pub async fn move_to_deleted(st: &AppState, dest: &str, deleted_id: i64) -> Result<()> {
    if dest.is_empty() || dest.contains('/') || dest.contains("..") {
        bail!("invalid dest {dest:?}");
    }
    let cfg = st.cfg();
    let src = cfg.paths.b.join(dest);
    let dst = cfg.paths.deleted.join("b").join(dest);
    let Ok(meta) = tokio::fs::symlink_metadata(&src).await else { return Ok(()) };
    if meta.file_type().is_symlink() {
        let _ = tokio::fs::copy(&src, &dst).await;
        let _ = tokio::fs::remove_file(&src).await;
        return Ok(());
    }
    let survivors = sql!(
        "SELECT id, dest FROM items WHERE id != ", deleted_id as i32,
        " AND is_deleted = false AND is_purged = false AND active = true AND dest IS NOT NULL"
    )
    .fetch_all(&st.db)
    .await?;
    let mut links = Vec::new();
    for s in survivors {
        let d = s.string("dest");
        if d.is_empty() || d.contains('/') {
            continue;
        }
        let p = cfg.paths.b.join(&d);
        if let Ok(target) = tokio::fs::read_link(&p).await {
            if cfg.paths.b.join(target) == src {
                links.push(d);
            }
        }
    }
    let _ = tokio::fs::copy(&src, &dst).await;
    let Some(promoted) = links.first() else {
        let _ = tokio::fs::remove_file(&src).await;
        return Ok(());
    };
    let promoted_path = cfg.paths.b.join(promoted);
    let res: std::io::Result<()> = async {
        tokio::fs::remove_file(&promoted_path).await?;
        tokio::fs::rename(&src, &promoted_path).await?;
        for other in &links[1..] {
            let p = cfg.paths.b.join(other);
            let _ = tokio::fs::remove_file(&p).await;
            #[cfg(unix)]
            let _ = tokio::fs::symlink(promoted, &p).await;
        }
        Ok(())
    }
    .await;
    if let Err(e) = res {
        tracing::error!("[DELETE] Promotion failed for b/{promoted}: {e}");
        let _ = tokio::fs::remove_file(&src).await;
    }
    Ok(())
}

/// Move a file between two directories (copy + unlink, errors ignored like the old `.catch(_ => {})`).
pub async fn move_file(from: &Path, to: &Path) {
    if tokio::fs::copy(from, to).await.is_ok() {
        let _ = tokio::fs::remove_file(from).await;
    }
}

/// Item tags (`lib.getTags(id)` without session data): `[{id, tag, normalized}]`, rating first.
pub async fn get_tags(st: &AppState, item_id: i64) -> Result<Vec<crate::db::Row>> {
    sql!(
        r#"SELECT tags.id, tags.tag, tags.normalized FROM tags_assign
           LEFT JOIN tags ON tags.id = tags_assign.tag_id
           WHERE tags_assign.item_id = "#,
        item_id as i32,
        r#" ORDER BY (CASE WHEN tags.id = 1 THEN 0 WHEN tags.id = 2 THEN 1 WHEN tags.normalized = 'nsfl' THEN 2 ELSE 3 END) ASC, tags.id ASC"#
    )
    .fetch_all(&st.db)
    .await
}

pub struct Counts {
    pub tagged: i64,
    pub untagged: i64,
    pub sfw: i64,
    pub nsfw: i64,
}

/// `lib.countf0cks()` (the fields the bot prints).
pub async fn count_f0cks(st: &AppState) -> Result<Counts> {
    let nsfl = st.cfg().nsfl_tag_id() as i32;
    let row = sql!(
        "SELECT
           count(*) FILTER (WHERE EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id)) AS tagged,
           count(*) FILTER (WHERE NOT EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id IN (1, 2, ", nsfl, "))) AS untagged,
           count(*) FILTER (WHERE EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 1)) AS sfw,
           count(*) FILTER (WHERE EXISTS (SELECT 1 FROM tags_assign ta WHERE ta.item_id = i.id AND ta.tag_id = 2)) AS nsfw
         FROM items i WHERE i.active = true AND i.is_deleted = false"
    )
    .fetch_optional(&st.db)
    .await?
    .unwrap_or_default();
    Ok(Counts {
        tagged: row.i64("tagged").unwrap_or(0),
        untagged: row.i64("untagged").unwrap_or(0),
        sfw: row.i64("sfw").unwrap_or(0),
        nsfw: row.i64("nsfw").unwrap_or(0),
    })
}
