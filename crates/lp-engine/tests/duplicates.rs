//! Finding logins that were saved twice (same username and password, same website).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lp_core::crypto::KdfParams;
use lp_engine::items::CustomFieldData;
use lp_engine::{DuplicateGroup, Engine, EngineConfig, EngineError, FileSecretStore, ItemInput};
use lp_sync::service::SyncConfig;

fn unlocked(dir: &Path) -> Engine {
    let mut c = EngineConfig::new(dir, "test");
    c.sync = SyncConfig {
        port: 0,
        bind: std::net::Ipv4Addr::LOCALHOST,
        mdns: false,
        tick: Duration::from_millis(200),
        resync_every: Duration::from_secs(3600),
        addrbook_every: Duration::from_secs(3600),
    };
    c.kdf = KdfParams::insecure_for_tests();
    let e = Engine::open(c, Arc::new(FileSecretStore::new(dir.join("device.key")))).unwrap();
    e.create_vault("test master pw", "Test device").unwrap();
    e
}

fn login(title: &str, urls: &[&str], user: &str, password: &str) -> ItemInput {
    ItemInput {
        title: title.into(),
        username: user.into(),
        password: password.into(),
        urls: urls.iter().map(|u| u.to_string()).collect(),
        ..Default::default()
    }
}

fn add(e: &Engine, title: &str, urls: &[&str], user: &str, password: &str) -> String {
    e.save_item(&login(title, urls, user, password)).unwrap()
}

/// The titles of each group, newest first, groups in the order the engine returns them.
fn titles(groups: &[DuplicateGroup]) -> Vec<Vec<String>> {
    groups
        .iter()
        .map(|g| g.items.iter().map(|i| i.summary.title.clone()).collect())
        .collect()
}

fn groups(e: &Engine, include_subdomains: bool) -> Vec<DuplicateGroup> {
    e.find_duplicate_logins(include_subdomains).unwrap()
}

#[test]
fn a_subdomain_is_its_own_site_unless_the_option_is_on() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "root", &["https://example.com"], "ann", "pw-one");
    add(
        &e,
        "sub a",
        &["https://a.example.com/login"],
        "ann",
        "pw-one",
    );
    add(&e, "sub b", &["https://b.example.com"], "ann", "pw-one");

    assert!(groups(&e, false).is_empty());

    let merged = groups(&e, true);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].sites, vec!["example.com"]);
    assert_eq!(merged[0].items.len(), 3);
}

#[test]
fn www_is_the_same_site_as_the_bare_domain() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "bare", &["example.com"], "ann", "pw-one");
    add(
        &e,
        "www",
        &["https://www.example.com/signin"],
        "ann",
        "pw-one",
    );
    add(&e, "http", &["http://EXAMPLE.com."], "ann", "pw-one");

    let found = groups(&e, false);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].sites, vec!["example.com"]);
    assert_eq!(found[0].items.len(), 3);
}

#[test]
fn different_passwords_or_users_are_not_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "one", &["https://example.com"], "ann", "pw-one");
    add(
        &e,
        "other password",
        &["https://example.com"],
        "ann",
        "pw-two",
    );
    add(&e, "other user", &["https://example.com"], "bob", "pw-one");
    // The password must match exactly, case included.
    add(&e, "other case", &["https://example.com"], "ann", "PW-ONE");
    assert!(groups(&e, false).is_empty());
    assert!(groups(&e, true).is_empty());
}

#[test]
fn usernames_match_without_case_and_without_surrounding_spaces() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(
        &e,
        "first",
        &["https://example.com"],
        "Ann@Example.com",
        "pw-one",
    );
    add(
        &e,
        "second",
        &["https://example.com"],
        "  ann@example.COM ",
        "pw-one",
    );

    let found = groups(&e, false);
    assert_eq!(found.len(), 1);
    // The group shows the username as the newest copy has it, trimmed.
    assert_eq!(found[0].username, "ann@example.COM");
}

#[test]
fn logins_without_a_username_match_each_other_only_on_the_same_site() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "pin 1", &["https://bank.example"], "", "1234-pin");
    add(&e, "pin 2", &["https://bank.example"], "", "1234-pin");
    add(
        &e,
        "pin elsewhere",
        &["https://other.example"],
        "",
        "1234-pin",
    );
    let found = groups(&e, false);
    assert_eq!(
        titles(&found),
        vec![vec!["pin 2".to_string(), "pin 1".to_string()]]
    );
    assert_eq!(found[0].username, "");
}

#[test]
fn trash_other_kinds_and_incomplete_logins_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "kept", &["https://example.com"], "ann", "pw-one");
    let trashed = add(&e, "trashed", &["https://example.com"], "ann", "pw-one");
    e.trash_item(&trashed).unwrap();
    // No website, no password: nothing to compare.
    add(&e, "no site", &[], "ann", "pw-one");
    add(
        &e,
        "blank site",
        &["   ", "ftp://example.com"],
        "ann",
        "pw-one",
    );
    add(&e, "no password", &["https://example.com"], "ann", "");
    // A note or a card that happens to carry the same fields is not a login.
    let mut note = login("a note", &["https://example.com"], "ann", "pw-one");
    note.kind = "note".into();
    e.save_item(&note).unwrap();
    assert!(groups(&e, false).is_empty());

    // Restoring the trashed copy makes the pair a duplicate again.
    e.restore_item(&trashed).unwrap();
    assert_eq!(groups(&e, false).len(), 1);
}

