//! Payloads carried inside ops.
//!
//! `DataOp`s are encrypted with the vault key; `RosterOp`s are plaintext (but
//! signed) so a locked device can still check who belongs to the vault.
//! Unknown variants decode to `Unknown` and are ignored, so newer app versions
//! can add op types without breaking older ones.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::Result;
use crate::crypto::{SigningKey, verify};
use crate::hlc::Hlc;
use crate::ids::{DeviceId, FolderId, ItemId, NoisePublicKey, OpRef, Sig64, VaultId};

/// Largest allowed serialized data op (before encryption).
pub const MAX_DATA_OP_LEN: usize = 48 * 1024;

/// Well-known item field keys. Custom fields use `custom.<id>`.
pub mod keys {
    pub const KIND: &str = "kind";
    pub const TITLE: &str = "title";
    pub const USERNAME: &str = "username";
    pub const PASSWORD: &str = "password";
    pub const URLS: &str = "urls";
    pub const NOTES: &str = "notes";
    pub const TOTP: &str = "totp";
    pub const FOLDER: &str = "folder";
    pub const FAVORITE: &str = "favorite";
    pub const TRASHED: &str = "trashed";
    pub const CARD_HOLDER: &str = "card.holder";
    pub const CARD_BRAND: &str = "card.brand";
    pub const CARD_NUMBER: &str = "card.number";
    pub const CARD_EXP_MONTH: &str = "card.exp_month";
    pub const CARD_EXP_YEAR: &str = "card.exp_year";
    pub const CARD_CVV: &str = "card.cvv";
    pub const CUSTOM_PREFIX: &str = "custom.";
    /// A passkey (WebAuthn credential): the site, the account, and the private key.
    pub const PASSKEY_RP_ID: &str = "passkey.rp_id";
    pub const PASSKEY_RP_NAME: &str = "passkey.rp_name";
    /// Credential id, base64url.
    pub const PASSKEY_CREDENTIAL_ID: &str = "passkey.credential_id";
    /// User handle chosen by the site, base64url.
    pub const PASSKEY_USER_HANDLE: &str = "passkey.user_handle";
    pub const PASSKEY_USER_NAME: &str = "passkey.user_name";
    pub const PASSKEY_USER_DISPLAY_NAME: &str = "passkey.user_display_name";
    /// PKCS#8 private key, base64url.
    pub const PASSKEY_PRIVATE_KEY: &str = "passkey.private_key";
    pub const PASSKEY_CREATED_MS: &str = "passkey.created_ms";

    /// Fields whose values are secrets: never sent to the UI in list views.
    pub fn is_secret(key: &str) -> bool {
        matches!(
            key,
            PASSWORD | TOTP | CARD_NUMBER | CARD_CVV | PASSKEY_PRIVATE_KEY
        )
    }
}

