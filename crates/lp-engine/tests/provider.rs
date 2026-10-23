//! The JSON entry point the Android services use, driven the way Kotlin drives it: a locked
//! vault, unlocking, listing, creating a passkey and signing in with it (checked as a relying
//! party would), and the refusals that keep one site's credentials from another.

use std::path::Path;
use std::sync::Arc;

use lp_core::crypto::{KdfParams, Key32, sha256};
use lp_core::passkey::{FLAG_UV, b64u, parse_authenticator_data};
use lp_engine::{Engine, EngineConfig, FileSecretStore, ItemInput, ItemQuery, provider};
use serde_json::{Value, json};

fn engine(dir: &Path) -> Engine {
    let mut c = EngineConfig::new(dir, "test");
    c.kdf = KdfParams::insecure_for_tests();
    Engine::open(c, Arc::new(FileSecretStore::new(dir.join("device.key")))).unwrap()
}

fn call(e: &Engine, request: Value) -> Value {
    serde_json::from_str(&provider::call(e, &request.to_string())).unwrap()
}

fn ok(e: &Engine, request: Value) -> Value {
    let reply = call(e, request);
    assert!(reply.get("error").is_none(), "unexpected error: {reply}");
    reply["ok"].clone()
}

fn error_code(e: &Engine, request: Value) -> String {
    let reply = call(e, request);
    reply["error"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected an error, got {reply}"))
        .to_string()
}

const ORIGIN: &str = "https://login.example.com";

fn creation_options(user: &str, exclude: &[&[u8]]) -> String {
    json!({
        "challenge": b64u::encode(b"challenge"),
        "rp": { "id": "example.com", "name": "Example" },
        "user": { "id": b64u::encode(user.as_bytes()), "name": user, "displayName": user },
        "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
        "excludeCredentials": exclude
            .iter()
            .map(|id| json!({ "type": "public-key", "id": b64u::encode(id) }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

fn request_options(allow: &[&[u8]]) -> String {
    json!({
        "challenge": b64u::encode(b"challenge"),
        "rpId": "example.com",
        "allowCredentials": allow
            .iter()
            .map(|id| json!({ "type": "public-key", "id": b64u::encode(id) }))
            .collect::<Vec<_>>(),
    })
    .to_string()
}

#[test]
fn a_locked_vault_serves_nothing_until_unlocked() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    assert_eq!(ok(&e, json!({"op": "status"}))["status"], "noVault");
    e.create_vault("master pw", "Phone").unwrap();
    e.lock();

    assert_eq!(ok(&e, json!({"op": "status"}))["status"], "locked");
    assert_eq!(
        error_code(&e, json!({"op": "findLogins", "origin": ORIGIN})),
        "locked"
    );
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": request_options(&[])})
        ),
        "locked"
    );
    assert_eq!(
        error_code(&e, json!({"op": "unlockPassword", "password": "wrong"})),
        "wrongPassword"
    );
    ok(&e, json!({"op": "unlockPassword", "password": "master pw"}));
    assert_eq!(ok(&e, json!({"op": "status"}))["status"], "unlocked");
    assert!(ok(&e, json!({"op": "findLogins", "origin": ORIGIN})).is_array());
}

#[test]
fn quick_unlock_key_unlocks() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    let key = Key32::random().unwrap();
    e.quick_unlock_enable(&key).unwrap();
    e.lock();
    assert_eq!(ok(&e, json!({"op": "status"}))["quickUnlock"], true);

    let wrong = data_encoding::BASE64.encode(Key32::random().unwrap().as_bytes());
    assert_ne!(
        error_code(&e, json!({"op": "unlockQuick", "key": wrong})),
        "",
        "a wrong key must not unlock"
    );
    assert_eq!(ok(&e, json!({"op": "status"}))["status"], "locked");

    ok(
        &e,
        json!({"op": "unlockQuick", "key": data_encoding::BASE64.encode(key.as_bytes())}),
    );
    assert_eq!(ok(&e, json!({"op": "status"}))["status"], "unlocked");
}

#[test]
fn logins_are_listed_and_filled_for_their_own_site_only() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    let id = e
        .save_item(&ItemInput {
            title: "Example".into(),
            username: "alice".into(),
            password: "s3cret".into(),
            urls: vec!["https://example.com".into()],
            ..Default::default()
        })
        .unwrap();

    let found = ok(&e, json!({"op": "findLogins", "origin": ORIGIN}));
    assert_eq!(found[0]["id"], id);
    assert!(found[0].get("password").is_none(), "lists carry no secrets");
    let fill = ok(
        &e,
        json!({"op": "fillLogin", "origin": ORIGIN, "itemId": id}),
    );
    assert_eq!(fill["password"], "s3cret");

    // Another site finds nothing and can't fetch the login by id either.
    let other = "https://example.com.evil.test";
    assert_eq!(
        ok(&e, json!({"op": "findLogins", "origin": other}))
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        error_code(
            &e,
            json!({"op": "fillLogin", "origin": other, "itemId": id})
        ),
        "invalid"
    );

    let saved = ok(
        &e,
        json!({"op": "saveLogin", "origin": "https://shop.test", "username": "bob", "password": "pw"}),
    );
    assert_eq!(saved["status"], "created");
}

/// What the autofill service does after the user taps Save: the website comes from the OS, the
/// username and password from the saved screen.
#[test]
fn a_login_saved_from_the_autofill_prompt_is_stored_once_and_updated_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    e.lock();
    let save = |origin: &str, username: &str, password: &str| {
        call(
            &e,
            json!({"op": "saveLogin", "origin": origin, "username": username, "password": password}),
        )
    };
    let count = || e.list_items(&ItemQuery::default()).unwrap().len();

    // Locked: the service must unlock first, and nothing is written meanwhile.
    assert_eq!(
        save("https://shop.example.com", "bob", "pw1")["error"]["code"],
        "locked"
    );
    ok(&e, json!({"op": "unlockPassword", "password": "master pw"}));
    assert_eq!(count(), 0);

    // A login the vault doesn't have: a new item named after the site.
    let made = save("https://shop.example.com", "bob", "pw1");
    assert_eq!(made["ok"]["status"], "created");
    assert!(
        made["ok"].get("password").is_none(),
        "the reply carries no secret"
    );
    let id = made["ok"]["id"].as_str().unwrap().to_string();
    let item = e.get_item(&id).unwrap();
    assert_eq!(item.title, "example.com");
    assert_eq!(item.urls, ["https://shop.example.com"]);
    assert_eq!(
        (item.username.as_str(), item.password.as_str()),
        ("bob", "pw1")
    );
    assert_eq!(item.kind, "login");

    // The same login again, even typed for another subdomain or with a different case: no change.
    for (origin, user) in [
        ("https://shop.example.com", "bob"),
        ("https://www.example.com", "Bob"),
    ] {
        let again = save(origin, user, "pw1");
        assert_eq!(again["ok"]["status"], "unchanged", "{origin} {user}");
        assert_eq!(again["ok"]["id"], id.as_str());
    }

    // A changed password updates that item and keeps the old one as history.
    let changed = save("https://shop.example.com", "bob", "pw2");
    assert_eq!(changed["ok"]["status"], "updated");
    assert_eq!(changed["ok"]["id"], id.as_str());
    assert_eq!(e.get_item(&id).unwrap().password, "pw2");
    assert_eq!(e.get_item(&id).unwrap().password_history[0].value, "pw1");
    assert_eq!(count(), 1, "never a duplicate");

    // The saved login is what the dropdown then offers and fills.
    let found = ok(
        &e,
        json!({"op": "findLogins", "origin": "https://shop.example.com"}),
    );
    assert_eq!(found.as_array().unwrap().len(), 1);
    let fill = ok(
        &e,
        json!({"op": "fillLogin", "origin": "https://shop.example.com", "itemId": id}),
    );
    assert_eq!(
        (fill["username"].as_str(), fill["password"].as_str()),
        (Some("bob"), Some("pw2"))
    );

    // Another account on the same site is a second login; another site is not touched.
    assert_eq!(
        save("https://shop.example.com", "carol", "pw3")["ok"]["status"],
        "created"
    );
    assert_eq!(
        save("https://other.test", "bob", "pw1")["ok"]["status"],
        "created"
    );
    assert_eq!(count(), 3);
    assert_eq!(e.get_item(&id).unwrap().password, "pw2");

    // Android reports no port, so a local test site is saved for its host alone.
    let local = save("http://localhost", "alice", "pw4");
    assert_eq!(local["ok"]["status"], "created");
    assert_eq!(
        e.get_item(local["ok"]["id"].as_str().unwrap())
            .unwrap()
            .title,
        "localhost"
    );

    // What is refused: no password, plain http for a real site, and a malformed request.
    assert_eq!(
        save("https://shop.example.com", "bob", "")["error"]["code"],
        "invalid"
    );
    assert_eq!(
        save("http://shop.example.com", "dave", "x")["error"]["code"],
        "invalid"
    );
    assert_eq!(
        save("not an origin", "dave", "x")["error"]["code"],
        "invalid"
    );
    assert_eq!(
        error_code(
            &e,
            json!({"op": "saveLogin", "origin": ORIGIN, "username": "dave"})
        ),
        "invalid"
    );
    assert_eq!(count(), 4);
}

