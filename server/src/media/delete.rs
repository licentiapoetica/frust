//! Safe deletion of media files for symlink-deduplicated uploads (`src/inc/lib_delete.mjs`).
//!
//! With `bypass_duplicate_check`, reposts are relative symlinks to the original file. Deleting the
//! original must not remove the data while another live item still points at it: one of those
//! symlinks is promoted to hold the real file and the others are re-targeted to it.

use super::is_plain_name;
use crate::db::RowExt;
use crate::sql;
use crate::state::AppState;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `path.resolve(dirname(link), readlink(link))` — lexical, like Node.
pub(crate) fn resolve_link(link: &Path, target: &Path) -> PathBuf {
    let base = link.parent().unwrap_or(Path::new("/"));
    let joined = base.join(target);
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Live items (other than `deleted_id`) whose `b/<dest>` is a symlink to `file_path`.
async fn symlink_survivors(st: &AppState, b_dir: &Path, file_path: &Path, deleted_id: i64) -> Vec<String> {
    let rows = match sql!(
        "SELECT id, dest FROM items WHERE id != ",
        deleted_id as i32,
        " AND is_deleted = false AND is_purged = false AND active = true AND dest IS NOT NULL"
    )
    .fetch_all(&st.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[DELETE] survivor lookup failed: {e}");
            return vec![];
        }
    };
    let mut out = Vec::new();
    for r in rows {
        let dest = r.string("dest");
        if !is_plain_name(&dest) {
            continue;
        }
        let p = b_dir.join(&dest);
        let Ok(meta) = tokio::fs::symlink_metadata(&p).await else { continue };
        if !meta.file_type().is_symlink() {
            continue;
        }
        if let Ok(target) = tokio::fs::read_link(&p).await {
            if resolve_link(&p, &target) == file_path {
                out.push(dest);
            }
        }
    }
    out
}

/// Promote the first survivor to hold the real file and re-target the rest.
async fn promote(b_dir: &Path, file_path: &Path, survivors: &[String], dest: &str) -> std::io::Result<()> {
    let promoted = &survivors[0];
    let promoted_path = b_dir.join(promoted);
    tokio::fs::remove_file(&promoted_path).await?;
    tokio::fs::rename(file_path, &promoted_path).await?;
    tracing::info!("[DELETE] Promoted b/{promoted} as new canonical file (was b/{dest})");
    for s in &survivors[1..] {
        let sp = b_dir.join(s);
        let r = async {
            tokio::fs::remove_file(&sp).await?;
            tokio::fs::symlink(promoted, &sp).await
        }
        .await;
        match r {
            Ok(()) => tracing::info!("[DELETE] Re-targeted symlink b/{s} → {promoted}"),
            Err(e) => tracing::error!("[DELETE] Failed to re-target b/{s}: {e}"),
        }
    }
    Ok(())
}

/// Remove `b/<dest>` of a deleted item without breaking reposts that symlink to it.
pub async fn safe_delete_media_file(st: &AppState, dest: &str, deleted_id: i64) {
    if !is_plain_name(dest) {
        return;
    }
    let b_dir = st.cfg().paths.b.clone();
    let file_path = b_dir.join(dest);
    let Ok(meta) = tokio::fs::symlink_metadata(&file_path).await else { return };
    if meta.file_type().is_symlink() {
        let _ = tokio::fs::remove_file(&file_path).await;
        tracing::info!("[DELETE] Unlinked symlink b/{dest}");
        return;
    }
    let survivors = symlink_survivors(st, &b_dir, &file_path, deleted_id).await;
    if survivors.is_empty() {
        let _ = tokio::fs::remove_file(&file_path).await;
        tracing::info!("[DELETE] Unlinked real file b/{dest} (no surviving references)");
        return;
    }
    if let Err(e) = promote(&b_dir, &file_path, &survivors, dest).await {
        tracing::error!("[DELETE] Failed to promote b/{}: {e}", survivors[0]);
        let _ = tokio::fs::remove_file(&file_path).await;
    }
}

/// Move `b/<dest>` to `deleted/b/<dest>` (trash), keeping reposts that symlink to it working.
pub async fn move_to_deleted(st: &AppState, dest: &str, deleted_id: i64) {
    if !is_plain_name(dest) {
        return;
    }
    let cfg = st.cfg();
    let b_dir = cfg.paths.b.clone();
    let src = b_dir.join(dest);
    let dst = cfg.paths.deleted.join("b").join(dest);
    let Ok(meta) = tokio::fs::symlink_metadata(&src).await else { return };
    if meta.file_type().is_symlink() {
        // Copy the real data so the trash entry is self-contained
        let _ = tokio::fs::copy(&src, &dst).await;
        let _ = tokio::fs::remove_file(&src).await;
        tracing::info!("[DELETE] Moved symlink b/{dest} to deleted/ (copied real data)");
        return;
    }
    let survivors = symlink_survivors(st, &b_dir, &src, deleted_id).await;
    let _ = tokio::fs::copy(&src, &dst).await;
    if survivors.is_empty() {
        let _ = tokio::fs::remove_file(&src).await;
        tracing::info!("[DELETE] Moved real file b/{dest} to deleted/");
        return;
    }
    if let Err(e) = promote(&b_dir, &src, &survivors, dest).await {
        tracing::error!("[DELETE] Promotion failed for b/{}: {e}", survivors[0]);
        let _ = tokio::fs::remove_file(&src).await;
    }
}

