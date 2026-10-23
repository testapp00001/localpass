//! The broker end to end over in-memory streams, with a real engine and a scripted host.
//!
//! Tests of the wire format itself (garbage, wrong proofs, hello order) talk raw frames; the
//! rest use the library client in `lp_broker::client`.

mod common;

use std::sync::atomic::Ordering;

use common::*;
use lp_broker::ClientKind;
use lp_broker::ClientStore;
use lp_broker::client::{Client, ClientError};
use lp_broker::framing::{read_message, write_message};
use lp_broker::protocol::{CLIENT_PROOF_LABEL, ClientMessage, ServerMessage, proof};
use lp_core::crypto::sha256;
use lp_core::passkey::{b64u, parse_authenticator_data};
use lp_engine::{ItemQuery, VaultStatus};
use serde_json::json;
use tokio::io::{AsyncWriteExt, DuplexStream};

/// A bare connection for tests that send hand-made frames.
struct Raw {
    stream: DuplexStream,
}

impl Raw {
    fn connect(s: &Setup) -> Self {
        Self {
            stream: connect_stream(s),
        }
    }

    async fn send_raw(&mut self, bytes: &[u8]) {
        write_message(&mut self.stream, bytes).await.unwrap();
    }

    async fn send<T: serde::Serialize>(&mut self, value: &T) {
        self.send_raw(&serde_json::to_vec(value).unwrap()).await;
    }

    async fn recv_bytes(&mut self) -> Option<Vec<u8>> {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            read_message(&mut self.stream),
        )
        .await
        .expect("timed out waiting for the broker")
        .ok()
        .flatten()
    }

    async fn recv_server(&mut self) -> ServerMessage {
        serde_json::from_slice(&self.recv_bytes().await.expect("connection closed")).unwrap()
    }

    async fn hello(&mut self, kind: ClientKind, client_id: Option<&str>) {
        self.send(&ClientMessage::Hello {
            v: 1,
            name: "Chrome extension".into(),
            kind,
            client_id: client_id.map(str::to_string),
        })
        .await;
    }
}

#[tokio::test]
async fn pairing_then_reconnecting_with_the_secret() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let id = &paired.credentials.client_id;
    let stored = s.host.clients.get(id).expect("client stored");
    assert_eq!(stored.secret, *paired.credentials.secret);
    assert_eq!(s.host.prompts.lock().unwrap()[0].name, "Chrome extension");

    let mut c = login(&s, &paired.credentials).await;
    let status = ok(c.request(&json!({"op": "status"})).await);
    assert_eq!(status["status"], "unlocked");
    assert_eq!(status["deviceName"], "Test PC");
    assert!(s.host.clients.get(id).unwrap().last_used_ms > 0);
}

#[tokio::test]
async fn denied_pairing_stores_nothing() {
    let s = setup(false, false);
    s.host.approve.store(false, Ordering::SeqCst);
    let mut c = Raw::connect(&s);
    c.hello(ClientKind::Extension, None).await;
    assert!(matches!(c.recv_server().await, ServerMessage::Unpaired));
    c.send(&ClientMessage::Pair).await;
    assert!(matches!(
        c.recv_server().await,
        ServerMessage::Pairing { .. }
    ));
    assert!(matches!(c.recv_server().await, ServerMessage::Denied));
    assert!(c.recv_bytes().await.is_none(), "connection is closed");
    assert!(s.host.clients.list().is_empty());
}

#[tokio::test]
async fn wrong_secret_and_unknown_ids_cannot_log_in() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let id = paired.credentials.client_id.clone();

    let mut c = Raw::connect(&s);
    c.hello(ClientKind::Extension, Some(&id)).await;
    let ServerMessage::Challenge { nonce } = c.recv_server().await else {
        panic!("challenge")
    };
    let nonce = b64u::decode(&nonce).unwrap();
    c.send(&ClientMessage::Auth {
        client_nonce: b64u::encode(&[1u8; 32]),
        proof: b64u::encode(&proof(
            CLIENT_PROOF_LABEL,
            b"not the secret",
            &nonce,
            &[1u8; 32],
            &id,
        )),
    })
    .await;
    assert!(matches!(c.recv_server().await, ServerMessage::Error { code, .. } if code == "auth"));
    assert!(c.recv_bytes().await.is_none());

    // A client the app doesn't know is treated as unpaired (and can pair again).
    let mut c = Raw::connect(&s);
    c.hello(ClientKind::Extension, Some("removed-client")).await;
    assert!(matches!(c.recv_server().await, ServerMessage::Unpaired));

    // Removing a client in the app locks it out.
    s.host.clients.remove(&id).unwrap();
    let mut c = Raw::connect(&s);
    c.hello(ClientKind::Extension, Some(&id)).await;
    assert!(matches!(c.recv_server().await, ServerMessage::Unpaired));
}

