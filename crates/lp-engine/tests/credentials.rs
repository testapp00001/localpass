//! What browsers and OS providers ask of the vault: logins for a site, saving a login,
//! creating and using passkeys (checked the way a relying party would), and passkeys moving
//! between devices.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lp_core::crypto::{KdfParams, sha256};
use lp_core::passkey::{self, FLAG_BE, FLAG_BS, FLAG_UP, FLAG_UV, b64u, parse_authenticator_data};
use lp_engine::{
    Engine, EngineConfig, EngineError, FileSecretStore, ItemInput, ItemQuery, PasskeyAssertRequest,
    PasskeyCreateRequest, SaveStatus,
};
use lp_sync::service::SyncConfig;

fn engine(dir: &Path) -> Engine {
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
    c.auto_approve_joins = true;
    Engine::open(c, Arc::new(FileSecretStore::new(dir.join("device.key")))).unwrap()
}

fn unlocked(dir: &Path) -> Engine {
    let e = engine(dir);
    e.create_vault("test master pw", "Test device").unwrap();
    e
}

fn login(title: &str, url: &str, user: &str, password: &str) -> ItemInput {
    ItemInput {
        title: title.into(),
        username: user.into(),
        password: password.into(),
        urls: vec![url.into()],
        ..Default::default()
    }
}

fn create_request(origin: &str, rp_id: &str, user: &str, handle: &[u8]) -> PasskeyCreateRequest {
    PasskeyCreateRequest {
        origin: origin.into(),
        rp_id: rp_id.into(),
        rp_name: "Example".into(),
        user_handle: handle.to_vec(),
        user_name: user.into(),
        user_display_name: user.to_uppercase(),
        algorithms: vec![-7, -257],
        exclude: vec![],
        user_verified: true,
    }
}

