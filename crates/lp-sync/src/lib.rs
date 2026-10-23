//! LocalPass replication.
//!
//! * [`replica`] owns the local op-log view: ingest rules, roster, membership proofs.
//! * [`channel`] is the Noise-encrypted, framed message channel.
//! * [`session`] runs one sync exchange between two members.
//! * [`pairing`] adds a new device with a short code (SPAKE2 → Noise).
//! * [`discovery`] advertises/browses mDNS with rotating, member-only tags.
//! * [`service`] ties it together: listener, scheduler, address book.

pub mod channel;
pub mod discovery;
pub mod error;
pub mod framing;
pub mod netutil;
pub mod pairing;
pub mod protocol;
pub mod replica;
pub mod service;
pub mod session;

pub use error::{Result, SyncError};
pub use replica::{IngestReport, Replica};
