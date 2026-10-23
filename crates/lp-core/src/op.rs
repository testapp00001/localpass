//! The signed op envelope.
//!
//! Header (94 bytes, fixed layout):
//! `version(1) | vault_id(16) | author(32) | seq(8, BE) | class(1) | body_len(4, BE) | sha256(body)(32)`
//!
//! The signature covers `"lp/op/v1\0" ‖ header`. Because the header commits to
//! the body by hash, a body can later be dropped (redaction) while the op stays
//! verifiable. Ops are stored and relayed as exact bytes and never re-serialized.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::cbor;
use crate::crypto::{self, Key32, SigningKey, sha256};
use crate::ids::{DeviceId, OpRef, Sig64, VaultId};
use crate::model::{DataOp, MAX_DATA_OP_LEN, RosterOp};
use crate::{CoreError, Result};

pub const HEADER_LEN: usize = 94;
pub const OP_VERSION: u8 = 1;
/// Hard cap on any op body (encrypted data op plus overhead, or roster op).
pub const MAX_BODY_LEN: usize = MAX_DATA_OP_LEN + 1024;

const SIG_DOMAIN: &[u8] = b"lp/op/v1\0";
const DATA_AAD_DOMAIN: &[u8] = b"lp/data/v1\0";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OpClass {
    Roster,
    Data,
}

impl OpClass {
    pub fn to_u8(self) -> u8 {
        match self {
            OpClass::Roster => 0,
            OpClass::Data => 1,
        }
    }

    pub fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(OpClass::Roster),
            1 => Ok(OpClass::Data),
            other => Err(CoreError::Malformed(format!("unknown op class {other}"))),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpHeader {
    pub vault_id: VaultId,
    pub author: DeviceId,
    pub seq: u64,
    pub class: OpClass,
    pub body_len: u32,
    pub body_hash: [u8; 32],
}

impl OpHeader {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0] = OP_VERSION;
        out[1..17].copy_from_slice(self.vault_id.as_bytes());
        out[17..49].copy_from_slice(self.author.as_bytes());
        out[49..57].copy_from_slice(&self.seq.to_be_bytes());
        out[57] = self.class.to_u8();
        out[58..62].copy_from_slice(&self.body_len.to_be_bytes());
        out[62..94].copy_from_slice(&self.body_hash);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != HEADER_LEN {
            return Err(CoreError::Malformed(format!(
                "op header must be {HEADER_LEN} bytes"
            )));
        }
        if bytes[0] != OP_VERSION {
            return Err(CoreError::Malformed(format!(
                "unsupported op version {}",
                bytes[0]
            )));
        }
        let seq = u64::from_be_bytes(bytes[49..57].try_into().expect("8 bytes"));
        if seq == 0 {
            return Err(CoreError::Malformed("op seq starts at 1".into()));
        }
        Ok(Self {
            vault_id: VaultId::from_slice(&bytes[1..17])?,
            author: DeviceId::from_slice(&bytes[17..49])?,
            seq,
            class: OpClass::from_u8(bytes[57])?,
            body_len: u32::from_be_bytes(bytes[58..62].try_into().expect("4 bytes")),
            body_hash: bytes[62..94].try_into().expect("32 bytes"),
        })
    }
}

/// An op as stored and relayed. Constructing one checks structure (header
/// layout, body length and hash) but not the signature; call
/// [`SignedOp::verify_signature`] before trusting it.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedOp {
    header_bytes: [u8; HEADER_LEN],
    header: OpHeader,
    sig: Sig64,
    body: Vec<u8>,
}

