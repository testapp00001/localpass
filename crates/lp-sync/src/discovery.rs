//! mDNS-SD advertising and browsing.
//!
//! Nothing identifying is broadcast: instance and host names are random, and
//! the TXT record only carries tags derived from the network key `NK` that
//! rotate every hour. Members can recognise them; outsiders can't link the
//! same device across home and work networks.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;

use data_encoding::HEXLOWER;
use lp_core::crypto::{Key32, hmac_sha256, random_bytes};
use lp_core::ids::DeviceId;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::sync::mpsc;

use crate::netutil::is_local_ip;
use crate::{Result, SyncError};

pub const SYNC_SERVICE: &str = "_localpass._tcp.local.";
pub const PAIR_SERVICE: &str = "_localpass-pair._tcp.local.";
const EPOCH_MS: u64 = 60 * 60 * 1000;

pub fn epoch_of(now_ms: u64) -> u64 {
    now_ms / EPOCH_MS
}

pub fn vault_tag(nk: &Key32, epoch: u64) -> String {
    HEXLOWER.encode(&hmac_sha256(nk.as_bytes(), &[b"lp/mdns/vault\0", &epoch.to_be_bytes()])[..8])
}

pub fn device_tag(nk: &Key32, device: &DeviceId, epoch: u64) -> String {
    HEXLOWER.encode(
        &hmac_sha256(
            nk.as_bytes(),
            &[b"lp/mdns/dev\0", device.as_bytes(), &epoch.to_be_bytes()],
        )[..8],
    )
}

/// Epochs accepted when matching tags (tolerates clock skew and rollover).
pub fn nearby_epochs(now_ms: u64) -> [u64; 3] {
    let e = epoch_of(now_ms);
    [e.saturating_sub(1), e, e + 1]
}

#[derive(Clone, Debug)]
pub struct Found {
    pub fullname: String,
    pub addrs: Vec<SocketAddr>,
    pub vault_tag: Option<String>,
    pub device_tag: Option<String>,
}

#[derive(Clone, Debug)]
pub enum DiscoveryEvent {
    Found(Found),
    Removed(String),
}

pub struct Discovery {
    daemon: ServiceDaemon,
    host: String,
    sync_name: Mutex<Option<String>>,
    pair_name: Mutex<Option<String>>,
}

fn mdns_err(e: mdns_sd::Error) -> SyncError {
    SyncError::Discovery(e.to_string())
}

fn random_label() -> Result<String> {
    Ok(format!("lp-{}", HEXLOWER.encode(&random_bytes::<5>()?)))
}

impl Discovery {
    pub fn start() -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        Ok(Self {
            daemon,
            host: format!("{}.local.", random_label()?),
            sync_name: Mutex::new(None),
            pair_name: Mutex::new(None),
        })
    }

    /// (Re-)advertises the sync service with the current tags.
    pub fn advertise_sync(&self, port: u16, vault_tag: &str, device_tag: &str) -> Result<()> {
        let props = [("v", "1"), ("vt", vault_tag), ("dt", device_tag)];
        let name = self.register(SYNC_SERVICE, port, &props[..])?;
        if let Some(old) = self.sync_name.lock().expect("lock").replace(name) {
            let _ = self.daemon.unregister(&old);
        }
        Ok(())
    }

    /// Advertises an open invite so a joiner can find the inviter without typing an IP.
    pub fn advertise_pairing(&self, port: u16) -> Result<()> {
        let name = self.register(PAIR_SERVICE, port, &[("v", "1")][..])?;
        if let Some(old) = self.pair_name.lock().expect("lock").replace(name) {
            let _ = self.daemon.unregister(&old);
        }
        Ok(())
    }

    pub fn stop_pairing(&self) {
        if let Some(old) = self.pair_name.lock().expect("lock").take() {
            let _ = self.daemon.unregister(&old);
        }
    }

    fn register(&self, service: &str, port: u16, props: &[(&str, &str)]) -> Result<String> {
        let info = ServiceInfo::new(service, &random_label()?, &self.host, "", port, props)
            .map_err(mdns_err)?
            .enable_addr_auto();
        let fullname = info.get_fullname().to_string();
        self.daemon.register(info).map_err(mdns_err)?;
        Ok(fullname)
    }

    /// Streams resolved services of `service` type. Only local addresses are kept.
    pub fn browse(&self, service: &str) -> Result<mpsc::UnboundedReceiver<DiscoveryEvent>> {
        let rx = self.daemon.browse(service).map_err(mdns_err)?;
        let (tx, out) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("lp-mdns-browse".into())
            .spawn(move || {
                while let Ok(ev) = rx.recv() {
                    let mapped = match ev {
                        ServiceEvent::ServiceResolved(info) => {
                            let port = info.get_port();
                            let mut addrs: Vec<SocketAddr> = info
                                .get_addresses()
                                .iter()
                                .map(|ip| ip.to_ip_addr())
                                .filter(|ip| {
                                    is_local_ip(ip) && !ip.is_loopback() && !is_v6_link_local(ip)
                                })
                                .map(|ip| SocketAddr::new(ip, port))
                                .collect();
                            addrs.sort_by_key(|a| (a.is_ipv6(), *a));
                            DiscoveryEvent::Found(Found {
                                fullname: info.get_fullname().to_string(),
                                addrs,
                                vault_tag: info
                                    .txt_properties
                                    .get_property_val_str("vt")
                                    .map(str::to_string),
                                device_tag: info
                                    .txt_properties
                                    .get_property_val_str("dt")
                                    .map(str::to_string),
                            })
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            DiscoveryEvent::Removed(fullname)
                        }
                        _ => continue,
                    };
                    if tx.send(mapped).is_err() {
                        break;
                    }
                }
            })
            .map_err(SyncError::Io)?;
        Ok(out)
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn is_v6_link_local(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_rotate_and_differ_per_device() {
        let nk = Key32::random().unwrap();
        let (a, b) = (DeviceId([1; 32]), DeviceId([2; 32]));
        assert_eq!(vault_tag(&nk, 5), vault_tag(&nk, 5));
        assert_ne!(vault_tag(&nk, 5), vault_tag(&nk, 6));
        assert_ne!(device_tag(&nk, &a, 5), device_tag(&nk, &b, 5));
        assert_ne!(vault_tag(&nk, 5), vault_tag(&Key32::random().unwrap(), 5));
        assert_eq!(vault_tag(&nk, 5).len(), 16);
        assert_eq!(nearby_epochs(2 * EPOCH_MS + 1), [1, 2, 3]);
    }
}
