//! A backup is what to rely on when every device is lost (README, Settings), so it must carry
//! passkeys: the private key cannot be recreated, and a passkey that came back as an empty login
//! would lock the user out of an account that has no password (phase 5.3, RB-1).

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lp_core::crypto::{KdfParams, sha256};
use lp_core::passkey::PasskeyPublic;
use lp_engine::{
    Engine, EngineConfig, FileSecretStore, ItemInput, ItemQuery, PasskeyAssertRequest,
    PasskeyCreateRequest,
};
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

fn create(e: &Engine, rp_id: &str, user: &str, handle: &[u8]) -> lp_engine::PasskeyCreated {
    e.passkey_create(&PasskeyCreateRequest {
        origin: format!("https://{rp_id}"),
        rp_id: rp_id.into(),
        rp_name: "Example".into(),
        user_handle: handle.to_vec(),
        user_name: user.into(),
        user_display_name: user.to_uppercase(),
        algorithms: vec![-7],
        exclude: vec![],
        user_verified: true,
    })
    .unwrap()
}

/// Signs in with the passkey for `rp_id` in `e` and checks the signature against `public`.
fn signs_in_as(e: &Engine, rp_id: &str, public: &PasskeyPublic) {
    let candidates = e
        .passkey_candidates(&format!("https://{rp_id}"), rp_id, &[])
        .unwrap();
    assert_eq!(candidates.len(), 1, "exactly one passkey for {rp_id}");
    let client_data = br#"{"type":"webauthn.get","challenge":"c2lnbi1pbg"}"#;
    let got = e
        .passkey_assert(&PasskeyAssertRequest {
            origin: format!("https://{rp_id}"),
            rp_id: rp_id.into(),
            item_id: candidates[0].item_id.clone(),
            client_data_hash: sha256(client_data).to_vec(),
            user_verified: true,
        })
        .unwrap();
    let mut signed = got.auth_data.clone();
    signed.extend_from_slice(&sha256(client_data));
    public
        .verify(&signed, &got.signature)
        .expect("the restored passkey signs with the original key");
}

fn vault_with_passkeys() -> (tempfile::TempDir, Engine, lp_engine::PasskeyCreated) {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    e.save_item(&ItemInput {
        title: "Shop".into(),
        username: "me".into(),
        password: "shop-pw".into(),
        urls: vec!["https://shop.example".into()],
        ..Default::default()
    })
    .unwrap();
    let created = create(&e, "example.com", "alice", b"handle-1");
    (dir, e, created)
}

#[test]
fn an_encrypted_backup_restores_passkeys_that_still_sign() {
    let (_dir, a, created) = vault_with_passkeys();
    let public = PasskeyPublic::from_cose(&created.public_key).unwrap();
    let blob = a.export_backup("backup password").unwrap();

    let dir_b = tempfile::tempdir().unwrap();
    let b = unlocked(dir_b.path());
    let report = b
        .import_file(&blob, Some("backup password"), None, true)
        .unwrap();
    assert_eq!((report.imported, report.duplicates), (2, 0), "{report:?}");
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    // Not an empty login: a passkey with its account, usable for sign-in.
    let all = b.passkey_list_all().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].credential_id, created.credential_id);
    assert_eq!(
        (all[0].rp_id.as_str(), all[0].user_name.as_str()),
        ("example.com", "alice")
    );
    assert_eq!(all[0].user_handle, b"handle-1");
    assert!(
        b.find_logins("https://example.com").unwrap().is_empty(),
        "no stub login"
    );
    let items = b.list_items(&ItemQuery::default()).unwrap();
    assert_eq!(items.iter().filter(|i| i.kind == "passkey").count(), 1);
    assert_eq!(items.iter().filter(|i| i.kind == "login").count(), 1);
    signs_in_as(&b, "example.com", &public);

    // Restoring again, with or without duplicate skipping, does not leave a second copy of the credential.
    for skip in [true, false] {
        b.import_file(&blob, Some("backup password"), None, skip)
            .unwrap();
        assert_eq!(
            b.passkey_list_all().unwrap().len(),
            1,
            "skip_duplicates = {skip}"
        );
    }
    signs_in_as(&b, "example.com", &public);
}

/// The sealed backup's own JSON, to look inside it.
fn backup_json(e: &Engine) -> serde_json::Value {
    let blob = e.export_backup("backup password").unwrap();
    let plain = lp_engine::backup::open(&blob, "backup password").unwrap();
    serde_json::from_slice(&plain).unwrap()
}

