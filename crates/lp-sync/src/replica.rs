//! The local replica: ingest rules, roster cache and membership proofs.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use lp_core::cbor;
use lp_core::hlc::now_ms;
use lp_core::ids::{DeviceId, NoisePublicKey, OpRef, VaultId};
use lp_core::model::RosterOp;
use lp_core::op::{OpClass, SignedOp};
use lp_core::roster::{DeviceStatus, RosterRecord, RosterView};
use lp_store::{Store, VersionVector};

use crate::protocol::MAX_PROOF_OPS;

/// Ops that cannot be placed yet (unknown author, or a gap before them) are kept for a while in case
/// the missing ones follow. They are not verified until they can be placed, so a peer could otherwise
/// make the receiver hold them without limit: past this many bytes the session ends.
pub const MAX_STAGED_BYTES: usize = 16 * 1024 * 1024;
use crate::{Result, SyncError};

/// Meta key holding the proof handed to a newly joined device, used until its
/// own `DeviceAdded` op arrives through sync.
pub const META_JOIN_PROOF: &str = "sync.join_proof";

#[derive(Clone)]
struct RosterCache {
    records: Vec<RosterRecord>,
    ops: BTreeMap<OpRef, SignedOp>,
    view: RosterView,
}

impl RosterCache {
    fn push(&mut self, op: &SignedOp, vault_id: VaultId, genesis: DeviceId) -> Result<()> {
        let decoded = op.decode_roster().unwrap_or(RosterOp::Unknown);
        self.records.push(RosterRecord {
            author: op.author(),
            seq: op.seq(),
            op: decoded,
        });
        self.ops.insert(op.op_ref(), op.clone());
        self.view = RosterView::compute(vault_id, genesis, &self.records)
            .ok_or_else(|| SyncError::Protocol("roster lost its genesis".into()))?;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct IngestReport {
    /// Ops newly written to the log, in (author, seq) order.
    pub accepted: Vec<SignedOp>,
    /// Ops whose author signed two different ops with the same seq.
    pub forks: Vec<OpRef>,
    pub rejected: usize,
    pub roster_changed: bool,
}

pub struct Replica {
    vault_id: VaultId,
    genesis: DeviceId,
    me: DeviceId,
    store: Arc<Mutex<Store>>,
    roster: RwLock<RosterCache>,
    /// Bumped whenever ops are written, so readers know to catch up.
    version: AtomicU64,
}

impl Replica {
    /// Opens the replica over a store that already contains the genesis op.
    pub fn open(
        store: Arc<Mutex<Store>>,
        vault_id: VaultId,
        genesis: DeviceId,
        me: DeviceId,
    ) -> Result<Self> {
        let roster_ops = store
            .lock()
            .expect("store lock")
            .ops_by_class(OpClass::Roster)?;
        let mut records = Vec::with_capacity(roster_ops.len());
        let mut ops = BTreeMap::new();
        for op in roster_ops {
            let decoded = op.decode_roster().unwrap_or(RosterOp::Unknown);
            records.push(RosterRecord {
                author: op.author(),
                seq: op.seq(),
                op: decoded,
            });
            ops.insert(op.op_ref(), op);
        }
        let view = RosterView::compute(vault_id, genesis, &records)
            .ok_or_else(|| SyncError::Protocol("store has no valid genesis op".into()))?;
        Ok(Self {
            vault_id,
            genesis,
            me,
            store,
            roster: RwLock::new(RosterCache { records, ops, view }),
            version: AtomicU64::new(0),
        })
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault_id
    }

    pub fn me(&self) -> DeviceId {
        self.me
    }

    pub fn genesis(&self) -> DeviceId {
        self.genesis
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().expect("store lock")
    }

    pub fn store_handle(&self) -> Arc<Mutex<Store>> {
        self.store.clone()
    }

    pub fn view(&self) -> RosterView {
        self.roster.read().expect("roster lock").view.clone()
    }

    /// Changes whenever ops are written (by sync or locally).
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Number of roster ops seen; changes whenever membership may have changed.
    pub fn roster_len(&self) -> usize {
        self.roster.read().expect("roster lock").records.len()
    }

    pub fn version_vector(&self) -> Result<VersionVector> {
        Ok(self.store().version_vector()?)
    }

    /// Roster ops proving this device's membership.
    pub fn membership_proof(&self) -> Result<Vec<SignedOp>> {
        {
            let cache = self.roster.read().expect("roster lock");
            if let Some(path) = cache.view.proof_path(&self.me) {
                return Ok(path
                    .iter()
                    .filter_map(|r| cache.ops.get(r).cloned())
                    .collect());
            }
        }
        match self.store().meta_get(META_JOIN_PROOF)? {
            Some(bytes) => Ok(cbor::from_slice(&bytes)?),
            None => Ok(Vec::new()),
        }
    }

    /// Identifies the peer behind a Noise static key. Unknown keys must come
    /// with a proof chain that makes them an active member.
    pub fn verify_peer(&self, remote: &NoisePublicKey, proof: &[SignedOp]) -> Result<DeviceId> {
        let cache = self.roster.read().expect("roster lock");
        if let Some((id, rec)) = cache.view.by_noise_key(remote) {
            return match rec.status {
                DeviceStatus::Active => Ok(id),
                DeviceStatus::Revoked { .. } => Err(SyncError::Revoked),
                DeviceStatus::Pending => Err(SyncError::NotMember),
            };
        }
        if proof.len() > MAX_PROOF_OPS {
            return Err(SyncError::Protocol("membership proof too long".into()));
        }
        let mut records = cache.records.clone();
        for op in proof {
            if op.header().vault_id != self.vault_id || op.class() != OpClass::Roster {
                return Err(SyncError::NotMember);
            }
            op.verify_signature().map_err(|_| SyncError::NotMember)?;
            if !cache.ops.contains_key(&op.op_ref()) {
                let decoded = op.decode_roster().map_err(|_| SyncError::NotMember)?;
                records.push(RosterRecord {
                    author: op.author(),
                    seq: op.seq(),
                    op: decoded,
                });
            }
        }
        let view = RosterView::compute(self.vault_id, self.genesis, &records)
            .ok_or(SyncError::NotMember)?;
        match view.by_noise_key(remote) {
            Some((id, rec)) if rec.status == DeviceStatus::Active => Ok(id),
            Some((_, rec)) if matches!(rec.status, DeviceStatus::Revoked { .. }) => {
                Err(SyncError::Revoked)
            }
            _ => Err(SyncError::NotMember),
        }
    }

    /// Everything we hold that a peer with version vector `peer` lacks.
    pub fn ops_missing_for(&self, peer: &VersionVector) -> Result<Vec<SignedOp>> {
        let store = self.store();
        let mine = store.version_vector()?;
        let mut out = Vec::new();
        for (author, max) in mine {
            let have = peer.get(&author).copied().unwrap_or(0);
            if max > have {
                out.extend(store.ops_after(&author, have, (max - have) as usize)?);
            }
        }
        Ok(out)
    }

    /// Validates and stores incoming ops. Ops that can't be placed yet (unknown
    /// author, gap) stay in `staged` and are retried as more ops arrive.
    pub fn ingest(
        &self,
        incoming: Vec<SignedOp>,
        staged: &mut Vec<SignedOp>,
    ) -> Result<IngestReport> {
        staged.extend(incoming);
        let mut report = IngestReport::default();
        let mut store = self.store();
        let mut vv = store.version_vector()?;
        let mut cache = self.roster.write().expect("roster lock");
        let mut next = cache.clone();
        let mut accepted: Vec<SignedOp> = Vec::new();

        loop {
            staged.sort_by_key(|o| (o.author(), o.seq()));
            staged
                .dedup_by(|a, b| a.op_ref() == b.op_ref() && a.header_bytes() == b.header_bytes());
            let mut progress = false;
            let mut keep = Vec::new();
            for op in std::mem::take(staged) {
                let author = op.author();
                if op.header().vault_id != self.vault_id {
                    report.rejected += 1;
                    continue;
                }
                let have = vv.get(&author).copied().unwrap_or(0);
                if op.seq() <= have {
                    let existing = match accepted.iter().find(|a| a.op_ref() == op.op_ref()) {
                        Some(a) => Some(a.clone()),
                        None => store.get_op(&author, op.seq())?,
                    };
                    if let Some(e) = existing
                        && e.header_bytes() != op.header_bytes()
                        && op.verify_signature().is_ok()
                    {
                        report.forks.push(op.op_ref());
                    }
                    continue;
                }
                if !next.view.is_known(&author) || op.seq() > have + 1 {
                    keep.push(op);
                    continue;
                }
                if op.verify_signature().is_err() {
                    report.rejected += 1;
                    continue;
                }
                if op.class() == OpClass::Roster {
                    next.push(&op, self.vault_id, self.genesis)?;
                    report.roster_changed = true;
                }
                vv.insert(author, op.seq());
                accepted.push(op);
                progress = true;
            }
            *staged = keep;
            if !progress {
                break;
            }
        }

        if !accepted.is_empty() {
            store.append_batch(&accepted, now_ms())?;
            *cache = next;
            self.version.fetch_add(1, Ordering::SeqCst);
        }
        report.accepted = accepted;
        if staged.iter().map(SignedOp::encoded_len).sum::<usize>() > MAX_STAGED_BYTES {
            staged.clear();
            return Err(SyncError::Protocol(
                "the other device sent too many ops that cannot be placed".into(),
            ));
        }
        Ok(report)
    }

    /// Appends an op authored by this device. `build` receives the seq to use.
    pub fn append_local(
        &self,
        build: impl FnOnce(u64) -> lp_core::Result<SignedOp>,
    ) -> Result<SignedOp> {
        let mut store = self.store();
        let seq = store.max_seq(&self.me)? + 1;
        let op = build(seq)?;
        if op.author() != self.me || op.seq() != seq {
            return Err(SyncError::Protocol(
                "local op has wrong author or seq".into(),
            ));
        }
        let mut cache = self.roster.write().expect("roster lock");
        let mut next = cache.clone();
        if op.class() == OpClass::Roster {
            next.push(&op, self.vault_id, self.genesis)?;
        }
        store.append_batch(std::slice::from_ref(&op), now_ms())?;
        *cache = next;
        self.version.fetch_add(1, Ordering::SeqCst);
        Ok(op)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::crypto::{Key32, NoiseKeypair, SigningKey};
    use lp_core::hlc::Hlc;
    use lp_core::ids::ItemId;
    use lp_core::model::{DataOp, DeviceKeys};
    use lp_core::op::{create_data_op, create_roster_op};

    pub(crate) struct TestDevice {
        pub sk: SigningKey,
        pub noise: NoiseKeypair,
    }

    impl TestDevice {
        pub fn new() -> Self {
            Self {
                sk: SigningKey::generate().unwrap(),
                noise: NoiseKeypair::generate().unwrap(),
            }
        }
        pub fn id(&self) -> DeviceId {
            self.sk.device_id()
        }
        pub fn keys(&self, v: &VaultId) -> DeviceKeys {
            DeviceKeys::new(&self.sk, v, self.noise.public())
        }
    }

    fn new_vault() -> (VaultId, TestDevice, SignedOp) {
        let v = VaultId::random().unwrap();
        let g = TestDevice::new();
        let genesis = create_roster_op(
            v,
            &g.sk,
            1,
            &RosterOp::Genesis {
                vault_id: v,
                keys: g.keys(&v),
                created_ms: 0,
            },
        )
        .unwrap();
        (v, g, genesis)
    }

    fn replica_with(v: VaultId, genesis: &SignedOp, me: DeviceId) -> Replica {
        let mut store = Store::open_in_memory().unwrap();
        store
            .append_batch(std::slice::from_ref(genesis), 0)
            .unwrap();
        Replica::open(Arc::new(Mutex::new(store)), v, genesis.author(), me).unwrap()
    }

    #[test]
    fn ingest_orders_ops_and_learns_new_authors_in_one_batch() {
        let (v, p, genesis) = new_vault();
        let w = TestDevice::new();
        let vk = Key32::random().unwrap();
        let add_w = create_roster_op(
            v,
            &p.sk,
            2,
            &RosterOp::DeviceAdded {
                keys: w.keys(&v),
                added_ms: 0,
            },
        )
        .unwrap();
        let w1 = create_data_op(
            v,
            &vk,
            &w.sk,
            1,
            &DataOp::ItemPurge {
                item: ItemId::random().unwrap(),
                hlc: Hlc::default(),
            },
        )
        .unwrap();

        let h = replica_with(v, &genesis, TestDevice::new().id());
        let mut staged = Vec::new();
        // W's op arrives before the roster op that introduces W.
        let rep = h
            .ingest(vec![w1.clone(), add_w.clone()], &mut staged)
            .unwrap();
        assert_eq!(rep.accepted.len(), 2);
        assert!(rep.roster_changed);
        assert!(staged.is_empty());
        assert!(h.view().is_active(&w.id()));

        // Re-ingesting is a no-op; a conflicting op at the same position is a fork.
        let rep = h.ingest(vec![w1], &mut staged).unwrap();
        assert!(rep.accepted.is_empty() && rep.forks.is_empty());
        let fork = create_data_op(
            v,
            &vk,
            &w.sk,
            1,
            &DataOp::ItemPurge {
                item: ItemId::random().unwrap(),
                hlc: Hlc::default(),
            },
        )
        .unwrap();
        let rep = h.ingest(vec![fork], &mut staged).unwrap();
        assert_eq!(rep.forks.len(), 1);
    }

    #[test]
    fn gaps_and_strangers_stay_staged_and_forgeries_are_rejected() {
        let (v, p, genesis) = new_vault();
        let vk = Key32::random().unwrap();
        let stranger = TestDevice::new();
        let s1 = create_data_op(v, &vk, &stranger.sk, 1, &DataOp::Unknown).unwrap();
        let p3 = create_data_op(v, &vk, &p.sk, 3, &DataOp::Unknown).unwrap();
        let h = replica_with(v, &genesis, TestDevice::new().id());
        let mut staged = Vec::new();
        let rep = h.ingest(vec![s1, p3], &mut staged).unwrap();
        assert!(rep.accepted.is_empty());
        assert_eq!(staged.len(), 2);

        // Forged signature on a well-formed op from a known author.
        let p2 = create_data_op(v, &vk, &p.sk, 2, &DataOp::Unknown).unwrap();
        let forged =
            SignedOp::from_parts(p2.header_bytes(), &[0u8; 64], p2.body().to_vec()).unwrap();
        let mut staged = Vec::new();
        let rep = h.ingest(vec![forged], &mut staged).unwrap();
        assert_eq!(rep.rejected, 1);
        assert!(rep.accepted.is_empty());
    }

    #[test]
    fn verify_peer_uses_roster_or_proof() {
        let (v, p, genesis) = new_vault();
        let (h, w) = (TestDevice::new(), TestDevice::new());
        let add_h = create_roster_op(
            v,
            &p.sk,
            2,
            &RosterOp::DeviceAdded {
                keys: h.keys(&v),
                added_ms: 0,
            },
        )
        .unwrap();
        let add_w = create_roster_op(
            v,
            &p.sk,
            3,
            &RosterOp::DeviceAdded {
                keys: w.keys(&v),
                added_ms: 0,
            },
        )
        .unwrap();

        // H's replica knows genesis + "P adds H" only.
        let hr = replica_with(v, &genesis, h.id());
        hr.ingest(vec![add_h], &mut Vec::new()).unwrap();
        assert_eq!(hr.verify_peer(&p.noise.public(), &[]).unwrap(), p.id());
        assert!(matches!(
            hr.verify_peer(&w.noise.public(), &[]),
            Err(SyncError::NotMember)
        ));
        // W presents genesis → "P adds W".
        assert_eq!(
            hr.verify_peer(&w.noise.public(), &[genesis.clone(), add_w.clone()])
                .unwrap(),
            w.id()
        );
        // A stranger can't borrow W's proof.
        let stranger = TestDevice::new();
        assert!(
            hr.verify_peer(&stranger.noise.public(), &[genesis, add_w])
                .is_err()
        );
    }

    #[test]
    fn append_local_assigns_next_seq() {
        let (v, p, genesis) = new_vault();
        let vk = Key32::random().unwrap();
        let r = replica_with(v, &genesis, p.id());
        let op = r
            .append_local(|seq| create_data_op(v, &vk, &p.sk, seq, &DataOp::Unknown))
            .unwrap();
        assert_eq!(op.seq(), 2);
        assert_eq!(r.membership_proof().unwrap(), vec![genesis]);
    }
}
