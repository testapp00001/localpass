//! State held only while the vault is unlocked.

use std::collections::BTreeSet;

use lp_core::crypto::{Key32, SigningKey};
use lp_core::hlc::{Hlc, HlcClock, now_ms};
use lp_core::ids::OpRef;
use lp_core::materialize::VaultState;
use lp_core::op::{OpClass, SignedOp};
use lp_core::roster::{DataPolicy, RosterView};
use lp_store::VersionVector;
use lp_sync::Replica;
use tracing::warn;

use crate::Result;

pub(crate) struct Unlocked {
    pub vk: Key32,
    pub sk: SigningKey,
    pub clock: HlcClock,
    pub state: VaultState,
    /// Highest seq per author already folded into `state`.
    pub applied: VersionVector,
    roster_len: usize,
    /// Replica version already folded in (see `Replica::version`).
    pub seen_version: u64,
    /// Ops applied from revoked devices after their cutoff.
    pub flagged: BTreeSet<OpRef>,
    pub undecryptable: usize,
}

impl Unlocked {
    /// Decrypts and folds the whole log.
    pub fn load(vk: Key32, sk: SigningKey, replica: &Replica, last_hlc: Hlc) -> Result<Self> {
        let mut s = Self {
            vk,
            sk,
            clock: HlcClock::new(last_hlc),
            state: VaultState::default(),
            applied: VersionVector::new(),
            roster_len: 0,
            seen_version: 0,
            flagged: BTreeSet::new(),
            undecryptable: 0,
        };
        s.rebuild(replica)?;
        Ok(s)
    }

    fn rebuild(&mut self, replica: &Replica) -> Result<()> {
        self.state.wipe();
        self.state = VaultState::default();
        self.applied.clear();
        self.flagged.clear();
        self.undecryptable = 0;
        self.seen_version = replica.version();
        self.roster_len = replica.roster_len();
        let view = replica.view();
        let store = replica.store();
        let mut ops = Vec::new();
        store.for_each_op(|op| ops.push(op))?;
        drop(store);
        for op in &ops {
            self.apply(&view, op);
        }
        self.clock.observe(self.state.max_hlc(), now_ms());
        Ok(())
    }

    /// Folds in ops that arrived since the last call. Returns whether anything changed.
    pub fn catch_up(&mut self, replica: &Replica) -> Result<bool> {
        let version = replica.version();
        if replica.roster_len() != self.roster_len {
            // Membership changed: which authors count may differ, so start over.
            self.rebuild(replica)?;
            return Ok(true);
        }
        let view = replica.view();
        let store = replica.store();
        let mut fresh = Vec::new();
        for (author, max) in store.version_vector()? {
            let have = self.applied.get(&author).copied().unwrap_or(0);
            if max > have {
                fresh.extend(store.ops_after(&author, have, (max - have) as usize)?);
            }
        }
        drop(store);
        for op in &fresh {
            self.apply(&view, op);
        }
        self.seen_version = version;
        Ok(!fresh.is_empty())
    }

    fn apply(&mut self, view: &RosterView, op: &SignedOp) {
        let entry = self.applied.entry(op.author()).or_insert(0);
        *entry = (*entry).max(op.seq());
        if op.class() != OpClass::Data {
            return;
        }
        let policy = view.data_policy(&op.author(), op.seq());
        if policy == DataPolicy::Skip {
            return;
        }
        match op.decode_data(&self.vk) {
            Ok(data) => {
                if let Some(hlc) = data.hlc() {
                    self.clock.observe(hlc, now_ms());
                }
                self.state.apply(op.author(), op.seq(), &data);
                if policy == DataPolicy::ApplyFlagged {
                    self.flagged.insert(op.op_ref());
                }
            }
            Err(e) => {
                self.undecryptable += 1;
                warn!(op = ?op.op_ref(), error = %e, "could not decrypt op");
            }
        }
    }
}

impl Drop for Unlocked {
    fn drop(&mut self) {
        self.state.wipe();
    }
}
