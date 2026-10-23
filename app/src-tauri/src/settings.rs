//! UI/app preferences, stored per device as JSON next to the vault.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Lock after this many minutes without activity (0 = never).
    pub auto_lock_minutes: u32,
    /// Clear copied secrets after this many seconds (0 = never).
    pub clipboard_clear_seconds: u32,
    /// Closing the window keeps LocalPass running in the tray so sync continues.
    pub close_to_tray: bool,
    pub start_at_login: bool,
    /// "system", "light" or "dark".
    pub theme: String,
    /// Ask Windows Hello for every passkey sign-in, even if the website says it doesn't
    /// require a user check.
    pub passkey_verify_every_time: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            auto_lock_minutes: 5,
            clipboard_clear_seconds: 30,
            close_to_tray: true,
            start_at_login: true,
            theme: "system".into(),
            passkey_verify_every_time: true,
        }
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

pub fn load(data_dir: &Path) -> Settings {
    std::fs::read(path(data_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save(data_dir: &Path, s: &Settings) -> std::io::Result<()> {
    let json = serde_json::to_vec_pretty(s).expect("settings serialize");
    std::fs::write(path(data_dir), json)
}
