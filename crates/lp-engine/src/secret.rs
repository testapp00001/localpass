//! The device secret (DS) protects the network keys so background sync can
//! run while the vault is locked. It never unlocks vault data.
//!
//! * Windows: a file encrypted with DPAPI (current user).
//! * Android: the app passes a secret kept in the Android Keystore
//!   ([`ProvidedSecret`]).
//! * Linux (the app, with the `keyring` feature): a file sealed with a key that lives in the
//!   desktop's Secret Service (GNOME Keyring, KWallet) and nowhere else ([`KeyedProtector`]).
//! * Elsewhere (development, tests, `lp-cli`): a plain file readable by the user.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// Protects a small blob at rest so that only this user on this machine can read it back. `purpose`
/// is mixed in so a blob made for one use cannot be opened as another.
pub trait LocalProtect: Send + Sync {
    fn protect(&self, purpose: &[u8], data: &[u8]) -> Result<Vec<u8>>;
    fn unprotect(&self, purpose: &[u8], blob: &[u8]) -> Result<Vec<u8>>;
}

/// The platform default: DPAPI on Windows, the bytes unchanged elsewhere. What [`protect_local`]
/// and [`FileSecretStore::new`] use, so tests and `lp-cli` never touch a keyring.
pub struct PlatformProtect;

impl LocalProtect for PlatformProtect {
    fn protect(&self, purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        platform::protect(purpose, data)
    }

    fn unprotect(&self, purpose: &[u8], blob: &[u8]) -> Result<Vec<u8>> {
        platform::unprotect(purpose, blob)
    }
}

pub fn platform_protector() -> Arc<dyn LocalProtect> {
    Arc::new(PlatformProtect)
}

/// Where a [`KeyedProtector`] gets its key. The key is created on first use and then returned
/// unchanged; a source that cannot reach its store fails instead of inventing a new key.
pub trait LocalKeySource: Send + Sync {
    fn local_key(&self) -> Result<Key32>;
}

/// First bytes of a blob made by [`KeyedProtector`], so another format is refused by name.
const KEYED_MAGIC: &[u8; 4] = b"LPL1";

/// `"LPL1" ‖ XChaCha20-Poly1305(data, aad = purpose)` under a key kept by a [`LocalKeySource`].
/// The key is read once and kept in memory (wiped on drop).
pub struct KeyedProtector<S> {
    source: S,
    key: Mutex<Option<Key32>>,
}

impl<S: LocalKeySource> KeyedProtector<S> {
    pub fn new(source: S) -> Self {
        Self {
            source,
            key: Mutex::new(None),
        }
    }

    fn key(&self) -> Result<Key32> {
        let mut cached = self.key.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(key) = cached.as_ref() {
            return Ok(key.clone());
        }
        let key = self.source.local_key()?;
        *cached = Some(key.clone());
        Ok(key)
    }
}

impl<S: LocalKeySource> LocalProtect for KeyedProtector<S> {
    fn protect(&self, purpose: &[u8], data: &[u8]) -> Result<Vec<u8>> {
        let sealed = lp_core::crypto::seal(&self.key()?, purpose, data)
            .map_err(|_| EngineError::Secret("could not protect local data".into()))?;
        let mut out = Vec::with_capacity(KEYED_MAGIC.len() + sealed.len());
        out.extend_from_slice(KEYED_MAGIC);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    fn unprotect(&self, purpose: &[u8], blob: &[u8]) -> Result<Vec<u8>> {
        let Some(sealed) = blob.strip_prefix(KEYED_MAGIC.as_slice()) else {
            return Err(EngineError::Secret(
                "local data was not protected by the system keyring (made by an older test build? remove it and set up again)"
                    .into(),
            ));
        };
        lp_core::crypto::open(&self.key()?, purpose, sealed)
            .map(|plain| plain.to_vec())
            .map_err(|_| {
                EngineError::Secret(
                    "local data cannot be opened with the system keyring's key (the keyring entry was replaced, or the file is damaged)"
                        .into(),
                )
            })
    }
}

/// The local key in the desktop's Secret Service (GNOME Keyring, KWallet), one entry per profile.
#[cfg(all(target_os = "linux", feature = "keyring"))]
pub struct SecretServiceKeys {
    profile: String,
    /// Files this key protects. While one exists, a missing keyring entry is an error, not a reason to make a new key.
    guarded: Vec<PathBuf>,
}

#[cfg(all(target_os = "linux", feature = "keyring"))]
impl SecretServiceKeys {
    /// `profile` is `LOCALPASS_PROFILE`; `None` is the normal install.
    pub fn new(profile: Option<&str>) -> Self {
        Self {
            profile: profile.unwrap_or("default").to_string(),
            guarded: Vec::new(),
        }
    }