/// A passkey's private key as the encrypted backup carries it (the interface cannot copy it and the
/// plain export leaves passkeys out).
fn private_key_in_backup(e: &Engine, credential_id: &[u8]) -> String {
    let blob = e.export_backup("a backup password").unwrap();
    let plain = lp_engine::backup::open(&blob, "a backup password").unwrap();
    let exported: serde_json::Value = serde_json::from_slice(&plain).unwrap();
    let wanted = b64u::encode(credential_id);
    let field = |item: &serde_json::Value, name: &str| {
        item["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["name"] == name)
            .and_then(|f| f["value"].as_str().map(str::to_string))
    };
    exported["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| field(i, "passkey.credential_id").as_deref() == Some(wanted.as_str()))
        .and_then(|i| field(i, "passkey.private_key"))
        .expect("the passkey is in the backup")
}

fn assert_request(
    origin: &str,
    rp_id: &str,
    item_id: &str,
    client_data: &[u8],
) -> PasskeyAssertRequest {
    PasskeyAssertRequest {
        origin: origin.into(),
        rp_id: rp_id.into(),
        item_id: item_id.into(),
        client_data_hash: sha256(client_data).to_vec(),
        user_verified: true,
    }
}

#[test]
fn logins_are_found_by_site_and_filled_only_for_that_site() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let github = e
        .save_item(&ItemInput {
            totp: "JBSWY3DPEHPK3PXP".into(),
            ..login("GitHub", "https://github.com", "octocat", "gh-pass")
        })
        .unwrap();
    let sub = e
        .save_item(&login(
            "Example login",
            "https://login.example.com",
            "alice",
            "alice-pass",
        ))
        .unwrap();
    let root = e
        .save_item(&login("Example root", "example.com", "bob", "bob-pass"))
        .unwrap();
    e.save_item(&login(
        "Elsewhere",
        "https://example.org",
        "eve",
        "eve-pass",
    ))
    .unwrap();
    let gone = e
        .save_item(&login(
            "Old example",
            "https://example.com",
            "old",
            "old-pass",
        ))
        .unwrap();
    e.trash_item(&gone).unwrap();
    e.save_item(&ItemInput {
        kind: "note".into(),
        ..login("A note", "https://example.com", "", "")
    })
    .unwrap();

    let found = e
        .find_logins("https://login.example.com/signin?next=/")
        .unwrap();
    let names: Vec<_> = found.iter().map(|m| m.title.as_str()).collect();
    assert_eq!(
        names,
        ["Example login", "Example root"],
        "exact host first; trash and notes excluded"
    );
    assert!(found[0].exact && !found[1].exact);
    let json = serde_json::to_string(&found).unwrap();
    assert!(
        !json.contains("alice-pass") && !json.contains("bob-pass"),
        "no secrets in matches"
    );

    assert!(
        e.find_logins("https://example.com.evil.net")
            .unwrap()
            .is_empty()
    );
    assert!(
        e.find_logins("http://example.com").is_err(),
        "plain http is refused"
    );
    assert!(e.find_logins("chrome://settings").is_err());

    let fill = e.fill_login(&root, "https://login.example.com").unwrap();
    assert_eq!(
        (fill.username.as_str(), fill.password.as_str()),
        ("bob", "bob-pass")
    );
    assert!(fill.totp_code.is_none());
    let fill = e.fill_login(&github, "https://github.com").unwrap();
    assert_eq!(fill.totp_code.as_deref().map(str::len), Some(6));
    assert!(
        e.fill_login(&github, "https://example.com").is_err(),
        "wrong site"
    );
    assert!(
        e.fill_login(&sub, "https://example.org").is_err(),
        "wrong site"
    );
    assert!(
        e.fill_login(&gone, "https://example.com").is_err(),
        "trashed"
    );

    e.lock();
    assert!(matches!(
        e.find_logins("https://example.com"),
        Err(EngineError::Locked)
    ));
    assert!(matches!(
        e.fill_login(&root, "https://example.com"),
        Err(EngineError::Locked)
    ));
}

#[test]
fn save_login_creates_updates_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());

    let made = e
        .save_login("https://shop.example.com", "Carol", "pw1", None)
        .unwrap();
    assert_eq!(made.status, SaveStatus::Created);
    let item = e.get_item(&made.id).unwrap();
    assert_eq!(item.title, "example.com");
    assert_eq!(item.urls, ["https://shop.example.com"]);
    assert_eq!(
        (item.username.as_str(), item.password.as_str()),
        ("Carol", "pw1")
    );

    let same = e
        .save_login("https://shop.example.com", "Carol", "pw1", None)
        .unwrap();
    assert_eq!(
        (same.status, same.id.as_str()),
        (SaveStatus::Unchanged, made.id.as_str())
    );

    let changed = e
        .save_login("https://shop.example.com", "carol", "pw2", None)
        .unwrap();
    assert_eq!(
        (changed.status, changed.id.as_str()),
        (SaveStatus::Updated, made.id.as_str())
    );
    let item = e.get_item(&made.id).unwrap();
    assert_eq!(item.password, "pw2");
    assert_eq!(item.title, "example.com", "other fields are kept");
    assert_eq!(item.password_history[0].value, "pw1");

    let other = e
        .save_login("https://shop.example.com", "dave", "pw3", Some("Shop"))
        .unwrap();
    assert_eq!(other.status, SaveStatus::Created);
    assert_ne!(other.id, made.id);
    assert_eq!(e.get_item(&other.id).unwrap().title, "Shop");

    assert!(
        e.save_login("https://shop.example.com", "erin", "", None)
            .is_err()
    );
    assert!(
        e.save_login("http://shop.example.com", "erin", "x", None)
            .is_err()
    );
    assert_eq!(e.list_items(&ItemQuery::default()).unwrap().len(), 2);
}