#[tokio::test]
async fn only_one_pairing_request_waits_at_a_time() {
    let s = setup(true, false);
    let mut first = Raw::connect(&s);
    first.hello(ClientKind::Extension, None).await;
    assert!(matches!(first.recv_server().await, ServerMessage::Unpaired));
    first.send(&ClientMessage::Pair).await;
    assert!(matches!(
        first.recv_server().await,
        ServerMessage::Pairing { .. }
    ));

    let mut second = Raw::connect(&s);
    second.hello(ClientKind::Extension, None).await;
    assert!(matches!(
        second.recv_server().await,
        ServerMessage::Unpaired
    ));
    second.send(&ClientMessage::Pair).await;
    assert!(
        matches!(second.recv_server().await, ServerMessage::Error { code, .. } if code == "busy")
    );

    s.host.approval_gate.as_ref().unwrap().notify_one();
    assert!(matches!(
        first.recv_server().await,
        ServerMessage::Paired { .. }
    ));
}

#[tokio::test]
async fn logins_over_the_protocol_with_a_locked_vault() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Example",
            "https://example.com",
            "alice",
            "alice-pass",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;

    let found = ok(c
        .request(&json!({"op": "findLogins", "origin": "https://login.example.com"}))
        .await);
    assert_eq!(found[0]["username"], "alice");
    assert!(!found.to_string().contains("alice-pass"));
    let hits = ok(c
        .request(&json!({"op": "searchItems", "query": "exam"}))
        .await);
    assert_eq!(hits[0]["title"], "Example");

    s.host.engine.lock();
    assert_eq!(
        error_code(
            c.request(&json!({"op": "findLogins", "origin": "https://example.com"}))
                .await
        ),
        "locked"
    );
    assert_eq!(
        s.host.unlocks.load(Ordering::SeqCst),
        0,
        "finding logins never prompts"
    );
    assert_eq!(
        ok(c.request(&json!({"op": "status"})).await)["status"],
        "locked"
    );

    let fill = ok(c
        .request(&json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"}))
        .await);
    assert_eq!(fill["password"], "alice-pass");
    assert_eq!(
        s.host.unlocks.load(Ordering::SeqCst),
        1,
        "filling asks the user to unlock"
    );
    assert_eq!(s.host.engine.status(), VaultStatus::Unlocked);

    // Wrong site, forced fill from the full list, and bad origins.
    let wrong = c
        .request(&json!({"op": "fillLogin", "itemId": id, "origin": "https://example.org"}))
        .await;
    assert_eq!(error_code(wrong), "invalid");
    let forced = ok(c
        .request(
            &json!({"op": "fillLogin", "itemId": id, "origin": "https://example.org", "force": true}),
        )
        .await);
    assert_eq!(forced["username"], "alice");
    let http = c
        .request(&json!({"op": "findLogins", "origin": "http://example.com"}))
        .await;
    assert_eq!(error_code(http), "invalid");

    // Saving a captured login, and the generator on a locked vault.
    let saved = ok(c
        .request(&json!({"op": "saveLogin", "origin": "https://shop.example.net", "username": "bob", "password": "pw"}))
        .await);
    assert_eq!(saved["status"], "created");
    s.host.engine.lock();
    let generated = ok(c
        .request(&json!({"op": "generatePassword", "options": {"length": 12}}))
        .await);
    assert_eq!(generated["password"].as_str().unwrap().len(), 12);
    assert_eq!(
        ok(c.request(&json!({"op": "lock"})).await)["status"],
        "locked"
    );
}