    /// Never creates a key while any of `files` exists: they were sealed with a key the keyring has lost, and a new
    /// one could not open them (the app would be stuck, with a spare entry left behind).
    pub fn guarding(mut self, files: impl IntoIterator<Item = PathBuf>) -> Self {
        self.guarded = files.into_iter().collect();
        self
    }
}

#[cfg(all(target_os = "linux", feature = "keyring"))]
impl LocalKeySource for SecretServiceKeys {
    fn local_key(&self) -> Result<Key32> {
        // The Secret Service client blocks on D-Bus replies and may wait for the user to unlock
        // the keyring. A thread of its own keeps that away from whichever (async) caller asked.
        let profile = self.profile.clone();
        let guarded = self.guarded.clone();
        std::thread::Builder::new()
            .name("lp-keyring".into())
            .spawn(move || keyring_backend::fetch_or_create(&profile, &guarded))
            .map_err(|e| EngineError::Secret(format!("could not reach the keyring: {e}")))?
            .join()
            .map_err(|_| EngineError::Secret("the keyring helper stopped unexpectedly".into()))?
    }
}

#[cfg(all(target_os = "linux", feature = "keyring"))]
mod keyring_backend {
    use std::collections::HashMap;

    use ::secret_service::blocking::{Item, SecretService};
    use ::secret_service::{EncryptionType, Error as SsError};
    use lp_core::crypto::Key32;
    use zeroize::Zeroizing;

    use crate::{EngineError, Result};

    const APPLICATION: &str = "app.localpass";
    const KIND: &str = "local-key";
    const LABEL: &str = "LocalPass local data key";

    fn unavailable(e: SsError) -> EngineError {
        EngineError::Secret(match e {
            SsError::Prompt | SsError::PromptDisconnected => {
                "the system keyring was not unlocked; LocalPass needs it to protect its local data"
                    .to_string()
            }
            e => format!(
                "the system keyring (Secret Service, e.g. GNOME Keyring) is not available: {e}"
            ),
        })
    }

    fn attributes(profile: &str) -> HashMap<&str, &str> {
        HashMap::from([
            ("application", APPLICATION),
            ("kind", KIND),
            ("profile", profile),
        ])
    }