#[test]
fn passkey_registration_and_sign_in_verify_like_a_relying_party() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let handle = b"user-handle-1";

    let created = e
        .passkey_create(&create_request(
            "https://login.example.com",
            "example.com",
            "alice",
            handle,
        ))
        .unwrap();
    assert_eq!(created.algorithm, -7);
    let auth = parse_authenticator_data(&created.auth_data).unwrap();
    assert_eq!(auth.rp_id_hash, passkey::rp_id_hash("example.com"));
    assert_eq!(
        auth.flags,
        FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS | passkey::FLAG_AT
    );
    assert_eq!(auth.sign_count, 0);
    let (credential_id, public) = auth.credential.expect("attested credential data");
    assert_eq!(credential_id, created.credential_id);
    assert_eq!(public.to_cose(), created.public_key);
    assert_eq!(public.to_spki_der(), created.public_key_spki);

    // The vault now has a passkey item, which is not a login.
    let items = e.list_items(&ItemQuery::default()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        (items[0].kind.as_str(), items[0].title.as_str()),
        ("passkey", "Example")
    );
    assert_eq!(items[0].subtitle, "alice");
    assert!(e.find_logins("https://example.com").unwrap().is_empty());

    let candidates = e
        .passkey_candidates("https://login.example.com", "example.com", &[])
        .unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].credential_id, created.credential_id);
    assert_eq!(candidates[0].user_handle, handle);
    assert_eq!(candidates[0].user_name, "alice");

    // Sign in: the relying party checks authData || SHA-256(clientDataJSON).
    let client_data =
        br#"{"type":"webauthn.get","challenge":"c2lnbi1pbg","origin":"https://login.example.com"}"#;
    let got = e
        .passkey_assert(&assert_request(
            "https://login.example.com",
            "example.com",
            &candidates[0].item_id,
            client_data,
        ))
        .unwrap();
    assert_eq!(got.credential_id, created.credential_id);
    assert_eq!(got.user_handle, handle);
    let parsed = parse_authenticator_data(&got.auth_data).unwrap();
    assert_eq!(parsed.rp_id_hash, passkey::rp_id_hash("example.com"));
    assert_eq!(parsed.flags, FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS);
    assert_eq!(parsed.sign_count, 0);
    let mut signed = got.auth_data.clone();
    signed.extend_from_slice(&sha256(client_data));
    public
        .verify(&signed, &got.signature)
        .expect("valid signature");
    let mut other = got.auth_data.clone();
    other.extend_from_slice(&sha256(b"another challenge"));
    assert!(public.verify(&other, &got.signature).is_err());

    // Without a user check the UV flag is not claimed.
    let mut unverified = assert_request(
        "https://login.example.com",
        "example.com",
        &candidates[0].item_id,
        client_data,
    );
    unverified.user_verified = false;
    let got = e.passkey_assert(&unverified).unwrap();
    assert_eq!(
        parse_authenticator_data(&got.auth_data).unwrap().flags,
        FLAG_UP | FLAG_BE | FLAG_BS
    );

    // The private key never appears in views, and requests serialize as the broker will see them.
    assert!(
        e.field_secret(&candidates[0].item_id, "passkey.private_key")
            .is_err(),
        "the interface cannot copy it (RB-2)"
    );
    // The only way out is a backup; read the key from there.
    let key = private_key_in_backup(&e, &created.credential_id);
    let listed = serde_json::to_string(&e.list_items(&ItemQuery::default()).unwrap()).unwrap();
    let viewed = serde_json::to_string(&e.get_item(&candidates[0].item_id).unwrap()).unwrap();
    assert!(!listed.contains(key.as_str()) && !viewed.contains(key.as_str()));
    let created_json = serde_json::to_string(&created).unwrap();
    assert!(!created_json.contains(key.as_str()));
    assert!(created_json.contains(&b64u::encode(&created.credential_id)));
}

