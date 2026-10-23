//! Noise-encrypted message channel over any byte stream.
//!
//! `Noise_XXpsk0`: both sides prove a static key, and the PSK is mixed in
//! before anything is sent, so someone without it learns nothing (not even
//! the responder's static key) and the handshake fails at message 1.

use std::time::Duration;

use lp_core::cbor;
use lp_core::crypto::Key32;
use lp_core::ids::NoisePublicKey;
use serde::Serialize;
use serde::de::DeserializeOwned;
use snow::{Builder, TransportState};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::framing::{MAX_FRAME, read_frame, write_frame};
use crate::{Result, SyncError};

pub const NOISE_PATTERN: &str = "Noise_XXpsk0_25519_ChaChaPoly_BLAKE2s";
/// Largest plaintext that fits in one Noise transport message.
pub const MAX_PLAINTEXT: usize = MAX_FRAME - 16;
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SecureChannel<S> {
    stream: S,
    transport: TransportState,
    remote_static: NoisePublicKey,
    handshake_hash: Vec<u8>,
    frame: Vec<u8>,
    plain: Vec<u8>,
    idle: Duration,
}

impl<S: AsyncRead + AsyncWrite + Unpin + Send> SecureChannel<S> {
    pub async fn initiate(stream: S, local: &Key32, psk: &Key32, prologue: &[u8]) -> Result<Self> {
        timeout(
            HANDSHAKE_TIMEOUT,
            Self::handshake(stream, local, psk, prologue, true),
        )
        .await?
    }

    pub async fn respond(stream: S, local: &Key32, psk: &Key32, prologue: &[u8]) -> Result<Self> {
        timeout(
            HANDSHAKE_TIMEOUT,
            Self::handshake(stream, local, psk, prologue, false),
        )
        .await?
    }

    async fn handshake(
        mut stream: S,
        local: &Key32,
        psk: &Key32,
        prologue: &[u8],
        initiator: bool,
    ) -> Result<Self> {
        let params = NOISE_PATTERN
            .parse()
            .map_err(|e: snow::Error| SyncError::Handshake(e.to_string()))?;
        let builder = Builder::new(params)
            .local_private_key(local.as_bytes())?
            .psk(0, psk.as_bytes())?
            .prologue(prologue)?;
        let mut hs = if initiator {
            builder.build_initiator()?
        } else {
            builder.build_responder()?
        };

        let mut frame = Vec::new();
        let mut out = vec![0u8; MAX_FRAME];
        let mut plain = vec![0u8; MAX_FRAME];
        while !hs.is_handshake_finished() {
            if hs.is_my_turn() {
                let n = hs.write_message(&[], &mut out)?;
                write_frame(&mut stream, &out[..n]).await?;
            } else {
                read_frame(&mut stream, &mut frame).await?;
                hs.read_message(&frame, &mut plain)?;
            }
        }
        let remote = hs
            .get_remote_static()
            .ok_or_else(|| SyncError::Handshake("peer sent no static key".into()))?;
        let remote_static = NoisePublicKey::from_slice(remote)?;
        let handshake_hash = hs.get_handshake_hash().to_vec();
        let transport = hs.into_transport_mode()?;
        Ok(Self {
            stream,
            transport,
            remote_static,
            handshake_hash,
            frame,
            plain,
            idle: IDLE_TIMEOUT,
        })
    }

    pub fn remote_static(&self) -> NoisePublicKey {
        self.remote_static
    }

    /// Unique per session; signing it proves key possession in this session.
    pub fn handshake_hash(&self) -> &[u8] {
        &self.handshake_hash
    }

    pub async fn send<T: Serialize>(&mut self, msg: &T) -> Result<()> {
        let bytes = cbor::to_vec(msg)?;
        if bytes.len() > MAX_PLAINTEXT {
            return Err(SyncError::Protocol(format!(
                "message of {} bytes is too large",
                bytes.len()
            )));
        }
        let mut out = vec![0u8; bytes.len() + 16];
        let n = self.transport.write_message(&bytes, &mut out)?;
        write_frame(&mut self.stream, &out[..n]).await?;
        Ok(())
    }

    pub async fn recv<T: DeserializeOwned>(&mut self) -> Result<T> {
        self.recv_within(self.idle).await
    }

    pub async fn recv_within<T: DeserializeOwned>(&mut self, wait: Duration) -> Result<T> {
        timeout(wait, read_frame(&mut self.stream, &mut self.frame)).await??;
        let n = self
            .transport
            .read_message(&self.frame, &mut self.plain)
            .map_err(|e| SyncError::Protocol(format!("bad encrypted frame: {e}")))?;
        Ok(cbor::from_slice(&self.plain[..n])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::crypto::NoiseKeypair;

    #[tokio::test]
    async fn handshake_and_messages_round_trip() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (ka, kb) = (
            NoiseKeypair::generate().unwrap(),
            NoiseKeypair::generate().unwrap(),
        );
        let psk = Key32::random().unwrap();
        let (psk_a, psk_b) = (psk.clone(), psk.clone());
        let ka_s = ka.secret().clone();
        let t = tokio::spawn(async move {
            let mut ch = SecureChannel::respond(b, kb.secret(), &psk_b, b"p")
                .await
                .unwrap();
            let got: String = ch.recv().await.unwrap();
            ch.send(&format!("{got}!")).await.unwrap();
            ch.remote_static()
        });
        let mut ch = SecureChannel::initiate(a, &ka_s, &psk_a, b"p")
            .await
            .unwrap();
        ch.send(&"hi".to_string()).await.unwrap();
        let back: String = ch.recv().await.unwrap();
        assert_eq!(back, "hi!");
        assert_eq!(t.await.unwrap(), ka.public());
    }

    #[tokio::test]
    async fn wrong_psk_fails() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (ka, kb) = (
            NoiseKeypair::generate().unwrap(),
            NoiseKeypair::generate().unwrap(),
        );
        let t = tokio::spawn(async move {
            SecureChannel::respond(b, kb.secret(), &Key32::random().unwrap(), b"p")
                .await
                .is_err()
        });
        let res = SecureChannel::initiate(a, ka.secret(), &Key32::random().unwrap(), b"p").await;
        assert!(
            t.await.unwrap(),
            "responder must reject a non-member at message 1"
        );
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn prologue_mismatch_fails() {
        let (a, b) = tokio::io::duplex(1 << 16);
        let (ka, kb) = (
            NoiseKeypair::generate().unwrap(),
            NoiseKeypair::generate().unwrap(),
        );
        let psk = Key32::random().unwrap();
        let psk_b = psk.clone();
        let t = tokio::spawn(async move {
            SecureChannel::respond(b, kb.secret(), &psk_b, b"vault-A")
                .await
                .is_err()
        });
        let res = SecureChannel::initiate(a, ka.secret(), &psk, b"vault-B").await;
        assert!(t.await.unwrap());
        assert!(res.is_err());
    }
}
