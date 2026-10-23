//! Settings > Windows passkeys: makes LocalPass an option in Windows' passkey sheet for every app and
//! browser. The provider itself is `crates/lp-winauth` (a packaged COM server); this module installs it.
//!
//! * `model`: the package identity, the file layout, the traits that touch the machine
//!   ([`CommandRunner`], [`System`], [`ClientBook`]) and the parsers for what PowerShell and the provider print.
//! * `flows`: status, set up, pair and remove, written against those traits (unit-tested with fakes).
//! * `real`: the real runner (child processes with a time limit) and the real registry / file version reads.
//! * `sync`: pushes the vault's passkeys into Windows' credential cache after changes (debounced).
//! * `manager`: what the Tauri commands and `lib.rs` use.
//!
//! Distribution is a Developer Mode loose registration (`Add-AppxPackage -Register`); a signed MSIX would
//! need a certificate the user trusts, which needs administrator rights. Without Developer Mode the user
//! gets plain guidance. See docs/windows-passkey-provider-notes.md, "Phase 4.5".

use serde::Serialize;

#[cfg(windows)]
mod flows;
#[cfg(windows)]
mod manager;
#[cfg(windows)]
mod model;
#[cfg(windows)]
mod real;
#[cfg(windows)]
mod sync;
#[cfg(all(test, windows))]
mod tests;

#[cfg(windows)]
pub use manager::{Manager, start};

/// What the Settings section shows. Everything is false on a build that cannot offer the feature
/// (`available`), so the UI can hide the section without knowing the platform.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// This build can offer it (Windows, and not a development profile).
    pub available: bool,
    /// Windows has the passkey plug-in API (webauthn.dll 10.0.26100.6725 or later).
    pub supported: bool,
    /// The webauthn.dll version, for the "needs a newer Windows" text.
    pub windows_version: Option<String>,
    /// Developer Mode is on, which the set-up needs (no certificate or administrator rights otherwise).
    pub developer_mode: bool,
    /// The provider program and its manifest are in this copy of LocalPass.
    pub provider_exe_found: bool,
    /// The package is registered for this Windows user.
    pub package_registered: bool,
    /// ... from this copy of LocalPass (a copy elsewhere, for example a development build's, is not).
    pub package_in_bundle: bool,
    /// LocalPass is in Windows' list of passkey providers.
    pub registered_in_windows: bool,
    /// The user turned it on in Windows settings (only the user can, with Windows Hello).
    pub enabled_in_windows: bool,
    /// The provider and this app know each other (paired).
    pub paired: bool,
    /// The registry tells the provider where this app is, so it can start it.
    pub app_path_ok: bool,
    /// Something could not be read; plain words for the user.
    pub problem: Option<String>,
}
