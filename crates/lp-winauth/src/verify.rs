//! Verification of the signature Windows puts on every plugin operation request.
//!
//! The key comes from `WebAuthNPluginGetOperationSigningPublicKey` as a CNG `BCRYPT_ECCPUBLIC_BLOB`:
//! `magic (u32 LE) | cbKey (u32 LE) | X (cbKey) | Y (cbKey)`. Spike finding: on this PC the blob is
//! `ECS3` (ECDSA P-384, cbKey 48, 104 bytes) although the registry says `SigningKeyAlgorithm = ECDSA_P256`
//! and the notes expected 72 bytes. So the curve is read from the magic, and the digest is tried as
//! SHA-256, SHA-384 and SHA-512 (CNG signs a caller-supplied digest; the notes only say SHA-256).
//! ECDSA signatures from CNG are raw `r || s`.

use sha2::{Digest, Sha256, Sha384, Sha512};
use windows::Win32::Security::Cryptography::{
    BCRYPT_ALG_HANDLE, BCRYPT_ECCPUBLIC_BLOB, BCRYPT_ECDSA_P256_ALGORITHM,
    BCRYPT_ECDSA_P384_ALGORITHM, BCRYPT_ECDSA_P521_ALGORITHM, BCRYPT_FLAGS, BCRYPT_KEY_HANDLE,
    BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS, BCryptCloseAlgorithmProvider, BCryptDestroyKey,
    BCryptImportKeyPair, BCryptOpenAlgorithmProvider, BCryptVerifySignature,
};
use windows::core::PCWSTR;

/// `BCRYPT_ECDSA_PUBLIC_P256_MAGIC` ("ECS1").
pub const ECS1_MAGIC: u32 = 0x3153_4345;
/// `BCRYPT_ECDSA_PUBLIC_P384_MAGIC` ("ECS3").
pub const ECS3_MAGIC: u32 = 0x3353_4345;
/// `BCRYPT_ECDSA_PUBLIC_P521_MAGIC` ("ECS5").
pub const ECS5_MAGIC: u32 = 0x3553_4345;
/// `BCRYPT_RSAPUBLIC_MAGIC` ("RSA1"), which the Contoso sample also handles (RSA-PSS). Not supported here.
pub const RSA1_MAGIC: u32 = 0x3141_5352;

/// Digest the request signature turned out to be over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlg {
    Sha256,
    Sha384,
    Sha512,
}