#[test]
fn passkey_rules_for_origin_algorithms_duplicates_and_trash() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let ok = |origin: &str, rp: &str, user: &str, handle: &[u8]| {
        e.passkey_create(&create_request(origin, rp, user, handle))
    };

    // A page can only register for its own site, never for a public suffix, never over http.
    assert!(ok("https://evil.example.net", "example.com", "a", b"1").is_err());
    assert!(ok("https://example.com", "com", "a", b"1").is_err());
    assert!(ok("http://example.com", "example.com", "a", b"1").is_err());
    assert!(
        ok("https://example.com", "example.com", "a", b"").is_err(),
        "empty user handle"
    );
    assert!(
        ok("https://example.com", "example.com", "a", &[0u8; 65]).is_err(),
        "long user handle"
    );
    assert!(
        ok("http://localhost:8080", "localhost", "dev", b"d").is_ok(),
        "localhost for development"
    );

    // ES256 must be among the algorithms the site accepts.
    let mut rsa_only = create_request("https://example.com", "example.com", "a", b"1");
    rsa_only.algorithms = vec![-257, -8];
    assert!(matches!(
        e.passkey_create(&rsa_only),
        Err(EngineError::Unsupported(_))
    ));
    rsa_only.algorithms = vec![];
    assert!(
        e.passkey_create(&rsa_only).is_ok(),
        "no preference means anything goes"
    );

    // The same account registering again replaces its passkey instead of adding another.
    let first = ok(
        "https://example.com",
        "example.com",
        "alice",
        b"alice-handle",
    )
    .unwrap();
    let second = ok(
        "https://example.com",
        "example.com",
        "alice",
        b"alice-handle",
    )
    .unwrap();
    assert_eq!(first.item_id, second.item_id);
    assert_ne!(first.credential_id, second.credential_id);
    let alice: Vec<_> = e
        .passkey_candidates("https://example.com", "example.com", &[])
        .unwrap()
        .into_iter()
        .filter(|c| c.user_handle == b"alice-handle")
        .collect();
    assert_eq!(alice.len(), 1);
    assert_eq!(alice[0].credential_id, second.credential_id);

    // Another account on the same site gets its own passkey.
    let bob = ok("https://example.com", "example.com", "bob", b"bob-handle").unwrap();
    assert_ne!(bob.item_id, first.item_id);

    // The site's exclude list blocks registering a duplicate.
    let mut excluded = create_request("https://example.com", "example.com", "bob", b"bob-handle");
    excluded.exclude = vec![bob.credential_id.clone()];
    assert!(matches!(
        e.passkey_create(&excluded),
        Err(EngineError::PasskeyExists)
    ));

    // Allow lists narrow the candidates; other sites never see these passkeys.
    let only_bob = e
        .passkey_candidates(
            "https://example.com",
            "example.com",
            std::slice::from_ref(&bob.credential_id),
        )
        .unwrap();
    assert_eq!(only_bob.len(), 1);
    assert_eq!(only_bob[0].user_name, "bob");
    assert!(
        e.passkey_candidates("https://example.org", "example.org", &[])
            .unwrap()
            .is_empty()
    );
    assert!(
        e.passkey_candidates("https://example.org", "example.com", &[])
            .is_err(),
        "rp id must fit the origin"
    );

    // Signing is tied to the site, needs a valid hash, and skips the trash.
    let data = b"client data";
    let bad_site = assert_request("https://example.org", "example.org", &bob.item_id, data);
    assert!(e.passkey_assert(&bad_site).is_err());
    let mut short = assert_request("https://example.com", "example.com", &bob.item_id, data);
    short.client_data_hash.pop();
    assert!(e.passkey_assert(&short).is_err());
    assert!(
        e.passkey_assert(&assert_request(
            "https://example.com",
            "example.com",
            &bob.item_id,
            data
        ))
        .is_ok()
    );
    e.trash_item(&bob.item_id).unwrap();
    assert!(
        e.passkey_assert(&assert_request(
            "https://example.com",
            "example.com",
            &bob.item_id,
            data
        ))
        .is_err()
    );
    assert!(
        e.passkey_candidates(
            "https://example.com",
            "example.com",
            std::slice::from_ref(&bob.credential_id)
        )
        .unwrap()
        .is_empty()
    );
    assert!(
        e.passkey_assert(&assert_request(
            "https://example.com",
            "example.com",
            "not-an-id",
            data
        ))
        .is_err()
    );

    e.lock();
    assert!(matches!(
        e.passkey_create(&create_request(
            "https://example.com",
            "example.com",
            "c",
            b"3"
        )),
        Err(EngineError::Locked)
    ));
}

