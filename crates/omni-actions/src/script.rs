//! `runScript`: run a script with one of the supported interpreters.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use omni_core::{ActionOutcome, Interpreter};
use tokio::process::Command;

use crate::proc::{capture, find_in_path, home_dir};
use crate::{ActionError, Result};

/// Longest output returned in the outcome message.
const MESSAGE_LIMIT: usize = 1500;

pub(crate) async fn run(interpreter: Interpreter, script: &str, timeout_secs: u32) -> Result<ActionOutcome> {
    let script = script.trim_end();
    if script.trim().is_empty() {
        return Err(ActionError::Invalid("the script is empty".into()));
    }
    // Keeps the temp file (if any) alive until the process is gone.
    let (mut cmd, _temp) = command(interpreter, script)?;
    cmd.env("OMNIPANEL", "1");
    if let Some(home) = home_dir() {
        cmd.current_dir(home);
    }

    let timeout = timeout_secs.max(1);
    let out = capture(&mut cmd, Duration::from_secs(u64::from(timeout))).await?;
    if out.status.success() {
        let text = [out.stdout.trim(), out.stderr.trim()].into_iter().find(|t| !t.is_empty());
        return Ok(ActionOutcome { message: text.map(truncate) });
    }
    let detail = [out.stderr.trim(), out.stdout.trim()].into_iter().find(|t| !t.is_empty());
    Err(ActionError::Failed(match (detail, out.status.code()) {
        (Some(text), _) => truncate(text),
        (None, Some(code)) => format!("script exited with code {code}"),
        (None, None) => "script was terminated".into(),
    }))
}

/// How to start an interpreter.
struct Spec {
    program: PathBuf,
    /// Arguments before the script.
    base: Vec<&'static str>,
    /// Flag that takes the script text.
    inline_flag: &'static str,
    /// Flag that takes a script file, when the file is not simply the last argument.
    file_flag: Option<&'static str>,
    /// Extension of the temp file.
    extension: &'static str,
}

/// Interpreter invocation. One-liners are passed inline, anything longer
/// goes through a temp file.
fn command(interpreter: Interpreter, script: &str) -> Result<(Command, Option<TempScript>)> {
    // Windows PowerShell prints in the OEM code page unless told otherwise.
    let utf8_output;
    let script = if cfg!(windows) && interpreter == Interpreter::PowerShell {
        utf8_output = format!("try {{ [Console]::OutputEncoding = [Text.Encoding]::UTF8 }} catch {{}}\n{script}");
        &utf8_output
    } else {
        script
    };
    let multiline = script.contains('\n');
    let spec = match interpreter {
        Interpreter::Shell if cfg!(windows) => return windows_cmd(script, multiline),
        Interpreter::Shell => {
            Spec { program: "/bin/sh".into(), base: vec![], inline_flag: "-c", file_flag: None, extension: "sh" }
        }
        Interpreter::Bash => Spec {
            program: find("bash", &["bash"])?,
            base: vec![],
            inline_flag: "-c",
            file_flag: None,
            extension: "sh",
        },
        Interpreter::PowerShell => Spec {
            program: find("PowerShell", if cfg!(windows) { &["powershell", "pwsh"] } else { &["pwsh"] })?,
            base: vec!["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass"],
            inline_flag: "-Command",
            file_flag: Some("-File"),
            extension: "ps1",
        },
        Interpreter::Python => Spec {
            // `python3` on Windows is often the Microsoft Store stub.
            program: find("Python", if cfg!(windows) { &["python", "py", "python3"] } else { &["python3", "python"] })?,
            base: vec![],
            inline_flag: "-c",
            file_flag: None,
            extension: "py",
        },
        Interpreter::Node => Spec {
            program: find("Node.js", &["node"])?,
            base: vec![],
            inline_flag: "-e",
            file_flag: None,
            extension: "js",
        },
        Interpreter::AppleScript if cfg!(target_os = "macos") => Spec {
            program: "/usr/bin/osascript".into(),
            base: vec![],
            inline_flag: "-e",
            file_flag: None,
            extension: "applescript",
        },
        Interpreter::AppleScript => return Err(ActionError::Unsupported("AppleScript only runs on macOS".into())),
    };

    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.base);
    if interpreter == Interpreter::Python {
        cmd.env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");
    }
    if !multiline {
        cmd.arg(spec.inline_flag).arg(script);
        return Ok((cmd, None));
    }
    let file = TempScript::create(script, spec.extension)?;
    if let Some(flag) = spec.file_flag {
        cmd.arg(flag);
    }
    cmd.arg(&file.0);
    Ok((cmd, Some(file)))
}

/// First of `names` found on `PATH`.
fn find(what: &str, names: &[&str]) -> Result<PathBuf> {
    names
        .iter()
        .find_map(|name| find_in_path(name))
        .ok_or_else(|| ActionError::Failed(format!("{what} is not installed (or not on PATH)")))
}