#[test]
fn the_plain_export_leaves_passkeys_out_and_says_how_many() {
    let (_dir, a, _created) = vault_with_passkeys();
    create(&a, "other.example", "bob", b"handle-2");
    let json = a.export_json().unwrap();
    // No private key, and nothing that looks like a passkey, in a file anyone can read.
    assert!(!json.contains("passkey."), "{json}");
    assert!(!json.contains(&private_key_of(&backup_json(&a), "example.com")));
    assert_eq!(a.passkeys_left_out_of_plain_export().unwrap(), 2);
    // What is in it is the rest of the vault, and restores without passkeys or stubs.
    let dir_b = tempfile::tempdir().unwrap();
    let b = unlocked(dir_b.path());
    let report = b.import_file(json.as_bytes(), None, None, true).unwrap();
    assert_eq!(report.imported, 1, "{report:?}");
    assert!(b.passkey_list_all().unwrap().is_empty());
    assert!(
        b.list_items(&ItemQuery::default())
            .unwrap()
            .iter()
            .all(|i| i.kind == "login")
    );
}

/// The private key of the passkey for `rp_id`, from a backup's JSON.
fn private_key_of(json: &serde_json::Value, rp_id: &str) -> String {
    let field = |item: &serde_json::Value, name: &str| {
        item["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .and_then(|f| f["value"].as_str().map(str::to_string))
    };
    json["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| field(i, "passkey.rp_id").as_deref() == Some(rp_id))
        .and_then(|i| field(i, "passkey.private_key"))
        .expect("the passkey is in the backup")
}

#[test]
fn a_trashed_passkey_stays_out_of_the_backup() {
    let (_dir, a, created) = vault_with_passkeys();
    let public = PasskeyPublic::from_cose(&created.public_key).unwrap();
    let second = create(&a, "other.example", "bob", b"handle-2");
    let trash = a
        .passkey_candidates("https://other.example", "other.example", &[])
        .unwrap();
    assert_eq!(trash[0].credential_id, second.credential_id);
    a.trash_item(&trash[0].item_id).unwrap();

    let dir_b = tempfile::tempdir().unwrap();
    let b = unlocked(dir_b.path());
    let blob = a.export_backup("backup password").unwrap();
    let report = b
        .import_file(&blob, Some("backup password"), None, true)
        .unwrap();
    assert_eq!(report.imported, 2, "{report:?}");
    assert_eq!(
        b.passkey_list_all().unwrap().len(),
        1,
        "the trashed passkey is not exported"
    );
    signs_in_as(&b, "example.com", &public);
}

#[test]
fn a_damaged_passkey_in_a_file_is_skipped_with_a_warning_not_turned_into_a_login() {
    let (_dir, a, _created) = vault_with_passkeys();
    // Damage the private key: still valid base64url, no longer a PKCS#8 key.
    let mut value = backup_json(&a);
    for item in value["items"].as_array_mut().unwrap() {
        for field in item["fields"].as_array_mut().unwrap() {
            if field["name"] == "passkey.private_key" {
                field["value"] = "AAAA".into();
            }
        }
    }
    let dir_b = tempfile::tempdir().unwrap();
    let b = unlocked(dir_b.path());
    let report = b
        .import_file(value.to_string().as_bytes(), None, None, true)
        .unwrap();
    assert_eq!(report.imported, 1, "only the shop login: {report:?}");
    assert!(
        report.warnings.iter().any(|w| w.contains("private key")),
        "{:?}",
        report.warnings
    );
    assert!(b.passkey_list_all().unwrap().is_empty());
    assert!(
        b.list_items(&ItemQuery::default())
            .unwrap()
            .iter()
            .all(|i| i.title != "Example"),
        "no stub login named for the site"
    );
}

/// RB-7: when two devices registered the same account while apart, two passkey items exist for it.
/// The "Replace this passkey?" question must name the one that is then replaced.
#[test]
fn the_passkey_named_in_the_replace_question_is_the_one_that_is_replaced() {
    let (_dir, a, _created) = vault_with_passkeys();
    let blob = a.export_backup("backup password").unwrap();

    // A vault with its own passkey for the same site and account (a different credential).
    let dir_b = tempfile::tempdir().unwrap();
    let b = unlocked(dir_b.path());
    create(&b, "example.com", "alice", b"handle-1");
    // Without duplicate skipping, as a person restoring a backup over a vault would choose.
    b.import_file(&blob, Some("backup password"), None, false)
        .unwrap();
    assert_eq!(
        b.passkey_list_all().unwrap().len(),
        2,
        "two passkeys for one account"
    );

    let request = PasskeyCreateRequest {
        origin: "https://example.com".into(),
        rp_id: "example.com".into(),
        rp_name: "Example".into(),
        user_handle: b"handle-1".to_vec(),
        user_name: "alice".into(),
        user_display_name: "ALICE".into(),
        algorithms: vec![-7],
        exclude: vec![],
        user_verified: true,
    };
    let named = b
        .passkey_would_replace(&request)
        .unwrap()
        .expect("one is replaced");
    let replaced = b.passkey_create(&request).unwrap();
    assert_eq!(replaced.item_id, named.item_id);
}
