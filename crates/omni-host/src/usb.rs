//! Android over USB: keeps `adb reverse tcp:<port> tcp:<port>` active on every
//! connected device so the deck can open `http://localhost:<port>`.
//!
//! Uses `adb track-devices` (event stream, no polling) while enabled.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use omni_core::{UsbDevice, UsbSettings, UsbStatus};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

pub struct UsbBridge {
    rt: Handle,
    status: watch::Sender<UsbStatus>,
    task: Mutex<Option<(UsbSettings, u16, JoinHandle<()>)>>,
}

impl UsbBridge {
    /// `configure` may be called from any thread; tasks run on `rt`.
    pub fn new(rt: Handle) -> Self {
        Self { rt, status: watch::Sender::new(UsbStatus::default()), task: Mutex::new(None) }
    }

    pub fn status(&self) -> UsbStatus {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<UsbStatus> {
        self.status.subscribe()
    }

    /// Start, stop or restart the bridge. No-op when nothing changed.
    pub fn configure(&self, settings: &UsbSettings, port: u16) {
        let mut task = self.task.lock().unwrap();
        if task.as_ref().is_some_and(|(s, p, _)| s == settings && *p == port) {
            return;
        }
        if let Some((_, old_port, handle)) = task.take() {
            handle.abort();
            let forwarded: Vec<String> =
                self.status.borrow().devices.iter().filter(|d| d.forwarded).map(|d| d.serial.clone()).collect();
            if let Some(adb) = self.status.borrow().adb_path.clone() {
                self.rt.spawn(remove_reverse(PathBuf::from(adb), forwarded, old_port));
            }
        }
        if !settings.enabled {
            self.status.send_replace(UsbStatus::default());
            return;
        }
        let Some(adb) = find_adb(settings.adb_path.as_deref()) else {
            self.status.send_replace(UsbStatus {
                enabled: true,
                error: Some("adb not found. Install Android platform-tools or set its path.".into()),
                ..UsbStatus::default()
            });
            *task = Some((settings.clone(), port, self.rt.spawn(async {})));
            return;
        };
        let status = self.status.clone();
        let handle = self.rt.spawn(run(adb, port, status));
        *task = Some((settings.clone(), port, handle));
    }
}

impl Drop for UsbBridge {
    fn drop(&mut self) {
        if let Some((_, _, handle)) = self.task.lock().unwrap().take() {
            handle.abort();
        }
    }
}

async fn run(adb: PathBuf, port: u16, status: watch::Sender<UsbStatus>) {
    let adb_path = Some(adb.display().to_string());
    let mut forwarded = HashSet::new();
    loop {
        status.send_replace(UsbStatus { enabled: true, adb_path: adb_path.clone(), ..UsbStatus::default() });
        let error = track(&adb, port, &status, &mut forwarded).await;
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
async fn track(adb: &PathBuf, port: u16, status: &watch::Sender<UsbStatus>, forwarded: &mut HashSet<String>) -> String {
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

async fn reverse(adb: &PathBuf, serial: &str, port: u16) -> bool {
    let spec = format!("tcp:{port}");
    adb_command(adb).args(["-s", serial, "reverse", &spec, &spec]).output().await.is_ok_and(|out| out.status.success())
}

async fn remove_reverse(adb: PathBuf, serials: Vec<String>, port: u16) {
    let spec = format!("tcp:{port}");
    for serial in serials {
        let _ = adb_command(&adb).args(["-s", &serial, "reverse", "--remove", &spec]).output().await;
    }
}

fn adb_command(adb: &PathBuf) -> Command {
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
}
