//! Android over USB: keeps `adb reverse tcp:<port> tcp:<port>` active on every
//! connected device so the deck can open `http://localhost:<port>`.
//!
//! Uses `adb track-devices` (event stream, no polling) while enabled. One
//! supervisor task applies settings changes in order: the previous bridge
//! stops and removes its rules before the next one starts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use omni_core::{UsbDevice, UsbSettings, UsbStatus};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Longest a one-shot adb command (`reverse`) may take.
const ADB_TIMEOUT: Duration = Duration::from_secs(10);

/// Bridge settings and deck server port; `None` until configured.
type Desired = Option<(UsbSettings, u16)>;

pub struct UsbBridge {
    desired: watch::Sender<Desired>,
    status: watch::Sender<UsbStatus>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl UsbBridge {
    /// Starts off; `configure` may be called from any thread, the bridge runs on `rt`.
    pub fn new(rt: &Handle) -> Self {
        let (desired, status) = (watch::Sender::new(None), watch::Sender::new(UsbStatus::default()));
        let task = rt.spawn(supervise(desired.subscribe(), status.clone()));
        Self { desired, status, task: Mutex::new(Some(task)) }
    }

    pub fn status(&self) -> UsbStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<UsbStatus> {
        self.status.subscribe()
    }

    /// Start, stop or restart the bridge. No-op when nothing changed.
    pub fn configure(&self, settings: &UsbSettings, port: u16) {
        let next = Some((settings.clone(), port));
        self.desired.send_if_modified(|desired| {
            let changed = *desired != next;
            *desired = next;
            changed
        });
    }

    /// Stop the bridge for good. Its reverse rules stay, like after a crash;
    /// the next start re-applies them.
    pub async fn stop(&self) {
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for UsbBridge {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap().take() {
            task.abort();
        }
    }
}

/// Runs the bridge for the latest settings; on every change stops it and
/// removes its reverse rules (with its own adb and port) before going on.
async fn supervise(mut desired: watch::Receiver<Desired>, status: watch::Sender<UsbStatus>) {
    loop {
        let current = desired.borrow_and_update().clone();
        match current {
            Some((settings, port)) if settings.enabled => match find_adb(settings.adb_path.as_deref()) {
                Some(adb) => {
                    let mut forwarded = HashSet::new();
                    tokio::select! {
                        () = run(&adb, port, &status, &mut forwarded) => {}
                        changed = desired.changed() => if changed.is_err() { return },
                    }
                    remove_reverse(&adb, &forwarded, port).await;
                    continue;
                }
                None => {
                    status.send_replace(UsbStatus {
                        enabled: true,
                        error: Some("adb not found. Install Android platform-tools or set its path.".into()),
                        ..UsbStatus::default()
                    });
                }
            },
            _ => {
                status.send_replace(UsbStatus::default());
            }
        }
        if desired.changed().await.is_err() {
            return;
        }
    }
}

/// Keeps devices forwarded until cancelled; `forwarded` holds the serials
/// with an active rule.
async fn run(adb: &Path, port: u16, status: &watch::Sender<UsbStatus>, forwarded: &mut HashSet<String>) {
    let adb_path = Some(adb.display().to_string());
    loop {
        status.send_replace(UsbStatus { enabled: true, adb_path: adb_path.clone(), ..UsbStatus::default() });
        let error = track(adb, port, status, forwarded).await;
        forwarded.clear();
        status.send_replace(UsbStatus {
            enabled: true,
            adb_path: adb_path.clone(),
            devices: vec![],
            error: Some(error),
        });
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// Follow `adb track-devices -l` until it exits; returns why it stopped.
async fn track(adb: &Path, port: u16, status: &watch::Sender<UsbStatus>, forwarded: &mut HashSet<String>) -> String {
    let mut child = match adb_command(adb).args(["track-devices", "-l"]).stdout(Stdio::piped()).spawn() {
        Ok(child) => child,
        Err(e) => return format!("can't start adb: {e}"),
    };
    let Some(mut stdout) = child.stdout.take() else { return "adb produced no output".into() };
    loop {
        // Each update is a 4-hex-digit length followed by the device list.
        let mut len = [0u8; 4];
        if stdout.read_exact(&mut len).await.is_err() {
            return "adb server stopped".into();
        }
        let Some(len) = std::str::from_utf8(&len).ok().and_then(|s| usize::from_str_radix(s, 16).ok()) else {
            return "unexpected adb output".into();
        };
        let mut body = vec![0u8; len];
        if stdout.read_exact(&mut body).await.is_err() {
            return "adb server stopped".into();
        }
        let mut devices = parse_devices(&String::from_utf8_lossy(&body));
        forwarded.retain(|serial| devices.iter().any(|d| &d.serial == serial));
        for device in devices.iter_mut().filter(|d| d.state == "device") {
            if !forwarded.contains(&device.serial) && reverse(adb, &device.serial, port).await {
                forwarded.insert(device.serial.clone());
            }
            device.forwarded = forwarded.contains(&device.serial);
        }
        status.send_modify(|s| {
            s.devices = devices;
            s.error = None;
        });
    }
}

async fn reverse(adb: &Path, serial: &str, port: u16) -> bool {
    let spec = format!("tcp:{port}");
    adb_ok(adb, &["-s", serial, "reverse", &spec, &spec]).await
}

async fn remove_reverse(adb: &Path, serials: &HashSet<String>, port: u16) {
    let spec = format!("tcp:{port}");
    for serial in serials {
        adb_ok(adb, &["-s", serial, "reverse", "--remove", &spec]).await;
    }
}

/// Run a one-shot adb command; a hung one is killed after `ADB_TIMEOUT`.
async fn adb_ok(adb: &Path, args: &[&str]) -> bool {
    let output = tokio::time::timeout(ADB_TIMEOUT, adb_command(adb).args(args).output()).await;
    matches!(output, Ok(Ok(out)) if out.status.success())
}

fn adb_command(adb: &Path) -> Command {
    let mut cmd = Command::new(adb);
    cmd.stdin(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// Lines like `R58M123 device product:x model:Galaxy_Tab_A device:y transport_id:3`.
fn parse_devices(list: &str) -> Vec<UsbDevice> {
    list.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next()?.to_string();
            let model = parts
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "))
                .unwrap_or_else(|| serial.clone());
            Some(UsbDevice { serial, model, state, forwarded: false })
        })
        .collect()
}

/// Configured path, then PATH, then the usual SDK install locations.
pub fn find_adb(configured: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = configured.filter(|p| !p.trim().is_empty()) {
        let path = PathBuf::from(path.trim());
        return path.is_file().then_some(path);
    }
    let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
    let from_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(exe));
    let home = std::env::var_os(if cfg!(windows) { "LOCALAPPDATA" } else { "HOME" }).map(PathBuf::from);
    let sdk = home.into_iter().flat_map(|h| {
        [
            h.join("Library/Android/sdk/platform-tools").join(exe),
            h.join("Android/Sdk/platform-tools").join(exe),
            h.join("Android/sdk/platform-tools").join(exe),
        ]
    });
    let fixed = ["/opt/homebrew/bin/adb", "/usr/local/bin/adb", "/usr/bin/adb"].map(PathBuf::from);
    from_path.chain(sdk).chain(fixed).find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_long_device_list() {
        let list = "R58M123\tdevice product:gta usb:1-1 model:Galaxy_Tab_A device:gta transport_id:3\n\
                    emulator-5554\tunauthorized transport_id:4\n";
        let devices = parse_devices(list);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].model, "Galaxy Tab A");
        assert_eq!(devices[0].state, "device");
        assert_eq!(devices[1].model, "emulator-5554");
        assert_eq!(devices[1].state, "unauthorized");
    }

    /// Fake adb that logs `reverse` calls and reports one device. Its rule
    /// removal is slow, so cleanup that runs concurrently with the next
    /// bridge would land after that bridge's `reverse`.
    #[cfg(unix)]
    fn fake_adb(dir: &Path, name: &str, log: &Path) -> String {
        use std::os::unix::fs::PermissionsExt;
        let list = "R58M123\tdevice model:Pixel_7\n";
        let script = format!(
            "#!/bin/sh\ncase \"$1\" in\n\
             track-devices) printf '{:04x}{list}'; exec sleep 30 ;;\n\
             -s) [ \"$4\" = --remove ] && sleep 0.3; echo \"{name} $4\" >> '{}' ;;\n\
             esac\n",
            list.len(),
            log.display()
        );
        let path = dir.join(name);
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[cfg(unix)]
    async fn forwarded_by(status: &mut watch::Receiver<UsbStatus>, adb: &str) {
        let done = |s: &UsbStatus| s.adb_path.as_deref() == Some(adb) && s.devices.iter().any(|d| d.forwarded);
        tokio::time::timeout(Duration::from_secs(5), status.wait_for(done)).await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reconfiguring_removes_old_rules_before_forwarding_again() {
        let dir = std::env::temp_dir().join(format!("omni-usb-{}", omni_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("calls.log");
        let (a, b) = (fake_adb(&dir, "a", &log), fake_adb(&dir, "b", &log));
        let bridge = UsbBridge::new(&Handle::current());
        let mut status = bridge.subscribe();

        bridge.configure(&UsbSettings { enabled: true, adb_path: Some(a.clone()) }, 7710);
        forwarded_by(&mut status, &a).await;
        // Same port, other adb: a's rule must be gone before b adds its own.
        bridge.configure(&UsbSettings { enabled: true, adb_path: Some(b.clone()) }, 7710);
        forwarded_by(&mut status, &b).await;

        let calls = std::fs::read_to_string(&log).unwrap();
        assert_eq!(calls.lines().collect::<Vec<_>>(), ["a tcp:7710", "a --remove", "b tcp:7710"]);
        drop(bridge);
        std::fs::remove_dir_all(dir).ok();
    }
}