#[test]
fn passkey_list_all_is_metadata_only_and_skips_the_trash() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    assert!(e.passkey_list_all().unwrap().is_empty());

    // A plain login is not a passkey.
    e.save_item(&login("Shop", "https://shop.example.net", "u", "pw"))
        .unwrap();
    let alice = e
        .passkey_create(&create_request(
            "https://example.com",
            "example.com",
            "Alice",
            b"1",
        ))
        .unwrap();
    let bob = e
        .passkey_create(&create_request(
            "https://login.shop.example.net",
            "shop.example.net",
            "bob",
            b"2",
        ))
        .unwrap();
    let carol = e
        .passkey_create(&create_request(
            "https://example.com",
            "example.com",
            "carol",
            b"3",
        ))
        .unwrap();

    // Every site at once, ordered by site and then user name.
    let all = e.passkey_list_all().unwrap();
    let users: Vec<_> = all.iter().map(|p| p.user_name.as_str()).collect();
    assert_eq!(users, ["Alice", "carol", "bob"]);
    assert_eq!(all[0].rp_id, "example.com");
    assert_eq!(all[0].rp_name, "Example");
    assert_eq!(all[0].user_display_name, "ALICE");
    assert_eq!(all[0].credential_id, alice.credential_id);
    assert_eq!(all[0].user_handle, b"1");
    assert_eq!(all[0].item_id, alice.item_id);
    assert_eq!(all[2].credential_id, bob.credential_id);

    // Metadata only: the private key is nowhere in what is handed out.
    assert!(
        e.field_secret(&alice.item_id, "passkey.private_key")
            .is_err()
    );
    let private = private_key_in_backup(&e, &alice.credential_id);
    let json = serde_json::to_string(&all).unwrap();
    assert!(!json.contains(private.as_str()));
    assert!(!json.to_lowercase().contains("private"));

    // The trash is skipped, and a restored passkey comes back.
    e.trash_item(&bob.item_id).unwrap();
    let ids: Vec<_> = e
        .passkey_list_all()
        .unwrap()
        .into_iter()
        .map(|p| p.credential_id)
        .collect();
    assert_eq!(ids, [alice.credential_id.clone(), carol.credential_id]);
    e.restore_item(&bob.item_id).unwrap();
    assert_eq!(e.passkey_list_all().unwrap().len(), 3);

    // A locked vault shows nothing.
    e.lock();
    assert!(matches!(e.passkey_list_all(), Err(EngineError::Locked)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_passkey_made_on_one_device_signs_on_another() {
    let (a_dir, b_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = unlocked(a_dir.path());
    a.start_sync().await.unwrap();
    let created = a
        .passkey_create(&create_request(
            "https://example.com",
            "example.com",
            "alice",
            b"alice-handle",
        ))
        .unwrap();

    let invite = a.invite_start().unwrap();
    let b = engine(b_dir.path());
    let a_addr = format!("127.0.0.1:{}", a.sync_status().port.unwrap());
    b.join(
        &invite.code,
        std::slice::from_ref(&a_addr),
        "other master pw",
        "Phone",
    )
    .await
    .unwrap();
    b.sync_with(a_addr.parse().unwrap(), false).await.unwrap();

    let candidates = b
        .passkey_candidates(
            "https://example.com",
            "example.com",
            std::slice::from_ref(&created.credential_id),
        )
        .unwrap();
    assert_eq!(
        candidates.len(),
        1,
        "the passkey arrived on the second device"
    );
    let listed = b.passkey_list_all().unwrap();
    assert_eq!(listed.len(), 1, "the whole-vault list sees synced passkeys");
    assert_eq!(listed[0].credential_id, created.credential_id);
    assert_eq!(listed[0].item_id, candidates[0].item_id);
    let data = br#"{"type":"webauthn.get","origin":"https://example.com"}"#;
    let got = b
        .passkey_assert(&assert_request(
            "https://example.com",
            "example.com",
            &candidates[0].item_id,
            data,
        ))
        .unwrap();
    let public = parse_authenticator_data(&created.auth_data)
        .unwrap()
        .credential
        .unwrap()
        .1;
    let mut signed = got.auth_data.clone();
    signed.extend_from_slice(&sha256(data));
    public
        .verify(&signed, &got.signature)
        .expect("signature made on the other device verifies");
}

#[test]
fn logins_for_another_site_are_reachable_only_as_live_logins() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let bank = e
        .save_item(&login("Bank", "https://bank.example", "alice", "bank-pw"))
        .unwrap();
    let old = e
        .save_item(&login("Old bank", "https://old.example", "alice", "old-pw"))
        .unwrap();
    e.trash_item(&old).unwrap();
    let note = e
        .save_item(&ItemInput {
            kind: "note".into(),
            title: "Bank note".into(),
            notes: "1234".into(),
            ..Default::default()
        })
        .unwrap();
    let key = e
        .passkey_create(&create_request(
            "https://bank.example",
            "bank.example",
            "alice",
            b"h",
        ))
        .unwrap();

    // Search lists live logins only: no trash, notes or passkeys, and it is capped.
    let hits = e.search_logins("bank", 10).unwrap();
    assert_eq!(
        hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        [&*bank]
    );
    for n in 0..5 {
        e.save_item(&login(
            &format!("Shop {n}"),
            "https://shop.example",
            "a",
            "pw",
        ))
        .unwrap();
    }
    assert_eq!(e.search_logins("shop", 3).unwrap().len(), 3);

    // Filling for another site works for a live login and for nothing else.
    let fill = e
        .fill_login_anywhere(&bank, "https://other.example")
        .unwrap();
    assert_eq!(fill.password, "bank-pw");
    assert!(e.fill_login(&bank, "https://other.example").is_err());
    for id in [&old, &note, &key.item_id, "not-an-id"] {
        assert!(
            e.fill_login_anywhere(id, "https://other.example").is_err(),
            "{id}"
        );
        assert!(e.login_label(id).is_err(), "{id}");
    }
    assert!(
        e.fill_login_anywhere(&bank, "http://other.example")
            .is_err(),
        "the page must still be a secure origin"
    );
    let label = e.login_label(&bank).unwrap();
    assert_eq!(
        (label.title.as_str(), label.username.as_str()),
        ("Bank", "alice")
    );
    e.lock();
    assert!(matches!(
        e.search_logins("bank", 10),
        Err(EngineError::Locked)
    ));
}

#[test]
fn text_and_list_sizes_of_a_passkey_request_are_capped() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let base = || {
        create_request(
            "https://example.com",
            "example.com",
            "alice",
            b"alice-handle",
        )
    };
    let refused = |req: PasskeyCreateRequest, what: &str| {
        assert!(
            matches!(e.passkey_can_create(&req), Err(EngineError::Invalid(_))),
            "{what} was accepted"
        );
        assert!(
            matches!(e.passkey_create(&req), Err(EngineError::Invalid(_))),
            "{what} was accepted by create"
        );
    };

    // 256 characters are fine, one more is not (characters, not bytes).
    let mut fine = base();
    fine.rp_name = "é".repeat(256);
    fine.user_name = "u".repeat(256);
    fine.user_display_name = "d".repeat(256);
    assert!(e.passkey_can_create(&fine).is_ok());
    let mut long = base();
    long.rp_name = "r".repeat(257);
    refused(long, "a long site name");
    let mut long = base();
    long.user_name = "u".repeat(257);
    refused(long, "a long user name");
    let mut long = base();
    long.user_display_name = "d".repeat(257);
    refused(long, "a long display name");

    // The exclude list: 256 entries of up to 1023 bytes.
    let mut list = base();
    list.exclude = (0..256u32).map(|n| n.to_be_bytes().to_vec()).collect();
    assert!(e.passkey_can_create(&list).is_ok());
    list.exclude.push(vec![9]);
    refused(list, "a long exclude list");
    let mut big = base();
    big.exclude = vec![vec![1u8; 1023]];
    assert!(e.passkey_can_create(&big).is_ok());
    big.exclude = vec![vec![1u8; 1024]];
    refused(big, "an oversized credential id");

    // The allow list of a sign-in is capped the same way.
    let many: Vec<Vec<u8>> = (0..257u32).map(|n| n.to_be_bytes().to_vec()).collect();
    assert!(
        e.passkey_candidates("https://example.com", "example.com", &many[..256])
            .is_ok()
    );
    assert!(matches!(
        e.passkey_candidates("https://example.com", "example.com", &many),
        Err(EngineError::Invalid(_))
    ));
    assert!(matches!(
        e.passkey_candidates("https://example.com", "example.com", &[vec![1u8; 1024]]),
        Err(EngineError::Invalid(_))
    ));
}

#[test]
fn a_login_for_another_subdomain_is_never_overwritten_silently() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let parent = e
        .save_item(&login("Example", "https://example.com", "alice", "pw1"))
        .unwrap();

    // A save from a sibling address makes a login of its own; the saved one keeps its password.
    let sibling = e
        .save_login("https://sub.example.com", "alice", "pw2", None)
        .unwrap();
    assert_eq!(sibling.status, SaveStatus::Created);
    assert_ne!(sibling.id, parent);
    assert_eq!(e.get_item(&parent).unwrap().password, "pw1");
    assert_eq!(e.get_item(&sibling.id).unwrap().password, "pw2");

    // The same address is the same login: its password is updated in place.
    let same = e
        .save_login("https://example.com", "alice", "pw3", None)
        .unwrap();
    assert_eq!(
        (same.status, same.id.as_str()),
        (SaveStatus::Updated, parent.as_str())
    );

    // And the other way round: saved for the subdomain, saved from the parent.
    let other = e
        .save_item(&login("Shop", "https://shop.example.org", "bob", "b1"))
        .unwrap();
    let from_parent = e
        .save_login("https://example.org", "bob", "b2", None)
        .unwrap();
    assert_eq!(from_parent.status, SaveStatus::Created);
    assert_eq!(e.get_item(&other).unwrap().password, "b1");
}

