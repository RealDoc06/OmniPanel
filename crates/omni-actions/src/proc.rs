//! Child process helpers: PATH lookup, detached spawning and bounded capture
//! with a timeout that kills the whole process tree.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::{ActionError, Result};

/// `CREATE_NO_WINDOW`: console programs run without flashing a window.
#[cfg(windows)]
pub(crate) const NO_WINDOW: u32 = 0x0800_0000;

/// Most output we keep per stream; the rest is read and discarded.
const CAPTURE_LIMIT: usize = 64 * 1024;
/// How long a started app may take to fail before it counts as running.
const START_GRACE: Duration = Duration::from_millis(400);
/// How long pipes may stay open after the process exited (a background
/// child can inherit them).
const PIPE_GRACE: Duration = Duration::from_millis(250);

/// Extra directories GUI apps do not get in `PATH` (Finder / Dock launches
/// only see `/usr/bin:/bin:...`), where Homebrew and user tools live.
#[cfg(unix)]
fn extra_path_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> =
        ["/opt/homebrew/bin", "/opt/homebrew/sbin", "/usr/local/bin", "/usr/local/sbin", "/snap/bin"]
            .iter()
            .map(PathBuf::from)
            .collect();
    if let Some(home) = home_dir() {
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

#[cfg(windows)]
fn extra_path_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// The user's `PATH` plus the usual tool directories missing from GUI launches.
fn search_path() -> &'static OsString {
    static PATH: OnceLock<OsString> = OnceLock::new();
    PATH.get_or_init(|| {
        let mut dirs: Vec<PathBuf> =
            std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        for dir in extra_path_dirs() {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        std::env::join_paths(dirs).unwrap_or_default()
    })
}

pub(crate) fn home_dir() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

/// Resolve a program name against `PATH` (plus [`extra_path_dirs`]).
pub(crate) fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(search_path()).find_map(|dir| executable_in(&dir, name))
}

fn executable_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat", ".com"] } else { &[""] };
    suffixes.iter().map(|suffix| dir.join(format!("{name}{suffix}"))).find(|path| is_executable(path))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Error for a program that could not be started.
pub(crate) fn spawn_error(program: &str, error: &io::Error) -> ActionError {
    if error.kind() == io::ErrorKind::NotFound {
        ActionError::Failed(format!("{program} not found (is it installed and on PATH?)"))
    } else {
        ActionError::Failed(format!("can't start {program}: {error}"))
    }
}

/// Settings every child gets: own process group (so the tree can be killed
/// and Ctrl+C in a terminal does not reach it), no console window, a
/// complete `PATH`.
fn isolate(cmd: &mut Command) {
    #[cfg(unix)]
    {
        cmd.process_group(0);
        cmd.env("PATH", search_path());
    }
    #[cfg(windows)]
    cmd.creation_flags(NO_WINDOW);
}

fn program_name(cmd: &Command) -> String {
    cmd.as_std().get_program().to_string_lossy().into_owned()
}

/// Start an app that keeps running on its own: it survives OmniPanel and the
/// tokio runtime reaps it when it exits. A program that fails right away
/// (bad path, crash) is reported, one that is still running is left alone.
pub(crate) async fn start(cmd: &mut Command) -> Result<()> {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(false);
    isolate(cmd);
    let program = program_name(cmd);
    let mut child = cmd.spawn().map_err(|e| spawn_error(&program, &e))?;
    match timeout(START_GRACE, child.wait()).await {
        Ok(Ok(status)) if !status.success() => Err(ActionError::Failed(match status.code() {
            Some(code) => format!("{program} exited with code {code}"),
            None => format!("{program} was terminated"),
        })),
        _ => Ok(()),
    }
}

pub(crate) struct Captured {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Run to completion, capturing output. After `limit` the process tree is
/// killed and [`ActionError::Timeout`] returned.
pub(crate) async fn capture(cmd: &mut Command, limit: Duration) -> Result<Captured> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    isolate(cmd);
    let mut child = cmd.spawn().map_err(|e| spawn_error(&program_name(cmd), &e))?;
    let pid = child.id();
    let (stdout, stdout_done) = collect(child.stdout.take());
    let (stderr, stderr_done) = collect(child.stderr.take());