    /// The oldest entry, so two starts that raced to create one agree on the same key.
    fn oldest<'a>(items: Vec<Item<'a>>) -> Option<Item<'a>> {
        items.into_iter().min_by_key(|item| {
            (
                item.get_created().unwrap_or(u64::MAX),
                item.item_path.to_string(),
            )
        })
    }

    fn read(item: &Item<'_>) -> Result<Key32> {
        if item.is_locked().map_err(unavailable)? {
            item.unlock().map_err(unavailable)?;
        }
        let secret = Zeroizing::new(item.get_secret().map_err(unavailable)?);
        // An entry that is not a key is never replaced: the data it protected would be lost.
        Key32::from_slice(&secret).map_err(|_| {
            EngineError::Secret(
                "the keyring entry for LocalPass is damaged; remove it in Passwords and Keys (search for \"LocalPass local data key\") only if you can set the vault up again"
                    .into(),
            )
        })
    }

    pub fn fetch_or_create(profile: &str, guarded: &[std::path::PathBuf]) -> Result<Key32> {
        let service = SecretService::connect(EncryptionType::Dh).map_err(unavailable)?;
        let collection = service.get_default_collection().map_err(unavailable)?;
        if collection.is_locked().map_err(unavailable)? {
            collection.unlock().map_err(unavailable)?;
        }
        let find = || {
            collection
                .search_items(attributes(profile))
                .map_err(unavailable)
        };
        if let Some(item) = oldest(find()?) {
            return read(&item);
        }
        if guarded.iter().any(|path| path.exists()) {
            return Err(EngineError::Secret(
                "LocalPass's key is missing from the system keyring, but local data sealed with it exists; \
                 restore the keyring (the \"LocalPass local data key\" entry) or remove the profile's data and set up again"
                    .into(),
            ));
        }
        let key = Key32::random().map_err(|e| EngineError::Secret(e.to_string()))?;
        collection
            .create_item(
                LABEL,
                attributes(profile),
                key.as_bytes(),
                false,
                "application/octet-stream",
            )
            .map_err(unavailable)?;
        // Read back what the service now holds, which also settles a race with another start.
        let stored = oldest(find()?)
            .ok_or_else(|| EngineError::Secret("the keyring did not keep the new entry".into()))?;
        read(&stored)
    }

    /// Removes a profile's entry (tests only).
    #[cfg(test)]
    pub fn delete(profile: &str) {
        let service = SecretService::connect(EncryptionType::Dh).expect("secret service");
        let collection = service.get_default_collection().expect("collection");
        for item in collection
            .search_items(attributes(profile))
            .expect("search")
        {
            item.delete().expect("delete");
        }
    }
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

/// A secret stored in a file, protected by a [`LocalProtect`] (DPAPI on Windows by default).
pub struct FileSecretStore {
    path: PathBuf,
    protector: Arc<dyn LocalProtect>,
}

