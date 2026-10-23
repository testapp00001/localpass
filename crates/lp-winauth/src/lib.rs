//! LocalPass Windows system passkey provider (Phase 4): a packaged COM server that Windows starts for
//! MakeCredential / GetAssertion / Cancel requests and that relays them to the running LocalPass app over
//! the broker pipe. Private keys never reach this process: the app's engine signs. Windows only.
//!
//! - [`sys`]: the `#[repr(C)]` plugin API types (pinned to the SDK headers by `tools/layout_check.c`)
//!   and [`sys::WebAuthn`], the `webauthn.dll` exports bound at run time with RAII owners for every
//!   OS-allocated buffer.
//! - [`verify`]: verification of the OS-signed operation requests.
//! - [`register`]: the CTAP `authenticatorGetInfo` blob and logo handed to `WebAuthNPluginAddAuthenticator`.
//! - [`translate`]: pure translation between decoded OS requests and the broker's passkey requests, and
//!   the HRESULT table.
//! - [`relay`]: the [`relay::Relay`] trait (the app, as the handlers see it) and its real implementation
//!   over `lp_broker::client`, one pipe connection per operation.
//! - [`handler`]: the operations themselves over plain bytes, with the in-flight map that makes Windows'
//!   retries and cancels safe.
//! - [`cache`]: the credential-cache diff and sync.
//! - [`store`], [`log`]: DPAPI-protected pairing secret and settings; the log file.
//! - [`provider`], [`com`]: the wiring to the real OS and the COM server.
//!
//! See `docs/windows-passkey-provider-notes.md` for the design, the spike results and the build notes.
#![cfg(windows)]

pub mod cache;
pub mod com;
pub mod handler;
pub mod log;
pub mod provider;
pub mod register;
pub mod relay;
pub mod store;
pub mod sys;
#[doc(hidden)]
pub mod testing;
pub mod translate;
pub mod verify;
