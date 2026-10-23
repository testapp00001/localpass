//! Encrypted backup file (`.lpbackup`).
//!
//! `"LPBACKUP" | version (1) | header length (u16 BE) | CBOR header {kdf, salt} |
//! XChaCha20-Poly1305(Argon2id(password, salt), aad = everything before)`
//! The plaintext is a Bitwarden-compatible JSON export, so a backup restores
//! through the normal import path.

use lp_core::cbor;
use lp_core::crypto::{self, KdfParams, derive_kek, random_bytes};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{EngineError, Result};

const MAGIC: &[u8; 8] = b"LPBACKUP";
const VERSION: u8 = 1;

#[derive(Serialize, Deserialize)]
struct Header {
    kdf: KdfParams,
    #[serde(with = "serde_bytes")]
    salt: Vec<u8>,
}

pub fn is_backup(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

pub fn seal(json: &str, password: &str, kdf: KdfParams) -> Result<Vec<u8>> {
    let salt: [u8; 16] = random_bytes()?;
    let header = cbor::to_vec(&Header {
        kdf,
        salt: salt.to_vec(),
    })?;
    let mut out = Vec::with_capacity(json.len() + 128);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(header.len() as u16).to_be_bytes());
    out.extend_from_slice(&header);
    let key = derive_kek(password, &salt, kdf)?;
    let sealed = crypto::seal(&key, &out, json.as_bytes())?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

pub fn open(bytes: &[u8], password: &str) -> Result<Zeroizing<Vec<u8>>> {
    let bad = || EngineError::Invalid("not a valid LocalPass backup".into());
    if !is_backup(bytes) || bytes.len() < MAGIC.len() + 3 {
        return Err(bad());
    }
    if bytes[MAGIC.len()] != VERSION {
        return Err(EngineError::Invalid(
            "backup was made by a newer LocalPass".into(),
        ));
    }
    let len_at = MAGIC.len() + 1;
    let header_len = u16::from_be_bytes([bytes[len_at], bytes[len_at + 1]]) as usize;
    let body_at = len_at + 2 + header_len;
    if bytes.len() < body_at {
        return Err(bad());
    }
    let header: Header = cbor::from_slice(&bytes[len_at + 2..body_at])?;
    let key = derive_kek(password, &header.salt, header.kdf)?;
    crypto::open(&key, &bytes[..body_at], &bytes[body_at..])
        .map_err(|_| EngineError::Invalid("wrong backup password or damaged file".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_wrong_password() {
        let kdf = KdfParams::insecure_for_tests();
        let blob = seal("{\"items\":[]}", "backup pw", kdf).unwrap();
        assert!(is_backup(&blob));
        assert_eq!(&open(&blob, "backup pw").unwrap()[..], b"{\"items\":[]}");
        assert!(open(&blob, "nope").is_err());
        let mut tampered = blob.clone();
        tampered[10] ^= 1;
        assert!(open(&tampered, "backup pw").is_err());
    }

    /// A backup file whose header asks for these key-derivation costs, otherwise valid.
    fn forged(kdf: KdfParams) -> Vec<u8> {
        let header = cbor::to_vec(&Header {
            kdf,
            salt: vec![7; 16],
        })
        .unwrap();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&(header.len() as u16).to_be_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&[0u8; 64]);
        out
    }

    /// `open` must say no, and say it quickly: the header is read before anything is authenticated.
    fn rejected_quickly(kdf: KdfParams) {
        let blob = forged(kdf);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(open(&blob, "pw").is_err());
        });
        let answer = rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap_or_else(|_| panic!("{kdf:?}: still computing after 3 s"));
        assert!(answer, "{kdf:?}: accepted");
    }

    #[test]
    fn a_backup_cannot_demand_absurd_key_derivation_costs() {
        // Time: millions of passes over a small block.
        rejected_quickly(KdfParams {
            m_kib: 64,
            t: 5_000_000,
            p: 1,
        });
        rejected_quickly(KdfParams {
            m_kib: 64,
            t: u32::MAX,
            p: 1,
        });
        // Memory: far more than any device has (these are refused before anything is allocated).
        rejected_quickly(KdfParams {
            m_kib: 4 * 1024 * 1024,
            t: 1,
            p: 1,
        });
        rejected_quickly(KdfParams {
            m_kib: u32::MAX,
            t: 1,
            p: 1,
        });
        // Lanes, and nonsense.
        rejected_quickly(KdfParams {
            m_kib: 64 * 1024,
            t: 1,
            p: 1_000_000,
        });
        rejected_quickly(KdfParams {
            m_kib: 64,
            t: 0,
            p: 1,
        });
        rejected_quickly(KdfParams {
            m_kib: 0,
            t: 1,
            p: 0,
        });
    }

    #[test]
    fn backups_with_the_normal_costs_still_open() {
        let blob = seal("{}", "pw", KdfParams::insecure_for_tests()).unwrap();
        assert!(open(&blob, "pw").is_ok());
    }
}