#[test]
fn a_passkey_is_created_and_signs_in_like_a_real_authenticator() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();

    // Check first, then create (what the services do around the fingerprint prompt).
    let options = creation_options("alice", &[]);
    let summary = ok(
        &e,
        json!({"op": "passkeyCheckCreate", "origin": ORIGIN, "optionsJson": options}),
    );
    assert_eq!(summary["rpId"], "example.com");
    assert_eq!(summary["userName"], "alice");

    let created = ok(
        &e,
        json!({"op": "passkeyCreate", "origin": ORIGIN, "optionsJson": options, "userVerified": true}),
    );
    let registration: Value =
        serde_json::from_str(created["responseJson"].as_str().unwrap()).unwrap();
    assert_eq!(registration["type"], "public-key");
    assert!(registration["response"].get("clientDataJSON").is_none());
    let credential_id = b64u::decode(registration["id"].as_str().unwrap()).unwrap();
    assert_eq!(registration["rawId"], registration["id"]);
    let auth_data = b64u::decode(
        registration["response"]["authenticatorData"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let parsed = parse_authenticator_data(&auth_data).unwrap();
    assert_eq!(parsed.rp_id_hash, sha256(b"example.com"));
    assert!(
        parsed.flags & FLAG_UV != 0,
        "the fingerprint check is reported"
    );
    let (id_in_auth, public) = parsed.credential.unwrap();
    assert_eq!(id_in_auth, credential_id);
    assert_eq!(
        b64u::decode(registration["response"]["publicKey"].as_str().unwrap()).unwrap(),
        public.to_spki_der()
    );

    // The list finds it, narrowed by the allow list.
    let listed = ok(
        &e,
        json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": request_options(&[])}),
    );
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["userName"], "alice");
    let item_id = listed[0]["itemId"].as_str().unwrap().to_string();
    let unrelated = ok(
        &e,
        json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": request_options(&[b"other"])}),
    );
    assert!(unrelated.as_array().unwrap().is_empty());

    // Sign a challenge; a relying party checks the signature over authData || clientDataHash.
    let client_data_hash = sha256(b"the browser's client data");
    let signed = ok(
        &e,
        json!({
            "op": "passkeyGet", "origin": ORIGIN, "optionsJson": request_options(&[]),
            "itemId": item_id, "clientDataHash": b64u::encode(&client_data_hash),
            "userVerified": true,
        }),
    );
    let assertion: Value = serde_json::from_str(signed["responseJson"].as_str().unwrap()).unwrap();
    assert_eq!(assertion["id"], registration["id"]);
    let assertion_auth =
        b64u::decode(assertion["response"]["authenticatorData"].as_str().unwrap()).unwrap();
    let signature = b64u::decode(assertion["response"]["signature"].as_str().unwrap()).unwrap();
    let message = [assertion_auth.as_slice(), client_data_hash.as_slice()].concat();
    public.verify(&message, &signature).unwrap();
    assert_eq!(
        b64u::decode(assertion["response"]["userHandle"].as_str().unwrap()).unwrap(),
        b"alice"
    );
}

