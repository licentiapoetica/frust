//! Running external tools (ffmpeg, magick, yt-dlp, exiftool…) without a shell.

use anyhow::{Result, bail};
use std::process::Stdio;
use std::time::Duration;

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
}

impl Default for Opts {
    fn default() -> Self {
        Opts { timeout: Duration::from_secs(600), ignore_exit_code: false, quiet: false, cwd: None, env: vec![], stdin: None }
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
