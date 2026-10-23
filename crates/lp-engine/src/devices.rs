//! Device list, invites and join input parsing.

use std::net::SocketAddr;
use std::sync::Weak;
use std::time::Duration;

use lp_core::roster::DeviceStatus;
use lp_sync::pairing::{self, BoxFuture, InviteHandler, JoinAccept, JoinRequest};
use lp_sync::service::DEFAULT_PORT;
use serde::Serialize;

use crate::{Engine, EngineError, Inner, Result};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub id: String,
    pub fingerprint: String,
    pub name: String,
    pub platform: String,
    /// "active", "revoked" or "pending".
    pub status: String,
    pub this_device: bool,
    pub genesis: bool,
    pub added_by: Option<String>,
    pub added_ms: u64,
    pub last_seen_ms: Option<u64>,
    pub last_sync_ms: Option<u64>,
    pub last_error: Option<String>,
    pub online: bool,
    pub addrs: Vec<String>,
}

pub(crate) fn status_name(s: DeviceStatus) -> &'static str {
    match s {
        DeviceStatus::Active => "active",
        DeviceStatus::Revoked { .. } => "revoked",
        DeviceStatus::Pending => "pending",
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfo {
    /// 8 digits.
    pub code: String,
    /// "1234-5678".
    pub code_display: String,
    pub addrs: Vec<String>,
    /// Payload of the QR code: `localpass://pair?c=…&a=…`.
    pub qr_text: String,
    pub qr_svg: String,
    pub expires_in_secs: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingRequest {
    pub request_id: u64,
    pub device_name: String,
    pub platform: String,
    pub fingerprint: String,
}

/// Splits user input into a code and any addresses it carries. Accepts
/// "1234-5678" or a QR payload `localpass://pair?c=12345678&a=ip:port,ip:port`.
pub fn parse_join_input(input: &str, extra_addrs: &[String]) -> Result<(String, Vec<SocketAddr>)> {
    let input = input.trim();
    let (code, mut addrs) = match input.strip_prefix("localpass://pair?") {
        Some(query) => {
            let (mut code, mut addrs) = (None, Vec::new());
            for pair in query.split('&') {
                match pair.split_once('=') {
                    Some(("c", v)) => code = pairing::normalize_code(v),
                    Some(("a", v)) => addrs.extend(v.split(',').filter_map(parse_addr)),
                    _ => {}
                }
            }
            (code, addrs)
        }
        None => (pairing::normalize_code(input), Vec::new()),
    };
    let code = code.ok_or_else(|| {
        EngineError::Invalid("enter the 8-digit code shown on the other device".into())
    })?;
    addrs.extend(extra_addrs.iter().filter_map(|a| parse_addr(a)));
    addrs.dedup();
    Ok((code, addrs))
}

/// "192.168.1.5:47821" or "192.168.1.5" (default port).
pub fn parse_addr(s: &str) -> Option<SocketAddr> {
    let s = s.trim();
    s.parse::<SocketAddr>().ok().or_else(|| {
        s.parse::<std::net::IpAddr>()
            .ok()
            .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
    })
}

pub(crate) fn qr_svg(text: &str) -> Result<String> {
    let code =
        qrcode::QrCode::new(text.as_bytes()).map_err(|e| EngineError::Invalid(e.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build())
}

/// Bridges pairing requests from the sync service to the UI.
pub(crate) struct EngineInviteHandler {
    pub engine: Weak<Inner>,
}

impl InviteHandler for EngineInviteHandler {
    fn approve(
        &self,
        request: JoinRequest,
    ) -> BoxFuture<std::result::Result<Option<JoinAccept>, String>> {
        let inner = self.engine.upgrade();
        Box::pin(async move {
            let engine = Engine {
                inner: inner.ok_or("the app is shutting down")?,
            };
            let approved = if engine.inner.config.auto_approve_joins {
                true
            } else {
                let rx = engine.register_pairing_request(&request);
                matches!(
                    tokio::time::timeout(pairing::APPROVAL_TIMEOUT - Duration::from_secs(5), rx)
                        .await,
                    Ok(Ok(true))
                )
            };
            if !approved {
                return Ok(None);
            }
            engine
                .author_device_added(&request)
                .map(Some)
                .map_err(|e| e.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_input_forms() {
        let (code, addrs) = parse_join_input("1234-5678", &["10.0.0.5".into()]).unwrap();
        assert_eq!(code, "12345678");
        assert_eq!(addrs, vec!["10.0.0.5:47821".parse().unwrap()]);
        let (code, addrs) = parse_join_input(
            "localpass://pair?c=87654321&a=192.168.1.4:5000,192.168.1.9:5000",
            &[],
        )
        .unwrap();
        assert_eq!(code, "87654321");
        assert_eq!(addrs.len(), 2);
        assert!(parse_join_input("12", &[]).is_err());
    }
}
