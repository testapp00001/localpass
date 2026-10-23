//! Property tests for the replica's ingest, replication and membership rules (phase 5.1).
//!
//! The ops that reach `Replica::ingest` come from a peer over the network, so they arrive in any
//! order, twice, with gaps, from strangers, forged or for another vault. The world below is one
//! small vault with three members and a stranger; the properties shuffle its ops and mix in bad
//! ones, and check what ends up in the store and in the roster.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock, Mutex};

use lp_core::crypto::{Key32, NoiseKeypair, SigningKey};
use lp_core::hlc::Hlc;
use lp_core::ids::{DeviceId, ItemId, VaultId};
use lp_core::model::{DataOp, DeviceKeys, RosterOp};
use lp_core::op::{OpClass, SignedOp, create_data_op, create_roster_op};
use lp_store::{Store, VersionVector};
use lp_sync::protocol::MAX_PROOF_OPS;
use lp_sync::{Replica, SyncError};
use proptest::collection::vec;
use proptest::prelude::*;

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

struct Device {
    sk: SigningKey,
    noise: NoiseKeypair,
}

impl Device {
    fn new() -> Self {
        Self {
            sk: SigningKey::generate().unwrap(),
            noise: NoiseKeypair::generate().unwrap(),
        }
    }
    fn id(&self) -> DeviceId {
        self.sk.device_id()
    }
    fn keys(&self, v: &VaultId) -> DeviceKeys {
        DeviceKeys::new(&self.sk, v, self.noise.public())
    }
}

/// Indices into `World::all`: genesis, `P adds W1`, `P adds W2`, P's data ops (seq 4-6), W1's data
/// ops (seq 1-3) and W2's data ops (seq 1-3).
const N: usize = 12;
const GENESIS: usize = 0;
const ADD_W1: usize = 1;
const ADD_W2: usize = 2;
const P_FIRST_DATA: usize = 3;
const W1_FIRST_DATA: usize = 6;
const W2_FIRST_DATA: usize = 9;

struct World {
    vault: VaultId,
    vk: Key32,
    p: Device,
    w1: Device,
    w2: Device,
    stranger: Device,
    /// The whole history, every author's ops in seq order.
    all: Vec<SignedOp>,
    /// `all` as header bytes per author, in seq order.
    expected: BTreeMap<DeviceId, Vec<Vec<u8>>>,
}

fn purge() -> DataOp {
    DataOp::ItemPurge {
        item: ItemId::random().unwrap(),
        hlc: Hlc::default(),
    }
}

impl World {
    fn new() -> Self {
        let vault = VaultId::random().unwrap();
        let vk = Key32::random().unwrap();
        let (p, w1, w2, stranger) = (Device::new(), Device::new(), Device::new(), Device::new());
        let mut all = vec![
            create_roster_op(
                vault,
                &p.sk,
                1,
                &RosterOp::Genesis {
                    vault_id: vault,
                    keys: p.keys(&vault),
                    created_ms: 0,
                },
            )
            .unwrap(),
            create_roster_op(
                vault,
                &p.sk,
                2,
                &RosterOp::DeviceAdded {
                    keys: w1.keys(&vault),
                    added_ms: 0,
                },
            )
            .unwrap(),
            create_roster_op(
                vault,
                &p.sk,
                3,
                &RosterOp::DeviceAdded {
                    keys: w2.keys(&vault),
                    added_ms: 0,
                },
            )
            .unwrap(),
        ];
        for seq in 4..=6 {
            all.push(create_data_op(vault, &vk, &p.sk, seq, &purge()).unwrap());
        }
        for dev in [&w1, &w2] {
            for seq in 1..=3 {
                all.push(create_data_op(vault, &vk, &dev.sk, seq, &purge()).unwrap());
            }
        }
        assert_eq!(all.len(), N);
        let mut expected: BTreeMap<DeviceId, Vec<Vec<u8>>> = BTreeMap::new();
        for op in &all {
            expected
                .entry(op.author())
                .or_default()
                .push(op.header_bytes().to_vec());
        }
        Self {
            vault,
            vk,
            p,
            w1,
            w2,
            stranger,
            all,
            expected,
        }
    }

    /// A replica that holds only the genesis op.
    fn replica(&self, me: DeviceId) -> Replica {
        let mut store = Store::open_in_memory().unwrap();
        store
            .append_batch(std::slice::from_ref(&self.all[GENESIS]), 0)
            .unwrap();
        Replica::open(Arc::new(Mutex::new(store)), self.vault, self.p.id(), me).unwrap()
    }