#[tokio::test]
async fn a_declined_unlock_is_reported_as_locked() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let mut c = paired_login(&s).await;
    s.host.engine.lock();
    s.host.unlock_works.store(false, Ordering::SeqCst);

    let fill = json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"});
    assert_eq!(error_code(c.request(&fill).await), "locked");
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(
        ok(c.request(&json!({"op": "status"})).await)["status"],
        "locked"
    );

    s.host.unlock_works.store(true, Ordering::SeqCst);
    assert_eq!(ok(c.request(&fill).await)["username"], "alice");
}

#[tokio::test]
async fn passkeys_over_the_protocol_need_the_user_check() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let create = |uv: &str| {
        json!({
            "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
            "rpName": "Example", "userHandle": b64u::encode(b"alice-handle"),
            "userName": "alice", "algorithms": [-7], "uv": uv,
        })
    };

    // Declined on Windows Hello: nothing is created.
    s.host.verify.store(false, Ordering::SeqCst);
    assert_eq!(error_code(c.request(&create("required")).await), "denied");
    assert_eq!(
        s.host
            .engine
            .list_items(&ItemQuery::default())
            .unwrap()
            .len(),
        0
    );

    // Requests that can't succeed fail before the user is asked.
    let verifies = s.host.verifies.load(Ordering::SeqCst);
    let mut bad = create("required");
    bad["origin"] = json!("https://evil.example.net");
    assert_eq!(error_code(c.request(&bad).await), "invalid");
    let mut rsa = create("required");
    rsa["algorithms"] = json!([-257]);
    assert_eq!(error_code(c.request(&rsa).await), "unsupported");
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        verifies,
        "no prompt for doomed requests"
    );

    // Approved: a passkey is made and the UV flag is claimed.
    s.host.verify.store(true, Ordering::SeqCst);
    let created = ok(c.request(&create("required")).await);
    let auth =
        parse_authenticator_data(&b64u::decode(created["authData"].as_str().unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        auth.flags & lp_core::passkey::FLAG_UV,
        lp_core::passkey::FLAG_UV
    );
    let public = auth.credential.unwrap().1;

    let candidates = ok(c
        .request(&json!({"op": "passkeyCandidates", "origin": "https://example.com", "rpId": "example.com"}))
        .await);
    assert_eq!(candidates.as_array().unwrap().len(), 1);
    let item_id = candidates[0]["itemId"].as_str().unwrap().to_string();

    let client_data = br#"{"type":"webauthn.get","origin":"https://example.com"}"#;
    let assertion = ok(c
        .request(&json!({
            "op": "passkeyAssert", "origin": "https://example.com", "rpId": "example.com",
            "itemId": item_id, "clientDataHash": b64u::encode(&sha256(client_data)), "uv": "preferred",
        }))
        .await);
    let mut signed = b64u::decode(assertion["authData"].as_str().unwrap()).unwrap();
    signed.extend_from_slice(&sha256(client_data));
    public
        .verify(
            &signed,
            &b64u::decode(assertion["signature"].as_str().unwrap()).unwrap(),
        )
        .expect("signature verifies");

    // The user is told which site is asking: the site's name and id when creating, the id when
    // signing in.
    let reasons = s.host.reasons.lock().unwrap().clone();
    assert_eq!(reasons[0], "Create a passkey for Example (example.com)");
    assert!(
        reasons.contains(&"Sign in to example.com".to_string()),
        "{reasons:?}"
    );

    // "Discouraged" skips the check only when the user's policy allows it.
    let before = s.host.verifies.load(Ordering::SeqCst);
    let sign = json!({
        "op": "passkeyAssert", "origin": "https://example.com", "rpId": "example.com",
        "itemId": item_id, "clientDataHash": b64u::encode(&sha256(client_data)), "uv": "discouraged",
    });
    ok(c.request(&sign).await);
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        before + 1,
        "policy asks every time"
    );
    s.host.verify_every.store(false, Ordering::SeqCst);
    let reply = ok(c.request(&sign).await);
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        before + 1,
        "no prompt when policy allows"
    );
    let flags =
        parse_authenticator_data(&b64u::decode(reply["authData"].as_str().unwrap()).unwrap())
            .unwrap()
            .flags;
    assert_eq!(
        flags & lp_core::passkey::FLAG_UV,
        0,
        "UV is not claimed when nobody was verified"
    );
}

fn create_request(handle: &[u8]) -> serde_json::Value {
    json!({
        "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
        "rpName": "Example", "userHandle": b64u::encode(handle),
        "userName": "alice", "algorithms": [-7], "uv": "required",
    })
}

