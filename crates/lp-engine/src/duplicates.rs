//! Logins that were saved twice: the same username and the same password for the same website.
//!
//! Only live logins (not in the trash, kind `login`) with a password and at least one readable
//! website are looked at. The username is compared without case and without surrounding spaces,
//! the password exactly. A website is named by [`lp_core::origin::site_key`], so `www.example.com`
//! and `example.com` are one site, other subdomains are separate unless `include_subdomains` is on.
//! A login with several websites joins every group it shares a website with, and the groups it
//! touches become one (union-find), so an item is listed in exactly one group.
//!
//! The result carries no secret: no password, note text, 2FA secret or custom field value, only
//! whether the last three exist, so the screen can warn before a copy with extras goes to the trash.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use lp_core::materialize::{ItemState, VaultState};
use lp_core::model::{FieldValue, keys as fkeys, kinds};
use lp_core::origin::site_key;
use serde::Serialize;

use crate::items::{self, ItemSummary};
use crate::{Engine, Result};

/// One saved copy in a group.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateItem {
    #[serde(flatten)]
    pub summary: ItemSummary,
    /// Every website saved on this copy, as typed.
    pub urls: Vec<String>,
    pub has_notes: bool,
    /// How many custom fields it has.
    pub custom_fields: usize,
    /// The device that last changed it.
    pub updated_by: String,
}

/// Logins that share a username, a password and at least one website.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateGroup {
    /// The websites that two or more of the copies share (see `site_key`).
    pub sites: Vec<String>,
    /// The username as the newest copy has it.
    pub username: String,
    /// Two or more copies, the most recently changed first.
    pub items: Vec<DuplicateItem>,
}

struct Candidate<'a> {
    item: &'a ItemState,
    /// Trimmed and lower case.
    username: String,
    password: &'a str,
    sites: BTreeSet<String>,
}

fn root(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn urls_of(item: &ItemState) -> &[String] {
    item.get(fkeys::URLS)
        .and_then(FieldValue::as_list)
        .unwrap_or(&[])
}

pub(crate) fn find(state: &VaultState, include_subdomains: bool) -> Vec<DuplicateGroup> {
    let candidates: Vec<Candidate> = state
        .items
        .values()
        .filter(|i| !i.is_trashed() && i.text(fkeys::KIND).unwrap_or(kinds::LOGIN) == kinds::LOGIN)
        .filter_map(|item| {
            let password = item.text(fkeys::PASSWORD).filter(|p| !p.is_empty())?;
            let sites: BTreeSet<String> = urls_of(item)
                .iter()
                .filter_map(|u| site_key(u, include_subdomains))
                .collect();
            if sites.is_empty() {
                return None;
            }
            Some(Candidate {
                item,
                username: item
                    .text(fkeys::USERNAME)
                    .unwrap_or("")
                    .trim()
                    .to_lowercase(),
                password,
                sites,
            })
        })
        .collect();

    // Two logins are joined when one (username, password, website) is on both. The keys borrow the
    // passwords from the vault state, so no secret is copied.
    let mut parent: Vec<usize> = (0..candidates.len()).collect();
    let mut seen: HashMap<(&str, &str, &str), usize> = HashMap::new();
    for (i, c) in candidates.iter().enumerate() {
        for site in &c.sites {
            match seen.entry((c.username.as_str(), c.password, site.as_str())) {
                Entry::Occupied(first) => {
                    let (a, b) = (root(&mut parent, i), root(&mut parent, *first.get()));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
                Entry::Vacant(slot) => {
                    slot.insert(i);
                }
            }
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..candidates.len() {
        let r = root(&mut parent, i);
        members.entry(r).or_default().push(i);
    }

    let mut groups: Vec<DuplicateGroup> = members
        .into_values()
        .filter(|m| m.len() >= 2)
        .map(|m| {
            let mut copies: Vec<&Candidate> = m.iter().map(|&i| &candidates[i]).collect();
            copies.sort_by_key(|c| (std::cmp::Reverse(c.item.updated.hlc), c.item.id));
            let mut count: BTreeMap<&str, usize> = BTreeMap::new();
            for c in &copies {
                for site in &c.sites {
                    *count.entry(site.as_str()).or_default() += 1;
                }
            }
            DuplicateGroup {
                sites: count
                    .into_iter()
                    .filter(|&(_, n)| n >= 2)
                    .map(|(site, _)| site.to_string())
                    .collect(),
                username: copies[0]
                    .item
                    .text(fkeys::USERNAME)
                    .unwrap_or("")
                    .trim()
                    .to_string(),
                items: copies.iter().map(|c| describe(state, c.item)).collect(),
            }
        })
        .collect();
    groups.sort_by_cached_key(|g| {
        (
            g.sites.first().cloned(),
            g.username.to_lowercase(),
            g.items.first().map(|i| i.summary.id.clone()),
        )
    });
    groups
}

fn describe(state: &VaultState, item: &ItemState) -> DuplicateItem {
    DuplicateItem {
        summary: items::summary(item),
        urls: urls_of(item).to_vec(),
        has_notes: item.text(fkeys::NOTES).is_some(),
        custom_fields: item
            .fields
            .keys()
            .filter(|k| k.starts_with(fkeys::CUSTOM_PREFIX) && item.get(k).is_some())
            .count(),
        updated_by: items::device_label(state, &item.updated.author),
    }
}

impl Engine {
    /// Groups of live logins with the same username (without case) and password for the same website.
    /// `include_subdomains` counts `a.example.com` and `example.com` as one site. Contains no secrets.
    pub fn find_duplicate_logins(&self, include_subdomains: bool) -> Result<Vec<DuplicateGroup>> {
        self.with_session(|s, _| Ok(find(&s.state, include_subdomains)))
    }
}
