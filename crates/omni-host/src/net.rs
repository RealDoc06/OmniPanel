//! LAN addresses the deck server is reachable at.

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