fn assert_request(item_id: &str) -> serde_json::Value {
    let hash = sha256(br#"{"type":"webauthn.get","origin":"https://example.com"}"#);
    json!({
        "op": "passkeyAssert", "origin": "https://example.com", "rpId": "example.com",
        "itemId": item_id, "clientDataHash": b64u::encode(&hash), "uv": "required",
    })
}

fn uv_flag(reply: &serde_json::Value) -> u8 {
    let auth = b64u::decode(reply["authData"].as_str().unwrap()).unwrap();
    parse_authenticator_data(&auth).unwrap().flags & lp_core::passkey::FLAG_UV
}

/// Makes a passkey on an unlocked vault, locks it again, and returns its item id.
async fn passkey_then_lock(s: &Setup, c: &mut Client<DuplexStream>) -> String {
    ok(c.request(&create_request(b"alice-handle")).await);
    let candidates = ok(c
        .request(&json!({"op": "passkeyCandidates", "origin": "https://example.com", "rpId": "example.com"}))
        .await);
    s.host.engine.lock();
    s.host.verifies.store(0, Ordering::SeqCst);
    candidates[0]["itemId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn unlocking_for_a_passkey_request_is_its_user_check() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let item_id = passkey_then_lock(&s, &mut c).await;

    // Sign in on a locked vault: one unlock (Windows Hello or the master password), no second check,
    // and the user is still reported as verified.
    let signed = ok(c.request(&assert_request(&item_id)).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(uv_flag(&signed), lp_core::passkey::FLAG_UV);

    // Creating on a locked vault is the same.
    s.host.engine.lock();
    let created = ok(c.request(&create_request(b"bob-handle")).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 2);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(uv_flag(&created), lp_core::passkey::FLAG_UV);
}

#[tokio::test]
async fn the_unlock_is_not_carried_over_to_the_next_request() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let item_id = passkey_then_lock(&s, &mut c).await;

    ok(c.request(&assert_request(&item_id)).await);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);
    // The vault is open now, so the next request asks the user once, as it always did.
    let again = ok(c.request(&assert_request(&item_id)).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
    assert_eq!(uv_flag(&again), lp_core::passkey::FLAG_UV);
    ok(c.request(&create_request(b"bob-handle")).await);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 2);

    // An explicit unlock request is not a user check for the passkey request that follows it.
    s.host.engine.lock();
    ok(c.request(&json!({"op": "unlock"})).await);
    ok(c.request(&assert_request(&item_id)).await);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn an_unlocked_vault_still_asks_the_user_once() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let item_id = passkey_then_lock(&s, &mut c).await;
    ok(c.request(&json!({"op": "unlock"})).await);
    s.host.unlocks.store(0, Ordering::SeqCst);

    let signed = ok(c.request(&assert_request(&item_id)).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 0);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
    assert_eq!(uv_flag(&signed), lp_core::passkey::FLAG_UV);

    // The policy for "discouraged" still applies to an unlocked vault.
    s.host.verify_every.store(false, Ordering::SeqCst);
    let mut relaxed = assert_request(&item_id);
    relaxed["uv"] = json!("discouraged");
    let signed = ok(c.request(&relaxed).await);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
    assert_eq!(uv_flag(&signed), 0);
}

#[tokio::test]
async fn a_refused_unlock_is_not_a_user_check_and_asks_nothing_more() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let item_id = passkey_then_lock(&s, &mut c).await;
    s.host.unlock_works.store(false, Ordering::SeqCst);

    for request in [assert_request(&item_id), create_request(b"bob-handle")] {
        assert_eq!(error_code(c.request(&request).await), "locked");
    }
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 2);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(s.host.engine.status(), VaultStatus::Locked);
}