    /// A replica holding `all[..upto]` (genesis included).
    fn replica_with(&self, upto: usize) -> Replica {
        let r = self.replica(self.w1.id());
        let mut staged = Vec::new();
        let rep = r
            .ingest(self.all[1..upto].to_vec(), &mut staged)
            .expect("ingest");
        assert_eq!(rep.accepted.len(), upto - 1);
        assert!(staged.is_empty());
        r
    }

    fn full(&self) -> Replica {
        self.replica_with(N)
    }

    fn sk_for(&self, author: DeviceId) -> &SigningKey {
        [&self.p, &self.w1, &self.w2]
            .into_iter()
            .find(|d| d.id() == author)
            .map(|d| &d.sk)
            .expect("a member")
    }

    fn pick(&self, indices: &[usize]) -> Vec<SignedOp> {
        indices.iter().map(|&i| self.all[i].clone()).collect()
    }
}

static WORLD: LazyLock<World> = LazyLock::new(World::new);

fn permutation() -> impl Strategy<Value = Vec<usize>> {
    Just((0..N).collect::<Vec<_>>()).prop_shuffle()
}

/// What the replica stores, as header bytes per author. Panics if a log has a gap or a stored op
/// sits in the wrong place: that is the invariant everything else relies on.
fn dump(r: &Replica) -> BTreeMap<DeviceId, Vec<Vec<u8>>> {
    let store = r.store();
    let vv = store.version_vector().unwrap();
    vv.into_iter()
        .map(|(author, max)| {
            let ops = store.ops_after(&author, 0, 10_000).unwrap();
            assert_eq!(ops.len() as u64, max, "gap in the log of {author:?}");
            for (i, op) in ops.iter().enumerate() {
                assert_eq!(op.seq(), i as u64 + 1);
                assert_eq!(op.author(), author);
                op.verify_signature().expect("a stored op verifies");
            }
            (
                author,
                ops.iter().map(|o| o.header_bytes().to_vec()).collect(),
            )
        })
        .collect()
}

#[derive(Default, Debug)]
struct Totals {
    accepted: usize,
    rejected: usize,
    forks: usize,
}

fn deliver(r: &Replica, ops: Vec<SignedOp>, chunk: usize, staged: &mut Vec<SignedOp>) -> Totals {
    let mut t = Totals::default();
    for part in ops.chunks(chunk) {
        let rep = r.ingest(part.to_vec(), staged).expect("ingest");
        t.accepted += rep.accepted.len();
        t.rejected += rep.rejected;
        t.forks += rep.forks.len();
    }
    t
}