#[test]
fn the_engine_says_what_a_save_would_change_without_changing_it() {
    use lp_engine::SavePlan;
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let parent = e
        .save_item(&login("Example", "https://example.com", "alice", "pw1"))
        .unwrap();

    let plan = |origin: &str, user: &str, password: &str| {
        e.plan_save_login(origin, user, password).unwrap()
    };
    assert_eq!(
        plan("https://example.com", "bob", "x"),
        SavePlan::Create,
        "another user name is a new login"
    );
    assert_eq!(
        plan("https://example.org", "alice", "x"),
        SavePlan::Create,
        "another site is a new login"
    );
    assert_eq!(
        plan("https://example.com", "ALICE", "pw1"),
        SavePlan::Unchanged { id: parent.clone() }
    );
    let SavePlan::Update(change) = plan("https://example.com", "alice", "pw2") else {
        panic!("a changed password updates the login")
    };
    assert_eq!(
        (change.id.as_str(), change.title.as_str(), change.same_host),
        (parent.as_str(), "Example", true)
    );
    assert_eq!(change.saved_url.as_deref(), Some("https://example.com"));
    let SavePlan::Update(change) = plan("https://login.example.com", "alice", "pw2") else {
        panic!("a sibling address would change the saved login too")
    };
    assert!(!change.same_host);

    // Planning changed nothing.
    assert_eq!(e.get_item(&parent).unwrap().password, "pw1");
    assert_eq!(e.list_items(&ItemQuery::default()).unwrap().len(), 1);
    assert!(
        e.plan_save_login("https://example.com", "alice", "")
            .is_err()
    );
    assert!(
        e.plan_save_login("http://example.com", "alice", "x")
            .is_err()
    );

    // Applying a confirmed change.
    let done = e.update_login_password(&parent, "pw2").unwrap();
    assert_eq!(
        (done.status, done.id.as_str()),
        (SaveStatus::Updated, parent.as_str())
    );
    assert_eq!(e.get_item(&parent).unwrap().password, "pw2");
    assert_eq!(
        e.get_item(&parent).unwrap().password_history[0].value,
        "pw1"
    );
    assert_eq!(
        e.update_login_password(&parent, "pw2").unwrap().status,
        SaveStatus::Unchanged
    );
    e.trash_item(&parent).unwrap();
    assert!(
        e.update_login_password(&parent, "pw4").is_err(),
        "not a live login"
    );
    assert!(e.update_login_password("not-an-id", "pw4").is_err());
    assert!(e.update_login_password(&parent, "").is_err());

    let made = e
        .create_login("https://example.com", "carol", "c1", Some("Carol"))
        .unwrap();
    assert_eq!(made.status, SaveStatus::Created);
    assert_eq!(e.get_item(&made.id).unwrap().title, "Carol");
}

