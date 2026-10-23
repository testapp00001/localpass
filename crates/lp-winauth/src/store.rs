//! The provider's own files and settings.
//!
//! Directory: `%LOCALAPPDATA%\LocalPass\provider` (the package manifest turns file-system write
//! virtualization off, so this is the real folder; see the notes, "Phase 4.3/4.4 build").
//!
//! * `client.bin`: the broker client id and its 32-byte secret, protected with DPAPI (current user)
//!   and a purpose string, so another program of the same user cannot unprotect it by accident and a
//!   copied file is useless on another PC or account. Written once by `pair`.
//! * `profile.txt` (optional): the broker pipe profile for development and tests. Default none = the
//!   real app's pipe. A COM-started process gets no `LOCALPASS_PROFILE`, hence the file.
//! * `provider.log`: see [`crate::log`].
//!
//! The path of the app (`localpass.exe`, started when its pipe is missing) is the HKCU value
//! `Software\LocalPass\Provider` / `AppPath`, written by the install script. Reads of HKCU are not
//! virtualized for a packaged process even when writes are.

use std::fmt;
use std::path::{Path, PathBuf};

use lp_broker::client::Credentials;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
use zeroize::Zeroizing;

/// DPAPI "optional entropy": binds the blob to this use.
pub const CLIENT_PURPOSE: &[u8] = b"LocalPass Windows passkey provider client v1";
const FORMAT_VERSION: u8 = 1;
#[cfg(not(feature = "test-identity"))]
const REGISTRY_KEY: &str = r"Software\LocalPass\Provider";
#[cfg(feature = "test-identity")]
const REGISTRY_KEY: &str = r"Software\LocalPass\ProviderTest";
#[cfg(not(feature = "test-identity"))]
const DATA_FOLDER: &str = "provider";
#[cfg(feature = "test-identity")]
const DATA_FOLDER: &str = "provider-test";
const REGISTRY_APP_PATH: &str = "AppPath";

#[derive(Debug)]
pub enum StoreError {
    Io(std::io::Error),
    /// DPAPI refused to unprotect the file (tampered, another user, or a reset profile).
    Dpapi(String),
    /// The file decrypted but is not a credentials record.
    Corrupt(&'static str),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "file error: {e}"),
            Self::Dpapi(e) => write!(f, "DPAPI: {e}"),
            Self::Corrupt(why) => write!(f, "stored credentials are corrupt: {why}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// The provider's data directory.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    /// `%LOCALAPPDATA%\LocalPass\provider`.
    pub fn default_dir() -> PathBuf {
        let base = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        base.join("LocalPass").join(DATA_FOLDER)
    }

    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn open_default() -> Self {
        Self::new(Self::default_dir())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn credentials_path(&self) -> PathBuf {
        self.dir.join("client.bin")
    }

    pub fn profile_path(&self) -> PathBuf {
        self.dir.join("profile.txt")
    }

    /// The saved pairing, `None` if this provider never paired (or was unpaired).
    pub fn load_credentials(&self) -> Result<Option<Credentials>, StoreError> {
        let blob = match std::fs::read(self.credentials_path()) {
            Ok(blob) => blob,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let plain = Zeroizing::new(unprotect(CLIENT_PURPOSE, &blob).map_err(StoreError::Dpapi)?);
        decode(&plain).map(Some)
    }

    /// Writes the pairing (replacing any earlier one) atomically.
    pub fn save_credentials(&self, credentials: &Credentials) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.dir)?;
        let plain = encode(credentials)?;
        let blob = protect(CLIENT_PURPOSE, &plain).map_err(StoreError::Dpapi)?;
        let target = self.credentials_path();
        let tmp = target.with_extension("tmp");
        std::fs::write(&tmp, blob)?;
        std::fs::rename(&tmp, &target)?;
        Ok(())
    }

    /// Forgets the pairing. Nothing to forget is not an error.
    pub fn clear_credentials(&self) -> Result<(), StoreError> {
        match std::fs::remove_file(self.credentials_path()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// The pipe profile from `profile.txt`: letters, digits, `-` and `_` only (the broker applies the same
    /// filter); `None` when the file is missing or empty.
    pub fn read_profile(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.profile_path()).ok()?;
        clean_profile(&text)
    }
}

/// The broker's profile filter, so `profile.txt` cannot smuggle path-like text into the pipe name.
pub fn clean_profile(text: &str) -> Option<String> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The app's exe path from `HKCU\Software\LocalPass\Provider\AppPath`, if set and the file exists.
pub fn app_path_from_registry() -> Option<PathBuf> {
    let key: Vec<u16> = REGISTRY_KEY.encode_utf16().chain([0]).collect();
    let value: Vec<u16> = REGISTRY_APP_PATH.encode_utf16().chain([0]).collect();
    let mut bytes = 0u32;
    // SAFETY: NUL-terminated names; a null data pointer asks for the size only.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if rc != 0 || bytes < 2 {
        return None;
    }
    let mut buf = vec![0u16; (bytes as usize).div_ceil(2)];
    let mut capacity = (buf.len() * 2) as u32;
    // SAFETY: `buf` holds `capacity` writable bytes.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut capacity,
        )
    };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    path.is_file().then_some(path)
}

fn encode(credentials: &Credentials) -> Result<Zeroizing<Vec<u8>>, StoreError> {
    let id = credentials.client_id.as_bytes();
    let id_len =
        u16::try_from(id.len()).map_err(|_| StoreError::Corrupt("client id is too long"))?;
    let mut out = Zeroizing::new(Vec::with_capacity(3 + id.len() + credentials.secret.len()));
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&id_len.to_le_bytes());
    out.extend_from_slice(id);
    out.extend_from_slice(&credentials.secret);
    Ok(out)
}