#[test]
fn passkeys_are_not_given_to_other_sites_or_duplicated() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    let options = creation_options("alice", &[]);
    let created = ok(
        &e,
        json!({"op": "passkeyCreate", "origin": ORIGIN, "optionsJson": options, "userVerified": false}),
    );
    let registration: Value =
        serde_json::from_str(created["responseJson"].as_str().unwrap()).unwrap();
    let credential_id = b64u::decode(registration["id"].as_str().unwrap()).unwrap();
    let item_id = created["itemId"].as_str().unwrap();

    // An origin that is not the site (or a parent of its relying party id) is refused everywhere.
    let evil = "https://example.com.evil.test";
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyList", "origin": evil, "optionsJson": request_options(&[])})
        ),
        "invalid"
    );
    assert_eq!(
        error_code(
            &e,
            json!({
                "op": "passkeyGet", "origin": evil, "optionsJson": request_options(&[]),
                "itemId": item_id, "clientDataHash": b64u::encode(&sha256(b"x")),
                "userVerified": true,
            })
        ),
        "invalid"
    );
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyCreate", "origin": "http://example.com", "optionsJson": options, "userVerified": true})
        ),
        "invalid",
        "plain http is refused"
    );

    // The site lists the credential it already has: no second passkey is made.
    let again = creation_options("alice", &[&credential_id]);
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyCheckCreate", "origin": ORIGIN, "optionsJson": again})
        ),
        "passkeyExists"
    );

    // A site that only accepts RS256 can't be served.
    let rs256_only = json!({
        "challenge": "AA", "rp": {"id": "example.com"}, "user": {"id": "dQ", "name": "u"},
        "pubKeyCredParams": [{"type": "public-key", "alg": -257}],
    })
    .to_string();
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyCheckCreate", "origin": ORIGIN, "optionsJson": rs256_only})
        ),
        "unsupported"
    );
}

