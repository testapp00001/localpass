//! Vault membership derived from roster ops.
//!
//! Rules (deterministic, no dependency cycles):
//! 1. *Known* devices: reachable from the pinned genesis device through
//!    `DeviceAdded` edges, ignoring revocations. Their ops may be stored/relayed.
//! 2. A device's *cutoff* is the lowest `cutoff_seq` of any revocation of it
//!    authored by a known device.
//! 3. An edge "A adds Y" made as A's op #s is *valid* when s ≤ cutoff(A).
//! 4. *Active* devices: reachable from genesis through valid edges and not
//!    revoked. Only active devices may connect.
//!
//! So a lost device can't add devices after its cutoff, and revoking the
//! genesis device doesn't orphan the devices it added earlier.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::ids::{DeviceId, NoisePublicKey, OpRef, VaultId};
use crate::model::{DeviceKeys, RosterOp};

#[derive(Clone, Debug)]
pub struct RosterRecord {
    pub author: DeviceId,
    pub seq: u64,
    pub op: RosterOp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    Active,
    Revoked {
        cutoff_seq: u64,
    },
    /// Known but only through edges voided by a revocation; needs re-approval.
    Pending,
}

#[derive(Clone, Debug)]
pub struct DeviceRecord {
    pub keys: DeviceKeys,
    pub status: DeviceStatus,
    pub is_genesis: bool,
    pub added_by: Option<DeviceId>,
    pub added_ms: u64,
}

/// What the materializer should do with a data op from a given author/seq.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataPolicy {
    Apply,
    /// Made by a revoked device after its cutoff: applied, but surfaced for review.
    ApplyFlagged,
    Skip,
}

struct Edge {
    author: DeviceId,
    seq: u64,
    keys: DeviceKeys,
    added_ms: u64,
}

#[derive(Clone, Debug)]
pub struct RosterView {
    vault_id: VaultId,
    genesis: DeviceId,
    devices: BTreeMap<DeviceId, DeviceRecord>,
    by_noise: BTreeMap<NoisePublicKey, DeviceId>,
    /// Edge op through which each validly reached device was first reached.
    parent_edge: BTreeMap<DeviceId, OpRef>,
}

impl RosterView {
    /// Builds the view. `records` must already be signature-checked; anything
    /// not reachable from `genesis` is ignored.
    pub fn compute(vault_id: VaultId, genesis: DeviceId, records: &[RosterRecord]) -> Option<Self> {
        let mut sorted: Vec<&RosterRecord> = records.iter().collect();
        sorted.sort_by_key(|r| (r.author, r.seq));

        let genesis_rec = sorted.iter().find_map(|r| match &r.op {
            RosterOp::Genesis {
                vault_id: v,
                keys,
                created_ms,
            } if r.author == genesis
                && r.seq == 1
                && *v == vault_id
                && keys.sign_pk == genesis
                && keys.verify_binding(&vault_id).is_ok() =>
            {
                Some((keys.clone(), *created_ms))
            }
            _ => None,
        })?;

        let mut edges: Vec<Edge> = Vec::new();
        let mut revocations: Vec<(DeviceId, DeviceId, u64)> = Vec::new();
        for r in &sorted {
            match &r.op {
                RosterOp::DeviceAdded { keys, added_ms }
                    if keys.sign_pk != r.author && keys.verify_binding(&vault_id).is_ok() =>
                {
                    edges.push(Edge {
                        author: r.author,
                        seq: r.seq,
                        keys: keys.clone(),
                        added_ms: *added_ms,
                    });
                }
                RosterOp::DeviceRevoked {
                    device, cutoff_seq, ..
                } => {
                    revocations.push((r.author, *device, *cutoff_seq));
                }
                _ => {}
            }
        }

        // 1. Known devices (revocations ignored).
        let mut known: BTreeSet<DeviceId> = BTreeSet::from([genesis]);
        loop {
            let before = known.len();
            for e in &edges {
                if known.contains(&e.author) {
                    known.insert(e.keys.sign_pk);
                }
            }
            if known.len() == before {
                break;
            }
        }

        // 2. Cutoffs from revocations by known devices.
        let mut cutoffs: BTreeMap<DeviceId, u64> = BTreeMap::new();
        for (author, target, cutoff) in &revocations {
            if known.contains(author) && known.contains(target) {
                let c = cutoffs.entry(*target).or_insert(*cutoff);
                *c = (*c).min(*cutoff);
            }
        }

        // 3. Breadth-first walk over valid edges, remembering the edge used.
        let mut parent_edge: BTreeMap<DeviceId, OpRef> = BTreeMap::new();
        let mut reached: BTreeSet<DeviceId> = BTreeSet::from([genesis]);
        let mut queue = VecDeque::from([genesis]);
        while let Some(node) = queue.pop_front() {
            let cutoff = cutoffs.get(&node).copied().unwrap_or(u64::MAX);
            for e in edges.iter().filter(|e| e.author == node && e.seq <= cutoff) {
                if reached.insert(e.keys.sign_pk) {
                    parent_edge.insert(
                        e.keys.sign_pk,
                        OpRef {
                            author: e.author,
                            seq: e.seq,
                        },
                    );
                    queue.push_back(e.keys.sign_pk);
                }
            }
        }

        // 4. Device records.
        let mut devices = BTreeMap::new();
        let status_of = |id: &DeviceId| match cutoffs.get(id) {
            Some(c) => DeviceStatus::Revoked { cutoff_seq: *c },
            None if reached.contains(id) => DeviceStatus::Active,
            None => DeviceStatus::Pending,
        };
        devices.insert(
            genesis,
            DeviceRecord {
                keys: genesis_rec.0,
                status: status_of(&genesis),
                is_genesis: true,
                added_by: None,
                added_ms: genesis_rec.1,
            },
        );
        for id in known.iter().filter(|id| **id != genesis) {
            let edge = match parent_edge.get(id) {
                Some(p) => edges
                    .iter()
                    .find(|e| e.author == p.author && e.seq == p.seq),
                None => edges
                    .iter()
                    .find(|e| e.keys.sign_pk == *id && known.contains(&e.author)),
            }
            .expect("every known device has an edge");
            devices.insert(
                *id,
                DeviceRecord {
                    keys: edge.keys.clone(),
                    status: status_of(id),
                    is_genesis: false,
                    added_by: Some(edge.author),
                    added_ms: edge.added_ms,
                },
            );
        }
        let by_noise = devices
            .iter()
            .map(|(id, d)| (d.keys.noise_pk, *id))
            .collect();

        Some(Self {
            vault_id,
            genesis,
            devices,
            by_noise,
            parent_edge,
        })
    }