impl SignedOp {
    pub fn create(
        vault_id: VaultId,
        sk: &SigningKey,
        seq: u64,
        class: OpClass,
        body: Vec<u8>,
    ) -> Result<Self> {
        if seq == 0 {
            return Err(CoreError::Invalid("op seq starts at 1".into()));
        }
        if body.len() > MAX_BODY_LEN {
            return Err(CoreError::Invalid(format!(
                "op body is {} bytes; the limit is {MAX_BODY_LEN}",
                body.len()
            )));
        }
        let header = OpHeader {
            vault_id,
            author: sk.device_id(),
            seq,
            class,
            body_len: body.len() as u32,
            body_hash: sha256(&body),
        };
        let header_bytes = header.encode();
        let sig = sk.sign(SIG_DOMAIN, &header_bytes);
        Ok(Self {
            header_bytes,
            header,
            sig,
            body,
        })
    }

    pub fn from_parts(header_bytes: &[u8], sig: &[u8], body: Vec<u8>) -> Result<Self> {
        let header = OpHeader::decode(header_bytes)?;
        if body.len() > MAX_BODY_LEN || body.len() != header.body_len as usize {
            return Err(CoreError::Malformed("op body length mismatch".into()));
        }
        if !crypto::ct_eq(&sha256(&body), &header.body_hash) {
            return Err(CoreError::Malformed("op body hash mismatch".into()));
        }
        Ok(Self {
            header_bytes: header_bytes.try_into().expect("length checked by decode"),
            header,
            sig: Sig64::from_slice(sig)?,
            body,
        })
    }

    pub fn verify_signature(&self) -> Result<()> {
        crypto::verify(
            &self.header.author,
            SIG_DOMAIN,
            &self.header_bytes,
            &self.sig,
        )
    }

    pub fn header(&self) -> &OpHeader {
        &self.header
    }

    pub fn header_bytes(&self) -> &[u8; HEADER_LEN] {
        &self.header_bytes
    }

    pub fn sig(&self) -> &Sig64 {
        &self.sig
    }

    pub fn body(&self) -> &[u8] {
        &self.body
    }

    pub fn author(&self) -> DeviceId {
        self.header.author
    }

    pub fn seq(&self) -> u64 {
        self.header.seq
    }

    pub fn class(&self) -> OpClass {
        self.header.class
    }

    pub fn op_ref(&self) -> OpRef {
        OpRef {
            author: self.header.author,
            seq: self.header.seq,
        }
    }

    /// Two ops with the same author and seq but different content hash are a fork.
    pub fn content_hash(&self) -> [u8; 32] {
        sha256(&self.header_bytes)
    }

    /// Approximate wire size, used for batching.
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + 64 + self.body.len() + 16
    }

    pub fn decode_roster(&self) -> Result<RosterOp> {
        if self.class() != OpClass::Roster {
            return Err(CoreError::Invalid("not a roster op".into()));
        }
        cbor::from_slice(&self.body)
    }

    pub fn decode_data(&self, vk: &Key32) -> Result<DataOp> {
        if self.class() != OpClass::Data {
            return Err(CoreError::Invalid("not a data op".into()));
        }
        let aad = data_aad(&self.header.vault_id, &self.header.author, self.header.seq);
        let plain = crypto::open(vk, &aad, &self.body)?;
        cbor::from_slice(&plain)
    }
}

impl fmt::Debug for SignedOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SignedOp({:?} #{} {:?} {}B)",
            self.header.author,
            self.header.seq,
            self.header.class,
            self.body.len()
        )
    }
}

#[derive(Serialize, Deserialize)]
struct WireOp {
    #[serde(with = "serde_bytes")]
    h: Vec<u8>,
    #[serde(with = "serde_bytes")]
    s: Vec<u8>,
    #[serde(with = "serde_bytes")]
    b: Vec<u8>,
}

impl Serialize for SignedOp {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        WireOp {
            h: self.header_bytes.to_vec(),
            s: self.sig.as_bytes().to_vec(),
            b: self.body.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SignedOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let w = WireOp::deserialize(deserializer)?;
        SignedOp::from_parts(&w.h, &w.s, w.b).map_err(serde::de::Error::custom)
    }
}

