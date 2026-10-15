//! `system` actions: lock the session, sleep, turn the display off.

use omni_core::SystemCommand;

use crate::Result;

pub(crate) async fn run(command: SystemCommand) -> Result<()> {
    imp::run(command).await
}

#[cfg(target_os = "macos")]
mod imp {
    use std::time::Duration;

    use omni_core::SystemCommand;
    use tokio::process::Command;

    use crate::proc::capture;
    use crate::{ActionError, Result, blocking};

    pub(super) async fn run(command: SystemCommand) -> Result<()> {
        match command {
            SystemCommand::Lock => match blocking(lock_screen).await {
                Ok(()) => Ok(()),
                // Without the lock symbol, sleeping the display locks if "require password" is immediate.
                Err(_) => pmset("displaysleepnow").await,
            },
            SystemCommand::Sleep => pmset("sleepnow").await,
            SystemCommand::DisplaySleep => pmset("displaysleepnow").await,
        }
    }

    async fn pmset(arg: &str) -> Result<()> {
        let out = capture(Command::new("/usr/bin/pmset").arg(arg), Duration::from_secs(10)).await?;
        if out.status.success() {
            Ok(())
        } else {
            Err(ActionError::Failed(format!("pmset {arg} failed: {}", out.stderr.trim())))
        }
    }

    /// `SACLockScreenImmediate` from login.framework: what the "Lock Screen" menu item
    /// calls. It is the one way to lock without synthesizing Ctrl+Cmd+Q, so it needs no
    /// permission; if Apple ever removes it the caller falls back to display sleep.
    fn lock_screen() -> Result<()> {
        const LOGIN: &std::ffi::CStr = c"/System/Library/PrivateFrameworks/login.framework/Versions/Current/login";
        const SYMBOL: &std::ffi::CStr = c"SACLockScreenImmediate";
        // SAFETY: dlopen / dlsym are thread-safe; the symbol is `int SACLockScreenImmediate(void)`.
        unsafe {
            let handle = libc::dlopen(LOGIN.as_ptr(), libc::RTLD_LAZY);
            if handle.is_null() {
                return Err(ActionError::Unsupported("login.framework is not available".into()));
            }
            let symbol = libc::dlsym(handle, SYMBOL.as_ptr());
            if symbol.is_null() {
                return Err(ActionError::Unsupported("this macOS has no lock screen entry point".into()));
            }
            let lock: extern "C" fn() -> i32 = std::mem::transmute(symbol);
            lock();
        }
        Ok(())
    }
}

#[cfg(windows)]
mod imp {
    use omni_core::SystemCommand;
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::System::Power::SetSuspendState;
    use windows_sys::Win32::System::Shutdown::LockWorkStation;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        HWND_BROADCAST, SC_MONITORPOWER, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SYSCOMMAND,
    };

    use crate::{ActionError, Result, blocking};

    pub(super) async fn run(command: SystemCommand) -> Result<()> {
        blocking(move || match command {
            SystemCommand::Lock => {
                // SAFETY: no arguments.
                if unsafe { LockWorkStation() } == 0 {
                    return Err(ActionError::Failed("Windows refused to lock the workstation".into()));
                }
                Ok(())
            }
            SystemCommand::Sleep => {
                // Suspend (not hibernate), do not force.
                // SAFETY: plain flags.
                if !unsafe { SetSuspendState(false, false, false) } {
                    return Err(ActionError::Failed("Windows refused to sleep".into()));
                }
                Ok(())
            }
            SystemCommand::DisplaySleep => {
                let broadcast: HWND = HWND_BROADCAST;
                // 2 = monitor off. A timeout keeps a hung window from blocking the broadcast.
                // SAFETY: documented use of SC_MONITORPOWER; the result pointer is null (unused).
                let sent = unsafe {
                    SendMessageTimeoutW(
                        broadcast,
                        WM_SYSCOMMAND,
                        SC_MONITORPOWER as usize,
                        2,
                        SMTO_ABORTIFHUNG,
                        1000,
                        std::ptr::null_mut(),
                    )
                };
                if sent == 0 {
                    return Err(ActionError::Failed("could not turn the display off".into()));
                }
                Ok(())
            }
        })
        .await
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use std::time::Duration;

    use omni_core::SystemCommand;
    use tokio::process::Command;

    use crate::proc::{capture, find_in_path};
    use crate::{ActionError, Result};

    pub(super) async fn run(command: SystemCommand) -> Result<()> {
        match command {
            SystemCommand::Lock => try_each(&[&["loginctl", "lock-session"], &["xdg-screensaver", "lock"]]).await,
            SystemCommand::Sleep => try_each(&[&["systemctl", "suspend"], &["loginctl", "suspend"]]).await,
            SystemCommand::DisplaySleep => {
                const GNOME: &[&str] = &[
                    "busctl",
                    "--user",
                    "set-property",
                    "org.gnome.Mutter.DisplayConfig",
                    "/org/gnome/Mutter/DisplayConfig",
                    "org.gnome.Mutter.DisplayConfig",
                    "PowerSaveMode",
                    "i",
                    "3",
                ];
                const KDE: &[&str] = &["kscreen-doctor", "--dpms", "off"];
                const X11: &[&str] = &["xset", "dpms", "force", "off"];
                // xset only reaches X11 clients under Wayland, so try the compositor tools first there.
                if std::env::var("XDG_SESSION_TYPE").is_ok_and(|t| t.eq_ignore_ascii_case("wayland")) {
                    try_each(&[GNOME, KDE, X11]).await
                } else {
                    try_each(&[X11, GNOME, KDE]).await
                }
            }
        }
    }

    /// Run the first command whose program is installed and succeeds.
    async fn try_each(commands: &[&[&str]]) -> Result<()> {
        let mut failure = None;
        for argv in commands {
            let Some(program) = find_in_path(argv[0]) else { continue };
            let mut cmd = Command::new(program);
            cmd.args(&argv[1..]);
            match capture(&mut cmd, Duration::from_secs(10)).await {
                Ok(out) if out.status.success() => return Ok(()),
                Ok(out) => failure = Some(format!("{} failed: {}", argv[0], out.stderr.trim())),
                Err(error) => failure = Some(format!("{} failed: {error}", argv[0])),
            }
        }
        Err(ActionError::Failed(failure.unwrap_or_else(|| {
            let tools: Vec<_> = commands.iter().map(|argv| argv[0]).collect();
            format!("none of these tools is installed: {}", tools.join(", "))
        })))
    }
}
