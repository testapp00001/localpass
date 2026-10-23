//! Metadata keys and the wrapped key bundles stored in them.
//!
//! * `keys.vault` — VK, SK, NK and the Noise key, sealed with the KEK
//!   (Argon2id of the master password). Needed to unlock.
//! * `keys.net`   — NK and the Noise key, sealed with the device secret, so
//!   sync runs while locked. Re-created from `keys.vault` if lost.
//! * `keys.quick` — VK, SK, NK and the Noise key, sealed with a biometric quick-unlock key.

use lp_core::cbor;
use lp_core::crypto::{self, Key32, NoiseKeypair, SigningKey};
use lp_core::ids::{DeviceId, VaultId};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{EngineError, Result};

pub mod meta {
    pub const VAULT_ID: &str = "vault.id";
    pub const GENESIS: &str = "vault.genesis";
    pub const DEVICE_ID: &str = "device.id";
    pub const DEVICE_NAME: &str = "device.name";
    pub const KDF_PARAMS: &str = "kdf.params";
    pub const KDF_SALT: &str = "kdf.salt";
    pub const KEYS_VAULT: &str = "keys.vault";
    pub const KEYS_NET: &str = "keys.net";
    pub const KEYS_QUICK: &str = "keys.quick";
    pub const HLC_LAST: &str = "hlc.last";
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct Bundle {
    #[serde(with = "serde_bytes")]
    vk: Vec<u8>,
    #[serde(with = "serde_bytes")]
    sk: Vec<u8>,
    #[serde(with = "serde_bytes")]
    nk: Vec<u8>,
    #[serde(with = "serde_bytes")]
    noise: Vec<u8>,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct NetBundle {
    #[serde(with = "serde_bytes")]
    nk: Vec<u8>,
    #[serde(with = "serde_bytes")]
    noise: Vec<u8>,
}

/// All of a device's secrets after a successful unlock.
pub struct VaultKeys {
    pub vk: Key32,
    pub sk: SigningKey,
    pub nk: Key32,
    pub noise: NoiseKeypair,
}

fn aad(kind: &[u8], vault: &VaultId, device: &DeviceId) -> Vec<u8> {
    let mut a = b"lp/keys/".to_vec();
    a.extend_from_slice(kind);
    a.push(0);
    a.extend_from_slice(vault.as_bytes());
    a.extend_from_slice(device.as_bytes());
    a
}

pub fn seal_vault_keys(
    wrapping: &Key32,
    kind: &[u8],
    vault: &VaultId,
    keys: &VaultKeys,
) -> Result<Vec<u8>> {
    let me = keys.sk.device_id();
    let bundle = Bundle {
        vk: keys.vk.as_bytes().to_vec(),
        sk: keys.sk.to_bytes().to_vec(),
        nk: keys.nk.as_bytes().to_vec(),
        noise: keys.noise.secret().as_bytes().to_vec(),
    };
    let plain = Zeroizing::new(cbor::to_vec(&bundle)?);
    Ok(crypto::seal(wrapping, &aad(kind, vault, &me), &plain)?)
}

/// Fails with [`EngineError::WrongPassword`] if the wrapping key is wrong.
pub fn open_vault_keys(
    wrapping: &Key32,
    kind: &[u8],
    vault: &VaultId,
    me: &DeviceId,
    blob: &[u8],
) -> Result<VaultKeys> {
    let plain = crypto::open(wrapping, &aad(kind, vault, me), blob)
        .map_err(|_| EngineError::WrongPassword)?;
    let bundle: Bundle = cbor::from_slice(&plain)?;
    let sk_bytes: Zeroizing<[u8; 32]> = Zeroizing::new(
        bundle
            .sk
            .as_slice()
            .try_into()
            .map_err(|_| EngineError::Invalid("corrupt key bundle".into()))?,
    );
    let sk = SigningKey::from_bytes(&sk_bytes);
    if sk.device_id() != *me {
        return Err(EngineError::Invalid(
            "key bundle belongs to another device".into(),
        ));
    }
    Ok(VaultKeys {
        vk: Key32::from_slice(&bundle.vk)?,
        sk,
        nk: Key32::from_slice(&bundle.nk)?,
        noise: NoiseKeypair::from_secret(Key32::from_slice(&bundle.noise)?),
    })
}

pub fn seal_net(
    ds: &Key32,
    vault: &VaultId,
    me: &DeviceId,
    nk: &Key32,
    noise: &NoiseKeypair,
) -> Result<Vec<u8>> {
    let bundle = NetBundle {
        nk: nk.as_bytes().to_vec(),
        noise: noise.secret().as_bytes().to_vec(),
    };
    let plain = Zeroizing::new(cbor::to_vec(&bundle)?);
    Ok(crypto::seal(ds, &aad(b"net", vault, me), &plain)?)
}

pub fn open_net(
    ds: &Key32,
    vault: &VaultId,
    me: &DeviceId,
    blob: &[u8],
) -> Result<(Key32, NoiseKeypair)> {
    let plain = crypto::open(ds, &aad(b"net", vault, me), blob).map_err(|_| {
        EngineError::Secret("device secret changed; unlock once to repair sync".into())
    })?;
    let bundle: NetBundle = cbor::from_slice(&plain)?;
    Ok((
        Key32::from_slice(&bundle.nk)?,
        NoiseKeypair::from_secret(Key32::from_slice(&bundle.noise)?),
    ))
}
