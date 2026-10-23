//! Cryptographic building blocks. Everything here is pure Rust so the same
//! code compiles for Windows and Android.

use std::fmt;

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use unicode_normalization::UnicodeNormalization;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::ids::{DeviceId, NoisePublicKey, Sig64};
use crate::{CoreError, Result};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

/// A 256-bit secret key, wiped from memory on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop, PartialEq, Eq)]
pub struct Key32([u8; KEY_LEN]);

impl Key32 {
    pub fn random() -> Result<Self> {
        Ok(Self(random_bytes()?))
    }

    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let arr: [u8; KEY_LEN] = bytes
            .try_into()
            .map_err(|_| CoreError::Malformed(format!("key must be {KEY_LEN} bytes")))?;
        Ok(Self(arr))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl fmt::Debug for Key32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key32(<redacted>)")
    }
}

pub fn fill_random(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|_| CoreError::Rng)
}

pub fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut out = [0u8; N];
    fill_random(&mut out)?;
    Ok(out)
}

/// Uniform random integer in `0..bound` (rejection sampling, no modulo bias).
pub fn random_below(bound: u32) -> Result<u32> {
    assert!(bound > 0, "bound must be positive");
    let zone = u32::MAX - (u32::MAX % bound);
    loop {
        let v = getrandom::u32().map_err(|_| CoreError::Rng)?;
        if v < zone {
            return Ok(v % bound);
        }
    }
}

/// XChaCha20-Poly1305 with a random nonce. Output layout: `nonce ‖ ciphertext ‖ tag`.
pub fn seal(key: &Key32, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let cipher = XChaCha20Poly1305::new(&(*key.as_bytes()).into());
    let nonce_bytes: [u8; NONCE_LEN] = random_bytes()?;
    let nonce = XNonce::from(nonce_bytes);
    let ct = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| CoreError::Encrypt)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Inverse of [`seal`]. Fails if the key, AAD or data don't match.
pub fn open(key: &Key32, aad: &[u8], sealed: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if sealed.len() < NONCE_LEN + TAG_LEN {
        return Err(CoreError::Decrypt);
    }
    let (nonce_bytes, ct) = sealed.split_at(NONCE_LEN);
    let nonce_arr: [u8; NONCE_LEN] = nonce_bytes.try_into().expect("split at NONCE_LEN");
    let cipher = XChaCha20Poly1305::new(&(*key.as_bytes()).into());
    cipher
        .decrypt(&XNonce::from(nonce_arr), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| CoreError::Decrypt)
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// HMAC-SHA256 over the concatenation of `parts`.
pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key size");
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

pub fn hkdf_sha256(ikm: &[u8], salt: &[u8], info: &[u8]) -> Key32 {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut okm = [0u8; KEY_LEN];
    hk.expand(info, &mut okm)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    Key32(okm)
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && bool::from(a.ct_eq(b))
}

/// Argon2id cost parameters, stored next to the salt so they can be raised later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

impl KdfParams {
    /// 64 MiB, 3 passes, 1 lane: roughly 0.5–1 s on a mid-range phone.
    pub const DEFAULT: Self = Self {
        m_kib: 64 * 1024,
        t: 3,
        p: 1,
    };

    /// Fast parameters that are only acceptable in tests.
    #[cfg(any(test, feature = "test-utils"))]
    pub const fn insecure_for_tests() -> Self {
        Self {
            m_kib: 64,
            t: 1,
            p: 1,
        }
    }

    /// Most memory a derivation may ask for (512 MiB, eight times today's default).
    pub const MAX_M_KIB: u32 = 512 * 1024;
    /// Most passes.
    pub const MAX_T: u32 = 10;
    /// Most lanes.
    pub const MAX_P: u32 = 8;

    /// Refuses costs no device could be asked for in good faith. The parameters of a backup file
    /// are read before anything in it is authenticated, so without limits a hostile file could
    /// make the app allocate gigabytes or compute for hours.
    pub fn validate(&self) -> Result<()> {
        let ok = (1..=Self::MAX_P).contains(&self.p)
            && (1..=Self::MAX_T).contains(&self.t)
            && self.m_kib >= 8 * self.p
            && self.m_kib <= Self::MAX_M_KIB;
        if ok {
            Ok(())
        } else {
            Err(CoreError::Kdf(format!(
                "key derivation cost out of range (memory {} KiB, passes {}, lanes {})",
                self.m_kib, self.t, self.p
            )))
        }
    }
}

/// Derives the key-encryption key from the master password (NFKC-normalized).
pub fn derive_kek(password: &str, salt: &[u8], params: KdfParams) -> Result<Key32> {
    params.validate()?;
    let normalized: Zeroizing<String> = Zeroizing::new(password.nfkc().collect());
    let argon = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(params.m_kib, params.t, params.p, Some(KEY_LEN))
            .map_err(|e| CoreError::Kdf(e.to_string()))?,
    );
    let mut out = [0u8; KEY_LEN];
    argon
        .hash_password_into(normalized.as_bytes(), salt, &mut out)
        .map_err(|e| CoreError::Kdf(e.to_string()))?;
    Ok(Key32(out))
}

/// A device's Ed25519 signing key. Signatures always carry a domain prefix.
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    pub fn generate() -> Result<Self> {
        let seed: Zeroizing<[u8; 32]> = Zeroizing::new(random_bytes()?);
        Ok(Self(ed25519_dalek::SigningKey::from_bytes(&seed)))
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(bytes))
    }

    pub fn to_bytes(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0.to_bytes())
    }

    pub fn device_id(&self) -> DeviceId {
        DeviceId(self.0.verifying_key().to_bytes())
    }

    pub fn sign(&self, domain: &[u8], msg: &[u8]) -> Sig64 {
        use ed25519_dalek::Signer;
        let mut buf = Vec::with_capacity(domain.len() + msg.len());
        buf.extend_from_slice(domain);
        buf.extend_from_slice(msg);
        Sig64(self.0.sign(&buf).to_bytes())
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({:?})", self.device_id())
    }
}

