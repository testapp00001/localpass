//! Windows' credential cache for the authenticator.
//!
//! Windows only routes a sign-in to a plugin for credentials it has in its cache (a registration needs
//! none), so the cache must mirror the passkeys in the vault. [`sync`] makes it so: list every passkey
//! from the app, read the cache, diff, and add or remove ONE row at a time (the Add/Remove calls take
//! arrays but return one HRESULT, so a single bad row would fail a batch). `NTE_EXISTS` and
//! `NTE_NOT_FOUND` count as success; `ERROR_FILE_NOT_FOUND` from the read is an empty cache (the
//! [`WebAuthn::all_credentials`] wrapper already says so).
//!
//! A failed or refused listing (app not running, vault locked, not paired) changes NOTHING: an error is
//! never diffed as an empty vault, which would wipe the cache.
//!
//! The calls need package identity (they act for the registering package), so only the packaged
//! provider can use [`OsCache`]; everything else here is pure and tested against a fake.

use std::collections::HashMap;

use windows::core::{GUID, HRESULT};

use crate::plog;
use crate::relay::Relay;
use crate::sys::{CredentialRow, NTE_EXISTS, NTE_NOT_FOUND, WebAuthn};
use crate::translate::cache_row;

/// The three cache operations, so [`sync`] can be tested without the OS.
pub trait CacheApi {
    fn get_all(&self) -> Result<Vec<CredentialRow>, HRESULT>;
    fn add(&self, row: &CredentialRow) -> Result<(), HRESULT>;
    fn remove(&self, row: &CredentialRow) -> Result<(), HRESULT>;
}

/// The real cache of one authenticator.
pub struct OsCache<'a> {
    pub api: &'a WebAuthn,
    pub clsid: GUID,
}

impl CacheApi for OsCache<'_> {
    fn get_all(&self) -> Result<Vec<CredentialRow>, HRESULT> {
        Ok(self.api.all_credentials(&self.clsid)?.rows())
    }

    fn add(&self, row: &CredentialRow) -> Result<(), HRESULT> {
        self.api
            .add_credentials(&self.clsid, std::slice::from_ref(row))
    }

    fn remove(&self, row: &CredentialRow) -> Result<(), HRESULT> {
        self.api
            .remove_credentials(&self.clsid, std::slice::from_ref(row))
    }
}

/// What a sync has to do.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub remove: Vec<CredentialRow>,
    pub add: Vec<CredentialRow>,
}

type RowKey = (Vec<u8>, String);

fn key(row: &CredentialRow) -> RowKey {
    (row.credential_id.clone(), row.rp_id.clone())
}

/// The difference between the cache (`have`) and the vault (`wanted`). Rows are identified by
/// credential id and RP id; a row whose display fields changed is removed and added again. Pure.
pub fn plan(wanted: &[CredentialRow], have: &[CredentialRow]) -> Plan {
    let mut wanted_by_key: HashMap<RowKey, &CredentialRow> = HashMap::new();
    for row in wanted {
        wanted_by_key.entry(key(row)).or_insert(row);
    }
    let mut have_by_key: HashMap<RowKey, &CredentialRow> = HashMap::new();
    for row in have {
        have_by_key.entry(key(row)).or_insert(row);
    }
    let mut plan = Plan::default();
    // Keep the input order so the result is deterministic.
    for row in have {
        match wanted_by_key.get(&key(row)) {
            None => plan.remove.push(row.clone()),
            Some(want) if *want != row => plan.remove.push(row.clone()),
            Some(_) => {}
        }
    }
    let mut queued = std::collections::HashSet::new();
    for row in wanted {
        let k = key(row);
        let current = have_by_key.get(&k).is_some_and(|have| *have == row);
        if !current && queued.insert(k) {
            plan.add.push(row.clone());
        }
    }
    plan.remove.dedup_by(|a, b| key(a) == key(b));
    plan
}

/// Adds one row; a row that is already there is fine.
pub fn add_one(cache: &dyn CacheApi, row: &CredentialRow) -> Result<(), HRESULT> {
    match cache.add(row) {
        Err(hr) if hr != NTE_EXISTS => Err(hr),
        _ => Ok(()),
    }
}