impl FileSecretStore {
    /// Uses the platform default protection (see [`PlatformProtect`]).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self::with_protector(path, platform_protector())
    }

    pub fn with_protector(path: impl Into<PathBuf>, protector: Arc<dyn LocalProtect>) -> Self {
        Self {
            path: path.into(),
            protector,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl DeviceSecretStore for FileSecretStore {
    fn load_or_create(&self) -> Result<Key32> {
        match std::fs::read(&self.path) {
            Ok(blob) => {
                let plain = Zeroizing::new(self.protector.unprotect(DEVICE_SECRET_PURPOSE, &blob)?);
                Key32::from_slice(&plain)
                    .map_err(|_| EngineError::Secret("device secret file is corrupt".into()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = Key32::random()?;
                let blob = self
                    .protector
                    .protect(DEVICE_SECRET_PURPOSE, key.as_bytes())?;
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

    /// A key source that counts how often it is asked.
    struct CountingKey {
        key: Key32,
        asked: std::sync::atomic::AtomicUsize,
    }

    impl CountingKey {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                key: Key32::random().unwrap(),
                asked: 0.into(),
            })
        }
    }

    impl LocalKeySource for Arc<CountingKey> {
        fn local_key(&self) -> Result<Key32> {
            self.asked.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.key.clone())
        }
    }

    #[test]
    fn keyed_protector_round_trips_and_hides_the_data() {
        let p = KeyedProtector::new(CountingKey::new());
        let blob = p.protect(b"purpose-a", b"secret bytes").unwrap();
        assert!(blob.starts_with(b"LPL1"));
        assert!(!blob.windows(12).any(|w| w == b"secret bytes"));
        assert_eq!(p.unprotect(b"purpose-a", &blob).unwrap(), b"secret bytes");
    }

    #[test]
    fn keyed_protector_binds_a_blob_to_its_purpose_and_key() {
        let source = CountingKey::new();
        let p = KeyedProtector::new(source.clone());
        let blob = p.protect(b"purpose-a", b"data").unwrap();
        assert!(p.unprotect(b"purpose-b", &blob).is_err());
        // The same key opens it from another protector (a later start of the app)...
        let again = KeyedProtector::new(source);
        assert_eq!(again.unprotect(b"purpose-a", &blob).unwrap(), b"data");
        // ...a different key does not.
        let other = KeyedProtector::new(CountingKey::new());
        assert!(other.unprotect(b"purpose-a", &blob).is_err());
    }

    #[test]
    fn keyed_protector_refuses_other_formats_and_tampering() {
        let p = KeyedProtector::new(CountingKey::new());
        // A plain blob (an earlier build) is refused by name, not read as garbage.
        let err = p
            .unprotect(b"purpose-a", b"0123456789abcdef0123456789abcdef")
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("not protected by the system keyring")
        );
        assert!(p.unprotect(b"purpose-a", b"").is_err());
        assert!(p.unprotect(b"purpose-a", b"LPL1").is_err());
        let blob = p.protect(b"purpose-a", b"data").unwrap();
        for i in 0..blob.len() {
            let mut bad = blob.clone();
            bad[i] ^= 1;
            assert!(
                p.unprotect(b"purpose-a", &bad).is_err(),
                "flipping byte {i}"
            );
        }
        assert!(p.unprotect(b"purpose-a", &blob[..blob.len() - 1]).is_err());
    }

    #[test]
    fn keyed_protector_asks_its_source_once() {
        let source = CountingKey::new();
        let p = KeyedProtector::new(source.clone());
        for _ in 0..3 {
            let blob = p.protect(b"p", b"x").unwrap();
            p.unprotect(b"p", &blob).unwrap();
        }
        assert_eq!(source.asked.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn a_failing_key_source_is_an_error_not_a_new_key() {
        struct Gone;
        impl LocalKeySource for Gone {
            fn local_key(&self) -> Result<Key32> {
                Err(EngineError::Secret("keyring is gone".into()))
            }
        }
        let p = KeyedProtector::new(Gone);
        assert!(p.protect(b"p", b"x").is_err());
        assert!(p.unprotect(b"p", b"LPL1....").is_err());
    }

    #[test]
    fn file_secret_store_uses_the_protector_it_is_given() {
        let dir = tempfile::tempdir().unwrap();
        let source = CountingKey::new();
        let store = FileSecretStore::with_protector(
            dir.path().join("device.key"),
            Arc::new(KeyedProtector::new(source.clone())),
        );
        let a = store.load_or_create().unwrap();
        let raw = std::fs::read(store.path()).unwrap();
        assert!(raw.starts_with(b"LPL1"));
        assert!(!raw.windows(32).any(|w| w == a.as_bytes()));
        // A new store over the same file and key reads the same secret back.
        let reopened = FileSecretStore::with_protector(
            dir.path().join("device.key"),
            Arc::new(KeyedProtector::new(source)),
        );
        assert_eq!(reopened.load_or_create().unwrap(), a);
        // A store with the platform default cannot read it.
        if !cfg!(windows) {
            let plain = FileSecretStore::new(dir.path().join("device.key"));
            assert_ne!(plain.load_or_create().ok(), Some(a));
        }
    }

    /// Against the real desktop keyring, with a profile of its own that the test deletes again.
    /// Run by hand: `cargo test -p lp-engine --features keyring -- --ignored secret_service`
    /// (the keyring may ask to be unlocked).
    #[cfg(all(target_os = "linux", feature = "keyring"))]
    #[test]
    #[ignore = "needs a desktop session with a Secret Service"]
    fn secret_service_keeps_one_key_per_profile() {
        let profile = format!(
            "test-{}",
            data_encoding::HEXLOWER.encode(&Key32::random().unwrap().as_bytes()[..6])
        );
        let one = SecretServiceKeys::new(Some(&profile)).local_key().unwrap();
        let again = SecretServiceKeys::new(Some(&profile)).local_key().unwrap();
        assert_eq!(one, again, "the same profile gets the same key");
        let other = format!("{profile}-b");
        let different = SecretServiceKeys::new(Some(&other)).local_key().unwrap();
        assert_ne!(one, different, "another profile gets its own key");
        // Used from inside a tokio runtime too, which is where the app calls it.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let inside =
            runtime.block_on(async { SecretServiceKeys::new(Some(&profile)).local_key().unwrap() });
        assert_eq!(one, inside);
        keyring_backend::delete(&profile);
        keyring_backend::delete(&other);
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
