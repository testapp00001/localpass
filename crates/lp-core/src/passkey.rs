//! WebAuthn passkeys (ES256 only): key generation, COSE public key, authenticator
//! data, "none" attestation and assertion signatures. Pure logic: the vault stores
//! the key, and callers decide when a user check has happened.
//!
//! These are synced ("multi-device") credentials, so the signature counter is
//! always 0 and the backup-eligible and backup-state flags are set.

use ciborium::Value;
use p256::ecdsa::signature::{Signer, Verifier};
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{random_bytes, sha256};
use crate::{CoreError, Result};

/// COSE algorithm id for ECDSA with P-256 and SHA-256.
pub const ALG_ES256: i64 = -7;

/// User present.
pub const FLAG_UP: u8 = 0x01;
/// User verified (Windows Hello, a fingerprint, ...).
pub const FLAG_UV: u8 = 0x04;
/// Credential can be backed up (synced).
pub const FLAG_BE: u8 = 0x08;
/// Credential is backed up.
pub const FLAG_BS: u8 = 0x10;
/// Attested credential data follows.
pub const FLAG_AT: u8 = 0x40;
/// Extension data follows.
pub const FLAG_ED: u8 = 0x80;

/// Base64url without padding, the encoding WebAuthn JSON uses for byte strings.
pub mod b64u {
    use data_encoding::BASE64URL_NOPAD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn encode(bytes: &[u8]) -> String {
        BASE64URL_NOPAD.encode(bytes)
    }

    pub fn decode(text: &str) -> Result<Vec<u8>, String> {
        BASE64URL_NOPAD
            .decode(text.trim_end_matches('=').as_bytes())
            .map_err(|e| e.to_string())
    }

    /// For `#[serde(with = "...")]` on `Vec<u8>` fields.
    pub fn serialize<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(d)?;
        decode(&text).map_err(serde::de::Error::custom)
    }
}