proptest! {
    #![proptest_config(config())]

    /// Whatever the order, the batch size and the repeats, once every op has arrived the replica
    /// holds exactly the history: nothing rejected, nothing left staged, no fork reported.
    #[test]
    fn ingest_converges_whatever_the_order_chunking_and_duplicates(
        perm in permutation(),
        dups in vec(0..N, 0..12),
        chunk in 1usize..6,
    ) {
        let w = &*WORLD;
        let r = w.replica(w.w1.id());
        let mut order = dups;
        order.extend(perm);
        let mut staged = Vec::new();
        let t = deliver(&r, w.pick(&order), chunk, &mut staged);
        prop_assert_eq!(t.rejected, 0);
        prop_assert_eq!(t.forks, 0);
        prop_assert_eq!(t.accepted, N - 1, "everything but the genesis op is new");
        prop_assert!(staged.is_empty());
        prop_assert_eq!(dump(&r), w.expected.clone());
        let view = r.view();
        prop_assert!(view.is_active(&w.w1.id()) && view.is_active(&w.w2.id()));
        prop_assert!(!view.is_known(&w.stranger.id()));
    }

    /// Any subset, in any order: the stored logs never have a gap, an op of a member is only stored
    /// after the roster op that introduced the member, and every op is either stored or still
    /// staged and unplaceable (no gap-free, known-author op is left behind).
    #[test]
    fn any_subset_keeps_the_log_gapless_and_loses_nothing(
        perm in permutation(),
        mask in vec(any::<bool>(), N),
        chunk in 1usize..6,
    ) {
        let w = &*WORLD;
        let r = w.replica(w.w2.id());
        let chosen: Vec<usize> = perm.into_iter().filter(|&i| mask[i]).collect();
        let mut staged = Vec::new();
        let t = deliver(&r, w.pick(&chosen), chunk, &mut staged);
        let unique: BTreeSet<usize> = chosen.iter().copied().filter(|&i| i != GENESIS).collect();
        prop_assert_eq!(t.rejected, 0);
        prop_assert_eq!(t.forks, 0);
        prop_assert_eq!(t.accepted + staged.len(), unique.len());

        let stored = dump(&r);
        for (author, headers) in &stored {
            prop_assert_eq!(headers.as_slice(), &w.expected[author][..headers.len()]);
        }
        let vv: VersionVector = r.version_vector().unwrap();
        let have = |d: &Device| vv.get(&d.id()).copied().unwrap_or(0);
        if have(&w.w1) > 0 {
            prop_assert!(have(&w.p) >= 2, "W1 is only known after `P adds W1`");
        }
        if have(&w.w2) > 0 {
            prop_assert!(have(&w.p) >= 3, "W2 is only known after `P adds W2`");
        }
        for op in &staged {
            let author = op.author();
            let known = author == w.p.id()
                || (author == w.w1.id() && have(&w.p) >= 2)
                || (author == w.w2.id() && have(&w.p) >= 3);
            let stored_seq = vv.get(&author).copied().unwrap_or(0);
            prop_assert!(op.seq() > stored_seq);
            prop_assert!(!known || op.seq() > stored_seq + 1, "a placeable op was left staged");
        }
    }

    /// Ops from a stranger stay staged and are never stored; ops for another vault are rejected;
    /// the real history around them is unaffected.
    #[test]
    fn strangers_and_foreign_vault_ops_are_never_stored(
        perm in permutation(),
        mask in vec(any::<bool>(), N),
        stranger_ops in 1u64..4,
        foreign_ops in 1u64..4,
        chunk in 1usize..6,
    ) {
        let w = &*WORLD;
        let r = w.replica_with(3);
        let other = VaultId::random().unwrap();
        let mut incoming: Vec<SignedOp> = perm
            .iter()
            .filter(|&&i| i >= P_FIRST_DATA && mask[i])
            .map(|&i| w.all[i].clone())
            .collect();
        let legit = incoming.len();
        for seq in 1..=stranger_ops {
            incoming.push(create_data_op(w.vault, &w.vk, &w.stranger.sk, seq, &purge()).unwrap());
        }
        for seq in 4..4 + foreign_ops {
            incoming.push(create_data_op(other, &w.vk, &w.p.sk, seq, &purge()).unwrap());
        }
        // Shuffle by rotating: deterministic but moves the bad ops between the good ones.
        let rotate = (perm[0] * 7) % incoming.len();
        incoming.rotate_left(rotate);

        let mut staged = Vec::new();
        let t = deliver(&r, incoming, chunk, &mut staged);
        prop_assert_eq!(t.rejected, foreign_ops as usize);
        prop_assert_eq!(t.forks, 0);
        prop_assert!(t.accepted <= legit);
        let vv = r.version_vector().unwrap();
        prop_assert!(!vv.contains_key(&w.stranger.id()));
        prop_assert!(staged.iter().filter(|o| o.author() == w.stranger.id()).count()
            == stranger_ops as usize);
        prop_assert!(!r.view().is_known(&w.stranger.id()));
        dump(&r);
    }

    /// An op with a bad signature is rejected and leaves no trace, and the genuine op sent next is
    /// still accepted (a forged copy cannot block the position).
    #[test]
    fn forged_signatures_are_rejected_and_do_not_block_the_real_op(
        which in 0usize..3,
        sig in vec(any::<u8>(), 64),
    ) {
        let w = &*WORLD;
        let r = w.replica_with(3);
        let real = &w.all[[P_FIRST_DATA, W1_FIRST_DATA, W2_FIRST_DATA][which]];
        let before = dump(&r);
        let forged =
            SignedOp::from_parts(real.header_bytes(), &sig, real.body().to_vec()).unwrap();
        let mut staged = Vec::new();
        let rep = r.ingest(vec![forged], &mut staged).unwrap();
        prop_assert_eq!(rep.rejected, 1);
        prop_assert!(rep.accepted.is_empty() && rep.forks.is_empty());
        prop_assert_eq!(dump(&r), before);

        let rep = r.ingest(vec![real.clone()], &mut staged).unwrap();
        prop_assert_eq!(rep.accepted.len(), 1);
        prop_assert_eq!(rep.rejected, 0);
    }

    /// A second validly signed op at a position that is already stored is reported as a fork. The
    /// stored op stays, the fork is not written and a roster op fork changes nobody's membership.
    #[test]
    fn a_second_signed_op_at_a_stored_position_is_a_fork_and_changes_nothing(
        at in 0..N,
        repeat in 1usize..3,
    ) {
        let w = &*WORLD;
        let r = w.full();
        let real = &w.all[at];
        let (author, seq) = (real.author(), real.seq());
        let fork = if real.class() == OpClass::Roster {
            let op = if at == GENESIS {
                RosterOp::Genesis {
                    vault_id: w.vault,
                    keys: w.stranger.keys(&w.vault),
                    created_ms: 1,
                }
            } else {
                RosterOp::DeviceAdded { keys: w.stranger.keys(&w.vault), added_ms: 1 }
            };
            create_roster_op(w.vault, w.sk_for(author), seq, &op).unwrap()
        } else {
            create_data_op(w.vault, &w.vk, w.sk_for(author), seq, &purge()).unwrap()
        };
        prop_assert_eq!(fork.op_ref(), real.op_ref());
        prop_assert_ne!(fork.header_bytes(), real.header_bytes());

        let before = dump(&r);
        let version = r.version();
        let mut staged = Vec::new();
        for _ in 0..repeat {
            let rep = r.ingest(vec![fork.clone()], &mut staged).unwrap();
            prop_assert_eq!(rep.forks.len(), 1);
            prop_assert!(rep.accepted.is_empty());
            prop_assert_eq!(rep.rejected, 0);
            prop_assert!(!rep.roster_changed);
        }
        prop_assert_eq!(dump(&r), before);
        prop_assert_eq!(r.version(), version, "nothing was written");
        let view = r.view();
        prop_assert!(!view.is_known(&w.stranger.id()));
        prop_assert!(view.is_active(&w.p.id()) && view.is_active(&w.w1.id())
            && view.is_active(&w.w2.id()));
    }

    /// `ops_missing_for` is exactly the part of the log beyond the peer's version vector, per
    /// author in seq order, whatever the peer claims (including `u64::MAX` and unknown authors).
    #[test]
    fn ops_missing_for_is_the_exact_suffix(
        known in vec(prop_oneof![0u64..=7, Just(u64::MAX)], 3),
        unknown in vec((vec(any::<u8>(), 32), any::<u64>()), 0..3),
    ) {
        let w = &*WORLD;
        let r = w.full();
        let members = [&w.p, &w.w1, &w.w2];
        let mut peer = VersionVector::new();
        for (d, n) in members.iter().zip(&known) {
            peer.insert(d.id(), *n);
        }
        for (id, n) in unknown {
            peer.insert(DeviceId::from_slice(&id).unwrap(), n);
        }
        let missing = r.ops_missing_for(&peer).unwrap();
        let mut got: BTreeMap<DeviceId, Vec<(u64, Vec<u8>)>> = BTreeMap::new();
        for op in &missing {
            got.entry(op.author()).or_default().push((op.seq(), op.header_bytes().to_vec()));
        }
        for d in members {
            let have = peer[&d.id()] as usize;
            let log = &w.expected[&d.id()];
            let want: Vec<(u64, Vec<u8>)> = log
                .iter()
                .enumerate()
                .skip(have.min(log.len()))
                .map(|(i, h)| (i as u64 + 1, h.clone()))
                .collect();
            prop_assert_eq!(got.remove(&d.id()).unwrap_or_default(), want);
        }
        prop_assert!(got.is_empty(), "nothing for authors the replica does not hold");
    }

    /// A replica that holds any part of the history is brought up to date by what the full replica
    /// says it is missing, in any batch size, and a second request finds nothing left.
    #[test]
    fn missing_ops_bring_a_partial_replica_up_to_date(
        perm in permutation(),
        mask in vec(any::<bool>(), N),
        chunk in 1usize..6,
        reversed in any::<bool>(),
    ) {
        let w = &*WORLD;
        let full = w.full();
        let partial = w.replica(w.w2.id());
        let chosen: Vec<usize> = perm.into_iter().filter(|&i| mask[i]).collect();
        deliver(&partial, w.pick(&chosen), chunk, &mut Vec::new());

        let mut missing = full.ops_missing_for(&partial.version_vector().unwrap()).unwrap();
        if reversed {
            missing.reverse();
        }
        let mut staged = Vec::new();
        let t = deliver(&partial, missing, chunk, &mut staged);
        prop_assert_eq!(t.rejected, 0);
        prop_assert_eq!(t.forks, 0);
        prop_assert!(staged.is_empty());
        prop_assert_eq!(dump(&partial), w.expected.clone());
        prop_assert!(full.ops_missing_for(&partial.version_vector().unwrap()).unwrap().is_empty());
        prop_assert!(partial.ops_missing_for(&full.version_vector().unwrap()).unwrap().is_empty());
        prop_assert!(partial.view().is_active(&w.w2.id()));
    }
}

