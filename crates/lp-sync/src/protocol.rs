//! Wire messages (CBOR inside the Noise channel).

use lp_core::ids::{DeviceId, VaultId};
use lp_core::op::SignedOp;
use serde::{Deserialize, Serialize};

pub const PROTO_VERSION: u32 = 1;
/// First 4 bytes on every TCP connection select the mode.
pub const PREAMBLE_SYNC: &[u8; 4] = b"LPS1";
pub const PREAMBLE_PAIR: &[u8; 4] = b"LPP1";

/// Ops per `Ops` message are capped so each message fits one Noise frame.
pub const BATCH_BYTES: usize = 56 * 1024;
pub const BATCH_OPS: usize = 256;
/// Upper bound on ops accepted from one peer in one session.
pub const MAX_OPS_PER_SESSION: usize = 500_000;
pub const MAX_PROOF_OPS: usize = 16;
pub const MAX_HINTS: usize = 16;

pub fn sync_prologue(vault_id: &VaultId) -> Vec<u8> {
    let mut p = b"localpass/sync/1\0".to_vec();
    p.extend_from_slice(vault_id.as_bytes());
    p
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum SyncMsg {
    #[serde(rename = "hello")]
    Hello(Hello),
    #[serde(rename = "ops")]
    Ops { ops: Vec<SignedOp> },
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "err")]
    Error { code: String, message: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Hello {
    pub proto: u32,
    pub now_ms: u64,
    /// Version vector: highest seq held per author.
    pub vv: Vec<(DeviceId, u64)>,
    /// Roster ops proving the sender's membership (genesis → … → sender).
    pub proof: Vec<SignedOp>,
    /// Addresses the sender listens on.
    pub listen: Vec<String>,
    /// Addresses of other members the sender has seen recently.
    #[serde(default)]
    pub hints: Vec<AddrHint>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AddrHint {
    pub device: DeviceId,
    pub addrs: Vec<String>,
    pub seen_ms: u64,
}

/// Splits ops into batches that each fit one message.
pub fn batches(ops: Vec<SignedOp>) -> Vec<Vec<SignedOp>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut size = 0;
    for op in ops {
        let len = op.encoded_len();
        if !cur.is_empty() && (size + len > BATCH_BYTES || cur.len() >= BATCH_OPS) {
            out.push(std::mem::take(&mut cur));
            size = 0;
        }
        size += len;
        cur.push(op);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