#[test]
fn the_engine_says_when_a_passkey_would_be_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let e = unlocked(dir.path());
    let request = |user: &str, handle: &[u8]| {
        create_request("https://example.com", "example.com", user, handle)
    };
    assert!(
        e.passkey_would_replace(&request("alice", b"a"))
            .unwrap()
            .is_none()
    );

    let first = e.passkey_create(&request("alice", b"a")).unwrap();
    assert!(
        e.passkey_would_replace(&request("bob", b"b"))
            .unwrap()
            .is_none(),
        "another account is a new passkey"
    );
    let other_site = create_request("https://example.org", "example.org", "alice", b"a");
    assert!(e.passkey_would_replace(&other_site).unwrap().is_none());
    let existing = e
        .passkey_would_replace(&request("alice", b"a"))
        .unwrap()
        .expect("same site and user handle");
    assert_eq!(existing.item_id, first.item_id);
    assert_eq!(existing.user_name, "alice");
    assert_eq!(existing.rp_id, "example.com");
    // Asking changes nothing.
    assert_eq!(
        e.passkey_candidates("https://example.com", "example.com", &[])
            .unwrap()[0]
            .credential_id,
        first.credential_id
    );
    // A request that could not be made is an error, as for `passkey_can_create`.
    let mut bad = request("alice", b"a");
    bad.rp_id = "com".into();
    assert!(e.passkey_would_replace(&bad).is_err());

    e.trash_item(&first.item_id).unwrap();
    assert!(
        e.passkey_would_replace(&request("alice", b"a"))
            .unwrap()
            .is_none()
    );
}