#[test]
fn a_passkey_that_would_be_replaced_is_reported_before_anything_is_created() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    let ask = |user: &str, exclude: &[&[u8]]| {
        call(
            &e,
            json!({"op": "passkeyWouldReplace", "origin": ORIGIN, "optionsJson": creation_options(user, exclude)}),
        )
    };

    // Nothing saved yet: nothing to replace.
    assert!(ask("alice", &[])["ok"]["replaces"].is_null());

    let created = ok(
        &e,
        json!({"op": "passkeyCreate", "origin": ORIGIN, "optionsJson": creation_options("alice", &[]), "userVerified": true}),
    );
    let item_id = created["itemId"].as_str().unwrap();

    // The same account again would replace it, and the reply names what the user is asked about.
    let same = ask("alice", &[]);
    assert_eq!(same["ok"]["replaces"]["itemId"], item_id);
    assert_eq!(same["ok"]["replaces"]["rpId"], "example.com");
    assert_eq!(same["ok"]["replaces"]["userName"], "alice");
    // Asking changed nothing.
    let count = || {
        ok(
            &e,
            json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": request_options(&[])}),
        )
        .as_array()
        .unwrap()
        .len()
    };
    assert_eq!(count(), 1);

    // Another account on the site, and the same account name on another site, replace nothing.
    assert!(ask("bob", &[])["ok"]["replaces"].is_null());
    let other_site = json!({
        "op": "passkeyWouldReplace", "origin": "https://other.test",
        "optionsJson": json!({
            "challenge": "AA", "rp": {"id": "other.test"}, "user": {"id": b64u::encode(b"alice"), "name": "alice"},
            "pubKeyCredParams": [{"type": "public-key", "alg": -7}],
        }).to_string(),
    });
    assert!(call(&e, other_site)["ok"]["replaces"].is_null());

    // The site's own exclude list still refuses before anything is offered, and a wrong origin or a
    // locked vault is an error, not "nothing to replace".
    let registration: Value =
        serde_json::from_str(created["responseJson"].as_str().unwrap()).unwrap();
    let credential_id = b64u::decode(registration["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        ask("alice", &[&credential_id])["error"]["code"],
        "passkeyExists"
    );
    assert_eq!(
        error_code(
            &e,
            json!({"op": "passkeyWouldReplace", "origin": "https://example.com.evil.test", "optionsJson": creation_options("alice", &[])})
        ),
        "invalid"
    );
    e.lock();
    assert_eq!(ask("alice", &[])["error"]["code"], "locked");
}

#[test]
fn malformed_requests_are_errors_not_panics() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("master pw", "Phone").unwrap();
    for bad in [
        "",
        "null",
        "{}",
        r#"{"op":"nope"}"#,
        r#"{"op":"findLogins"}"#,
        "[1,2,3]",
    ] {
        let reply: Value = serde_json::from_str(&provider::call(&e, bad)).unwrap();
        assert_eq!(reply["error"]["code"], "invalid", "{bad}");
    }
}