    pub fn vault_id(&self) -> VaultId {
        self.vault_id
    }

    pub fn genesis(&self) -> DeviceId {
        self.genesis
    }

    pub fn device(&self, id: &DeviceId) -> Option<&DeviceRecord> {
        self.devices.get(id)
    }

    pub fn devices(&self) -> impl Iterator<Item = (&DeviceId, &DeviceRecord)> {
        self.devices.iter()
    }

    pub fn by_noise_key(&self, pk: &NoisePublicKey) -> Option<(DeviceId, &DeviceRecord)> {
        let id = self.by_noise.get(pk)?;
        self.devices.get(id).map(|d| (*id, d))
    }

    /// Reachable from genesis (any status): its ops may be stored and relayed.
    pub fn is_known(&self, id: &DeviceId) -> bool {
        self.devices.contains_key(id)
    }

    pub fn is_active(&self, id: &DeviceId) -> bool {
        self.devices
            .get(id)
            .is_some_and(|d| d.status == DeviceStatus::Active)
    }

    /// Roster ops proving `id`'s membership: genesis first, then each edge.
    pub fn proof_path(&self, id: &DeviceId) -> Option<Vec<OpRef>> {
        if !self.is_active(id) {
            return None;
        }
        let mut path = Vec::new();
        let mut cur = *id;
        while cur != self.genesis {
            let edge = self.parent_edge.get(&cur)?;
            path.push(*edge);
            cur = edge.author;
        }
        path.push(OpRef {
            author: self.genesis,
            seq: 1,
        });
        path.reverse();
        Some(path)
    }