/// The signature covers the whole header: flipping any one bit of it (the signature stays) never
/// gets an op stored, never shows up as a fork of the real op and never changes the roster. A
/// random byte mostly lands on the author, vault or hash fields, so every bit is tried instead.
#[test]
fn a_changed_header_is_never_stored() {
    let w = &*WORLD;
    let r = w.replica_with(3);
    let before = dump(&r);
    let version = r.version();
    let mut reached_ingest = 0;
    for first in [P_FIRST_DATA, W1_FIRST_DATA, W2_FIRST_DATA] {
        let real = &w.all[first];
        for at in 0..real.header_bytes().len() {
            for bit in 0..8 {
                let mut header = real.header_bytes().to_vec();
                header[at] ^= 1 << bit;
                let Ok(changed) =
                    SignedOp::from_parts(&header, real.sig().as_bytes(), real.body().to_vec())
                else {
                    continue; // the body hash or length no longer matches: not even an op
                };
                reached_ingest += 1;
                let rep = r.ingest(vec![changed], &mut Vec::new()).unwrap();
                assert!(rep.accepted.is_empty(), "byte {at} bit {bit} of op {first}");
                assert!(rep.forks.is_empty(), "byte {at} bit {bit} of op {first}");
                assert!(!rep.roster_changed);
            }
        }
    }
    assert!(
        reached_ingest > 500,
        "only {reached_ingest} changed headers parsed"
    );
    assert_eq!(dump(&r), before);
    assert_eq!(r.version(), version, "nothing was written");
}

