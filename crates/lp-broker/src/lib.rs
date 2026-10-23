//! The broker: how a browser extension (through the native-messaging helper) or an OS
//! passkey provider talks to the running, unlocked LocalPass app.
//!
//! * [`framing`]: length-prefixed JSON messages over any byte stream.
//! * [`protocol`]: the messages and requests.
//! * [`clients`]: paired clients and their secrets.
//! * [`pipe`]: the Windows named pipe the app listens on, and how clients find it.
//! * [`client`] (feature `client`): a Rust client that does the handshake and sends requests.
//!   It needs nothing from the engine, so the provider process stays small.
//! * [`server`] (feature `server`): the handshake (pairing or challenge-response) and request
//!   handling, written against the [`server::Host`] trait so the app supplies prompts and tests
//!   supply fakes.
//! * [`nmhost`] (feature `server`): the helper a browser starts, which relays between the
//!   browser and the pipe.

#[cfg(feature = "client")]
pub mod client;
pub mod clients;
pub mod framing;
#[cfg(feature = "server")]
pub mod nmhost;
pub mod pipe;
pub mod protocol;
#[cfg(feature = "server")]
pub mod server;

pub use clients::{ClientKind, ClientRecord, ClientStore, MemoryClientStore};
#[cfg(feature = "server")]
pub use server::{Broker, Host, OverwriteKind, OverwritePrompt, PairingPrompt, Policy};