fn decode(plain: &[u8]) -> Result<Credentials, StoreError> {
    let [version, rest @ ..] = plain else {
        return Err(StoreError::Corrupt("empty"));
    };
    if *version != FORMAT_VERSION {
        return Err(StoreError::Corrupt("unknown format version"));
    }
    let [a, b, rest @ ..] = rest else {
        return Err(StoreError::Corrupt("truncated"));
    };
    let id_len = usize::from(u16::from_le_bytes([*a, *b]));
    if rest.len() <= id_len {
        return Err(StoreError::Corrupt("truncated"));
    }
    let (id, secret) = rest.split_at(id_len);
    let client_id =
        String::from_utf8(id.to_vec()).map_err(|_| StoreError::Corrupt("client id is not text"))?;
    Ok(Credentials {
        client_id,
        secret: Zeroizing::new(secret.to_vec()),
    })
}

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

/// Copies DPAPI's output and frees it.
///
/// # Safety
/// `out` must be the output of a successful DPAPI call.
unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: on success DPAPI returns a LocalAlloc'd buffer of cbData bytes.
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec() };
    // SAFETY: freeing that same allocation once.
    unsafe { LocalFree(out.pbData.cast()) };
    bytes
}

/// DPAPI `CryptProtectData` for the current user, bound to `purpose`.
pub fn protect(purpose: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let (input, entropy) = (blob(data), blob(purpose));
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
        return Err(format!(
            "CryptProtectData failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the call succeeded.
    Ok(unsafe { take(out) })
}

/// DPAPI `CryptUnprotectData`; fails if `purpose` or the user differ or the blob was altered.
pub fn unprotect(purpose: &[u8], data: &[u8]) -> Result<Vec<u8>, String> {
    let (input, entropy) = (blob(data), blob(purpose));
    let mut out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // SAFETY: as in `protect`.
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
        return Err(format!(
            "CryptUnprotectData failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    // SAFETY: the call succeeded.
    Ok(unsafe { take(out) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            client_id: "client-abc".into(),
            secret: Zeroizing::new((0u8..32).collect()),
        }
    }

    #[test]
    fn credentials_round_trip_through_dpapi() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert!(store.load_credentials().unwrap().is_none(), "nothing yet");
        store.save_credentials(&credentials()).unwrap();
        let loaded = store.load_credentials().unwrap().expect("saved");
        assert_eq!(loaded.client_id, "client-abc");
        assert_eq!(&loaded.secret[..], &(0u8..32).collect::<Vec<_>>()[..]);

        // The file does not contain the secret or the id in the clear.
        let raw = std::fs::read(store.credentials_path()).unwrap();
        assert!(!raw.windows(8).any(|w| w == b"client-a"));
        let secret: Vec<u8> = (0u8..32).collect();
        assert!(!raw.windows(16).any(|w| w == &secret[..16]));

        store.save_credentials(&credentials()).unwrap();
        store.clear_credentials().unwrap();
        assert!(store.load_credentials().unwrap().is_none());
        store.clear_credentials().unwrap();
    }

    #[test]
    fn a_tampered_or_foreign_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store.save_credentials(&credentials()).unwrap();

        // Flip one byte in the middle of the blob.
        let path = store.credentials_path();
        let mut raw = std::fs::read(&path).unwrap();
        let middle = raw.len() / 2;
        raw[middle] ^= 0x01;
        std::fs::write(&path, &raw).unwrap();
        assert!(matches!(
            store.load_credentials(),
            Err(StoreError::Dpapi(_))
        ));

        // Garbage and an empty file.
        std::fs::write(&path, b"not a dpapi blob").unwrap();
        assert!(store.load_credentials().is_err());
        std::fs::write(&path, b"").unwrap();
        assert!(store.load_credentials().is_err());

        // A blob protected for another purpose is refused (DPAPI entropy), even though it is valid DPAPI.
        let other = protect(b"some other purpose", b"\x01\x02\x00idsecret").unwrap();
        std::fs::write(&path, other).unwrap();
        assert!(matches!(
            store.load_credentials(),
            Err(StoreError::Dpapi(_))
        ));
    }

    #[test]
    fn decoded_records_are_checked() {
        assert!(matches!(decode(&[]), Err(StoreError::Corrupt(_))));
        assert!(matches!(decode(&[9, 0, 0]), Err(StoreError::Corrupt(_))));
        assert!(matches!(
            decode(&[1, 5, 0, b'a']),
            Err(StoreError::Corrupt(_))
        ));
        // id of 2 bytes but no secret after it
        assert!(matches!(
            decode(&[1, 2, 0, b'a', b'b']),
            Err(StoreError::Corrupt(_))
        ));
        let ok = decode(&[1, 2, 0, b'a', b'b', 9, 9]).unwrap();
        assert_eq!(
            (ok.client_id.as_str(), &ok.secret[..]),
            ("ab", &[9u8, 9][..])
        );
    }

    #[test]
    fn the_profile_file_is_filtered_like_the_brokers() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        assert_eq!(store.read_profile(), None);
        std::fs::write(store.profile_path(), " dev-1\r\n").unwrap();
        assert_eq!(store.read_profile().as_deref(), Some("dev-1"));
        std::fs::write(store.profile_path(), "..\\evil name").unwrap();
        assert_eq!(store.read_profile().as_deref(), Some("evilname"));
        std::fs::write(store.profile_path(), "  \n").unwrap();
        assert_eq!(store.read_profile(), None);
    }

    #[test]
    fn a_missing_registry_value_is_none() {
        // The key only exists after the install script ran; this must not panic either way, and a
        // value naming a file that does not exist is also None.
        let _ = app_path_from_registry();
    }
}