/// Pinned: `ingest` drops a repeat of an op by (author, seq) and header before it looks at
/// signatures, so a forged copy that comes first in a batch shadows the genuine op for that batch.
/// The version vector does not move, so the next sync sends the op again: the peer could have
/// withheld it anyway. If this ever fails because the genuine op is accepted, the rule got better.
#[test]
fn a_forged_copy_ahead_of_the_real_op_in_one_batch_shadows_it_until_the_next_sync() {
    let w = &*WORLD;
    let r = w.replica_with(3);
    let real = &w.all[P_FIRST_DATA];
    let forged =
        SignedOp::from_parts(real.header_bytes(), &[9u8; 64], real.body().to_vec()).unwrap();
    let mut staged = Vec::new();
    let rep = r
        .ingest(vec![forged.clone(), real.clone()], &mut staged)
        .unwrap();
    assert_eq!((rep.accepted.len(), rep.rejected), (0, 1));
    let rep = r.ingest(vec![real.clone()], &mut staged).unwrap();
    assert_eq!((rep.accepted.len(), rep.rejected), (1, 0));
    // The other order is fine in one batch.
    let r = w.replica_with(3);
    let rep = r
        .ingest(vec![real.clone(), forged], &mut Vec::new())
        .unwrap();
    assert_eq!((rep.accepted.len(), rep.rejected), (1, 0));
}

/// RB-6: ops that cannot be placed are not verified, so a peer could make the receiver keep them
/// without limit. Past `MAX_STAGED_BYTES` the session ends with an error (and ops that could be
/// placed before that point are kept).
#[test]
fn unplaceable_ops_cannot_pile_up_without_a_bound() {
    use lp_core::model::{FieldSet, FieldValue};
    use lp_sync::replica::MAX_STAGED_BYTES;
    let w = &*WORLD;
    let r = w.replica_with(3);
    let big = |seq: u64| {
        let op = DataOp::ItemSet {
            item: ItemId::random().unwrap(),
            hlc: Hlc::default(),
            fields: vec![FieldSet {
                key: "notes".into(),
                value: FieldValue::Text("x".repeat(40_000)),
                base: None,
            }],
        };
        create_data_op(w.vault, &w.vk, &w.p.sk, seq, &op).unwrap()
    };
    // P's seq 4 never arrives: everything from seq 5 on has a gap before it.
    let mut staged = Vec::new();
    let mut seq = 5;
    let mut sent = 0usize;
    let ended = loop {
        let batch: Vec<SignedOp> = (0..20)
            .map(|_| {
                seq += 1;
                big(seq)
            })
            .collect();
        sent += batch.iter().map(SignedOp::encoded_len).sum::<usize>();
        match r.ingest(batch, &mut staged) {
            Ok(rep) => assert!(rep.accepted.is_empty()),
            Err(e) => break e,
        }
        assert!(sent < 4 * MAX_STAGED_BYTES, "never refused");
    };
    assert!(matches!(ended, SyncError::Protocol(_)), "{ended:?}");
    assert!(
        (MAX_STAGED_BYTES..2 * MAX_STAGED_BYTES).contains(&sent),
        "{sent} bytes"
    );
    assert!(staged.is_empty(), "what was held is let go");
}