#[tokio::test]
async fn two_requests_waiting_for_one_unlock_share_it_but_only_one_gets_its_check() {
    let s = setup(false, true);
    let mut c = paired_login(&s).await;
    let item_id = passkey_then_lock(&s, &mut c).await;

    let first = c.send_request(&assert_request(&item_id)).await.unwrap();
    let second = c.send_request(&assert_request(&item_id)).await.unwrap();
    eventually("the unlock prompt", || {
        s.host.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    let (first, second) = (c.response(first).await, c.response(second).await);
    // One request's user unlocked the vault; the other one rode on it and so asks for its own check.
    assert_eq!(uv_flag(&ok(first)), lp_core::passkey::FLAG_UV);
    assert_eq!(uv_flag(&ok(second)), lp_core::passkey::FLAG_UV);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
}

fn assert_any_request(allow: &[&str]) -> serde_json::Value {
    let hash = sha256(br#"{"type":"webauthn.get","origin":"https://example.com"}"#);
    json!({
        "op": "passkeyAssertAny", "origin": "https://example.com", "rpId": "example.com",
        "allow": allow, "clientDataHash": b64u::encode(&hash), "uv": "required",
    })
}

async fn provider_login(s: &Setup) -> Client<DuplexStream> {
    let (_first, paired) = pair(s, ClientKind::Provider).await;
    login(s, &paired.credentials).await
}

/// Makes a passkey for example.com and returns its credential id (base64url).
async fn make_passkey(c: &mut Client<DuplexStream>, handle: &[u8]) -> String {
    let created = ok(c.request(&create_request(handle)).await);
    created["credentialId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn one_request_signs_in_with_one_user_check_whether_the_vault_was_locked_or_not() {
    let s = setup(false, false);
    let mut c = provider_login(&s).await;
    let credential = make_passkey(&mut c, b"alice-handle").await;

    // Locked: one unlock (Hello or master password) is the whole user check.
    s.host.engine.lock();
    s.host.verifies.store(0, Ordering::SeqCst);
    let signed = ok(c.request(&assert_any_request(&[])).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);
    assert_eq!(uv_flag(&signed), lp_core::passkey::FLAG_UV);
    assert_eq!(signed["credentialId"], credential);
    assert_eq!(signed["userName"], "alice");
    assert!(signed["userHandle"].as_str().is_some_and(|h| !h.is_empty()));
    let auth = b64u::decode(signed["authData"].as_str().unwrap()).unwrap();
    assert_eq!(&auth[..32], &sha256(b"example.com"));

    // Unlocked: asks once, with the site named, and the unlock was not carried over.
    let signed = ok(c.request(&assert_any_request(&[&credential])).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
    assert_eq!(uv_flag(&signed), lp_core::passkey::FLAG_UV);
    assert_eq!(
        s.host.reasons.lock().unwrap().last().unwrap(),
        "Sign in to example.com"
    );
}

#[tokio::test]
async fn the_signature_of_a_single_request_sign_in_verifies() {
    let s = setup(false, false);
    let mut c = provider_login(&s).await;
    let created = ok(c.request(&create_request(b"alice-handle")).await);
    let public =
        parse_authenticator_data(&b64u::decode(created["authData"].as_str().unwrap()).unwrap())
            .unwrap()
            .credential
            .unwrap()
            .1;
    let client_data = br#"{"type":"webauthn.get","origin":"https://example.com"}"#;
    let signed = ok(c.request(&assert_any_request(&[])).await);
    let mut message = b64u::decode(signed["authData"].as_str().unwrap()).unwrap();
    message.extend_from_slice(&sha256(client_data));
    public
        .verify(
            &message,
            &b64u::decode(signed["signature"].as_str().unwrap()).unwrap(),
        )
        .expect("signature verifies");
}

#[tokio::test]
async fn no_match_or_several_matches_sign_nothing_and_ask_nothing() {
    let s = setup(false, false);
    let mut c = provider_login(&s).await;

    // No passkey at all: also after unlocking a locked vault.
    assert_eq!(
        error_code(c.request(&assert_any_request(&[])).await),
        "notFound"
    );
    s.host.engine.lock();
    assert_eq!(
        error_code(c.request(&assert_any_request(&[])).await),
        "notFound"
    );
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);

    // Two accounts on one site, and no allow list or one naming both: the provider must choose.
    let alice = make_passkey(&mut c, b"alice-handle").await;
    let bob = make_passkey(&mut c, b"bob-handle").await;
    s.host.verifies.store(0, Ordering::SeqCst);
    assert_eq!(
        error_code(c.request(&assert_any_request(&[])).await),
        "ambiguous"
    );
    assert_eq!(
        error_code(c.request(&assert_any_request(&[&alice, &bob])).await),
        "ambiguous"
    );
    // An allow list naming a passkey we do not hold.
    let other = b64u::encode(b"not-ours");
    assert_eq!(
        error_code(c.request(&assert_any_request(&[&other])).await),
        "notFound"
    );
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        0,
        "nobody was asked"
    );

    // The allow list narrows it to one.
    let signed = ok(c.request(&assert_any_request(&[&bob, &other])).await);
    assert_eq!(signed["credentialId"], bob);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_single_request_sign_in_with_a_refused_unlock_or_check_signs_nothing() {
    let s = setup(false, false);
    let mut c = provider_login(&s).await;
    make_passkey(&mut c, b"alice-handle").await;
    s.host.engine.lock();
    s.host.verifies.store(0, Ordering::SeqCst);

    s.host.unlock_works.store(false, Ordering::SeqCst);
    assert_eq!(
        error_code(c.request(&assert_any_request(&[])).await),
        "locked"
    );
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);

    // Unlocked, then Hello refused.
    s.host.unlock_works.store(true, Ordering::SeqCst);
    ok(c.request(&json!({"op": "unlock"})).await);
    s.host.verify.store(false, Ordering::SeqCst);
    assert_eq!(
        error_code(c.request(&assert_any_request(&[])).await),
        "denied"
    );
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);

    // A bad client data hash is refused before anybody is asked.
    s.host.verify.store(true, Ordering::SeqCst);
    let mut bad = assert_any_request(&[]);
    bad["clientDataHash"] = json!(b64u::encode(b"short"));
    assert_eq!(error_code(c.request(&bad).await), "invalid");
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn only_providers_may_use_the_single_request_sign_in() {
    let s = setup(false, false);
    let mut extension = paired_login(&s).await;
    ok(extension.request(&create_request(b"alice-handle")).await);
    assert_eq!(
        error_code(extension.request(&assert_any_request(&[])).await),
        "forbidden"
    );
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1, "only the create");
    // Listing every passkey with no prompt is the provider's (RA-2); the extension has no use for it.
    assert_eq!(
        error_code(extension.request(&json!({"op": "passkeyListAll"})).await),
        "forbidden"
    );

    let mut provider = provider_login(&s).await;
    ok(provider.request(&assert_any_request(&[])).await);
    ok(provider.request(&json!({"op": "passkeyListAll"})).await);
}

#[tokio::test]
async fn provider_clients_can_only_use_passkey_requests() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    // Providers log in the same way; the kind comes from what was stored at pairing time.
    let mut c = login(&s, &paired.credentials).await;
    for op in [
        json!({"op": "findLogins", "origin": "https://example.com"}),
        json!({"op": "searchItems", "query": ""}),
        json!({"op": "saveLogin", "origin": "https://example.com", "username": "a", "password": "b"}),
        json!({"op": "fillLogin", "itemId": "x", "origin": "https://example.com"}),
        json!({"op": "generatePassword"}),
        json!({"op": "lock"}),
    ] {
        assert_eq!(error_code(c.request(&op).await), "forbidden");
    }
    let candidates = ok(c
        .request(&json!({"op": "passkeyCandidates", "origin": "https://example.com", "rpId": "example.com"}))
        .await);
    assert!(candidates.as_array().unwrap().is_empty());

    // Listing every passkey is a provider operation too, and shows metadata only.
    let none = ok(c.request(&json!({"op": "passkeyListAll"})).await);
    assert!(none.as_array().unwrap().is_empty());
    ok(c.request(&json!({
        "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
        "rpName": "Example", "userHandle": b64u::encode(b"alice-handle"),
        "userName": "alice", "userDisplayName": "Alice", "algorithms": [-7], "uv": "required",
    }))
    .await);
    let all = ok(c.request(&json!({"op": "passkeyListAll"})).await);
    let all = all.as_array().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0]["rpId"], "example.com");
    assert_eq!(all[0]["rpName"], "Example");
    assert_eq!(all[0]["userName"], "alice");
    assert_eq!(all[0]["userDisplayName"], "Alice");
    assert_eq!(all[0]["userHandle"], b64u::encode(b"alice-handle"));
    assert!(
        all[0]["credentialId"]
            .as_str()
            .is_some_and(|c| !c.is_empty())
    );
    assert!(
        !all[0].to_string().to_lowercase().contains("private"),
        "no key material"
    );
}

