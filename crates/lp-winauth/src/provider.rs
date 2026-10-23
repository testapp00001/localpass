//! The provider process: the real OS pieces wired to the [`Handler`], and the registration calls the
//! command-line modes make. Everything here needs the real `webauthn.dll`; the identity-bound parts
//! (operation-signing key, credential cache, add/update/remove authenticator) also need package identity,
//! so they only work in the packaged process (start it through the execution alias).

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use windows::core::{GUID, HRESULT};

use crate::cache::{self, OsCache, SyncOutcome};
use crate::handler::{CacheSink, Handler, OpVerifier};
use crate::log::hr;
use crate::plog;
use crate::register::{authenticator_info, logo_base64};
use crate::relay::{RealRelay, RelayConfig};
use crate::store::{self, Store};
use crate::sys::{AuthenticatorDetails, CredentialRow, NTE_NOT_FOUND, WebAuthn};
use crate::verify::verify_request;

/// The plugin's COM class, generated once for the real provider (the spike had its own). It must match
/// `Class Id` in `package/AppxManifest.xml` (the unit test `the_shipped_manifest_names_this_class` checks
/// it; the build scripts also read it from `lp-winauth-provider clsid`).
#[cfg(not(feature = "test-identity"))]
pub const CLSID: GUID = GUID::from_u128(0x91e9966b_58ae_455a_ab60_4b2fdb66a730);
#[cfg(not(feature = "test-identity"))]
pub const CLSID_TEXT: &str = "{91E9966B-58AE-455A-AB60-4B2FDB66A730}";
/// Feature `test-identity` builds a provider that can be registered NEXT TO the real one without touching
/// it: another class id, authenticator name, data folder (`store.rs`) and registry key. Only for the
/// real-OS check of the app's set-up code (notes, "Phase 4.5"); never shipped.
#[cfg(feature = "test-identity")]
pub const CLSID: GUID = GUID::from_u128(0x5c1d0a7e_3b94_4f1a_8e62_7d0b9a4c2f18);
#[cfg(feature = "test-identity")]
pub const CLSID_TEXT: &str = "{5C1D0A7E-3B94-4F1A-8E62-7D0B9A4C2F18}";
/// What Windows' passkey settings list.
#[cfg(not(feature = "test-identity"))]
pub const AUTHENTICATOR_NAME: &str = "LocalPass";
#[cfg(feature = "test-identity")]
pub const AUTHENTICATOR_NAME: &str = "LocalPass (test)";
/// `pwszPluginRpId` must not be NULL (measured in the spike); LocalPass makes no nested WebAuthn calls,
/// so a name under the reserved `.invalid` TLD.
pub const PLUGIN_RP_ID: &str = "localpass.invalid";

/// Loads `webauthn.dll` once for the process.
pub fn load_api() -> Result<&'static WebAuthn, String> {
    static API: OnceLock<Result<WebAuthn, String>> = OnceLock::new();
    API.get_or_init(|| WebAuthn::load().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(Clone::clone)
}

/// Checks request signatures against the key Windows holds for this authenticator.
pub struct OsVerifier {
    pub api: &'static WebAuthn,
    pub clsid: GUID,
}

impl OpVerifier for OsVerifier {
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<String, String> {
        // Fetched for every call: registering again (remove + add) rotates the key.
        let key = self
            .api
            .operation_signing_public_key(&self.clsid)
            .map_err(|code| format!("GetOperationSigningPublicKey hr={}", hr(code)))?;
        verify_request(key.as_slice(), message, signature)
            .map(|digest| format!("{digest:?}"))
            .map_err(|e| e.to_string())
    }
}

/// Adds a freshly created credential to Windows' cache.
pub struct OsCacheSink {
    pub api: &'static WebAuthn,
    pub clsid: GUID,
}

impl CacheSink for OsCacheSink {
    fn credential_created(&self, row: &CredentialRow) {
        let cache = OsCache {
            api: self.api,
            clsid: self.clsid,
        };
        match cache::add_one(&cache, row) {
            Ok(()) => plog!(
                "credential cache: added {}",
                crate::log::short_id(&row.credential_id)
            ),
            Err(code) => plog!("credential cache: add failed hr={}", hr(code)),
        }
    }
}

