//! Identifier newtypes. Fixed-size byte ids serialize as CBOR byte strings.

use std::fmt;

use data_encoding::{BASE32_NOPAD, HEXLOWER};
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::crypto::{random_bytes, sha256};
use crate::{CoreError, Result};

macro_rules! byte_id {
    ($(#[$meta:meta])* $name:ident, $len:expr) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub [u8; $len]);

        impl $name {
            pub const LEN: usize = $len;

            pub fn as_bytes(&self) -> &[u8; $len] {
                &self.0
            }

            pub fn from_slice(bytes: &[u8]) -> Result<Self> {
                let arr: [u8; $len] = bytes.try_into().map_err(|_| {
                    CoreError::Malformed(format!(
                        "{} must be {} bytes, got {}",
                        stringify!($name),
                        $len,
                        bytes.len()
                    ))
                })?;
                Ok(Self(arr))
            }

            pub fn to_hex(self) -> String {
                HEXLOWER.encode(&self.0)
            }

            pub fn from_hex(s: &str) -> Result<Self> {
                let bytes = HEXLOWER
                    .decode(s.trim().to_ascii_lowercase().as_bytes())
                    .map_err(|e| CoreError::Malformed(e.to_string()))?;
                Self::from_slice(&bytes)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({}…)", stringify!($name), &self.to_hex()[..8])
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_bytes(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
                struct V;
                impl<'de> Visitor<'de> for V {
                    type Value = $name;

                    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                        write!(f, "{} bytes", $len)
                    }

                    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> std::result::Result<$name, E> {
                        $name::from_slice(v).map_err(E::custom)
                    }

                    fn visit_byte_buf<E: de::Error>(self, v: Vec<u8>) -> std::result::Result<$name, E> {
                        self.visit_bytes(&v)
                    }

                    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<$name, A::Error> {
                        let mut out = [0u8; $len];
                        for (i, slot) in out.iter_mut().enumerate() {
                            *slot = seq
                                .next_element()?
                                .ok_or_else(|| de::Error::invalid_length(i, &self))?;
                        }
                        Ok($name(out))
                    }
                }
                deserializer.deserialize_bytes(V)
            }
        }
    };
}

byte_id!(
    /// Random identifier of a vault, shared by all of its devices.
    VaultId,
    16
);
byte_id!(
    /// A device is identified by its Ed25519 public signing key.
    DeviceId,
    32
);
byte_id!(
    /// X25519 static public key used for the Noise transport.
    NoisePublicKey,
    32
);
byte_id!(
    /// Ed25519 signature bytes.
    Sig64,
    64
);

impl VaultId {
    pub fn random() -> Result<Self> {
        Ok(Self(random_bytes()?))
    }
}

impl DeviceId {
    /// Human-comparable fingerprint, e.g. `ABCD-EFGH-IJKL-MNOP`.
    pub fn fingerprint(&self) -> String {
        let digest = sha256(&self.0);
        let encoded = BASE32_NOPAD.encode(&digest);
        encoded.as_bytes()[..16]
            .chunks(4)
            .map(|c| std::str::from_utf8(c).expect("base32 is ascii"))
            .collect::<Vec<_>>()
            .join("-")
    }
}

/// Points at one op: the author's device and its position in that device's log.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OpRef {
    #[serde(rename = "a")]
    pub author: DeviceId,
    #[serde(rename = "s")]
    pub seq: u64,
}

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn random() -> Result<Self> {
                Ok(Self(uuid::Builder::from_random_bytes(random_bytes()?).into_uuid()))
            }

            pub fn parse(s: &str) -> Result<Self> {
                Uuid::parse_str(s.trim())
                    .map(Self)
                    .map_err(|e| CoreError::Malformed(e.to_string()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_id!(
    /// Identifier of a vault item (login, note, card…).
    ItemId
);
uuid_id!(
    /// Identifier of a folder.
    FolderId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_ids_round_trip_through_cbor_as_bytes() {
        let id = DeviceId([7u8; 32]);
        let bytes = crate::cbor::to_vec(&id).unwrap();
        // 0x58 0x20 = CBOR byte string of length 32.
        assert_eq!(&bytes[..2], &[0x58, 0x20]);
        let back: DeviceId = crate::cbor::from_slice(&bytes).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn fingerprint_is_grouped() {
        let fp = DeviceId([1u8; 32]).fingerprint();
        assert_eq!(fp.len(), 19);
        assert_eq!(fp.matches('-').count(), 3);
    }

    #[test]
    fn uuid_ids_are_v4() {
        let id = ItemId::random().unwrap();
        assert_eq!(id.0.get_version_num(), 4);
        assert_eq!(ItemId::parse(&id.to_string()).unwrap(), id);
    }
}
