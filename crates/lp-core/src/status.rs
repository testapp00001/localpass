//! Small shared types that cross crate boundaries (the engine, the broker and its clients).

use serde::{Deserialize, Serialize};

/// Whether this device has a vault and whether it is unlocked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultStatus {
    NoVault,
    Locked,
    Unlocked,
}