#[tokio::test]
async fn bad_input_never_breaks_the_broker() {
    let s = setup(false, false);

    // Garbage before the handshake finishes.
    for garbage in [
        &b"not json"[..],
        &b"{}"[..],
        &b"[1,2,3]"[..],
        &b"\xff\xfe"[..],
    ] {
        let mut c = Raw::connect(&s);
        c.send_raw(garbage).await;
        assert!(c.recv_bytes().await.is_none(), "connection closed");
    }
    // A protocol version we don't speak.
    let mut c = Raw::connect(&s);
    c.send(&json!({"type": "hello", "v": 99, "name": "x"}))
        .await;
    assert!(
        matches!(c.recv_server().await, ServerMessage::Error { code, .. } if code == "version")
    );
    // An oversized frame.
    let mut c = Raw::connect(&s);
    c.stream.write_all(&u32::MAX.to_le_bytes()).await.unwrap();
    assert!(c.recv_bytes().await.is_none());

    // After login: unknown requests with an id are answered, garbage without one hangs up.
    let mut c = paired_login(&s).await;
    assert_eq!(
        error_code(c.request(&json!({"op": "doesNotExist"})).await),
        "unsupported"
    );
    assert_eq!(
        error_code(c.request(&json!({"op": "findLogins"})).await),
        "unsupported"
    );
    assert!(ok(c.request(&json!({"op": "status"})).await)["status"] == "unlocked");
    let mut raw = Raw {
        stream: c.into_stream(),
    };
    raw.send_raw(b"{\"nonsense\":true}").await;
    assert!(raw.recv_bytes().await.is_none());

    // The broker is still healthy afterwards.
    let mut c = paired_login(&s).await;
    assert_eq!(
        ok(c.request(&json!({"op": "status"})).await)["status"],
        "unlocked"
    );
}