/// Delete all album sub-item files (and their thumbnails) of an item, then the rows.
pub async fn safe_delete_album_files(st: &AppState, item_id: i64) {
    let rows = match sql!("SELECT dest FROM album_items WHERE item_id = ", item_id as i32).fetch_all(&st.db).await {
        Ok(r) => r,
        Err(e) => return tracing::error!("[DELETE] Failed to delete album files for item #{item_id}: {e}"),
    };
    let cfg = st.cfg();
    let (t_dir, pending) = (cfg.paths.t.clone(), cfg.paths.pending.clone());
    for r in rows {
        let dest = r.string("dest");
        if dest.is_empty() {
            continue;
        }
        safe_delete_media_file(st, &dest, item_id).await;
        let thumb = album_thumb_name(&dest);
        if is_plain_name(&thumb) {
            let _ = tokio::fs::remove_file(t_dir.join(&thumb)).await;
            let _ = tokio::fs::remove_file(pending.join("t").join(&thumb)).await;
        }
        // Sub-files of items still pending approval
        if is_plain_name(&dest) {
            let _ = tokio::fs::remove_file(pending.join("b").join(&dest)).await;
        }
    }
    let _ = sql!("DELETE FROM album_items WHERE item_id = ", item_id as i32).execute(&st.db).await;
}

/// `dest.replace(/\.[^.]+$/, '.webp')`
pub fn album_thumb_name(dest: &str) -> String {
    match dest.rfind('.') {
        Some(i) if i + 1 < dest.len() => format!("{}.webp", &dest[..i]),
        _ => dest.to_string(),
    }
}

/// Purge items whose `expires_at` has passed: files, thumbnails, cover art, album files, and
/// mark the row deleted + purged.
pub async fn purge_expired_uploads(st: &AppState) {
    let cfg = st.cfg();
    if !crate::settings::enable_expiring_uploads(&cfg) {
        return;
    }
    let now = crate::util::now_secs();
    let items = match sql!("SELECT id, dest, mime FROM items WHERE expires_at IS NOT NULL AND expires_at <= ", now, " AND is_purged = false")
        .fetch_all(&st.db)
        .await
    {
        Ok(r) => r,
        Err(e) => return tracing::error!("[EXPIRING UPLOADS] Failed running purgeExpiredUploads check: {e}"),
    };
    if items.is_empty() {
        return;
    }
    tracing::info!("[EXPIRING UPLOADS] Found {} expired item(s) to purge.", items.len());
    for item in items {
        let Some(id) = item.i64("id") else { continue };
        let dest = item.string("dest");
        let res = async {
            if !dest.is_empty() {
                safe_delete_media_file(st, &dest, id).await;
            }
            safe_delete_album_files(st, id).await;
            let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("{id}.webp"))).await;
            let _ = tokio::fs::remove_file(cfg.paths.t.join(format!("{id}_blur.webp"))).await;
            if item.string("mime").starts_with("audio") {
                let _ = tokio::fs::remove_file(cfg.paths.ca.join(format!("{id}.webp"))).await;
            }
            // Items that expired while still pending approval: their files live in pending/
            // (never a symlink target — reposts only link to files in b/).
            let p = &cfg.paths.pending;
            if is_plain_name(&dest) {
                let _ = tokio::fs::remove_file(p.join("b").join(&dest)).await;
            }
            for f in [p.join("t").join(format!("{id}.webp")), p.join("t").join(format!("{id}_blur.webp")), p.join("ca").join(format!("{id}.webp"))] {
                let _ = tokio::fs::remove_file(f).await;
            }
            sql!("UPDATE items SET is_deleted = true, is_purged = true, active = false WHERE id = ", id as i32).execute(&st.db).await?;
            crate::private_items::remove_private(st, id, &dest);
            anyhow::Ok(())
        }
        .await;
        match res {
            Ok(()) => tracing::info!("[EXPIRING UPLOADS] Successfully purged expired item #{id}"),
            Err(e) => tracing::error!("[EXPIRING UPLOADS] Error purging item #{id}: {e}"),
        }
    }
}

/// Run [`purge_expired_uploads`] 10 s after boot and then every 30 s (the old setTimeout/setInterval).
pub fn spawn_expiry_purger(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            purge_expired_uploads(&st).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_resolution() {
        let b = Path::new("/srv/b/new.webm");
        assert_eq!(resolve_link(b, Path::new("orig.webm")), PathBuf::from("/srv/b/orig.webm"));
        assert_eq!(resolve_link(Path::new("/srv/pending/b/x.webm"), Path::new("../../b/orig.webm")), PathBuf::from("/srv/b/orig.webm"));
        assert_eq!(resolve_link(b, Path::new("/abs/file")), PathBuf::from("/abs/file"));
    }

    #[test]
    fn album_thumbs() {
        assert_eq!(album_thumb_name("abc.png"), "abc.webp");
        assert_eq!(album_thumb_name("yt:dQw4w9WgXcQ"), "yt:dQw4w9WgXcQ");
        assert_eq!(album_thumb_name("a.b.mp4"), "a.b.webp");
    }
}
