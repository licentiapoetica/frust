//! Running external tools (ffmpeg, magick, yt-dlp, exiftool…) without a shell.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

/// ImageMagick policy for user files (src/magick-policy-uploads.xml, inside the crate so builds of
/// server/ alone, like the Dockerfile's, have it): no SVG/MVG/MSL/TEXT/URL/PS/PDF
/// coders, no `@file` or `/etc` reads, resource limits. A file named `.png` can still be an SVG.
const UPLOAD_MAGICK_POLICY: &str = include_str!("magick-policy-uploads.xml");

static MAGICK_POLICY_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Write the upload policy to `<tmp>/magick-policy-uploads` and use it for every `magick` run from
/// now on (unless the call sets its own `MAGICK_CONFIGURE_PATH` or is `trusted_input`).
pub fn install_magick_policy(tmp: &Path) {
    let dir = tmp.join("magick-policy-uploads");
    let written = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("policy.xml"), UPLOAD_MAGICK_POLICY));
    match written {
        Ok(()) => {
            let _ = MAGICK_POLICY_DIR.set(dir);
        }
        Err(e) => tracing::error!("[PROC] Could not write the ImageMagick policy to {}: {e}", dir.display()),
    }
}

pub struct Output {
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub code: Option<i32>,
}

impl Output {
    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

pub struct Opts {
    /// Kill the process after this long (default 10 minutes).
    pub timeout: Duration,
    /// Don't fail on a non-zero exit code.
    pub ignore_exit_code: bool,
    /// Don't log stderr.
    pub quiet: bool,
    pub cwd: Option<std::path::PathBuf>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<Vec<u8>>,
    /// The input is the admin's own (favicon SVG): run `magick` without the upload policy.
    pub trusted_input: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { timeout: Duration::from_secs(600), ignore_exit_code: false, quiet: false, cwd: None, env: vec![], stdin: None, trusted_input: false }
    }
}

/// `queue.spawn(cmd, args)` — arguments are passed as-is, never through a shell.
pub async fn spawn(cmd: &str, args: &[&str]) -> Result<Output> {
    spawn_with(cmd, args, Opts::default()).await
}

pub async fn spawn_with(cmd: &str, args: &[&str], opts: Opts) -> Result<Output> {
    let mut c = tokio::process::Command::new(cmd);
    c.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if opts.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .kill_on_drop(true);
    if let Some(d) = &opts.cwd {
        c.current_dir(d);
    }
    for (k, v) in &opts.env {
        c.env(k, v);
    }
    if cmd == "magick" && !opts.trusted_input && !opts.env.iter().any(|(k, _)| k == "MAGICK_CONFIGURE_PATH") {
        if let Some(dir) = MAGICK_POLICY_DIR.get() {
            c.env("MAGICK_CONFIGURE_PATH", dir);
        }
    }
    let mut child = c.spawn().map_err(|e| anyhow::anyhow!("failed to start {cmd}: {e}"))?;
    if let Some(input) = opts.stdin {
        use tokio::io::AsyncWriteExt;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&input).await.ok();
        }
    }
    let out = match tokio::time::timeout(opts.timeout, child.wait_with_output()).await {
        Ok(r) => r?,
        Err(_) => bail!("Command '{cmd}' timed out after {:?}", opts.timeout),
    };
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() && !opts.ignore_exit_code {
        bail!("Command '{cmd} {}' failed with code {:?}: {}", args.join(" "), out.status.code(), stderr.trim());
    }
    if !stderr.is_empty() && !opts.quiet {
        tracing::debug!("[{cmd}] {}", stderr.trim());
    }
    Ok(Output { stdout: out.stdout, stderr, code: out.status.code() })
}