/// Verifies a domain-prefixed signature made by [`SigningKey::sign`].
pub fn verify(signer: &DeviceId, domain: &[u8], msg: &[u8], sig: &Sig64) -> Result<()> {
    let vk = ed25519_dalek::VerifyingKey::from_bytes(signer.as_bytes())
        .map_err(|_| CoreError::BadSignature)?;
    let sig = ed25519_dalek::Signature::from_bytes(sig.as_bytes());
    let mut buf = Vec::with_capacity(domain.len() + msg.len());
    buf.extend_from_slice(domain);
    buf.extend_from_slice(msg);
    vk.verify_strict(&buf, &sig)
        .map_err(|_| CoreError::BadSignature)
}

/// X25519 static key pair for the Noise transport.
#[derive(Clone)]
pub struct NoiseKeypair {
    secret: Key32,
    public: NoisePublicKey,
}

impl NoiseKeypair {
    pub fn generate() -> Result<Self> {
        Ok(Self::from_secret(Key32::random()?))
    }

    pub fn from_secret(secret: Key32) -> Self {
        let ss = x25519_dalek::StaticSecret::from(*secret.as_bytes());
        let public = NoisePublicKey(x25519_dalek::PublicKey::from(&ss).to_bytes());
        Self { secret, public }
    }

    pub fn secret(&self) -> &Key32 {
        &self.secret
    }

    pub fn public(&self) -> NoisePublicKey {
        self.public
    }
}

impl fmt::Debug for NoiseKeypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NoiseKeypair({:?})", self.public)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_derivation_costs_are_limited() {
        let cost = |m_kib, t, p| KdfParams { m_kib, t, p };
        assert!(KdfParams::DEFAULT.validate().is_ok());
        assert!(KdfParams::insecure_for_tests().validate().is_ok());
        assert!(
            cost(KdfParams::MAX_M_KIB, KdfParams::MAX_T, KdfParams::MAX_P)
                .validate()
                .is_ok()
        );
        for bad in [
            cost(KdfParams::MAX_M_KIB + 1, 1, 1),
            cost(u32::MAX, 1, 1),
            cost(64, KdfParams::MAX_T + 1, 1),
            cost(64, u32::MAX, 1),
            cost(64, 0, 1),
            cost(64, 1, 0),
            cost(64 * 1024, 1, KdfParams::MAX_P + 1),
            cost(64 * 1024, 1, u32::MAX),
            cost(0, 1, 1),
            cost(8, 1, 2),
        ] {
            assert!(bad.validate().is_err(), "{bad:?}");
            // `derive_kek` refuses them before computing anything.
            let started = std::time::Instant::now();
            assert!(derive_kek("pw", &[0u8; 16], bad).is_err(), "{bad:?}");
            assert!(
                started.elapsed() < std::time::Duration::from_secs(1),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn seal_open_round_trip() {
        let key = Key32::random().unwrap();
        let sealed = seal(&key, b"aad", b"secret").unwrap();
        assert_eq!(sealed.len(), NONCE_LEN + 6 + TAG_LEN);
        assert_eq!(&open(&key, b"aad", &sealed).unwrap()[..], b"secret");
    }

    #[test]
    fn open_rejects_wrong_aad_key_or_tamper() {
        let key = Key32::random().unwrap();
        let sealed = seal(&key, b"aad", b"secret").unwrap();
        assert!(open(&key, b"other", &sealed).is_err());
        assert!(open(&Key32::random().unwrap(), b"aad", &sealed).is_err());
        let mut tampered = sealed.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(open(&key, b"aad", &tampered).is_err());
        assert!(open(&key, b"aad", &sealed[..10]).is_err());
    }

    #[test]
    fn kek_is_deterministic_and_normalized() {
        let p = KdfParams::insecure_for_tests();
        let a = derive_kek("pa\u{0301}ss", b"saltsaltsaltsalt", p).unwrap();
        // "á" composed vs decomposed must derive the same key.
        let b = derive_kek("p\u{00e1}ss", b"saltsaltsaltsalt", p).unwrap();
        assert_eq!(a, b);
        let c = derive_kek("pass", b"saltsaltsaltsalt", p).unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn signatures_verify_and_are_domain_separated() {
        let sk = SigningKey::generate().unwrap();
        let sig = sk.sign(b"lp/a\0", b"msg");
        verify(&sk.device_id(), b"lp/a\0", b"msg", &sig).unwrap();
        assert!(verify(&sk.device_id(), b"lp/b\0", b"msg", &sig).is_err());
        assert!(verify(&sk.device_id(), b"lp/a\0", b"msh", &sig).is_err());
        let other = SigningKey::generate().unwrap();
        assert!(verify(&other.device_id(), b"lp/a\0", b"msg", &sig).is_err());
    }

    #[test]
    fn random_below_stays_in_range() {
        for _ in 0..1000 {
            assert!(random_below(7).unwrap() < 7);
        }
    }

    #[test]
    fn noise_public_key_matches_secret() {
        let kp = NoiseKeypair::generate().unwrap();
        let again = NoiseKeypair::from_secret(kp.secret().clone());
        assert_eq!(kp.public(), again.public());
    }
}