#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    BadKeyBlob(&'static str),
    UnsupportedKey(u32),
    BadSignatureEncoding,
    Cng(&'static str, i32),
    Mismatch,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadKeyBlob(why) => write!(f, "bad key blob: {why}"),
            Self::UnsupportedKey(magic) => write!(f, "unsupported key magic 0x{magic:08X}"),
            Self::BadSignatureEncoding => {
                write!(f, "signature length is not 2 * the key size (raw r||s)")
            }
            Self::Cng(call, status) => {
                write!(f, "{call} failed with NTSTATUS 0x{:08X}", *status as u32)
            }
            Self::Mismatch => write!(
                f,
                "signature does not match SHA-256, SHA-384 or SHA-512 of the message"
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Curve name for diagnostics, from the blob's magic (`None` if it is not a known public blob).
pub fn curve_name(blob: &[u8]) -> Option<&'static str> {
    match u32::from_le_bytes(blob.get(..4)?.try_into().ok()?) {
        ECS1_MAGIC => Some("ECDSA P-256"),
        ECS3_MAGIC => Some("ECDSA P-384"),
        ECS5_MAGIC => Some("ECDSA P-521"),
        RSA1_MAGIC => Some("RSA"),
        _ => None,
    }
}

fn curve(blob: &[u8]) -> Result<(PCWSTR, usize), VerifyError> {
    let header = blob
        .get(..8)
        .ok_or(VerifyError::BadKeyBlob("shorter than 8 bytes"))?;
    let magic = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let cb_key = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as usize;
    let (alg, expected) = match magic {
        ECS1_MAGIC => (BCRYPT_ECDSA_P256_ALGORITHM, 32),
        ECS3_MAGIC => (BCRYPT_ECDSA_P384_ALGORITHM, 48),
        ECS5_MAGIC => (BCRYPT_ECDSA_P521_ALGORITHM, 66),
        other => return Err(VerifyError::UnsupportedKey(other)),
    };
    if cb_key != expected || blob.len() != 8 + 2 * cb_key {
        return Err(VerifyError::BadKeyBlob("size does not match the curve"));
    }
    Ok((alg, cb_key))
}

/// Verify `signature` (raw `r || s`) over `message`, the encoded request, with the blob's key. Returns the
/// digest algorithm that matched.
pub fn verify_request(
    key_blob: &[u8],
    message: &[u8],
    signature: &[u8],
) -> Result<HashAlg, VerifyError> {
    let (alg_id, cb_key) = curve(key_blob)?;
    if signature.len() != 2 * cb_key {
        return Err(VerifyError::BadSignatureEncoding);
    }
    let digests: [(HashAlg, Vec<u8>); 3] = [
        (HashAlg::Sha256, Sha256::digest(message).to_vec()),
        (HashAlg::Sha384, Sha384::digest(message).to_vec()),
        (HashAlg::Sha512, Sha512::digest(message).to_vec()),
    ];
    let mut alg = BCRYPT_ALG_HANDLE::default();
    // SAFETY: plain CNG calls over local handles, all released before returning.
    unsafe {
        let status = BCryptOpenAlgorithmProvider(
            &mut alg,
            alg_id,
            PCWSTR::null(),
            BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
        );
        if status.is_err() {
            return Err(VerifyError::Cng("BCryptOpenAlgorithmProvider", status.0));
        }
        let mut key = BCRYPT_KEY_HANDLE::default();
        let status = BCryptImportKeyPair(alg, None, BCRYPT_ECCPUBLIC_BLOB, &mut key, key_blob, 0);
        if status.is_err() {
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            return Err(VerifyError::Cng("BCryptImportKeyPair", status.0));
        }
        let mut found = None;
        for (hash, digest) in &digests {
            if BCryptVerifySignature(key, None, digest, signature, BCRYPT_FLAGS(0)).is_ok() {
                found = Some(*hash);
                break;
            }
        }
        let _ = BCryptDestroyKey(key);
        let _ = BCryptCloseAlgorithmProvider(alg, 0);
        found.ok_or(VerifyError::Mismatch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Security::Cryptography::{
        BCryptExportKey, BCryptFinalizeKeyPair, BCryptGenerateKeyPair, BCryptSignHash,
    };

    /// Generate a CNG key, export its public blob and sign `digest` (raw r||s), like Windows does.
    fn cng_sign(alg_id: PCWSTR, bits: u32, digest: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut alg = BCRYPT_ALG_HANDLE::default();
        let mut key = BCRYPT_KEY_HANDLE::default();
        let flags = BCRYPT_FLAGS(0);
        // SAFETY: CNG calls over locals; sizes are queried before the buffers are filled.
        unsafe {
            let open = BCryptOpenAlgorithmProvider(
                &mut alg,
                alg_id,
                PCWSTR::null(),
                BCRYPT_OPEN_ALGORITHM_PROVIDER_FLAGS(0),
            );
            assert!(open.is_ok());
            assert!(BCryptGenerateKeyPair(alg, &mut key, bits, 0).is_ok());
            assert!(BCryptFinalizeKeyPair(key, 0).is_ok());
            let mut len = 0u32;
            assert!(BCryptExportKey(key, None, BCRYPT_ECCPUBLIC_BLOB, None, &mut len, 0).is_ok());
            let mut blob = vec![0u8; len as usize];
            let export = BCryptExportKey(
                key,
                None,
                BCRYPT_ECCPUBLIC_BLOB,
                Some(&mut blob),
                &mut len,
                0,
            );
            assert!(export.is_ok());
            let mut sig_len = 0u32;
            assert!(BCryptSignHash(key, None, digest, None, &mut sig_len, flags).is_ok());
            let mut sig = vec![0u8; sig_len as usize];
            let signed = BCryptSignHash(key, None, digest, Some(&mut sig), &mut sig_len, flags);
            assert!(signed.is_ok());
            sig.truncate(sig_len as usize);
            let _ = BCryptDestroyKey(key);
            let _ = BCryptCloseAlgorithmProvider(alg, 0);
            (blob, sig)
        }
    }

    #[test]
    fn cng_signed_requests_verify_on_every_curve() {
        let msg = b"encoded request bytes";
        let cases: [(PCWSTR, u32, usize, u32, HashAlg, Vec<u8>); 4] = [
            (
                BCRYPT_ECDSA_P256_ALGORITHM,
                256,
                72,
                ECS1_MAGIC,
                HashAlg::Sha256,
                Sha256::digest(msg).to_vec(),
            ),
            (
                BCRYPT_ECDSA_P384_ALGORITHM,
                384,
                104,
                ECS3_MAGIC,
                HashAlg::Sha384,
                Sha384::digest(msg).to_vec(),
            ),
            (
                BCRYPT_ECDSA_P384_ALGORITHM,
                384,
                104,
                ECS3_MAGIC,
                HashAlg::Sha256,
                Sha256::digest(msg).to_vec(),
            ),
            (
                BCRYPT_ECDSA_P521_ALGORITHM,
                521,
                140,
                ECS5_MAGIC,
                HashAlg::Sha512,
                Sha512::digest(msg).to_vec(),
            ),
        ];
        for (alg, bits, blob_len, magic, hash, digest) in cases {
            let (blob, sig) = cng_sign(alg, bits, &digest);
            assert_eq!(blob.len(), blob_len);
            assert_eq!(u32::from_le_bytes(blob[..4].try_into().unwrap()), magic);
            assert_eq!(verify_request(&blob, msg, &sig), Ok(hash), "bits {bits}");
            assert_eq!(
                verify_request(&blob, b"other", &sig),
                Err(VerifyError::Mismatch)
            );
        }
    }

    #[test]
    fn a_p256_signature_from_another_library_verifies() {
        use p256::ecdsa::SigningKey;
        use p256::ecdsa::signature::Signer;
        let sk = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let point = sk.verifying_key().to_sec1_point(false);
        let mut blob = Vec::new();
        blob.extend_from_slice(&ECS1_MAGIC.to_le_bytes());
        blob.extend_from_slice(&32u32.to_le_bytes());
        blob.extend_from_slice(&point.as_bytes()[1..]);
        let sig: p256::ecdsa::Signature = sk.sign(b"request");
        assert_eq!(
            verify_request(&blob, b"request", &sig.to_bytes()),
            Ok(HashAlg::Sha256)
        );
        assert_eq!(
            verify_request(&blob, b"requesT", &sig.to_bytes()),
            Err(VerifyError::Mismatch)
        );
    }

    #[test]
    fn bad_inputs_are_refused() {
        let mut rsa = RSA1_MAGIC.to_le_bytes().to_vec();
        rsa.extend_from_slice(&[0; 300]);
        assert_eq!(
            verify_request(&rsa, b"x", &[0; 64]),
            Err(VerifyError::UnsupportedKey(RSA1_MAGIC))
        );
        assert!(matches!(
            verify_request(&[1, 2], b"x", &[0; 64]),
            Err(VerifyError::BadKeyBlob(_))
        ));
        let (blob, _) = cng_sign(BCRYPT_ECDSA_P384_ALGORITHM, 384, &Sha384::digest(b"x"));
        assert_eq!(
            verify_request(&blob, b"x", &[1, 2, 3]),
            Err(VerifyError::BadSignatureEncoding)
        );
        assert_eq!(curve_name(&blob), Some("ECDSA P-384"));
    }
}
