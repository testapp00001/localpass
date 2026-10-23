//! LocalPass core: crypto primitives, the signed op-log format, the CRDT
//! materializer and the device roster. Pure logic, no I/O.

pub mod cbor;
pub mod crypto;
pub mod error;
pub mod hlc;
pub mod ids;
pub mod materialize;
pub mod model;
pub mod op;
pub mod origin;
pub mod passgen;
pub mod passkey;
pub mod roster;
pub mod status;
pub mod totp;

pub use error::{CoreError, Result};
