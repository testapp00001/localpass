//! The device secret (DS) protects the network keys so background sync can
//! run while the vault is locked. It never unlocks vault data.
//!
//! * Windows: a file encrypted with DPAPI (current user).
//! * Android: the app passes a secret kept in the Android Keystore
//!   ([`ProvidedSecret`]).
//! * Elsewhere (development only): a plain file readable by the user.

use std::path::{Path, PathBuf};

use lp_core::crypto::Key32;
use zeroize::Zeroizing;

use crate::{EngineError, Result};

const DEVICE_SECRET_PURPOSE: &[u8] = b"localpass-device-secret-v1";

/// Encrypts `data` so only this Windows user can read it back (DPAPI). `purpose` is mixed in so
/// a blob made for one use can't be opened as another. Elsewhere (development only) the data is
/// returned unchanged.
pub fn protect_local(purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
    platform::protect(purpose, data)
}

/// Reverses [`protect_local`].
pub fn unprotect_local(purpose: &[u8], blob: &[u8]) -> Result<Vec<u8>> {
    platform::unprotect(purpose, blob)
}

pub trait DeviceSecretStore: Send + Sync {
    /// Loads the device secret, creating and persisting one on first use.
    fn load_or_create(&self) -> Result<Key32>;
    /// Forgets the secret (used when the device leaves its vault).
    fn delete(&self) -> Result<()>;
}

/// A secret managed outside the engine (e.g. Android Keystore).
pub struct ProvidedSecret(pub Key32);

impl DeviceSecretStore for ProvidedSecret {
    fn load_or_create(&self) -> Result<Key32> {
        Ok(self.0.clone())
    }

    fn delete(&self) -> Result<()> {
        Ok(())
    }
}

/// A secret stored in a file, protected with DPAPI on Windows.
pub struct FileSecretStore {
    path: PathBuf,
}

impl FileSecretStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DeviceSecretStore for FileSecretStore {
    fn load_or_create(&self) -> Result<Key32> {
        match std::fs::read(&self.path) {
            Ok(blob) => {
                let plain = Zeroizing::new(platform::unprotect(DEVICE_SECRET_PURPOSE, &blob)?);
                Key32::from_slice(&plain)
                    .map_err(|_| EngineError::Secret("device secret file is corrupt".into()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Key32::random()?;
                let blob = platform::protect(DEVICE_SECRET_PURPOSE, key.as_bytes())?;
                if let Some(dir) = self.path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let tmp = self.path.with_extension("tmp");
                std::fs::write(&tmp, blob)?;
                std::fs::rename(&tmp, &self.path)?;
                Ok(key)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self) -> Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };

    use crate::{EngineError, Result};

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        }
    }

    /// Copies DPAPI's output and frees it.
    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        // SAFETY: on success DPAPI returns a LocalAlloc'd buffer of cbData bytes.
        let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
        unsafe { LocalFree(out.pbData as _) };
        bytes
    }

    pub fn protect(purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(purpose);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: all pointers reference live buffers for the duration of the call.
        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(EngineError::Secret(format!(
                "DPAPI protect failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(unsafe { take(out) })
    }

    pub fn unprotect(purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let input = blob(data);
        let entropy = blob(purpose);
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        // SAFETY: as above.
        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                &entropy,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 {
            return Err(EngineError::Secret(format!(
                "DPAPI unprotect failed (was the Windows profile reset?): {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(unsafe { take(out) })
    }
}

#[cfg(not(windows))]
mod platform {
    use crate::Result;

    pub fn protect(_purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }

    pub fn unprotect(_purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        Ok(data.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protect_local_round_trips_and_binds_to_its_purpose() {
        let blob = protect_local(b"purpose-a", b"secret bytes").unwrap();
        assert_eq!(
            unprotect_local(b"purpose-a", &blob).unwrap(),
            b"secret bytes"
        );
        if cfg!(windows) {
            assert!(!blob.windows(12).any(|w| w == b"secret bytes"));
            assert!(unprotect_local(b"purpose-b", &blob).is_err());
        }
    }

    #[test]
    fn file_secret_is_stable_and_protected() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileSecretStore::new(dir.path().join("device.key"));
        let a = store.load_or_create().unwrap();
        let b = store.load_or_create().unwrap();
        assert_eq!(a, b);
        let raw = std::fs::read(store.path()).unwrap();
        if cfg!(windows) {
            assert!(
                !raw.windows(32).any(|w| w == a.as_bytes()),
                "DPAPI must not store the key in clear"
            );
        }
        store.delete().unwrap();
        assert_ne!(store.load_or_create().unwrap(), a);
    }
}