#[tokio::test]
async fn a_request_waiting_for_the_user_does_not_block_others() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Example",
            "https://example.com",
            "alice",
            "alice-pass",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;
    s.host.engine.lock();

    let slow = c
        .send_request(&json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"}))
        .await
        .unwrap();
    let quick = c.send_request(&json!({"op": "status"})).await.unwrap();
    // The status answer arrives first even though fillLogin was sent earlier.
    let first = c.response(quick).await;
    assert_eq!(ok(first)["status"], "locked");
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    let second = c.response(slow).await;
    assert_eq!(ok(second)["password"], "alice-pass");
}

#[tokio::test]
async fn a_dropped_connection_withdraws_a_pending_unlock_prompt() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let mut c = paired_login(&s).await;
    s.host.engine.lock();

    c.send_request(&json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"}))
        .await
        .unwrap();
    eventually("the unlock prompt", || {
        s.host.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(s.host.cancelled.unlocks.load(Ordering::SeqCst), 0);

    drop(c);
    eventually("the unlock prompt to be dropped", || {
        s.host.cancelled.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(s.host.engine.status(), VaultStatus::Locked);
}

#[tokio::test]
async fn a_dropped_connection_withdraws_a_pending_user_check_and_makes_no_passkey() {
    let s = setup_with(false, false, true);
    let mut c = paired_login(&s).await;
    // Two passkey requests wait on the user; the second queues behind the first's prompt, and
    // both go with the connection.
    for handle in [b"1", b"2"] {
        c.send_request(&json!({
            "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
            "rpName": "Example", "userHandle": b64u::encode(handle),
            "userName": "alice", "algorithms": [-7], "uv": "required",
        }))
        .await
        .unwrap();
    }
    eventually("the user check", || {
        s.host.verifies.load(Ordering::SeqCst) == 1
    })
    .await;

    drop(c);
    eventually("the user check to be dropped", || {
        s.host.cancelled.verifies.load(Ordering::SeqCst) == 1
    })
    .await;
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        1,
        "the queued request never got its own prompt"
    );
    // Nothing signs or saves for a caller that has left.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        s.host
            .engine
            .list_items(&ItemQuery::default())
            .unwrap()
            .is_empty(),
        "no passkey was made"
    );
}

#[tokio::test]
async fn a_dropped_connection_withdraws_a_pairing_request_and_frees_the_slot() {
    let s = setup(true, false);
    let first = tokio::spawn({
        let stream = connect_stream(&s);
        async move { Client::pair(stream, &options(ClientKind::Provider), |_| {}).await }
    });
    eventually("the pairing prompt", || {
        s.host.prompts.lock().unwrap().len() == 1
    })
    .await;

    // The client gives up (the OS cancelled the operation).
    first.abort();
    eventually("the pairing prompt to be withdrawn", || {
        s.host.cancelled.approvals.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(s.host.clients.list().is_empty());

    // The next client is not told "busy", and can be approved.
    s.host.approval_gate.as_ref().unwrap().notify_one();
    let (_client, paired) = pair(&s, ClientKind::Provider).await;
    assert!(s.host.clients.get(&paired.credentials.client_id).is_some());
    assert_eq!(s.host.clients.list().len(), 1);
}

#[tokio::test]
async fn the_library_client_reports_why_a_request_failed() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    s.host.engine.lock();
    s.host.unlock_works.store(false, Ordering::SeqCst);
    let fill = json!({"op": "fillLogin", "itemId": "x", "origin": "https://example.com"});
    assert!(matches!(c.request(&fill).await, Err(ClientError::Locked)));
    s.host.unlock_works.store(true, Ordering::SeqCst);
    s.host.verify.store(false, Ordering::SeqCst);
    // The vault is open, so the passkey request has to ask the user itself (unlocking inside the
    // request would have been the check).
    ok(c.request(&json!({"op": "unlock"})).await);
    let create = json!({
        "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
        "userHandle": b64u::encode(b"h"), "algorithms": [-7], "uv": "required",
    });
    assert!(matches!(c.request(&create).await, Err(ClientError::Denied)));
}

/// The same handshake over the real Windows named pipe (a private name for this test).
#[cfg(windows)]
#[tokio::test]
async fn the_real_named_pipe_works_and_cannot_be_squatted() {
    let profile = format!("brokertest-{}", std::process::id());
    // SAFETY: this is the only test in the binary that touches the environment, and it runs
    // before any pipe code reads it.
    unsafe { std::env::set_var("LOCALPASS_PROFILE", &profile) };
    assert!(lp_broker::pipe::pipe_name().contains("brokertest-"));
    assert_eq!(
        lp_broker::pipe::pipe_name_for(Some(&profile)),
        lp_broker::pipe::pipe_name(),
        "a client that is told the profile finds the same pipe"
    );
    assert_ne!(
        lp_broker::pipe::pipe_name_for(None),
        lp_broker::pipe::pipe_name()
    );

    let s = setup(false, false);
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(lp_broker::pipe::serve(s.broker.clone(), stopped.clone()));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    // A second listener on the same name is refused instead of sharing it.
    assert!(
        lp_broker::pipe::serve(s.broker.clone(), stopped.clone())
            .await
            .is_err(),
        "the pipe name must not be shareable"
    );

    // The environment-selected pipe, with hand-made frames.
    let mut pipe = lp_broker::pipe::connect()
        .await
        .expect("connect to the pipe");
    let hello = json!({"type": "hello", "v": 1, "name": "Test client", "kind": "extension"});
    write_message(&mut pipe, &serde_json::to_vec(&hello).unwrap())
        .await
        .unwrap();
    let reply: ServerMessage =
        serde_json::from_slice(&read_message(&mut pipe).await.unwrap().unwrap()).unwrap();
    assert!(matches!(reply, ServerMessage::Unpaired));
    drop(pipe);

    // The library client with an explicit profile: pair, then log in again with the secret.
    let options = options(ClientKind::Provider);
    let (mut client, paired) =
        lp_broker::client::pair_pipe(Some(&profile), &options, |code| assert_eq!(code.len(), 6))
            .await
            .expect("pair over the pipe");
    let status = ok(client.request(&json!({"op": "status"})).await);
    assert_eq!(status["status"], "unlocked");
    drop(client);
    let mut again = lp_broker::client::connect_pipe(Some(&profile), &options, &paired.credentials)
        .await
        .expect("log in over the pipe");
    assert_eq!(again.status(), VaultStatus::Unlocked);
    assert_eq!(
        ok(again.request(&json!({"op": "status"})).await)["status"],
        "unlocked"
    );
    // No app on another profile's pipe.
    let missing =
        lp_broker::client::connect_pipe(Some("nobody-listens-here"), &options, &paired.credentials)
            .await
            .expect_err("nothing listens there");
    assert!(missing.app_not_running(), "{missing:?}");

    stop.send(true).unwrap();
    server.await.unwrap().expect("server stops cleanly");
}