/// `cmd /C`. With `/S` the outer quotes are stripped and the rest is run verbatim,
/// so the script text must bypass Rust's argument escaping.
#[cfg(windows)]
fn windows_cmd(script: &str, multiline: bool) -> Result<(Command, Option<TempScript>)> {
    use std::os::windows::process::CommandExt;
    // `chcp 65001` makes the output UTF-8 instead of the OEM code page.
    let batch = format!("@chcp 65001>nul\r\n{}", script.replace("\r\n", "\n").replace('\n', "\r\n"));
    let temp = if multiline { Some(TempScript::create(&batch, "cmd")?) } else { None };
    let line = match &temp {
        Some(file) => format!("\"{}\"", file.0.display()),
        None => format!("chcp 65001>nul & {script}"),
    };
    let mut cmd = Command::new("cmd");
    cmd.as_std_mut().raw_arg(format!("/S /C \"{line}\""));
    Ok((cmd, temp))
}

#[cfg(not(windows))]
fn windows_cmd(_: &str, _: bool) -> Result<(Command, Option<TempScript>)> {
    unreachable!("only used on Windows")
}

/// Script file in the temp dir, removed on drop.
struct TempScript(PathBuf);

impl TempScript {
    fn create(script: &str, extension: &str) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("omnipanel-{}.{extension}", omni_core::new_id()));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let fail = |e: std::io::Error| ActionError::Failed(format!("can't write the script file: {e}"));
        let mut file = options.open(&path).map_err(fail)?;
        let temp = Self(path);
        // Windows PowerShell reads BOM-less files as ANSI.
        if extension == "ps1" {
            file.write_all(b"\xEF\xBB\xBF").map_err(fail)?;
        }
        file.write_all(script.as_bytes()).map_err(fail)?;
        Ok(temp)
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn truncate(text: &str) -> String {
    if text.len() <= MESSAGE_LIMIT {
        return text.to_owned();
    }
    let end = (0..=MESSAGE_LIMIT).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}\u{2026}", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_a_char_boundary() {
        assert_eq!(truncate("short"), "short");
        let long = "\u{e9}".repeat(MESSAGE_LIMIT);
        let cut = truncate(&long);
        assert!(cut.len() <= MESSAGE_LIMIT + '\u{2026}'.len_utf8());
        assert!(cut.ends_with('\u{2026}'));
    }

    #[test]
    fn temp_script_is_removed_on_drop() {
        let file = TempScript::create("echo hi", "sh").unwrap();
        let path = file.0.clone();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo hi");
        drop(file);
        assert!(!path.exists());
    }

    #[cfg(unix)]
    mod unix {
        use super::*;

        async fn shell(script: &str, timeout: u32) -> Result<ActionOutcome> {
            run(Interpreter::Shell, script, timeout).await
        }

        #[tokio::test]
        async fn returns_trimmed_stdout() {
            assert_eq!(shell("echo '  hello  '", 5).await.unwrap().message.as_deref(), Some("hello"));
            assert_eq!(shell("true", 5).await.unwrap().message, None);
        }

        #[tokio::test]
        async fn multi_line_scripts_run_from_a_file() {
            let out = shell("A=one\nB=two\necho \"$A $B\"\necho $OMNIPANEL", 5).await.unwrap();
            assert_eq!(out.message.as_deref(), Some("one two\n1"));
        }

        #[tokio::test]
        async fn non_zero_exit_reports_stderr_or_code() {
            let err = shell("echo broken >&2; exit 2", 5).await.unwrap_err();
            assert!(matches!(&err, ActionError::Failed(m) if m == "broken"), "{err}");
            let err = shell("exit 7", 5).await.unwrap_err();
            assert!(matches!(&err, ActionError::Failed(m) if m == "script exited with code 7"), "{err}");
        }

        #[tokio::test]
        async fn timeout_is_enforced() {
            let started = std::time::Instant::now();
            let err = shell("sleep 30", 1).await.unwrap_err();
            assert!(matches!(err, ActionError::Timeout(1)), "{err}");
            assert!(started.elapsed() < Duration::from_secs(5));
        }

        #[tokio::test]
        async fn long_output_is_truncated() {
            let out = shell("head -c 5000 /dev/zero | tr '\\0' 'x'", 5).await.unwrap();
            assert_eq!(out.message.unwrap().chars().count(), MESSAGE_LIMIT + 1);
        }

        #[tokio::test]
        async fn bash_and_python_when_installed() {
            let out = run(Interpreter::Bash, "echo $((6 * 7))", 5).await.unwrap();
            assert_eq!(out.message.as_deref(), Some("42"));
            if find_in_path("python3").is_some() {
                let out = run(Interpreter::Python, "print(6 * 7)", 10).await.unwrap();
                assert_eq!(out.message.as_deref(), Some("42"));
                let out = run(Interpreter::Python, "import sys\nprint(sys.argv[0] != '')", 10).await.unwrap();
                assert_eq!(out.message.as_deref(), Some("True"));
            }
        }

        #[tokio::test]
        async fn empty_script_is_invalid() {
            assert!(matches!(shell("  \n ", 5).await, Err(ActionError::Invalid(_))));
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn applescript_runs_on_macos() {
        let out = run(Interpreter::AppleScript, "return 6 * 7", 10).await.unwrap();
        assert_eq!(out.message.as_deref(), Some("42"));
    }
}
