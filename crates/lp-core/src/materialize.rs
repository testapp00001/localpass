//! Folds decrypted data ops into the vault state.
//!
//! Every field is a last-writer-wins register ordered by [`Stamp`]. All values
//! ever written are kept, sorted, so the result is the same in any apply order
//! and old values double as history (password history, conflict recovery).

use std::collections::BTreeMap;

use crate::hlc::Hlc;
use crate::ids::{DeviceId, FolderId, ItemId, OpRef};
use crate::model::{DataOp, FieldValue, keys};

/// Total order for LWW: HLC first, then author and seq as tie-breakers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Stamp {
    pub hlc: Hlc,
    pub author: DeviceId,
    pub seq: u64,
}

impl Stamp {
    pub fn op_ref(&self) -> OpRef {
        OpRef {
            author: self.author,
            seq: self.seq,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FieldEntry {
    pub stamp: Stamp,
    pub value: FieldValue,
    pub base: Option<OpRef>,
}

#[derive(Clone, Debug)]
pub struct ItemState {
    pub id: ItemId,
    /// Every value written per field, ascending by stamp; the last one wins.
    pub fields: BTreeMap<String, Vec<FieldEntry>>,
    pub created: Stamp,
    pub updated: Stamp,
}

impl ItemState {
    fn new(id: ItemId, stamp: Stamp) -> Self {
        Self {
            id,
            fields: BTreeMap::new(),
            created: stamp,
            updated: stamp,
        }
    }

    pub fn winner(&self, key: &str) -> Option<&FieldEntry> {
        self.fields.get(key).and_then(|v| v.last())
    }

    /// Current value of a field; `None` if never set or cleared.
    pub fn get(&self, key: &str) -> Option<&FieldValue> {
        self.winner(key).map(|e| &e.value).filter(|v| !v.is_null())
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.get(key).and_then(FieldValue::as_text)
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key).and_then(FieldValue::as_bool).unwrap_or(false)
    }

    pub fn is_trashed(&self) -> bool {
        self.flag(keys::TRASHED)
    }

    /// Older values of a field, newest first (excluding the current one).
    pub fn history(&self, key: &str) -> impl Iterator<Item = &FieldEntry> {
        self.fields
            .get(key)
            .map(|v| &v[..v.len().saturating_sub(1)])
            .unwrap_or(&[])
            .iter()
            .rev()
    }

    /// Values written concurrently with the current one (same base, different
    /// value). Non-empty means two devices changed this field at the same time.
    pub fn conflicts(&self, key: &str) -> Vec<&FieldEntry> {
        let Some(entries) = self.fields.get(key) else {
            return Vec::new();
        };
        let Some(winner) = entries.last() else {
            return Vec::new();
        };
        entries[..entries.len() - 1]
            .iter()
            .filter(|e| e.base == winner.base && e.value != winner.value)
            .collect()
    }

    pub fn conflicted_fields(&self) -> Vec<&str> {
        self.fields
            .keys()
            .filter(|k| !self.conflicts(k).is_empty())
            .map(String::as_str)
            .collect()
    }

    fn wipe(&mut self) {
        for entries in self.fields.values_mut() {
            for e in entries.iter_mut() {
                e.value.wipe();
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct FolderEntry {
    pub stamp: Stamp,
    /// `None` = deleted (tombstone kept so older renames can't resurrect it).
    pub name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeviceMetaEntry {
    pub stamp: Stamp,
    pub name: String,
    pub platform: String,
}

#[derive(Clone, Debug, Default)]
pub struct VaultState {
    pub items: BTreeMap<ItemId, ItemState>,
    pub folders: BTreeMap<FolderId, FolderEntry>,
    pub devices: BTreeMap<DeviceId, DeviceMetaEntry>,
    pub purged: BTreeMap<ItemId, Stamp>,
}

impl VaultState {
    /// Applies one op. Idempotent and order-independent.
    pub fn apply(&mut self, author: DeviceId, seq: u64, op: &DataOp) {
        let Some(hlc) = op.hlc() else { return };
        let stamp = Stamp { hlc, author, seq };
        match op {
            DataOp::ItemSet { item, fields, .. } => {
                if self.purged.contains_key(item) {
                    return;
                }
                let state = self
                    .items
                    .entry(*item)
                    .or_insert_with(|| ItemState::new(*item, stamp));
                state.created = state.created.min(stamp);
                state.updated = state.updated.max(stamp);
                for f in fields {
                    let entries = state.fields.entry(f.key.clone()).or_default();
                    if let Err(pos) = entries.binary_search_by(|e| e.stamp.cmp(&stamp)) {
                        entries.insert(
                            pos,
                            FieldEntry {
                                stamp,
                                value: f.value.clone(),
                                base: f.base,
                            },
                        );
                    }
                }
            }
            DataOp::ItemPurge { item, .. } => {
                let entry = self.purged.entry(*item).or_insert(stamp);
                *entry = (*entry).min(stamp);
                if let Some(mut removed) = self.items.remove(item) {
                    removed.wipe();
                }
            }
            DataOp::FolderSet { folder, name, .. } => {
                let newer = self.folders.get(folder).is_none_or(|cur| stamp > cur.stamp);
                if newer {
                    self.folders.insert(
                        *folder,
                        FolderEntry {
                            stamp,
                            name: name.clone(),
                        },
                    );
                }
            }
            DataOp::DeviceMetaSet {
                device,
                name,
                platform,
                ..
            } => {
                let newer = self.devices.get(device).is_none_or(|cur| stamp > cur.stamp);
                if newer {
                    self.devices.insert(
                        *device,
                        DeviceMetaEntry {
                            stamp,
                            name: name.clone(),
                            platform: platform.clone(),
                        },
                    );
                }
            }
            DataOp::Unknown => {}
        }
    }

    /// Highest HLC seen, to seed the local clock after loading.
    pub fn max_hlc(&self) -> Hlc {
        let items = self.items.values().map(|i| i.updated.hlc);
        let folders = self.folders.values().map(|f| f.stamp.hlc);
        let devices = self.devices.values().map(|d| d.stamp.hlc);
        let purged = self.purged.values().map(|s| s.hlc);
        items
            .chain(folders)
            .chain(devices)
            .chain(purged)
            .max()
            .unwrap_or_default()
    }

    pub fn folder_name(&self, id: &FolderId) -> Option<&str> {
        self.folders.get(id).and_then(|f| f.name.as_deref())
    }

    /// Best-effort wipe of decrypted secrets (call before dropping on lock).
    pub fn wipe(&mut self) {
        for item in self.items.values_mut() {
            item.wipe();
        }
        self.items.clear();
        self.folders.clear();
        self.devices.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FieldSet;
    use proptest::prelude::*;

    fn dev(n: u8) -> DeviceId {
        DeviceId([n; 32])
    }

    fn set(item: ItemId, wall: u64, key: &str, v: &str, base: Option<OpRef>) -> DataOp {
        DataOp::ItemSet {
            item,
            hlc: Hlc {
                wall_ms: wall,
                counter: 0,
            },
            fields: vec![FieldSet {
                key: key.into(),
                value: FieldValue::Text(v.into()),
                base,
            }],
        }
    }

    #[test]
    fn last_writer_wins_and_history_is_kept() {
        let item = ItemId::random().unwrap();
        let mut s = VaultState::default();
        s.apply(dev(1), 1, &set(item, 10, keys::PASSWORD, "one", None));
        let base = Some(OpRef {
            author: dev(1),
            seq: 1,
        });
        s.apply(dev(2), 1, &set(item, 20, keys::PASSWORD, "two", base));
        let it = &s.items[&item];
        assert_eq!(it.text(keys::PASSWORD), Some("two"));
        let hist: Vec<_> = it
            .history(keys::PASSWORD)
            .map(|e| e.value.clone())
            .collect();
        assert_eq!(hist, vec![FieldValue::Text("one".into())]);
        assert!(it.conflicts(keys::PASSWORD).is_empty());
    }

    #[test]
    fn concurrent_edits_are_flagged_as_conflicts() {
        let item = ItemId::random().unwrap();
        let mut s = VaultState::default();
        s.apply(dev(1), 1, &set(item, 10, keys::PASSWORD, "orig", None));
        let base = Some(OpRef {
            author: dev(1),
            seq: 1,
        });
        s.apply(dev(2), 1, &set(item, 20, keys::PASSWORD, "home", base));
        s.apply(dev(3), 1, &set(item, 21, keys::PASSWORD, "work", base));
        let it = &s.items[&item];
        assert_eq!(it.text(keys::PASSWORD), Some("work"));
        let c = it.conflicts(keys::PASSWORD);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].value, FieldValue::Text("home".into()));
        assert_eq!(it.conflicted_fields(), vec![keys::PASSWORD]);
    }

    #[test]
    fn purge_wins_over_edits_in_any_order() {
        let item = ItemId::random().unwrap();
        let edit = set(item, 50, keys::TITLE, "late edit", None);
        let purge = DataOp::ItemPurge {
            item,
            hlc: Hlc {
                wall_ms: 10,
                counter: 0,
            },
        };
        let mut a = VaultState::default();
        a.apply(dev(1), 1, &edit);
        a.apply(dev(2), 1, &purge);
        let mut b = VaultState::default();
        b.apply(dev(2), 1, &purge);
        b.apply(dev(1), 1, &edit);
        assert!(a.items.is_empty() && b.items.is_empty());
    }

    #[test]
    fn folder_tombstone_beats_older_rename() {
        let f = FolderId::random().unwrap();
        let mut s = VaultState::default();
        let del = DataOp::FolderSet {
            folder: f,
            hlc: Hlc {
                wall_ms: 20,
                counter: 0,
            },
            name: None,
        };
        let ren = DataOp::FolderSet {
            folder: f,
            hlc: Hlc {
                wall_ms: 10,
                counter: 0,
            },
            name: Some("Work".into()),
        };
        s.apply(dev(1), 2, &del);
        s.apply(dev(1), 1, &ren);
        assert_eq!(s.folder_name(&f), None);
    }

    fn arb_op(items: Vec<ItemId>) -> impl Strategy<Value = (u8, DataOp)> {
        (
            0u8..3,
            0usize..items.len(),
            0u64..20,
            0usize..3,
            "[a-c]{1,2}",
            any::<bool>(),
        )
            .prop_map(move |(author, idx, wall, key_idx, value, purge)| {
                let item = items[idx];
                let key = [keys::TITLE, keys::PASSWORD, keys::USERNAME][key_idx];
                let op = if purge && wall % 7 == 0 {
                    DataOp::ItemPurge {
                        item,
                        hlc: Hlc {
                            wall_ms: wall,
                            counter: 0,
                        },
                    }
                } else {
                    set(item, wall, key, &value, None)
                };
                (author, op)
            })
    }

    type FieldHistory = Vec<(Stamp, FieldValue)>;
    type Snapshot = Vec<(ItemId, Vec<(String, FieldHistory)>)>;

    fn snapshot(s: &VaultState) -> Snapshot {
        s.items
            .iter()
            .map(|(id, it)| {
                let fields = it
                    .fields
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            v.iter().map(|e| (e.stamp, e.value.clone())).collect(),
                        )
                    })
                    .collect();
                (*id, fields)
            })
            .collect()
    }

    proptest! {
        #[test]
        fn apply_order_does_not_matter(
            ops in proptest::collection::vec(
                arb_op((0..3).map(|i| ItemId(uuid::Uuid::from_u128(i))).collect()), 1..40),
            seed in any::<u64>(),
        ) {
            // Assign seqs per author in generation order.
            let mut next = [0u64; 3];
            let numbered: Vec<_> = ops.into_iter().map(|(a, op)| {
                next[a as usize] += 1;
                (dev(a), next[a as usize], op)
            }).collect();

            let mut forward = VaultState::default();
            for (a, s, op) in &numbered { forward.apply(*a, *s, op); }

            let mut shuffled = numbered.clone();
            let mut x = seed | 1;
            for i in (1..shuffled.len()).rev() {
                x ^= x << 13; x ^= x >> 7; x ^= x << 17;
                shuffled.swap(i, (x % (i as u64 + 1)) as usize);
            }
            let mut other = VaultState::default();
            for (a, s, op) in &shuffled { other.apply(*a, *s, op); }
            // Applying everything twice changes nothing.
            for (a, s, op) in &shuffled { other.apply(*a, *s, op); }

            prop_assert_eq!(snapshot(&forward), snapshot(&other));
        }
    }
}