    pub fn data_policy(&self, author: &DeviceId, seq: u64) -> DataPolicy {
        match self.devices.get(author).map(|d| d.status) {
            Some(DeviceStatus::Active) => DataPolicy::Apply,
            Some(DeviceStatus::Revoked { cutoff_seq }) if seq <= cutoff_seq => DataPolicy::Apply,
            Some(DeviceStatus::Revoked { .. }) => DataPolicy::ApplyFlagged,
            Some(DeviceStatus::Pending) | None => DataPolicy::Skip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{NoiseKeypair, SigningKey};

    struct Dev {
        sk: SigningKey,
        keys: DeviceKeys,
    }

    fn dev(vault: &VaultId) -> Dev {
        let sk = SigningKey::generate().unwrap();
        let keys = DeviceKeys::new(&sk, vault, NoiseKeypair::generate().unwrap().public());
        Dev { sk, keys }
    }

    fn genesis(vault: VaultId, g: &Dev) -> RosterRecord {
        RosterRecord {
            author: g.sk.device_id(),
            seq: 1,
            op: RosterOp::Genesis {
                vault_id: vault,
                keys: g.keys.clone(),
                created_ms: 0,
            },
        }
    }

    fn add(by: &Dev, seq: u64, who: &Dev) -> RosterRecord {
        RosterRecord {
            author: by.sk.device_id(),
            seq,
            op: RosterOp::DeviceAdded {
                keys: who.keys.clone(),
                added_ms: 0,
            },
        }
    }

    fn revoke(by: &Dev, seq: u64, who: &Dev, cutoff_seq: u64) -> RosterRecord {
        RosterRecord {
            author: by.sk.device_id(),
            seq,
            op: RosterOp::DeviceRevoked {
                device: who.sk.device_id(),
                cutoff_seq,
                revoked_ms: 0,
            },
        }
    }

    fn id(d: &Dev) -> DeviceId {
        d.sk.device_id()
    }

    #[test]
    fn transitive_membership_and_proof_paths() {
        let v = VaultId::random().unwrap();
        let (p, h, w) = (dev(&v), dev(&v), dev(&v));
        let records = vec![genesis(v, &p), add(&p, 2, &h), add(&p, 3, &w)];
        let view = RosterView::compute(v, id(&p), &records).unwrap();
        assert!(view.is_active(&id(&h)) && view.is_active(&id(&w)));
        assert_eq!(
            view.proof_path(&id(&w)).unwrap(),
            vec![
                OpRef {
                    author: id(&p),
                    seq: 1
                },
                OpRef {
                    author: id(&p),
                    seq: 3
                }
            ]
        );
        assert_eq!(view.by_noise_key(&w.keys.noise_pk).unwrap().0, id(&w));

        // H only knew {P, H}; W's proof (genesis + "P adds W") makes W active for H.
        let h_local = vec![genesis(v, &p), add(&p, 2, &h)];
        let before = RosterView::compute(v, id(&p), &h_local).unwrap();
        assert!(!before.is_known(&id(&w)));
        let mut combined = h_local.clone();
        combined.push(add(&p, 3, &w));
        let after = RosterView::compute(v, id(&p), &combined).unwrap();
        assert!(after.is_active(&id(&w)));
    }

    #[test]
    fn missing_or_wrong_genesis_yields_no_view() {
        let v = VaultId::random().unwrap();
        let p = dev(&v);
        assert!(RosterView::compute(v, id(&p), &[]).is_none());
        assert!(
            RosterView::compute(VaultId::random().unwrap(), id(&p), &[genesis(v, &p)]).is_none()
        );
    }

    #[test]
    fn ops_from_strangers_and_bad_bindings_are_ignored() {
        let v = VaultId::random().unwrap();
        let (p, stranger, x) = (dev(&v), dev(&v), dev(&v));
        let mut bad = dev(&v);
        bad.keys.noise_pk = NoiseKeypair::generate().unwrap().public();
        let records = vec![genesis(v, &p), add(&stranger, 1, &x), add(&p, 2, &bad)];
        let view = RosterView::compute(v, id(&p), &records).unwrap();
        assert!(!view.is_known(&id(&x)));
        assert!(!view.is_known(&id(&bad)));
    }

    #[test]
    fn revocation_voids_later_additions_but_keeps_earlier_ones() {
        let v = VaultId::random().unwrap();
        let (p, h, w, rogue) = (dev(&v), dev(&v), dev(&v), dev(&v));
        // P (genesis) added H as #2 and W as #3, then was lost; H revokes P at cutoff 3.
        // A thief later uses P to add a rogue device as #4.
        let records = vec![
            genesis(v, &p),
            add(&p, 2, &h),
            add(&p, 3, &w),
            add(&p, 4, &rogue),
            revoke(&h, 1, &p, 3),
        ];
        let view = RosterView::compute(v, id(&p), &records).unwrap();
        assert_eq!(
            view.device(&id(&p)).unwrap().status,
            DeviceStatus::Revoked { cutoff_seq: 3 }
        );
        assert!(view.is_active(&id(&h)));
        assert!(view.is_active(&id(&w)));
        assert_eq!(
            view.device(&id(&rogue)).unwrap().status,
            DeviceStatus::Pending
        );
        assert_eq!(view.data_policy(&id(&p), 3), DataPolicy::Apply);
        assert_eq!(view.data_policy(&id(&p), 9), DataPolicy::ApplyFlagged);
        assert_eq!(view.data_policy(&id(&rogue), 1), DataPolicy::Skip);
        assert!(view.proof_path(&id(&rogue)).is_none());
    }

    #[test]
    fn pending_device_can_be_reapproved() {
        let v = VaultId::random().unwrap();
        let (p, h, w) = (dev(&v), dev(&v), dev(&v));
        // H revoked P with a stale cutoff (2) without knowing P had added W as #3.
        let mut records = vec![
            genesis(v, &p),
            add(&p, 2, &h),
            add(&p, 3, &w),
            revoke(&h, 1, &p, 2),
        ];
        let view = RosterView::compute(v, id(&p), &records).unwrap();
        assert_eq!(view.device(&id(&w)).unwrap().status, DeviceStatus::Pending);
        // H re-approves W.
        records.push(add(&h, 2, &w));
        let view = RosterView::compute(v, id(&p), &records).unwrap();
        assert!(view.is_active(&id(&w)));
        assert_eq!(view.proof_path(&id(&w)).unwrap().len(), 3);
    }
}
