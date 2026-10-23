//! SQLite persistence: the op-log, small key/value metadata, and the local
//! peer address book. Ops are stored as the exact signed bytes.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use lp_core::ids::DeviceId;
use lp_core::op::{OpClass, SignedOp};
use rusqlite::{Connection, OptionalExtension, params};

mod migrate;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("corrupt record: {0}")]
    Corrupt(String),
    #[error("op {author:?}#{seq} is out of order (expected #{expected})")]
    Gap {
        author: DeviceId,
        seq: u64,
        expected: u64,
    },
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

pub type VersionVector = BTreeMap<DeviceId, u64>;

/// What happened to an op offered to [`Store::append_batch`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppendOutcome {
    Inserted,
    /// Already stored with identical content.
    Duplicate,
    /// Same author and seq but different content: the author's log forked.
    Fork,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerRow {
    pub device: DeviceId,
    pub addrs: Vec<String>,
    pub last_seen_ms: Option<u64>,
    pub last_sync_ms: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManualPeer {
    pub addr: String,
    pub allow_public: bool,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        migrate::retry_busy(|| conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(())))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate::run(&mut conn)?;
        Ok(Self { conn })
    }

    /// Deletes all vault data from this device (used when leaving a vault).
    pub fn wipe(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM ops; DELETE FROM meta; DELETE FROM peers; DELETE FROM manual_peers;",
        )?;
        tx.commit()?;
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    // ---- metadata ----

    pub fn meta_get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &[u8]) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn meta_delete(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    /// Sets several metadata keys atomically.
    pub fn meta_set_many(&mut self, entries: &[(&str, &[u8])]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for (k, v) in entries {
            tx.execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- ops ----

    pub fn max_seq(&self, author: &DeviceId) -> Result<u64> {
        let max: Option<i64> = self.conn.query_row(
            "SELECT MAX(seq) FROM ops WHERE author = ?1",
            [author.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        Ok(max.unwrap_or(0) as u64)
    }

    pub fn version_vector(&self) -> Result<VersionVector> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT author, MAX(seq) FROM ops GROUP BY author")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)))?;
        let mut vv = VersionVector::new();
        for row in rows {
            let (author, seq) = row?;
            vv.insert(device_id(&author)?, seq as u64);
        }
        Ok(vv)
    }

    pub fn op_count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM ops", [], |r| r.get(0))?;
        Ok(n as u64)
    }

    pub fn get_op(&self, author: &DeviceId, seq: u64) -> Result<Option<SignedOp>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT header, sig, body FROM ops WHERE author = ?1 AND seq = ?2")?;
        let row = stmt
            .query_row(
                params![author.as_bytes().as_slice(), seq as i64],
                read_parts,
            )
            .optional()?;
        row.map(to_op).transpose()
    }

    /// Ops by `author` with seq in `(after, after + limit]`, ascending.
    pub fn ops_after(&self, author: &DeviceId, after: u64, limit: usize) -> Result<Vec<SignedOp>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT header, sig, body FROM ops WHERE author = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![author.as_bytes().as_slice(), after as i64, limit as i64],
            read_parts,
        )?;
        rows.map(|r| to_op(r?)).collect()
    }

    pub fn ops_by_class(&self, class: OpClass) -> Result<Vec<SignedOp>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT header, sig, body FROM ops WHERE class = ?1 ORDER BY author, seq",
        )?;
        let rows = stmt.query_map([class.to_u8()], read_parts)?;
        rows.map(|r| to_op(r?)).collect()
    }

    /// Appends ops in one transaction. Each op must be the next seq for its
    /// author (relative to the store plus earlier ops in the batch); otherwise
    /// nothing is written and [`StoreError::Gap`] is returned.
    pub fn append_batch(&mut self, ops: &[SignedOp], now_ms: u64) -> Result<Vec<AppendOutcome>> {
        let tx = self.conn.transaction()?;
        let mut outcomes = Vec::with_capacity(ops.len());
        {
            let mut max_stmt = tx.prepare_cached("SELECT MAX(seq) FROM ops WHERE author = ?1")?;
            let mut get_stmt =
                tx.prepare_cached("SELECT header FROM ops WHERE author = ?1 AND seq = ?2")?;
            let mut ins_stmt = tx.prepare_cached(
                "INSERT INTO ops (author, seq, class, header, sig, body, received_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for op in ops {
                let author = op.author();
                let max: Option<i64> =
                    max_stmt.query_row([author.as_bytes().as_slice()], |r| r.get(0))?;
                let max = max.unwrap_or(0) as u64;
                if op.seq() <= max {
                    let stored: Vec<u8> = get_stmt.query_row(
                        params![author.as_bytes().as_slice(), op.seq() as i64],
                        |r| r.get(0),
                    )?;
                    outcomes.push(if stored == op.header_bytes().as_slice() {
                        AppendOutcome::Duplicate
                    } else {
                        AppendOutcome::Fork
                    });
                    continue;
                }
                if op.seq() != max + 1 {
                    return Err(StoreError::Gap {
                        author,
                        seq: op.seq(),
                        expected: max + 1,
                    });
                }
                ins_stmt.execute(params![
                    author.as_bytes().as_slice(),
                    op.seq() as i64,
                    op.class().to_u8(),
                    op.header_bytes().as_slice(),
                    op.sig().as_bytes().as_slice(),
                    op.body(),
                    now_ms as i64,
                ])?;
                outcomes.push(AppendOutcome::Inserted);
            }
        }
        tx.commit()?;
        Ok(outcomes)
    }

    /// Visits every op in (author, seq) order without loading all into memory.
    pub fn for_each_op(&self, mut f: impl FnMut(SignedOp)) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT header, sig, body FROM ops ORDER BY author, seq")?;
        let rows = stmt.query_map([], read_parts)?;
        for row in rows {
            f(to_op(row?)?);
        }
        Ok(())
    }

    // ---- peers (local only, never synced) ----

    pub fn peers(&self) -> Result<Vec<PeerRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT device, addrs, last_seen_ms, last_sync_ms, last_error FROM peers ORDER BY device",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (device, addrs, seen, sync, err) = row?;
            Ok(PeerRow {
                device: device_id(&device)?,
                addrs: serde_json::from_str(&addrs)
                    .map_err(|e| StoreError::Corrupt(e.to_string()))?,
                last_seen_ms: seen.map(|v| v as u64),
                last_sync_ms: sync.map(|v| v as u64),
                last_error: err,
            })
        })
        .collect()
    }

    /// Merges newly seen addresses into a peer's list (most recent first, capped).
    pub fn peer_seen(&self, device: &DeviceId, addrs: &[String], now_ms: u64) -> Result<()> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT addrs FROM peers WHERE device = ?1",
                [device.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let mut merged: Vec<String> = addrs.to_vec();
        if let Some(json) = existing {
            let old: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
            merged.extend(old.into_iter().filter(|a| !addrs.contains(a)));
        }
        merged.truncate(8);
        let json = serde_json::to_string(&merged).expect("strings serialize");
        self.conn.execute(
            "INSERT INTO peers (device, addrs, last_seen_ms) VALUES (?1, ?2, ?3)
             ON CONFLICT(device) DO UPDATE SET addrs = excluded.addrs, last_seen_ms = excluded.last_seen_ms",
            params![device.as_bytes().as_slice(), json, now_ms as i64],
        )?;
        Ok(())
    }

    /// Merges addresses another member reported for `device` (doesn't touch timestamps).
    pub fn peer_hint(&self, device: &DeviceId, addrs: &[String]) -> Result<()> {
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT addrs FROM peers WHERE device = ?1",
                [device.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        let mut merged: Vec<String> = existing
            .map(|json| serde_json::from_str(&json).unwrap_or_default())
            .unwrap_or_default();
        for a in addrs {
            if !merged.contains(a) {
                merged.push(a.clone());
            }
        }
        merged.truncate(8);
        let json = serde_json::to_string(&merged).expect("strings serialize");
        self.conn.execute(
            "INSERT INTO peers (device, addrs) VALUES (?1, ?2)
             ON CONFLICT(device) DO UPDATE SET addrs = excluded.addrs",
            params![device.as_bytes().as_slice(), json],
        )?;
        Ok(())
    }

    pub fn peer_sync_result(
        &self,
        device: &DeviceId,
        error: Option<&str>,
        now_ms: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO peers (device, addrs, last_sync_ms, last_error) VALUES (?1, '[]', ?2, ?3)
             ON CONFLICT(device) DO UPDATE SET
               last_sync_ms = CASE WHEN ?3 IS NULL THEN ?2 ELSE peers.last_sync_ms END,
               last_error = ?3",
            params![device.as_bytes().as_slice(), now_ms as i64, error],
        )?;
        Ok(())
    }

    pub fn manual_peers(&self) -> Result<Vec<ManualPeer>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT addr, allow_public FROM manual_peers ORDER BY addr")?;
        let rows = stmt.query_map([], |r| {
            Ok(ManualPeer {
                addr: r.get(0)?,
                allow_public: r.get::<_, i64>(1)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn add_manual_peer(&self, addr: &str, allow_public: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO manual_peers (addr, allow_public) VALUES (?1, ?2)
             ON CONFLICT(addr) DO UPDATE SET allow_public = excluded.allow_public",
            params![addr, allow_public as i64],
        )?;
        Ok(())
    }

    pub fn remove_manual_peer(&self, addr: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM manual_peers WHERE addr = ?1", [addr])?;
        Ok(())
    }
}

type Parts = (Vec<u8>, Vec<u8>, Vec<u8>);

fn read_parts(r: &rusqlite::Row<'_>) -> rusqlite::Result<Parts> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
}

fn to_op((header, sig, body): Parts) -> Result<SignedOp> {
    SignedOp::from_parts(&header, &sig, body).map_err(|e| StoreError::Corrupt(e.to_string()))
}

fn device_id(bytes: &[u8]) -> Result<DeviceId> {
    DeviceId::from_slice(bytes).map_err(|e| StoreError::Corrupt(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::crypto::{Key32, SigningKey};
    use lp_core::hlc::Hlc;
    use lp_core::ids::{ItemId, VaultId};
    use lp_core::model::DataOp;
    use lp_core::op::create_data_op;

    /// On Android the UI and the background sync worker can both open a brand-new database
    /// at the same moment; neither may fail while creating the schema.
    #[test]
    fn concurrent_first_open_is_safe() {
        for round in 0..40 {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("vault.db");
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    let (path, barrier) = (path.clone(), barrier.clone());
                    std::thread::spawn(move || {
                        barrier.wait();
                        Store::open(&path).map(|_| ())
                    })
                })
                .collect();
            for h in handles {
                h.join()
                    .unwrap()
                    .unwrap_or_else(|e| panic!("round {round}: {e}"));
            }
        }
    }

    fn op(vault: VaultId, vk: &Key32, sk: &SigningKey, seq: u64) -> SignedOp {
        let data = DataOp::ItemPurge {
            item: ItemId::random().unwrap(),
            hlc: Hlc::default(),
        };
        create_data_op(vault, vk, sk, seq, &data).unwrap()
    }

    #[test]
    fn append_is_contiguous_and_detects_duplicates_and_forks() {
        let mut store = Store::open_in_memory().unwrap();
        let (vault, vk, sk) = (
            VaultId::random().unwrap(),
            Key32::random().unwrap(),
            SigningKey::generate().unwrap(),
        );
        let a1 = op(vault, &vk, &sk, 1);
        let a2 = op(vault, &vk, &sk, 2);
        assert_eq!(
            store.append_batch(&[a1.clone(), a2.clone()], 0).unwrap(),
            vec![AppendOutcome::Inserted, AppendOutcome::Inserted]
        );
        assert_eq!(store.max_seq(&sk.device_id()).unwrap(), 2);

        let fork = op(vault, &vk, &sk, 2);
        assert_eq!(
            store.append_batch(&[a1.clone(), fork], 0).unwrap(),
            vec![AppendOutcome::Duplicate, AppendOutcome::Fork]
        );

        let gap = op(vault, &vk, &sk, 5);
        assert!(matches!(
            store.append_batch(&[gap], 0),
            Err(StoreError::Gap { expected: 3, .. })
        ));

        assert_eq!(store.get_op(&sk.device_id(), 2).unwrap().unwrap(), a2);
        assert_eq!(store.ops_after(&sk.device_id(), 1, 10).unwrap(), vec![a2]);
        let vv = store.version_vector().unwrap();
        assert_eq!(vv.get(&sk.device_id()), Some(&2));
    }

    #[test]
    fn failed_batch_writes_nothing() {
        let mut store = Store::open_in_memory().unwrap();
        let (vault, vk, sk) = (
            VaultId::random().unwrap(),
            Key32::random().unwrap(),
            SigningKey::generate().unwrap(),
        );
        let res = store.append_batch(&[op(vault, &vk, &sk, 1), op(vault, &vk, &sk, 3)], 0);
        assert!(res.is_err());
        assert_eq!(store.op_count().unwrap(), 0);
    }

    #[test]
    fn meta_and_peers_persist_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.db");
        let dev = DeviceId([9; 32]);
        {
            let mut store = Store::open(&path).unwrap();
            store.meta_set("a", b"1").unwrap();
            store.meta_set_many(&[("b", b"2"), ("a", b"3")]).unwrap();
            store
                .peer_seen(&dev, &["10.0.0.2:47821".into()], 5)
                .unwrap();
            store
                .peer_seen(&dev, &["192.168.1.9:47821".into()], 6)
                .unwrap();
            store.peer_sync_result(&dev, None, 7).unwrap();
            store.peer_sync_result(&dev, Some("timeout"), 8).unwrap();
            store.add_manual_peer("10.1.2.3:47821", false).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.meta_get("a").unwrap().as_deref(), Some(&b"3"[..]));
        assert_eq!(store.meta_get("b").unwrap().as_deref(), Some(&b"2"[..]));
        assert_eq!(store.meta_get("zzz").unwrap(), None);
        let peers = store.peers().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers[0].addrs,
            vec![
                "192.168.1.9:47821".to_string(),
                "10.0.0.2:47821".to_string()
            ]
        );
        assert_eq!(peers[0].last_seen_ms, Some(6));
        assert_eq!(peers[0].last_sync_ms, Some(7));
        assert_eq!(peers[0].last_error.as_deref(), Some("timeout"));
        assert_eq!(
            store.manual_peers().unwrap(),
            vec![ManualPeer {
                addr: "10.1.2.3:47821".into(),
                allow_public: false
            }]
        );
    }
}