/// For `#[serde(with = "...")]` on `Vec<Vec<u8>>` fields (lists of credential ids).
pub mod b64u_list {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(items: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error> {
        items
            .iter()
            .map(|b| super::b64u::encode(b))
            .collect::<Vec<_>>()
            .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Vec<u8>>, D::Error> {
        Vec::<String>::deserialize(d)?
            .iter()
            .map(|t| super::b64u::decode(t).map_err(serde::de::Error::custom))
            .collect()
    }
}

// The request and response shapes below are the broker's wire format. They live here (not in the
// engine) so a client such as the Windows passkey provider can use them without linking the
// engine; `lp-engine` re-exports them.

/// A passkey the vault holds (no private key).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyCandidate {
    pub item_id: String,
    pub rp_id: String,
    pub rp_name: String,
    pub user_name: String,
    pub user_display_name: String,
    #[serde(with = "b64u")]
    pub credential_id: Vec<u8>,
    #[serde(with = "b64u")]
    pub user_handle: Vec<u8>,
    pub created_ms: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyCreateRequest {
    /// The page's origin, as reported by the browser or OS.
    pub origin: String,
    pub rp_id: String,
    #[serde(default)]
    pub rp_name: String,
    #[serde(with = "b64u")]
    pub user_handle: Vec<u8>,
    #[serde(default)]
    pub user_name: String,
    #[serde(default)]
    pub user_display_name: String,
    /// COSE algorithm ids the site accepts, in its order of preference. Empty means any.
    #[serde(default)]
    pub algorithms: Vec<i64>,
    /// Credential ids the site already has for this user; refuse if we hold one of them.
    #[serde(default, with = "b64u_list")]
    pub exclude: Vec<Vec<u8>>,
    /// Whether the caller has just verified the user (Windows Hello, a fingerprint).
    #[serde(default)]
    pub user_verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyCreated {
    pub item_id: String,
    #[serde(with = "b64u")]
    pub credential_id: Vec<u8>,
    #[serde(with = "b64u")]
    pub auth_data: Vec<u8>,
    #[serde(with = "b64u")]
    pub attestation_object: Vec<u8>,
    /// COSE_Key of the public key.
    #[serde(with = "b64u")]
    pub public_key: Vec<u8>,
    /// SubjectPublicKeyInfo DER of the public key (what `getPublicKey()` returns).
    #[serde(with = "b64u")]
    pub public_key_spki: Vec<u8>,
    pub algorithm: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyAssertRequest {
    pub origin: String,
    pub rp_id: String,
    pub item_id: String,
    /// SHA-256 of the client data JSON.
    #[serde(with = "b64u")]
    pub client_data_hash: Vec<u8>,
    #[serde(default)]
    pub user_verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyAssertion {
    pub item_id: String,
    #[serde(with = "b64u")]
    pub credential_id: Vec<u8>,
    #[serde(with = "b64u")]
    pub auth_data: Vec<u8>,
    #[serde(with = "b64u")]
    pub signature: Vec<u8>,
    #[serde(with = "b64u")]
    pub user_handle: Vec<u8>,
}

/// A passkey's private key.
pub struct PasskeyKey(SigningKey);

impl PasskeyKey {
    pub fn generate() -> Result<Self> {
        loop {
            let bytes = Zeroizing::new(random_bytes::<32>()?);
            // Almost always valid; a scalar of zero or above the group order is retried.
            if let Ok(key) = SigningKey::from_slice(bytes.as_slice()) {
                return Ok(Self(key));
            }
        }
    }

    pub fn from_pkcs8_der(der: &[u8]) -> Result<Self> {
        SigningKey::from_pkcs8_der(der)
            .map(Self)
            .map_err(|e| CoreError::Malformed(format!("passkey private key: {e}")))
    }

    pub fn to_pkcs8_der(&self) -> Result<Zeroizing<Vec<u8>>> {
        let doc = self
            .0
            .to_pkcs8_der()
            .map_err(|e| CoreError::Encode(format!("passkey private key: {e}")))?;
        Ok(Zeroizing::new(doc.as_bytes().to_vec()))
    }

    pub fn public(&self) -> PasskeyPublic {
        PasskeyPublic::from_verifying_key(self.0.verifying_key())
    }

    /// ECDSA over SHA-256 of `message`, DER encoded as WebAuthn expects.
    pub fn sign(&self, message: &[u8]) -> Vec<u8> {
        let signature: Signature = self.0.sign(message);
        signature.to_der().as_bytes().to_vec()
    }

    /// The signature a relying party checks for a sign-in: over authenticator data followed
    /// by the hash of the client data.
    pub fn assertion_signature(&self, auth_data: &[u8], client_data_hash: &[u8]) -> Vec<u8> {
        let mut message = Vec::with_capacity(auth_data.len() + client_data_hash.len());
        message.extend_from_slice(auth_data);
        message.extend_from_slice(client_data_hash);
        self.sign(&message)
    }
}

/// A passkey's public key (an uncompressed P-256 point).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasskeyPublic {
    pub x: [u8; 32],
    pub y: [u8; 32],
}

impl PasskeyPublic {
    fn from_verifying_key(key: &VerifyingKey) -> Self {
        let point = key.to_sec1_point(false);
        let mut x = [0u8; 32];
        let mut y = [0u8; 32];
        x.copy_from_slice(point.x().expect("uncompressed point has x"));
        y.copy_from_slice(point.y().expect("uncompressed point has y"));
        Self { x, y }
    }

    fn verifying_key(&self) -> Result<VerifyingKey> {
        let mut sec1 = Vec::with_capacity(65);
        sec1.push(0x04);
        sec1.extend_from_slice(&self.x);
        sec1.extend_from_slice(&self.y);
        VerifyingKey::from_sec1_bytes(&sec1)
            .map_err(|_| CoreError::Malformed("passkey public key is not a P-256 point".into()))
    }

    /// COSE_Key in CTAP2 canonical form: {1: 2, 3: -7, -1: 1, -2: x, -3: y}.
    pub fn to_cose(&self) -> Vec<u8> {
        let map = Value::Map(vec![
            (Value::Integer(1.into()), Value::Integer(2.into())),
            (Value::Integer(3.into()), Value::Integer(ALG_ES256.into())),
            (Value::Integer((-1).into()), Value::Integer(1.into())),
            (Value::Integer((-2).into()), Value::Bytes(self.x.to_vec())),
            (Value::Integer((-3).into()), Value::Bytes(self.y.to_vec())),
        ]);
        let mut out = Vec::with_capacity(77);
        ciborium::ser::into_writer(&map, &mut out).expect("writing CBOR to a Vec cannot fail");
        out
    }

    pub fn from_cose(bytes: &[u8]) -> Result<Self> {
        let mut rest = bytes;
        let value = decode_cbor_item(&mut rest)?;
        if !rest.is_empty() {
            return Err(CoreError::Malformed("COSE key: trailing bytes".into()));
        }
        Self::from_cose_value(&value)
    }

    fn from_cose_value(value: &Value) -> Result<Self> {
        let bad = |what: &str| CoreError::Malformed(format!("COSE key: {what}"));
        let Value::Map(entries) = value else {
            return Err(bad("not a map"));
        };
        if entries
            .iter()
            .enumerate()
            .any(|(i, (key, _))| entries[..i].iter().any(|(earlier, _)| earlier == key))
        {
            return Err(bad("a label appears twice"));
        }
        let get = |label: i64| {
            entries.iter().find_map(|(k, v)| match k {
                Value::Integer(i) if i128::from(*i) == i128::from(label) => Some(v),
                _ => None,
            })
        };
        let int = |label: i64| match get(label) {
            Some(Value::Integer(i)) => Some(i128::from(*i)),
            _ => None,
        };
        let coord = |label: i64| match get(label) {
            Some(Value::Bytes(b)) => <[u8; 32]>::try_from(b.as_slice()).ok(),
            _ => None,
        };
        if int(1) != Some(2) || int(3) != Some(i128::from(ALG_ES256)) || int(-1) != Some(1) {
            return Err(bad("not an ES256 key"));
        }
        Ok(Self {
            x: coord(-2).ok_or_else(|| bad("missing x"))?,
            y: coord(-3).ok_or_else(|| bad("missing y"))?,
        })
    }

    /// SubjectPublicKeyInfo DER, what `getPublicKey()` returns in the browser.
    pub fn to_spki_der(&self) -> Vec<u8> {
        // Fixed header for an uncompressed P-256 key: SEQUENCE { SEQUENCE { id-ecPublicKey,
        // prime256v1 }, BIT STRING { 04 || x || y } }.
        const HEADER: [u8; 27] = [
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00, 0x04,
        ];
        let mut out = Vec::with_capacity(91);
        out.extend_from_slice(&HEADER);
        out.extend_from_slice(&self.x);
        out.extend_from_slice(&self.y);
        out
    }

    /// Checks a DER encoded ECDSA signature over SHA-256 of `message`.
    pub fn verify(&self, message: &[u8], der_signature: &[u8]) -> Result<()> {
        let signature = Signature::from_der(der_signature).map_err(|_| CoreError::BadSignature)?;
        self.verifying_key()?
            .verify(message, &signature)
            .map_err(|_| CoreError::BadSignature)
    }
}

/// SHA-256 of the relying party id, the first field of authenticator data.
pub fn rp_id_hash(rp_id: &str) -> [u8; 32] {
    sha256(rp_id.as_bytes())
}

/// A fresh random credential id (32 bytes).
pub fn new_credential_id() -> Result<Vec<u8>> {
    Ok(random_bytes::<32>()?.to_vec())
}

/// Authenticator data. With `attested`, the new credential's id and public key are
/// included and the AT flag is set. The AAGUID is all zeros (no attestation).
pub fn authenticator_data(
    rp_id: &str,
    flags: u8,
    sign_count: u32,
    attested: Option<(&[u8], &PasskeyPublic)>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(37 + 18 + 32 + 77);
    out.extend_from_slice(&rp_id_hash(rp_id));
    out.push(if attested.is_some() {
        flags | FLAG_AT
    } else {
        flags
    });
    out.extend_from_slice(&sign_count.to_be_bytes());
    if let Some((credential_id, public)) = attested {
        assert!(
            credential_id.len() <= usize::from(u16::MAX),
            "a credential id is at most 65535 bytes"
        );
        out.extend_from_slice(&[0u8; 16]);
        out.extend_from_slice(&(credential_id.len() as u16).to_be_bytes());
        out.extend_from_slice(credential_id);
        out.extend_from_slice(&public.to_cose());
    }
    out
}

/// The attestation object for the "none" format: {"fmt": "none", "attStmt": {}, "authData": ...}.
pub fn attestation_object_none(auth_data: &[u8]) -> Vec<u8> {
    let map = Value::Map(vec![
        (Value::Text("fmt".into()), Value::Text("none".into())),
        (Value::Text("attStmt".into()), Value::Map(vec![])),
        (
            Value::Text("authData".into()),
            Value::Bytes(auth_data.to_vec()),
        ),
    ]);
    let mut out = Vec::with_capacity(auth_data.len() + 40);
    ciborium::ser::into_writer(&map, &mut out).expect("writing CBOR to a Vec cannot fail");
    out
}

/// Authenticator data taken apart (used to verify what we produce).
#[derive(Debug)]
pub struct ParsedAuthData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub sign_count: u32,
    pub credential: Option<(Vec<u8>, PasskeyPublic)>,
}

pub fn parse_authenticator_data(bytes: &[u8]) -> Result<ParsedAuthData> {
    let bad = |what: &str| CoreError::Malformed(format!("authenticator data: {what}"));
    if bytes.len() < 37 {
        return Err(bad("too short"));
    }
    let rp_id_hash: [u8; 32] = bytes[..32].try_into().expect("32 bytes");
    let flags = bytes[32];
    let sign_count = u32::from_be_bytes(bytes[33..37].try_into().expect("4 bytes"));
    let credential = if flags & FLAG_AT != 0 {
        let rest = &bytes[37..];
        if rest.len() < 18 {
            return Err(bad("attested data too short"));
        }
        let id_len = usize::from(u16::from_be_bytes([rest[16], rest[17]]));
        let after_header = &rest[18..];
        if after_header.len() < id_len {
            return Err(bad("credential id truncated"));
        }
        let (id, mut cose) = after_header.split_at(id_len);
        // The COSE key is the first CBOR item; extension data may follow it (ED flag).
        let value = decode_cbor_item(&mut cose).map_err(|e| bad(&e.to_string()))?;
        if !cose.is_empty() && flags & FLAG_ED == 0 {
            return Err(bad("unexpected bytes after the credential public key"));
        }
        Some((id.to_vec(), PasskeyPublic::from_cose_value(&value)?))
    } else {
        if bytes.len() > 37 && flags & FLAG_ED == 0 {
            return Err(bad("unexpected bytes after the header"));
        }
        None
    };
    Ok(ParsedAuthData {
        rp_id_hash,
        flags,
        sign_count,
        credential,
    })
}

/// Reads one CBOR item from the front of `bytes` and leaves what follows in `bytes`.
fn decode_cbor_item(bytes: &mut &[u8]) -> Result<Value> {
    ciborium::de::from_reader(bytes).map_err(|e| CoreError::Malformed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify_round_trip_and_reject_tampering() {
        let key = PasskeyKey::generate().unwrap();
        let public = key.public();
        let sig = key.sign(b"hello");
        public.verify(b"hello", &sig).unwrap();
        assert!(public.verify(b"hellp", &sig).is_err());
        let other = PasskeyKey::generate().unwrap().public();
        assert!(other.verify(b"hello", &sig).is_err());
    }

    #[test]
    fn spki_matches_the_library_encoding() {
        use p256::pkcs8::EncodePublicKey;
        let key = PasskeyKey::generate().unwrap();
        let expected = key.0.verifying_key().to_public_key_der().unwrap();
        assert_eq!(key.public().to_spki_der(), expected.as_bytes());
    }

    #[test]
    fn private_key_survives_pkcs8() {
        let key = PasskeyKey::generate().unwrap();
        let der = key.to_pkcs8_der().unwrap();
        let again = PasskeyKey::from_pkcs8_der(&der).unwrap();
        assert_eq!(key.public(), again.public());
        assert!(PasskeyKey::from_pkcs8_der(&der[..der.len() - 3]).is_err());
    }

    #[test]
    fn cose_key_is_canonical_and_round_trips() {
        let public = PasskeyKey::generate().unwrap().public();
        let cose = public.to_cose();
        // map(5), 1: 2, 3: -7, -1: 1, -2: bytes(32)
        assert_eq!(
            &cose[..10],
            &[0xa5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20]
        );
        assert_eq!(cose.len(), 77);
        assert_eq!(PasskeyPublic::from_cose(&cose).unwrap(), public);
    }

    #[test]
    fn cose_parsing_rejects_other_algorithms() {
        let public = PasskeyKey::generate().unwrap().public();
        let mut cose = public.to_cose();
        cose[5] = 0x27; // alg -8 instead of -7
        assert!(PasskeyPublic::from_cose(&cose).is_err());
        assert!(PasskeyPublic::from_cose(&[0x80]).is_err());
    }

    #[test]
    fn cose_keys_must_be_one_clean_item() {
        let public = PasskeyKey::generate().unwrap().public();
        let cose = public.to_cose();
        let mut trailing = cose.clone();
        trailing.push(0);
        assert!(
            PasskeyPublic::from_cose(&trailing).is_err(),
            "trailing bytes"
        );

        // The same label twice (here a second, different x) is ambiguous: refuse it.
        let int = |i: i64| Value::Integer(i.into());
        let key = |x: &[u8]| Value::Bytes(x.to_vec());
        let twice = Value::Map(vec![
            (int(1), int(2)),
            (int(3), int(ALG_ES256)),
            (int(-1), int(1)),
            (int(-2), key(&public.x)),
            (int(-2), key(&[9u8; 32])),
            (int(-3), key(&public.y)),
        ]);
        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&twice, &mut encoded).unwrap();
        assert!(
            PasskeyPublic::from_cose(&encoded).is_err(),
            "duplicate label"
        );
    }

    #[test]
    fn authenticator_data_with_unexpected_trailing_bytes_is_refused() {
        let key = PasskeyKey::generate().unwrap();
        let public = key.public();
        let id = new_credential_id().unwrap();
        let flags = FLAG_UP | FLAG_BE | FLAG_BS;
        let attested = authenticator_data("example.com", flags, 0, Some((&id, &public)));
        let bare = authenticator_data("example.com", flags, 0, None);
        for good in [&attested, &bare] {
            assert!(parse_authenticator_data(good).is_ok());
            let mut extra = good.clone();
            extra.push(0);
            assert!(parse_authenticator_data(&extra).is_err(), "trailing byte");
            // Extensions may follow when the ED flag says so.
            extra[32] |= 0x80;
            assert!(parse_authenticator_data(&extra).is_ok(), "extension data");
        }
    }

    #[test]
    #[should_panic(expected = "65535")]
    fn credential_ids_do_not_fit_a_truncated_length() {
        let public = PasskeyKey::generate().unwrap().public();
        let id = vec![1u8; 65_536];
        let _ = authenticator_data("example.com", FLAG_UP, 0, Some((&id, &public)));
    }

    #[test]
    fn registration_then_sign_in_as_a_relying_party_would_check() {
        let rp = "example.com";
        let key = PasskeyKey::generate().unwrap();
        let credential_id = new_credential_id().unwrap();

        // Registration: the relying party reads the public key out of the attestation object.
        let created = authenticator_data(
            rp,
            FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS,
            0,
            Some((&credential_id, &key.public())),
        );
        let object = attestation_object_none(&created);
        let Value::Map(entries) = decode_cbor_item(&mut object.as_slice()).unwrap() else {
            panic!("attestation object is a map")
        };
        let field = |name: &str| {
            entries
                .iter()
                .find(|(k, _)| matches!(k, Value::Text(t) if t == name))
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(field("fmt"), Value::Text("none".into()));
        assert_eq!(field("attStmt"), Value::Map(vec![]));
        let Value::Bytes(auth_bytes) = field("authData") else {
            panic!("authData is bytes")
        };
        let parsed = parse_authenticator_data(&auth_bytes).unwrap();
        assert_eq!(parsed.rp_id_hash, rp_id_hash(rp));
        assert_eq!(
            parsed.flags,
            FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS | FLAG_AT
        );
        assert_eq!(parsed.sign_count, 0);
        let (stored_id, stored_public) = parsed.credential.unwrap();
        assert_eq!(stored_id, credential_id);

        // Sign-in: the relying party verifies authData || SHA-256(clientDataJSON).
        let client_data =
            br#"{"type":"webauthn.get","challenge":"abc","origin":"https://example.com"}"#;
        let client_hash = sha256(client_data);
        let auth = authenticator_data(rp, FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS, 0, None);
        assert_eq!(auth.len(), 37);
        let signature = key.assertion_signature(&auth, &client_hash);
        let mut signed = auth.clone();
        signed.extend_from_slice(&client_hash);
        stored_public.verify(&signed, &signature).unwrap();
        // A different challenge must not verify.
        let mut other = auth.clone();
        other.extend_from_slice(&sha256(b"different"));
        assert!(stored_public.verify(&other, &signature).is_err());
    }

    #[test]
    fn authenticator_data_parsing_never_panics_on_garbage() {
        for len in 0..120usize {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 31 + 7) as u8).collect();
            let _ = parse_authenticator_data(&bytes);
            let mut with_at = bytes.clone();
            if with_at.len() > 32 {
                with_at[32] |= FLAG_AT;
            }
            let _ = parse_authenticator_data(&with_at);
        }
    }

    #[test]
    fn base64url_helpers() {
        assert_eq!(b64u::encode(&[0xfb, 0xff]), "-_8");
        assert_eq!(b64u::decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert_eq!(b64u::decode("-_8=").unwrap(), vec![0xfb, 0xff]);
        assert!(b64u::decode("not base64!").is_err());
    }
}