/// The running provider: the handler plus what the command-line modes need.
pub struct Provider {
    pub api: &'static WebAuthn,
    pub store: Store,
    pub relay: Arc<RealRelay>,
    pub handler: Handler,
}

impl Provider {
    /// The provider for this PC: pipe profile from `profile.txt` and the app path from the HKCU value.
    /// Only the binary uses this; tests build a [`Provider`] from explicit parts.
    pub fn open(store: Store) -> Result<Self, String> {
        let profile = store.read_profile();
        // A development profile talks to a throwaway host: never start the real app for it.
        let app_path = if profile.is_some() {
            None
        } else {
            store::app_path_from_registry()
        };
        Self::with_config(store, RelayConfig::new(profile, app_path))
    }

    pub fn with_config(store: Store, config: RelayConfig) -> Result<Self, String> {
        let api = load_api()?;
        let relay = Arc::new(RealRelay::new(store.clone(), config).map_err(|e| e.to_string())?);
        let handler = Handler::new(
            api,
            relay.clone(),
            Arc::new(OsVerifier { api, clsid: CLSID }),
            Arc::new(OsCacheSink { api, clsid: CLSID }),
        );
        Ok(Self {
            api,
            store,
            relay,
            handler,
        })
    }

    /// Mirrors the vault's passkeys into Windows' credential cache. Never starts the app, never prompts;
    /// when the app cannot list them nothing changes.
    pub fn sync_cache(&self) -> SyncOutcome {
        cache::sync(
            self.relay.as_ref(),
            &OsCache {
                api: self.api,
                clsid: CLSID,
            },
        )
    }
}

/// What `register` did.
#[derive(Debug, PartialEq, Eq)]
pub enum Registered {
    /// New registration; the curve of the operation-signing key Windows made for it.
    Added { op_sign_key: String },
    /// Already registered: name, logos and info were updated, the key kept.
    Updated,
}

/// Registers the authenticator with Windows (disabled until the user enables it in Settings), or updates
/// its details when it is already registered. Needs package identity. Never enables it.
pub fn register(api: &WebAuthn) -> Result<Registered, HRESULT> {
    let info = authenticator_info([0u8; 16]);
    let (light, dark) = (logo_base64(false), logo_base64(true));
    let details = AuthenticatorDetails {
        name: AUTHENTICATOR_NAME,
        clsid: CLSID,
        plugin_rp_id: Some(PLUGIN_RP_ID),
        light_logo_svg: Some(&light),
        dark_logo_svg: Some(&dark),
        authenticator_info: &info,
        supported_rp_ids: &[],
    };
    match api.authenticator_state(&CLSID) {
        Err(code) if code == NTE_NOT_FOUND => {
            let response = api.add_authenticator(&details)?;
            let curve = crate::verify::curve_name(response.op_sign_public_key())
                .unwrap_or("unknown")
                .to_owned();
            Ok(Registered::Added { op_sign_key: curve })
        }
        Ok(_) => {
            // The same CLSID keeps the operation-signing key.
            api.update_authenticator(&details, &CLSID)?;
            Ok(Registered::Updated)
        }
        Err(code) => Err(code),
    }
}

/// Removes the registration (call before removing the package). Not registered counts as done.
pub fn unregister(api: &WebAuthn) -> Result<(), HRESULT> {
    match api.authenticator_state(&CLSID) {
        Err(code) if code == NTE_NOT_FOUND => Ok(()),
        _ => api.remove_authenticator(&CLSID),
    }
}

/// Where the data lives, for `status` output.
pub fn data_dir() -> PathBuf {
    Store::default_dir()
}

#[cfg(all(test, not(feature = "test-identity")))]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_manifest_names_this_class() {
        let manifest = include_str!("../package/AppxManifest.xml");
        let bare = CLSID_TEXT.trim_matches(['{', '}']);
        assert!(
            manifest.contains(&format!("<com:Class Id=\"{bare}\"")),
            "package/AppxManifest.xml must contain the class id {bare}"
        );
        assert_eq!(format!("{CLSID:?}").to_uppercase(), bare);
        assert!(
            !manifest.contains('@'),
            "no placeholder may be left in the shipped manifest"
        );
    }
}