    let status = match timeout(limit, child.wait()).await {
        Ok(status) => status.map_err(|e| ActionError::Failed(format!("can't wait for {}: {e}", program_name(cmd))))?,
        Err(_) => {
            if let Some(pid) = pid {
                kill_tree(pid).await;
            }
            let _ = child.wait().await;
            return Err(ActionError::Timeout(u32::try_from(limit.as_secs()).unwrap_or(u32::MAX)));
        }
    };
    // A background child can keep the pipes open for good. Its output is then
    // discarded (not left unread, which would kill it with SIGPIPE) and not waited for.
    let _ = timeout(PIPE_GRACE, async {
        let _ = tokio::join!(stdout_done, stderr_done);
    })
    .await;

    let text = |buf: &Mutex<Vec<u8>>| {
        String::from_utf8_lossy(&buf.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
    };
    Ok(Captured { status, stdout: text(&stdout), stderr: text(&stderr) })
}

/// Read a pipe to EOF in the background, keeping the first [`CAPTURE_LIMIT`] bytes.
fn collect(reader: Option<impl AsyncRead + Unpin + Send + 'static>) -> (Arc<Mutex<Vec<u8>>>, JoinHandle<()>) {
    let buf = Arc::new(Mutex::new(Vec::new()));
    let shared = buf.clone();
    let task = tokio::spawn(async move {
        let Some(mut reader) = reader else { return };
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut buf = shared.lock().unwrap_or_else(PoisonError::into_inner);
                    let room = CAPTURE_LIMIT.saturating_sub(buf.len());
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
    });
    (buf, task)
}

/// Kill a child started through [`capture`] and everything it spawned.
async fn kill_tree(pid: u32) {
    #[cfg(unix)]
    if let Ok(pid) = i32::try_from(pid) {
        // The child leads its own process group (see `isolate`).
        // SAFETY: plain syscall; a negative pid addresses the group.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        taskkill.args(["/PID", &pid.to_string(), "/T", "/F"]).stdout(Stdio::null()).stderr(Stdio::null());
        taskkill.creation_flags(NO_WINDOW);
        let _ = taskkill.status().await;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn captures_output_and_exit_status() {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
        let out = capture(&mut cmd, Duration::from_secs(5)).await.unwrap();
        assert_eq!(out.status.code(), Some(3));
        assert_eq!((out.stdout.trim(), out.stderr.trim()), ("out", "err"));
    }

    #[tokio::test]
    async fn timeout_kills_the_process_tree() {
        let marker = std::env::temp_dir().join(format!("omni-actions-tree-{}", omni_core::new_id()));
        let script = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        let started = std::time::Instant::now();
        let result = capture(&mut cmd, Duration::from_millis(300)).await;
        assert!(matches!(result, Err(ActionError::Timeout(0))), "{:?}", result.err());
        assert!(started.elapsed() < Duration::from_millis(900));
        tokio::time::sleep(Duration::from_millis(1400)).await;
        assert!(!marker.exists(), "background child survived the timeout");
    }

    #[tokio::test]
    async fn background_child_holding_pipes_neither_blocks_nor_dies() {
        let marker = std::env::temp_dir().join(format!("omni-actions-bg-{}", omni_core::new_id()));
        let script = format!("echo done; (sleep 1; echo late; touch {}) &", marker.display());
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", &script]);
        let started = std::time::Instant::now();
        let out = capture(&mut cmd, Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.stdout.trim(), "done");
        assert!(started.elapsed() < Duration::from_millis(900));
        // Writing to the closed-off pipe must not kill it.
        tokio::time::sleep(Duration::from_millis(1400)).await;
        assert!(marker.exists());
        let _ = std::fs::remove_file(marker);
    }
}