/// Removes one row; a row that is already gone is fine.
pub fn remove_one(cache: &dyn CacheApi, row: &CredentialRow) -> Result<(), HRESULT> {
    match cache.remove(row) {
        Err(hr) if hr != NTE_NOT_FOUND => Err(hr),
        _ => Ok(()),
    }
}

/// The outcome of [`sync`].
#[derive(Debug, PartialEq, Eq)]
pub enum SyncOutcome {
    /// The cache was brought in line (some rows may have failed: see `failed`).
    Done {
        added: usize,
        removed: usize,
        unchanged: usize,
        failed: usize,
    },
    /// Nothing was changed, and why: the app could not list the passkeys, or the cache could not be read.
    Skipped(String),
}

/// Mirrors the vault's passkeys into the cache.
pub fn sync(relay: &dyn Relay, cache: &dyn CacheApi) -> SyncOutcome {
    let passkeys = match relay.list_all() {
        Ok(passkeys) => passkeys,
        Err(e) => return SyncOutcome::Skipped(format!("the app could not list passkeys: {e}")),
    };
    let wanted: Vec<CredentialRow> = passkeys.iter().map(cache_row).collect();
    let have = match cache.get_all() {
        Ok(have) => have,
        Err(hr) => {
            return SyncOutcome::Skipped(format!(
                "cannot read the credential cache (hr {})",
                crate::log::hr(hr)
            ));
        }
    };
    let plan = plan(&wanted, &have);
    let unchanged = have.len().saturating_sub(plan.remove.len());
    let (mut added, mut removed, mut failed) = (0, 0, 0);
    for row in &plan.remove {
        match remove_one(cache, row) {
            Ok(()) => removed += 1,
            Err(hr) => {
                failed += 1;
                plog!("cache: remove failed hr={}", crate::log::hr(hr));
            }
        }
    }
    for row in &plan.add {
        match add_one(cache, row) {
            Ok(()) => added += 1,
            Err(hr) => {
                failed += 1;
                plog!(
                    "cache: add {} failed hr={}",
                    crate::log::short_id(&row.credential_id),
                    crate::log::hr(hr)
                );
            }
        }
    }
    SyncOutcome::Done {
        added,
        removed,
        unchanged,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relay::RelayError;
    use crate::testing::{FakeRelay, MemoryCache};
    use lp_core::passkey::PasskeyCandidate;
    use windows::Win32::Foundation::E_FAIL;

    fn row(id: u8, rp: &str, user: &str) -> CredentialRow {
        CredentialRow {
            credential_id: vec![id; 32],
            rp_id: rp.into(),
            rp_name: rp.into(),
            user_id: vec![id],
            user_name: user.into(),
            user_display_name: user.into(),
        }
    }

    fn candidate(id: u8, rp: &str, user: &str) -> PasskeyCandidate {
        PasskeyCandidate {
            item_id: format!("item-{id}"),
            rp_id: rp.into(),
            rp_name: rp.into(),
            user_name: user.into(),
            user_display_name: user.into(),
            credential_id: vec![id; 32],
            user_handle: vec![id],
            created_ms: 1,
        }
    }

    #[test]
    fn the_diff_adds_removes_and_replaces() {
        let a = row(1, "a.example", "alice");
        let b = row(2, "b.example", "bob");
        let c = row(3, "c.example", "carol");
        // Nothing to do.
        assert_eq!(
            plan(&[a.clone(), b.clone()], &[b.clone(), a.clone()]),
            Plan::default()
        );
        // Missing in the cache -> add; extra in the cache -> remove.
        let p = plan(&[a.clone(), b.clone()], &[b.clone(), c.clone()]);
        assert_eq!((p.add, p.remove), (vec![a.clone()], vec![c.clone()]));
        // Empty both ways.
        assert_eq!(plan(&[], &[]), Plan::default());
        assert_eq!(plan(std::slice::from_ref(&a), &[]).add, vec![a.clone()]);
        assert_eq!(plan(&[], std::slice::from_ref(&a)).remove, vec![a.clone()]);
        // A changed display name is a remove of the old row plus an add of the new one.
        let renamed = CredentialRow {
            user_display_name: "Alice Q.".into(),
            ..a.clone()
        };
        let p = plan(std::slice::from_ref(&renamed), std::slice::from_ref(&a));
        assert_eq!((p.remove, p.add), (vec![a.clone()], vec![renamed]));
        // Duplicates in the vault list are added once; the same id for another RP is another row.
        let p = plan(&[a.clone(), a.clone()], &[]);
        assert_eq!(p.add.len(), 1);
        let other_rp = CredentialRow {
            rp_id: "z.example".into(),
            ..a.clone()
        };
        assert_eq!(
            plan(&[a.clone(), other_rp.clone()], &[a]).add,
            vec![other_rp]
        );
    }

    #[test]
    fn sync_mirrors_the_vault_one_row_at_a_time() {
        let relay = FakeRelay::with_passkeys(vec![
            candidate(1, "a.example", "alice"),
            candidate(2, "b.example", "bob"),
        ]);
        let cache = MemoryCache::default();
        cache.push(row(9, "gone.example", "old"));
        cache.push(row(1, "a.example", "alice"));
        let outcome = sync(&relay, &cache);
        assert_eq!(
            outcome,
            SyncOutcome::Done {
                added: 1,
                removed: 1,
                unchanged: 1,
                failed: 0
            }
        );
        let ids: Vec<u8> = cache.rows().iter().map(|r| r.credential_id[0]).collect();
        assert_eq!(ids, vec![1, 2]);
        // A second run has nothing to do.
        assert_eq!(
            sync(&relay, &cache),
            SyncOutcome::Done {
                added: 0,
                removed: 0,
                unchanged: 2,
                failed: 0
            }
        );
    }

    #[test]
    fn an_empty_but_successful_listing_empties_the_cache() {
        let relay = FakeRelay::with_passkeys(vec![]);
        let cache = MemoryCache::default();
        cache.push(row(1, "a.example", "alice"));
        assert!(matches!(
            sync(&relay, &cache),
            SyncOutcome::Done { removed: 1, .. }
        ));
        assert!(cache.rows().is_empty());
    }

    #[test]
    fn a_failed_listing_changes_nothing() {
        for error in [
            RelayError::Cancelled, // a locked vault that stayed locked
            RelayError::NotPaired,
            RelayError::Unreachable("not running".into()),
            RelayError::Other("boom".into()),
        ] {
            let relay = FakeRelay::failing_list(error);
            let cache = MemoryCache::default();
            cache.push(row(1, "a.example", "alice"));
            cache.push(row(2, "b.example", "bob"));
            let outcome = sync(&relay, &cache);
            assert!(matches!(outcome, SyncOutcome::Skipped(_)), "{outcome:?}");
            assert_eq!(cache.rows().len(), 2, "the cache is untouched");
            assert!(
                cache.calls().is_empty(),
                "not even read: {:?}",
                cache.calls()
            );
        }
    }

    #[test]
    fn an_unreadable_cache_is_skipped_without_writes() {
        let relay = FakeRelay::with_passkeys(vec![candidate(1, "a.example", "alice")]);
        let cache = MemoryCache::failing_reads();
        assert!(matches!(sync(&relay, &cache), SyncOutcome::Skipped(_)));
        assert_eq!(cache.calls().as_slice(), ["get_all"]);
    }

    #[test]
    fn one_bad_row_does_not_stop_the_rest() {
        let relay = FakeRelay::with_passkeys(vec![
            candidate(1, "a.example", "alice"),
            candidate(2, "b.example", "bob"),
            candidate(3, "c.example", "carol"),
        ]);
        let cache = MemoryCache::failing_add_for(2);
        assert_eq!(
            sync(&relay, &cache),
            SyncOutcome::Done {
                added: 2,
                removed: 0,
                unchanged: 0,
                failed: 1
            }
        );
    }

    #[test]
    fn existing_and_missing_rows_count_as_success() {
        let cache = MemoryCache::default();
        let r = row(1, "a.example", "alice");
        assert!(add_one(&cache, &r).is_ok());
        assert!(add_one(&cache, &r).is_ok(), "NTE_EXISTS is fine");
        assert!(remove_one(&cache, &r).is_ok());
        assert!(remove_one(&cache, &r).is_ok(), "NTE_NOT_FOUND is fine");
        let failing = MemoryCache::failing_add_for(1);
        assert_eq!(add_one(&failing, &r), Err(E_FAIL));
    }
}