// verify_peer. The replica knows genesis and `P adds W1`; W2 and the stranger are unknown to it.
const GOOD_ROSTER_OPS: usize = 3; // pool entries 0-2: genesis, add W1, add W2
const POOL: usize = 6; // 3 is a data op, 4 a roster op of another vault, 5 `add W2` with a bad signature

fn proof_pool(w: &World) -> Vec<SignedOp> {
    let other = VaultId::random().unwrap();
    let foreign = create_roster_op(
        other,
        &w.p.sk,
        2,
        &RosterOp::DeviceAdded {
            keys: w.w2.keys(&other),
            added_ms: 0,
        },
    )
    .unwrap();
    let add_w2 = &w.all[ADD_W2];
    let bad_sig =
        SignedOp::from_parts(add_w2.header_bytes(), &[7u8; 64], add_w2.body().to_vec()).unwrap();
    vec![
        w.all[GENESIS].clone(),
        w.all[ADD_W1].clone(),
        w.all[ADD_W2].clone(),
        w.all[W1_FIRST_DATA].clone(),
        foreign,
        bad_sig,
    ]
}

proptest! {
    #![proptest_config(config())]

    /// A key the replica does not know is accepted only with a proof that holds the roster op that
    /// adds it and nothing invalid, and a stranger's key is never accepted whatever the proof.
    #[test]
    fn verify_peer_follows_the_proof_rules(picks in vec(0..POOL, 0..=20)) {
        let w = &*WORLD;
        let r = w.replica_with(2);
        let pool = proof_pool(w);
        let proof: Vec<SignedOp> = picks.iter().map(|&i| pool[i].clone()).collect();

        let stranger = r.verify_peer(&w.stranger.noise.public(), &proof);
        prop_assert!(stranger.is_err(), "stranger accepted with {picks:?}");

        let result = r.verify_peer(&w.w2.noise.public(), &proof);
        let bad = picks.iter().any(|&i| i >= GOOD_ROSTER_OPS);
        let has_add = picks.contains(&2);
        if proof.len() > MAX_PROOF_OPS {
            prop_assert!(matches!(result, Err(SyncError::Protocol(_))), "{result:?}");
        } else if bad || !has_add {
            prop_assert!(matches!(result, Err(SyncError::NotMember)), "{picks:?}: {result:?}");
        } else {
            prop_assert_eq!(result.unwrap(), w.w2.id(), "{:?}", picks);
        }

        // Keys already in the roster do not need the proof at all.
        prop_assert_eq!(r.verify_peer(&w.p.noise.public(), &proof).unwrap(), w.p.id());
        prop_assert_eq!(r.verify_peer(&w.w1.noise.public(), &[]).unwrap(), w.w1.id());
    }
}

/// What a device hands to a peer it has never met is enough to be recognised, and short.
#[test]
fn membership_proofs_are_enough_for_a_new_peer() {
    let w = &*WORLD;
    for device in [&w.p, &w.w1, &w.w2] {
        // `me` only matters for `membership_proof`: build the replica as that device.
        let mut store = Store::open_in_memory().unwrap();
        store.append_batch(&w.all[..N], 0).unwrap();
        let mine =
            Replica::open(Arc::new(Mutex::new(store)), w.vault, w.p.id(), device.id()).unwrap();
        let proof = mine.membership_proof().unwrap();
        assert!(!proof.is_empty() && proof.len() <= MAX_PROOF_OPS);

        // A replica that has only the genesis op and has never heard of the device.
        let fresh = w.replica(w.stranger.id());
        assert_eq!(
            fresh
                .verify_peer(&device.noise.public(), &proof)
                .expect("recognised"),
            device.id()
        );
        // The same proof does not help someone else.
        assert!(
            fresh
                .verify_peer(&w.stranger.noise.public(), &proof)
                .is_err()
        );
    }
}
