//! The play time of videos and songs (`items.duration`, seconds), for the timecodes on thumbnails.
//!
//! One task fills it for every way an item comes in (upload, link, bot, import) and for the items
//! from before: every 30 s it probes the newest items without a duration with ffprobe. 0 means
//! unknown (no file, nothing to read), so an item isn't probed again and again; YouTube items have
//! no file and are left out.

use crate::db::RowExt;
use crate::proc::{self, Opts};
use crate::sql;
use crate::state::AppState;
use std::path::Path;
use std::time::Duration;

/// Items probed per round.
const BATCH: i64 = 40;

/// Seconds of a media file (ffprobe's format duration).
pub async fn probe(path: &Path) -> Option<f64> {
    let p = path.to_string_lossy().into_owned();
    let out = proc::spawn_with(
        "ffprobe",
        &["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0", &p],
        Opts { quiet: true, ignore_exit_code: true, timeout: Duration::from_secs(30), ..Opts::default() },
    )
    .await
    .ok()?;
    parse(&out.stdout_str())
}

fn parse(text: &str) -> Option<f64> {
    let secs: f64 = text.trim().lines().next()?.trim().parse().ok()?;
    (secs.is_finite() && secs > 0.0).then_some(secs)
}

/// One round: the newest videos and songs without a duration get theirs. The number done.
async fn fill(st: &AppState) -> usize {
    let rows = match sql!(
        "SELECT id, dest FROM items WHERE duration IS NULL AND active = true AND is_deleted = false \
         AND (mime LIKE 'video/%' OR mime LIKE 'audio/%') AND mime <> 'video/youtube' ORDER BY id DESC LIMIT ",
        BATCH
    )
    .fetch_all(&st.db)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[DURATION] {e}");
            return 0;
        }
    };
    let dir = st.cfg().paths.b.clone();
    for row in &rows {
        let id = row.i64("id").unwrap_or(0) as i32;
        let dest = row.string("dest");
        // the file name only (a dest is a plain name in /b)
        let name = Path::new(&dest).file_name().map(|n| n.to_owned());
        let secs = match name {
            Some(n) => probe(&dir.join(n)).await.unwrap_or(0.0),
            None => 0.0,
        };
        if let Err(e) = sql!("UPDATE items SET duration = (", secs, ")::real WHERE id = ", id).execute(&st.db).await {
            tracing::warn!("[DURATION] item {id}: {e}");
        }
    }
    rows.len()
}

/// Fills durations 20 s after boot and then every 30 s; a full round goes on right away (the items
/// from before the column, a batch at a time).
pub fn spawn_filler(st: &AppState) {
    let st = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(20)).await;
        let mut total = 0usize;
        loop {
            let n = fill(&st).await;
            total += n;
            if n as i64 >= BATCH {
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
            if total > 0 {
                tracing::info!("[DURATION] filled {total} items");
                total = 0;
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_ffprobe() {
        assert_eq!(super::parse("12.345000\n"), Some(12.345));
        assert_eq!(super::parse("N/A\n"), None);
        assert_eq!(super::parse(""), None);
        assert_eq!(super::parse("0.000000"), None);
    }
}