/// Property tests (phase 5.1): the file is chosen by the user but may have come from anywhere.
#[cfg(test)]
mod props {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use std::sync::OnceLock;

    /// 64 cases by default (each derives a key); `PROPTEST_CASES` overrides it.
    fn config() -> ProptestConfig {
        let cases = std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(64);
        ProptestConfig::with_cases(cases)
    }

    const JSON: &str = r#"{"items":[{"name":"example"}]}"#;

    /// A valid backup, made once.
    fn blob() -> &'static Vec<u8> {
        static BLOB: OnceLock<Vec<u8>> = OnceLock::new();
        BLOB.get_or_init(|| seal(JSON, "backup pw", KdfParams::insecure_for_tests()).unwrap())
    }

    /// `open` with a deadline: the header is read before anything is authenticated, so no input
    /// may make it compute for long.
    fn open_quickly(bytes: Vec<u8>, password: &'static str) -> Result<Zeroizing<Vec<u8>>> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(open(&bytes, password));
        });
        rx.recv_timeout(std::time::Duration::from_secs(10))
            .expect("open() did not come back within 10 s")
    }

    proptest! {
        #![proptest_config(config())]

        #[test]
        fn backup_open_never_panics_on_random_files(
            bytes in vec(any::<u8>(), 0..300),
            header in vec(any::<u8>(), 0..80),
            tail in vec(any::<u8>(), 0..80),
            declared in prop_oneof![Just(None), any::<u16>().prop_map(Some)],
        ) {
            prop_assert!(open(&bytes, "pw").is_err());
            // Starting like a backup: magic, version, a length (right or random), then noise.
            let mut file = MAGIC.to_vec();
            file.push(VERSION);
            file.extend_from_slice(&declared.unwrap_or(header.len() as u16).to_be_bytes());
            file.extend_from_slice(&header);
            file.extend_from_slice(&tail);
            prop_assert!(open_quickly(file, "pw").is_err());
        }

        #[test]
        fn backup_truncations_are_errors(cut in any::<prop::sample::Index>()) {
            let whole = blob();
            let shorter = &whole[..cut.index(whole.len())];
            prop_assert!(open_quickly(shorter.to_vec(), "backup pw").is_err());
        }

        #[test]
        fn backup_bit_flips_are_errors(at in any::<prop::sample::Index>(), xor in 1u8..=255) {
            let mut file = blob().clone();
            let i = at.index(file.len());
            file[i] ^= xor;
            prop_assert!(open_quickly(file, "backup pw").is_err(), "a change at byte {} was accepted", i);
        }

        #[test]
        fn backup_appended_bytes_are_errors(extra in vec(any::<u8>(), 1..16)) {
            let mut file = blob().clone();
            file.extend_from_slice(&extra);
            prop_assert!(open_quickly(file, "backup pw").is_err());
        }

        /// V15: any key-derivation cost outside the allowed range is refused, quickly, whatever
        /// the file says.
        #[test]
        fn backup_rejects_absurd_kdf_params(m_kib in any::<u32>(), t in any::<u32>(), p in any::<u32>()) {
            let kdf = KdfParams { m_kib, t, p };
            prop_assume!(kdf.validate().is_err());
            let header = cbor::to_vec(&Header { kdf, salt: vec![7; 16] }).unwrap();
            let mut file = MAGIC.to_vec();
            file.push(VERSION);
            file.extend_from_slice(&(header.len() as u16).to_be_bytes());
            file.extend_from_slice(&header);
            file.extend_from_slice(&[0u8; 64]);
            prop_assert!(open_quickly(file, "pw").is_err());
        }

        #[test]
        fn backup_round_trips_any_text_and_password(json in any::<String>(), password in "[ -~]{1,40}") {
            let sealed = seal(&json, &password, KdfParams::insecure_for_tests()).unwrap();
            prop_assert!(is_backup(&sealed));
            prop_assert_eq!(&open(&sealed, &password).unwrap()[..], json.as_bytes());
            let wrong = format!("{password}!");
            prop_assert!(open(&sealed, &wrong).is_err());
        }
    }
}
