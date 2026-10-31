//! Registers the LocalPass native-messaging host with the browsers, per user (no administrator
//! rights). The host is the app's own executable: see `native_host`. Windows writes registry keys
//! for Chrome and Edge; Linux writes Firefox's host manifest file.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
pub use self::linux::*;
#[cfg(windows)]
pub use self::windows::*;
