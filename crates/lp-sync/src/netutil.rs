//! Address helpers. Sync only ever talks to private / link-local addresses
//! unless a manual peer explicitly allows otherwise.

use std::net::{IpAddr, SocketAddr};

/// RFC1918, CGNAT (phone hotspots), link-local, IPv6 ULA and loopback.
pub fn is_local_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                || v4.is_loopback()
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        IpAddr::V6(v6) => {
            let seg = v6.segments()[0];
            v6.is_loopback() || (seg & 0xfe00) == 0xfc00 || (seg & 0xffc0) == 0xfe80
        }
    }
}

/// Non-loopback local addresses of this machine (for `Hello.listen` and QR codes).
pub fn local_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = if_addrs::get_if_addrs()
        .map(|ifs| {
            ifs.into_iter()
                .filter(|i| !i.is_loopback())
                .map(|i| i.ip())
                .filter(|ip| is_local_ip(ip) && !is_link_local_v6(ip))
                .collect()
        })
        .unwrap_or_default();
    // IPv4 first: link-local IPv6 needs scope ids and confuses users.
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips.dedup();
    ips
}

pub fn local_addrs(port: u16) -> Vec<SocketAddr> {
    local_ips()
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect()
}

fn is_link_local_v6(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

/// IPv4 networks this machine is directly attached to, as (address, netmask).
fn local_v4_nets() -> Vec<(u32, u32)> {
    if_addrs::get_if_addrs()
        .map(|ifs| {
            ifs.into_iter()
                .filter_map(|i| match i.addr {
                    if_addrs::IfAddr::V4(v4) if !v4.ip.is_loopback() => {
                        Some((u32::from(v4.ip), u32::from(v4.netmask)))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Orders addresses so the likeliest to work are tried first: IPv4 on one of
/// our own subnets, then other IPv4 (VPN, virtual adapters), then IPv6.
pub fn rank_dial_addrs(addrs: &mut Vec<SocketAddr>) {
    let nets = local_v4_nets();
    let on_our_subnet = |a: &SocketAddr| match a.ip() {
        IpAddr::V4(v4) => {
            let ip = u32::from(v4);
            nets.iter()
                .any(|(net, mask)| *mask != 0 && ip & mask == net & mask)
        }
        IpAddr::V6(_) => false,
    };
    addrs.sort_by_key(|a| (!on_our_subnet(a), a.is_ipv6()));
    addrs.dedup();
}

/// Parses `ip:port` strings, keeping only addresses we are allowed to dial.
pub fn parse_dialable(addrs: &[String], allow_public: bool) -> Vec<SocketAddr> {
    addrs
        .iter()
        .filter_map(|a| a.parse::<SocketAddr>().ok())
        .filter(|a| allow_public || is_local_ip(&a.ip()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_addresses() {
        for ok in [
            "192.168.1.5",
            "10.0.0.1",
            "172.16.4.4",
            "100.64.1.1",
            "169.254.3.3",
            "fd00::1",
            "fe80::1",
            "127.0.0.1",
        ] {
            assert!(is_local_ip(&ok.parse().unwrap()), "{ok}");
        }
        for bad in ["8.8.8.8", "100.128.0.1", "2001:4860::8888", "172.32.0.1"] {
            assert!(!is_local_ip(&bad.parse().unwrap()), "{bad}");
        }
    }

    #[test]
    fn parse_dialable_filters_public() {
        let addrs = vec![
            "192.168.1.2:1".to_string(),
            "8.8.8.8:1".into(),
            "junk".into(),
        ];
        assert_eq!(parse_dialable(&addrs, false).len(), 1);
        assert_eq!(parse_dialable(&addrs, true).len(), 2);
    }
}
