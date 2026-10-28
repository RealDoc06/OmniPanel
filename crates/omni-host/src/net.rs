//! LAN addresses and names the deck server is reachable at.

use std::net::{IpAddr, Ipv4Addr};

/// `http://<ip>:<port>` for every private IPv4 address, most common LAN ranges first.
pub fn deck_urls(port: u16) -> Vec<String> {
    let mut ips: Vec<Ipv4Addr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) if ip.is_private() && !iface.is_loopback() => Some(ip),
            _ => None,
        })
        .collect();
    ips.sort_by_key(|ip| (rank(ip), *ip));
    ips.dedup();
    ips.into_iter().map(|ip| format!("http://{ip}:{port}")).collect()
}

/// Home routers first, then corporate ranges, then container/VM bridges.
fn rank(ip: &Ipv4Addr) -> u8 {
    match ip.octets() {
        [192, 168, ..] => 0,
        [10, ..] => 1,
        [172, 17..=31, ..] => 3,
        _ => 2,
    }
}

/// Names this machine answers to besides IP addresses: `localhost`, the
/// hostname with and without its domain, and the mDNS `.local` name.
/// Lowercase, for matching Host headers.
pub fn local_names() -> Vec<String> {
    let host = gethostname::gethostname().to_string_lossy().trim_end_matches('.').to_ascii_lowercase();
    let short = host.split('.').next().unwrap_or_default().to_string();
    let mut names = vec!["localhost".to_string(), format!("{short}.local"), short, host];
    // Bonjour advertises the "Local Hostname", which can differ from the hostname.
    #[cfg(target_os = "macos")]
    if let Some(local) = scutil("LocalHostName") {
        let local = local.to_ascii_lowercase();
        names.push(format!("{local}.local"));
        names.push(local);
    }
    names.retain(|n| !n.is_empty() && !n.starts_with('.'));
    names.sort();
    names.dedup();
    names
}

/// A macOS system name: `ComputerName` ("Doc's Mac mini"), `LocalHostName` ...
#[cfg(target_os = "macos")]
pub fn scutil(key: &str) -> Option<String> {
    std::process::Command::new("scutil")
        .args(["--get", key])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
}
