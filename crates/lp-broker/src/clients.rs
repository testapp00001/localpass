//! Paired clients: each has an id and a random secret that proves who it is on every
//! connection. The app keeps them (protected by the operating system); tests use memory.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::Mutex;

use lp_core::passkey::b64u;
use serde::{Deserialize, Serialize};

/// What a client may do. Extensions can do everything; an OS passkey provider only passkeys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClientKind {
    #[default]
    Extension,
    Provider,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientRecord {
    pub id: String,
    pub name: String,
    pub kind: ClientKind,
    #[serde(with = "b64u")]
    pub secret: Vec<u8>,
    pub created_ms: u64,
    pub last_used_ms: u64,
}

impl fmt::Debug for ClientRecord {
    // The secret must never end up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientRecord")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("secret", &"<hidden>")
            .finish()
    }
}

pub trait ClientStore: Send + Sync {
    fn get(&self, id: &str) -> Option<ClientRecord>;
    fn insert(&self, record: ClientRecord) -> io::Result<()>;
    fn remove(&self, id: &str) -> io::Result<()>;
    fn list(&self) -> Vec<ClientRecord>;
    /// Records that the client was just used (best effort).
    fn touch(&self, id: &str, now_ms: u64);
}

#[derive(Default)]
pub struct MemoryClientStore {
    clients: Mutex<BTreeMap<String, ClientRecord>>,
}

impl ClientStore for MemoryClientStore {
    fn get(&self, id: &str) -> Option<ClientRecord> {
        self.clients.lock().expect("clients").get(id).cloned()
    }

    fn insert(&self, record: ClientRecord) -> io::Result<()> {
        self.clients
            .lock()
            .expect("clients")
            .insert(record.id.clone(), record);
        Ok(())
    }

    fn remove(&self, id: &str) -> io::Result<()> {
        self.clients.lock().expect("clients").remove(id);
        Ok(())
    }

    fn list(&self) -> Vec<ClientRecord> {
        self.clients
            .lock()
            .expect("clients")
            .values()
            .cloned()
            .collect()
    }

    fn touch(&self, id: &str, now_ms: u64) {
        if let Some(c) = self.clients.lock().expect("clients").get_mut(id) {
            c.last_used_ms = now_ms;
        }
    }
}