#[test]
fn ip_addresses_ports_and_localhost_keep_their_host() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "nas", &["https://192.168.1.10"], "ann", "pw-one");
    add(&e, "nas again", &["192.168.1.10/login"], "ann", "pw-one");
    add(
        &e,
        "nas port",
        &["https://192.168.1.10:8443"],
        "ann",
        "pw-one",
    );
    add(&e, "other nas", &["https://192.168.1.11"], "ann", "pw-one");
    add(&e, "dev", &["http://localhost:3000"], "ann", "pw-one");
    add(&e, "dev again", &["localhost:3000"], "ann", "pw-one");
    add(
        &e,
        "dev other port",
        &["http://localhost:4000"],
        "ann",
        "pw-one",
    );

    for option in [false, true] {
        let found = groups(&e, option);
        let mut sites: Vec<_> = found.iter().map(|g| g.sites.clone()).collect();
        sites.sort();
        assert_eq!(
            sites,
            vec![vec!["192.168.1.10"], vec!["localhost:3000"]],
            "include_subdomains = {option}"
        );
    }
}

#[test]
fn the_public_suffix_list_decides_what_a_subdomain_is() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "uk root", &["https://example.co.uk"], "ann", "pw-one");
    add(
        &e,
        "uk shop",
        &["https://shop.example.co.uk"],
        "ann",
        "pw-one",
    );
    // `co.uk` is a public suffix, so `other.co.uk` is another site, not a sibling.
    add(&e, "uk other", &["https://other.co.uk"], "ann", "pw-one");
    add(&e, "alice", &["https://alice.github.io"], "ann", "pw-one");
    add(&e, "bob", &["https://bob.github.io"], "ann", "pw-one");

    assert!(groups(&e, false).is_empty());
    let merged = groups(&e, true);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].sites, vec!["example.co.uk"]);
    assert_eq!(merged[0].items.len(), 2);
}

#[test]
fn a_login_with_several_sites_joins_everything_it_shares_into_one_group() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(
        &e,
        "both",
        &["https://a.test", "https://b.test"],
        "ann",
        "pw-one",
    );
    add(&e, "only a", &["https://a.test"], "ann", "pw-one");
    add(&e, "only b", &["https://b.test"], "ann", "pw-one");
    add(&e, "only c", &["https://c.test"], "ann", "pw-one");
    // Same site and user, other password: stays out.
    add(
        &e,
        "a, other password",
        &["https://a.test"],
        "ann",
        "pw-two",
    );

    let found = groups(&e, false);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].sites, vec!["a.test", "b.test"]);
    let mut names: Vec<_> = found[0]
        .items
        .iter()
        .map(|i| i.summary.title.as_str())
        .collect();
    names.sort();
    assert_eq!(names, vec!["both", "only a", "only b"]);
    // Every item is in exactly one group.
    let total: usize = found.iter().map(|g| g.items.len()).sum();
    assert_eq!(total, 3);
}

#[test]
fn groups_list_the_newest_copy_first_and_say_what_each_copy_has_extra() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "old", &["https://example.com"], "ann", "pw-one");
    let mut rich = login(
        "new",
        &["https://www.example.com", "https://example.com/x"],
        "ann",
        "pw-one",
    );
    rich.notes = "recovery codes: 1111".into();
    rich.totp = "JBSWY3DPEHPK3PXP".into();
    rich.custom = vec![CustomFieldData {
        id: None,
        label: "pin".into(),
        value: "9876".into(),
        hidden: true,
    }];
    e.save_item(&rich).unwrap();

    let found = groups(&e, false);
    assert_eq!(found.len(), 1);
    let [newest, oldest] = &found[0].items[..] else {
        panic!("two copies expected: {found:?}");
    };
    assert_eq!(newest.summary.title, "new");
    assert_eq!(oldest.summary.title, "old");
    assert!(newest.has_notes && newest.summary.has_totp);
    assert_eq!(newest.custom_fields, 1);
    assert_eq!(
        newest.urls,
        vec!["https://www.example.com", "https://example.com/x"]
    );
    assert!(!oldest.has_notes && !oldest.summary.has_totp);
    assert_eq!(oldest.custom_fields, 0);
    assert_eq!(newest.updated_by, "Test device");
}

#[test]
fn the_result_carries_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let mut a = login(
        "a",
        &["https://example.com"],
        "ann",
        "Hunter2-secret-password",
    );
    a.notes = "note-body-secret".into();
    a.totp = "JBSWY3DPEHPK3PXP".into();
    a.custom = vec![CustomFieldData {
        id: None,
        label: "label".into(),
        value: "custom-value-secret".into(),
        hidden: true,
    }];
    e.save_item(&a).unwrap();
    add(
        &e,
        "b",
        &["https://example.com"],
        "ann",
        "Hunter2-secret-password",
    );

    let found = groups(&e, false);
    assert_eq!(found.len(), 1);
    let json = serde_json::to_string(&found).unwrap();
    for secret in [
        "Hunter2-secret-password",
        "note-body-secret",
        "JBSWY3DPEHPK3PXP",
        "custom-value-secret",
    ] {
        assert!(!json.contains(secret), "{secret} leaked into {json}");
    }
    assert!(!format!("{found:?}").contains("Hunter2-secret-password"));
    // The fields the screen reads are there under their camelCase names.
    for name in [
        "sites",
        "username",
        "items",
        "hasNotes",
        "customFields",
        "updatedBy",
        "hasTotp",
        "title",
    ] {
        assert!(
            json.contains(&format!("\"{name}\"")),
            "{name} missing from {json}"
        );
    }
}

#[test]
fn a_locked_vault_answers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    add(&e, "one", &["https://example.com"], "ann", "pw-one");
    e.lock();
    assert!(matches!(
        e.find_duplicate_logins(false),
        Err(EngineError::Locked)
    ));
}