/// Item kinds stored in the `kind` field.
pub mod kinds {
    pub const LOGIN: &str = "login";
    pub const NOTE: &str = "note";
    pub const CARD: &str = "card";
    pub const IDENTITY: &str = "identity";
    pub const PASSKEY: &str = "passkey";
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum DataOp {
    /// Upserts some fields of an item (field-level last-writer-wins).
    #[serde(rename = "item")]
    ItemSet {
        item: ItemId,
        hlc: Hlc,
        fields: Vec<FieldSet>,
    },
    /// Permanently deletes an item; later edits to it are ignored.
    #[serde(rename = "purge")]
    ItemPurge { item: ItemId, hlc: Hlc },
    /// Creates, renames (`Some`) or deletes (`None`) a folder.
    #[serde(rename = "folder")]
    FolderSet {
        folder: FolderId,
        hlc: Hlc,
        name: Option<String>,
    },
    /// Display metadata about a device (encrypted, unlike the roster).
    #[serde(rename = "devmeta")]
    DeviceMetaSet {
        device: DeviceId,
        hlc: Hlc,
        name: String,
        platform: String,
    },
    #[serde(other)]
    Unknown,
}

impl DataOp {
    pub fn hlc(&self) -> Option<Hlc> {
        match self {
            DataOp::ItemSet { hlc, .. }
            | DataOp::ItemPurge { hlc, .. }
            | DataOp::FolderSet { hlc, .. }
            | DataOp::DeviceMetaSet { hlc, .. } => Some(*hlc),
            DataOp::Unknown => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FieldSet {
    pub key: String,
    pub value: FieldValue,
    /// The op whose value of this field the editor saw when making this edit.
    /// Two edits with the same base were made concurrently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<OpRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k", content = "v")]
pub enum FieldValue {
    #[serde(rename = "s")]
    Text(String),
    #[serde(rename = "b")]
    Bool(bool),
    #[serde(rename = "l")]
    List(Vec<String>),
    #[serde(rename = "c")]
    Custom(CustomField),
    /// The field was cleared.
    #[serde(rename = "n")]
    Null,
    #[serde(other)]
    Unknown,
}

impl FieldValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Text(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            FieldValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            FieldValue::List(l) => Some(l),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, FieldValue::Null)
    }

    /// Best-effort wipe of secret strings before the value is dropped.
    pub fn wipe(&mut self) {
        match self {
            FieldValue::Text(s) => s.zeroize(),
            FieldValue::List(l) => l.iter_mut().for_each(|s| s.zeroize()),
            FieldValue::Custom(c) => {
                c.label.zeroize();
                c.value.zeroize();
            }
            FieldValue::Bool(_) | FieldValue::Null | FieldValue::Unknown => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomField {
    pub label: String,
    pub value: String,
    #[serde(default)]
    pub hidden: bool,
}

const BIND_DOMAIN: &[u8] = b"lp/bind/v1\0";

/// A device's public keys. `bind_sig` is made with the device's own signing key
/// over its Noise key, so nobody else can pair a device id with another key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceKeys {
    pub sign_pk: DeviceId,
    pub noise_pk: NoisePublicKey,
    pub bind_sig: Sig64,
}

impl DeviceKeys {
    pub fn new(sk: &SigningKey, vault_id: &VaultId, noise_pk: NoisePublicKey) -> Self {
        let sign_pk = sk.device_id();
        let bind_sig = sk.sign(BIND_DOMAIN, &Self::bind_msg(vault_id, &noise_pk));
        Self {
            sign_pk,
            noise_pk,
            bind_sig,
        }
    }

    pub fn verify_binding(&self, vault_id: &VaultId) -> Result<()> {
        verify(
            &self.sign_pk,
            BIND_DOMAIN,
            &Self::bind_msg(vault_id, &self.noise_pk),
            &self.bind_sig,
        )
    }

    fn bind_msg(vault_id: &VaultId, noise_pk: &NoisePublicKey) -> Vec<u8> {
        let mut msg = Vec::with_capacity(48);
        msg.extend_from_slice(vault_id.as_bytes());
        msg.extend_from_slice(noise_pk.as_bytes());
        msg
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum RosterOp {
    /// First op of the vault creator; the root of trust every member pins.
    #[serde(rename = "genesis")]
    Genesis {
        vault_id: VaultId,
        keys: DeviceKeys,
        created_ms: u64,
    },
    /// Authored by an existing member when it invites a new device.
    #[serde(rename = "add")]
    DeviceAdded { keys: DeviceKeys, added_ms: u64 },
    /// Revokes a device. Roster ops it authored after `cutoff_seq` are void.
    #[serde(rename = "revoke")]
    DeviceRevoked {
        device: DeviceId,
        cutoff_seq: u64,
        revoked_ms: u64,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbor;
    use crate::crypto::NoiseKeypair;

    #[test]
    fn data_op_round_trips() {
        let op = DataOp::ItemSet {
            item: ItemId::random().unwrap(),
            hlc: Hlc {
                wall_ms: 5,
                counter: 1,
            },
            fields: vec![
                FieldSet {
                    key: keys::PASSWORD.into(),
                    value: FieldValue::Text("hunter2".into()),
                    base: Some(OpRef {
                        author: DeviceId([3; 32]),
                        seq: 9,
                    }),
                },
                FieldSet {
                    key: keys::FAVORITE.into(),
                    value: FieldValue::Bool(true),
                    base: None,
                },
                FieldSet {
                    key: "custom.x".into(),
                    value: FieldValue::Custom(CustomField {
                        label: "PIN".into(),
                        value: "1234".into(),
                        hidden: true,
                    }),
                    base: None,
                },
            ],
        };
        let bytes = cbor::to_vec(&op).unwrap();
        assert_eq!(cbor::from_slice::<DataOp>(&bytes).unwrap(), op);
    }

    #[test]
    fn unknown_variants_decode_as_unknown() {
        #[derive(Serialize)]
        #[serde(tag = "t")]
        enum Future {
            #[serde(rename = "attachment")]
            Attachment { size: u64 },
        }
        let bytes = cbor::to_vec(&Future::Attachment { size: 3 }).unwrap();
        assert_eq!(cbor::from_slice::<DataOp>(&bytes).unwrap(), DataOp::Unknown);
        assert_eq!(
            cbor::from_slice::<RosterOp>(&bytes).unwrap(),
            RosterOp::Unknown
        );
    }

    #[test]
    fn device_key_binding_is_checked() {
        let sk = SigningKey::generate().unwrap();
        let vault = VaultId::random().unwrap();
        let noise = NoiseKeypair::generate().unwrap();
        let keys = DeviceKeys::new(&sk, &vault, noise.public());
        keys.verify_binding(&vault).unwrap();
        assert!(keys.verify_binding(&VaultId::random().unwrap()).is_err());
        let mut swapped = keys.clone();
        swapped.noise_pk = NoiseKeypair::generate().unwrap().public();
        assert!(swapped.verify_binding(&vault).is_err());
    }
}
