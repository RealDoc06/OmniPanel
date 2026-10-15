//! `openApp` and `open`: launch applications, open URLs / files / folders
//! with the system default handler.

use std::path::PathBuf;

use crate::proc::home_dir;
use crate::{ActionError, Result};

/// Launch an application by name, bundle / executable path or (Linux) desktop entry id.
pub(crate) async fn open_app(app: &str, args: &[String]) -> Result<()> {
    let app = app.trim();
    if app.is_empty() {
        return Err(ActionError::Invalid("no application given".into()));
    }
    imp::open_app(app, args).await
}

/// Open a URL, file or folder with its default handler.
pub(crate) async fn open(target: &str) -> Result<()> {
    imp::open(&Target::parse(target)?).await
}

/// What an `open` target refers to.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Url(String),
    Path(PathBuf),
}

impl Target {
    /// `https://...`, `mailto:...` and friends are URLs, existing paths are
    /// files, and a bare `example.com/page` is taken as a web address.
    fn parse(target: &str) -> Result<Self> {
        let target = target.trim();
        if target.is_empty() {
            return Err(ActionError::Invalid("nothing to open".into()));
        }
        if has_scheme(target) {
            return Ok(Self::Url(target.to_owned()));
        }
        let path = expand_home(target);
        if path.exists() {
            return Ok(Self::Path(path));
        }
        if looks_like_host(target) {
            return Ok(Self::Url(format!("https://{target}")));
        }
        Err(ActionError::Failed(format!("{target} does not exist")))
    }

    fn as_str(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Url(url) => url.into(),
            Self::Path(path) => path.to_string_lossy(),
        }
    }
}

/// `scheme:rest` with a scheme of two or more characters (so `C:\file` is a path).
fn has_scheme(target: &str) -> bool {
    let Some((scheme, rest)) = target.split_once(':') else { return false };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.starts_with('\\')
}

fn looks_like_host(target: &str) -> bool {
    let host = target.split('/').next().unwrap_or_default();
    !target.contains(char::is_whitespace) && host.contains('.') && !host.starts_with('.') && !host.ends_with('.')
}

fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), home_dir()) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use tokio::process::Command;

    use super::*;
    use crate::proc::{capture, start};

    /// Run `/usr/bin/open`, which returns as soon as LaunchServices has the request.
    async fn run_open(cmd: &mut Command) -> Result<()> {
        let out = capture(cmd, Duration::from_secs(15)).await?;
        if out.status.success() {
            return Ok(());
        }
        let reason = out.stderr.trim();
        Err(ActionError::Failed(if reason.is_empty() { "open failed".into() } else { reason.to_owned() }))
    }

    pub(super) async fn open_app(app: &str, args: &[String]) -> Result<()> {
        let path = expand_home(app);
        if path.is_file() {
            // A plain executable, not an app bundle.
            return start(Command::new(path).args(args)).await;
        }
        let mut cmd = Command::new("/usr/bin/open");
        if path.is_dir() || app.ends_with(".app") || !is_bundle_id(app) {
            cmd.arg("-a").arg(path);
        } else {
            cmd.arg("-b").arg(app);
        }
        if !args.is_empty() {
            cmd.arg("--args").args(args);
        }
        run_open(&mut cmd).await
    }

    pub(super) async fn open(target: &Target) -> Result<()> {
        run_open(Command::new("/usr/bin/open").arg(&*target.as_str())).await
    }

    /// `com.company.App`
    fn is_bundle_id(app: &str) -> bool {
        app.split('.').count() >= 3
            && app
                .split('.')
                .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn bundle_ids() {
            assert!(is_bundle_id("com.spotify.client"));
            assert!(is_bundle_id("org.mozilla.firefox"));
            assert!(!is_bundle_id("Spotify"));
            assert!(!is_bundle_id("Visual Studio Code"));
            assert!(!is_bundle_id("Google Chrome.app"));
            assert!(!is_bundle_id("node.js"));
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;

    use tokio::process::Command;
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoUninitialize,
    };
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    use super::*;
    use crate::blocking;
    use crate::proc::{NO_WINDOW, start};

    pub(super) async fn open_app(app: &str, args: &[String]) -> Result<()> {
        let path = expand_home(app);
        if path.exists() {
            let params = (!args.is_empty()).then(|| args.iter().map(|a| quote(a)).collect::<Vec<_>>().join(" "));
            return shell_execute(path.into_os_string(), params).await;
        }
        // Bare names ("spotify", "chrome"): let `start` resolve them like the Run dialog does.
        let mut line = format!("/S /C \"start \"\" {}", quote(app));
        for arg in args {
            line.push(' ');
            line.push_str(&quote(arg));
        }
        line.push('"');
        let mut cmd = Command::new("cmd");
        cmd.as_std_mut().raw_arg(line);
        cmd.creation_flags(NO_WINDOW);
        // `start` reports an unknown program with exit code 1.
        start(&mut cmd).await.map_err(|_| ActionError::Failed(format!("can't find {app}")))
    }

    pub(super) async fn open(target: &Target) -> Result<()> {
        shell_execute(target.as_str().into_owned().into(), None).await
    }

    /// `ShellExecuteW` "open": resolves .lnk, .exe, documents, folders and URLs.
    async fn shell_execute(file: std::ffi::OsString, params: Option<String>) -> Result<()> {
        blocking(move || {
            let wide = |s: &OsStr| s.encode_wide().chain(Some(0)).collect::<Vec<u16>>();
            let (verb, file_w) = (wide("open".as_ref()), wide(&file));
            let params_w = params.as_deref().map(|p| wide(p.as_ref()));
            // SAFETY: all strings are NUL-terminated and outlive the call. Shell extensions
            // may use COM, which the docs ask to initialize on the calling thread.
            let code = unsafe {
                let com = CoInitializeEx(std::ptr::null(), (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32);
                let result = ShellExecuteW(
                    std::ptr::null_mut(),
                    verb.as_ptr(),
                    file_w.as_ptr(),
                    params_w.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                    std::ptr::null(),
                    SW_SHOWNORMAL,
                );
                if com >= 0 {
                    CoUninitialize();
                }
                result as isize
            };
            if code > 32 {
                return Ok(());
            }
            let reason = match code {
                2 | 3 => "not found",
                5 => "access denied",
                31 => "no app is associated with this type",
                _ => "Windows could not open it",
            };
            Err(ActionError::Failed(format!("{}: {reason}", file.to_string_lossy())))
        })
        .await
    }

    /// Quote for the Windows command line (CommandLineToArgvW rules).
    fn quote(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
            return arg.to_owned();
        }
        let mut out = String::from("\"");
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    out.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    out.push('"');
                    backslashes = 0;
                }
                c => {
                    out.extend(std::iter::repeat_n('\\', backslashes));
                    out.push(c);
                    backslashes = 0;
                }
            }
        }
        out.extend(std::iter::repeat_n('\\', backslashes * 2));
        out.push('"');
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn quoting() {
            assert_eq!(quote("plain"), "plain");
            assert_eq!(quote("two words"), "\"two words\"");
            assert_eq!(quote(r#"say "hi""#), r#""say \"hi\"""#);
            assert_eq!(quote(r"C:\dir with space\"), r#""C:\dir with space\\""#);
            assert_eq!(quote(""), "\"\"");
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::time::Duration;

    use tokio::process::Command;

    use super::*;
    use crate::desktop;
    use crate::proc::{capture, find_in_path, start};

    pub(super) async fn open_app(app: &str, args: &[String]) -> Result<()> {
        if let Some(file) = desktop::find(app) {
            // gtk-launch knows terminals, D-Bus activation and startup notification,
            // but cannot pass arguments; fall back to the Exec line for those.
            if args.is_empty()
                && let (Some(gtk_launch), Some(id)) = (find_in_path("gtk-launch"), file.file_name())
            {
                let mut cmd = Command::new(gtk_launch);
                cmd.arg(id);
                if capture(&mut cmd, Duration::from_secs(10)).await.is_ok_and(|out| out.status.success()) {
                    return Ok(());
                }
            }
            let text = std::fs::read_to_string(&file)
                .map_err(|e| ActionError::Failed(format!("can't read {}: {e}", file.display())))?;
            let entry = desktop::parse(&text)
                .ok_or_else(|| ActionError::Failed(format!("{} is not an application entry", file.display())))?;
            let mut argv = desktop::command_line(&entry, args).into_iter();
            let program =
                argv.next().ok_or_else(|| ActionError::Failed(format!("{} has no Exec line", file.display())))?;
            let mut cmd = Command::new(program);
            cmd.args(argv);
            if let Some(dir) = &entry.path {
                cmd.current_dir(dir);
            }
            return start(&mut cmd).await;
        }

        let program = if app.contains('/') {
            expand_home(app)
        } else {
            find_in_path(app).ok_or_else(|| {
                ActionError::Failed(format!("{app} is not installed (no desktop entry or program with that name)"))
            })?
        };
        start(Command::new(program).args(args)).await
    }

    pub(super) async fn open(target: &Target) -> Result<()> {
        let xdg_open =
            find_in_path("xdg-open").ok_or_else(|| ActionError::Failed("xdg-open is not installed".into()))?;
        // xdg-open may stay in the foreground until the handler quits, so it is started like an app.
        start(Command::new(xdg_open).arg(&*target.as_str())).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_paths_and_hosts() {
        for url in ["https://example.com/a?b=1", "mailto:me@example.com", "spotify:track:abc", "file:///tmp/x"] {
            assert_eq!(Target::parse(url).unwrap(), Target::Url(url.into()), "{url}");
        }
        assert_eq!(Target::parse("example.com/docs").unwrap(), Target::Url("https://example.com/docs".into()));
        let dir = std::env::temp_dir();
        assert_eq!(Target::parse(dir.to_str().unwrap()).unwrap(), Target::Path(dir));
        assert!(matches!(Target::parse("definitely not a thing"), Err(ActionError::Failed(_))));
        assert!(matches!(Target::parse("  "), Err(ActionError::Invalid(_))));
    }

    #[test]
    fn windows_drive_paths_are_not_schemes() {
        assert!(!has_scheme(r"C:\Users\me"));
        assert!(!has_scheme("C:/Users/me"));
        assert!(has_scheme("steam://run/10"));
    }
}