fn data_aad(vault_id: &VaultId, author: &DeviceId, seq: u64) -> Vec<u8> {
    let mut aad = Vec::with_capacity(DATA_AAD_DOMAIN.len() + 56);
    aad.extend_from_slice(DATA_AAD_DOMAIN);
    aad.extend_from_slice(vault_id.as_bytes());
    aad.extend_from_slice(author.as_bytes());
    aad.extend_from_slice(&seq.to_be_bytes());
    aad
}

/// Encrypts and signs a data op as `seq` in the signer's log.
pub fn create_data_op(
    vault_id: VaultId,
    vk: &Key32,
    sk: &SigningKey,
    seq: u64,
    op: &DataOp,
) -> Result<SignedOp> {
    let plain = zeroize::Zeroizing::new(cbor::to_vec(op)?);
    if plain.len() > MAX_DATA_OP_LEN {
        return Err(CoreError::Invalid(format!(
            "item is too large ({} bytes, limit {MAX_DATA_OP_LEN})",
            plain.len()
        )));
    }
    let body = crypto::seal(vk, &data_aad(&vault_id, &sk.device_id(), seq), &plain)?;
    SignedOp::create(vault_id, sk, seq, OpClass::Data, body)
}

/// Signs a roster op (plaintext body) as `seq` in the signer's log.
pub fn create_roster_op(
    vault_id: VaultId,
    sk: &SigningKey,
    seq: u64,
    op: &RosterOp,
) -> Result<SignedOp> {
    SignedOp::create(vault_id, sk, seq, OpClass::Roster, cbor::to_vec(op)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hlc::Hlc;
    use crate::ids::ItemId;

    fn sample() -> (VaultId, Key32, SigningKey, SignedOp) {
        let vault = VaultId::random().unwrap();
        let vk = Key32::random().unwrap();
        let sk = SigningKey::generate().unwrap();
        let op = DataOp::ItemPurge {
            item: ItemId::random().unwrap(),
            hlc: Hlc::default(),
        };
        let signed = create_data_op(vault, &vk, &sk, 1, &op).unwrap();
        (vault, vk, sk, signed)
    }

    #[test]
    fn header_round_trip() {
        let (_, _, _, op) = sample();
        assert_eq!(OpHeader::decode(op.header_bytes()).unwrap(), *op.header());
    }

    #[test]
    fn signed_data_op_verifies_and_decrypts() {
        let (_, vk, _, op) = sample();
        op.verify_signature().unwrap();
        assert!(matches!(
            op.decode_data(&vk).unwrap(),
            DataOp::ItemPurge { .. }
        ));
        assert!(op.decode_data(&Key32::random().unwrap()).is_err());
    }

    #[test]
    fn wire_round_trip_preserves_bytes() {
        let (_, _, _, op) = sample();
        let bytes = cbor::to_vec(&op).unwrap();
        let back: SignedOp = cbor::from_slice(&bytes).unwrap();
        assert_eq!(back, op);
        back.verify_signature().unwrap();
    }

    #[test]
    fn tampering_is_detected() {
        let (_, _, _, op) = sample();
        // Body change breaks the committed hash.
        let mut body = op.body().to_vec();
        body[0] ^= 1;
        assert!(SignedOp::from_parts(op.header_bytes(), op.sig().as_bytes(), body).is_err());
        // Header change breaks the signature.
        let mut header = *op.header_bytes();
        header[56] ^= 2; // seq 1 -> 3
        let forged =
            SignedOp::from_parts(&header, op.sig().as_bytes(), op.body().to_vec()).unwrap();
        assert!(forged.verify_signature().is_err());
    }

    #[test]
    fn ciphertext_is_bound_to_its_position() {
        let (vault, vk, sk, op) = sample();
        // Same ciphertext re-signed at another seq must not decrypt.
        let moved = SignedOp::create(vault, &sk, 2, OpClass::Data, op.body().to_vec()).unwrap();
        assert!(moved.decode_data(&vk).is_err());
    }
}
