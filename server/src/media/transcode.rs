//! Re-encode video browsers can't play (MPEG-1/2 program streams, .mpg/.mpeg) to web MP4
//! (`src/inc/transcode.mjs`). A plain remux is not enough: MPEG-1/2 video has to become H.264.

use std::path::Path;

/// H.264/AAC MP4 at `dest`: yadif only touches frames flagged as interlaced, even dimensions +
/// yuv420p for broad decoder support, faststart so playback starts before the full download.
pub async fn transcode_to_mp4(src: &Path, dest: &Path) -> anyhow::Result<()> {
    let (s, d) = (src.to_string_lossy(), dest.to_string_lossy());
    crate::proc::spawn(
        "ffmpeg",
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-i", &s,
            "-map", "0:v:0", "-map", "0:a?",
            "-vf", "yadif=deint=interlaced,scale=trunc(iw/2)*2:trunc(ih/2)*2",
            "-c:v", "libx264", "-preset", "veryfast", "-crf", "22", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-b:a", "160k",
            "-movflags", "+faststart",
            "-f", "mp4",
            &d,
        ],
    )
    .await?;
    Ok(())
}
