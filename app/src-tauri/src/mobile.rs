//! Bridge to the Kotlin `LpMobilePlugin` (sensitive clipboard, biometrics).

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, PluginHandle, TauriPlugin};
use tauri::{Manager, Runtime};

pub struct MobileBridge<R: Runtime>(PluginHandle<R>);

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("lp-mobile")
        .setup(|app, api| {
            let handle = api.register_android_plugin("app.localpass", "LpMobilePlugin")?;
            app.manage(MobileBridge(handle));
            Ok(())
        })
        .build()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CopyArgs<'a> {
    text: &'a str,
    clear_after_ms: u64,
}

#[derive(Deserialize)]
struct Available {
    available: bool,
}

#[derive(Serialize)]
struct Target<'a> {
    target: &'a str,
}

#[derive(Serialize, Deserialize)]
struct Blob {
    blob: String,
}

#[derive(Serialize, Deserialize)]
struct Secret {
    secret: String,
}

impl<R: Runtime> MobileBridge<R> {
    pub fn copy_sensitive(&self, text: &str, clear_after_ms: u64) -> Result<(), String> {
        self.0
            .run_mobile_plugin::<serde_json::Value>(
                "copySensitive",
                CopyArgs {
                    text,
                    clear_after_ms,
                },
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Where autofill / passkey provider / local-network set-up stands (see `LpMobilePlugin.setupStatus`).
    pub fn setup_status(&self) -> Result<crate::commands::AndroidSetup, String> {
        self.0
            .run_mobile_plugin::<crate::commands::AndroidSetup>("setupStatus", ())
            .map_err(|e| e.to_string())
    }

    /// Opens a system settings page: "autofill", "credentials" or "app".
    pub fn open_setup(&self, target: &str) -> Result<(), String> {
        self.0
            .run_mobile_plugin::<serde_json::Value>("openSetup", Target { target })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn biometric_available(&self) -> bool {
        self.0
            .run_mobile_plugin::<Available>("biometricStatus", ())
            .map(|a| a.available)
            .unwrap_or(false)
    }

    /// Prompts for biometrics and returns the Keystore-encrypted form of `secret` (base64).
    pub fn biometric_enroll(&self, secret_b64: String) -> Result<String, String> {
        self.0
            .run_mobile_plugin::<Blob>("biometricEnroll", Secret { secret: secret_b64 })
            .map(|b| b.blob)
            .map_err(|e| e.to_string())
    }

    /// Prompts for biometrics and returns the decrypted secret (base64).
    pub fn biometric_decrypt(&self, blob_b64: String) -> Result<String, String> {
        self.0
            .run_mobile_plugin::<Secret>("biometricDecrypt", Blob { blob: blob_b64 })
            .map(|s| s.secret)
            .map_err(|e| e.to_string())
    }
}
