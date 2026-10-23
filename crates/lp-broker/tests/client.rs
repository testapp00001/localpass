//! The library client against the real broker (and against fake servers for what the real one
//! never does): handshake, proof checks, pairing, errors, timeouts and typed requests.

mod common;

use std::sync::atomic::Ordering;
use std::time::Duration;

use common::*;
use lp_broker::ClientKind;
use lp_broker::ClientStore;
use lp_broker::client::{Client, ClientError, Credentials, Options};
use lp_broker::framing::{read_message, write_message};
use lp_broker::protocol::{
    CLIENT_PROOF_LABEL, ClientMessage, Request, SERVER_PROOF_LABEL, ServerMessage,
    UserVerification, proof,
};
use lp_core::crypto::sha256;
use lp_core::passkey::{
    PasskeyAssertRequest, PasskeyAssertion, PasskeyCandidate, PasskeyCreateRequest, PasskeyCreated,
    b64u, parse_authenticator_data,
};
use lp_core::status::VaultStatus;
use serde_json::json;
use tokio::io::DuplexStream;
use zeroize::Zeroizing;

/// What a fake server does after the hello.
#[derive(Clone, Copy, PartialEq)]
enum Fake {
    /// Follows the protocol with the right secret.
    Honest,
    /// Proves it knows a different secret.
    WrongProof,
    /// Never answers.
    Silent,
    /// Completes the handshake, then never answers a request.
    SilentAfterReady,
    /// Hangs up on the hello.
    HangUp,
}

const SECRET: &[u8] = b"a secret of thirty-two bytes....";

fn creds(id: &str) -> Credentials {
    Credentials {
        client_id: id.into(),
        secret: Zeroizing::new(SECRET.to_vec()),
    }
}

fn fake_server(behavior: Fake, id: &str) -> DuplexStream {
    let (client, mut server) = tokio::io::duplex(1 << 16);
    let id = id.to_string();
    tokio::spawn(async move {
        let hello = read_message(&mut server).await.unwrap().unwrap();
        let hello: ClientMessage = serde_json::from_slice(&hello).unwrap();
        let ClientMessage::Hello {
            v,
            name,
            kind,
            client_id,
        } = hello
        else {
            panic!("the client starts with a hello")
        };
        assert_eq!(v, 1);
        assert_eq!(name, "Windows passkey provider");
        assert_eq!(kind, ClientKind::Provider);
        assert_eq!(client_id.as_deref(), Some(id.as_str()));
        if behavior == Fake::HangUp {
            return;
        }
        let nonce = [9u8; 32];
        let challenge = ServerMessage::Challenge {
            nonce: b64u::encode(&nonce),
        };
        if behavior == Fake::Silent {
            std::future::pending::<()>().await;
        }
        write_message(&mut server, &serde_json::to_vec(&challenge).unwrap())
            .await
            .unwrap();
        let auth = read_message(&mut server).await.unwrap().unwrap();
        let ClientMessage::Auth {
            client_nonce,
            proof: client_proof,
        } = serde_json::from_slice(&auth).unwrap()
        else {
            panic!("the client answers with auth")
        };
        let client_nonce = b64u::decode(&client_nonce).unwrap();
        // The client must prove itself with the right label and secret.
        let expected = proof(CLIENT_PROOF_LABEL, SECRET, &nonce, &client_nonce, &id);
        assert_eq!(b64u::decode(&client_proof).unwrap(), expected);
        let key: &[u8] = if behavior == Fake::WrongProof {
            b"not the secret"
        } else {
            SECRET
        };
        let ready = ServerMessage::Ready {
            status: VaultStatus::Locked,
            server_proof: b64u::encode(&proof(SERVER_PROOF_LABEL, key, &nonce, &client_nonce, &id)),
            app_version: "9.9.9".into(),
        };
        write_message(&mut server, &serde_json::to_vec(&ready).unwrap())
            .await
            .unwrap();
        if behavior == Fake::SilentAfterReady {
            // Hold the connection open and ignore the requests.
            while let Ok(Some(_)) = read_message(&mut server).await {}
        }
        // Honest: this fake answers nothing after the handshake.
        std::future::pending::<()>().await;
    });
    client
}

fn provider_options() -> Options {
    Options::new("Windows passkey provider", ClientKind::Provider)
}

#[tokio::test]
async fn connecting_with_the_stored_secret_logs_in() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    let mut c = Client::connect(connect_stream(&s), &provider_options(), &paired.credentials)
        .await
        .expect("log in");
    assert_eq!(c.status(), VaultStatus::Unlocked);
    assert_eq!(c.app_version(), env!("CARGO_PKG_VERSION"));
    let status = ok(c.request(&Request::Status).await);
    assert_eq!(status["status"], "unlocked");
    assert_eq!(status["deviceName"], "Test PC");
}

#[tokio::test]
async fn an_unpaired_client_pairs_once_and_then_reconnects() {
    let s = setup(false, false);
    let options = provider_options();

    // Never paired: the app doesn't know the id.
    let stranger = Client::connect(connect_stream(&s), &options, &creds("made-up-id")).await;
    assert!(matches!(stranger, Err(ClientError::Unpaired)));

    // Pairing shows the same code the user is asked to confirm, and the stored kind is ours.
    let (first, paired) = pair(&s, ClientKind::Provider).await;
    drop(first);
    let id = paired.credentials.client_id.clone();
    assert_eq!(s.host.clients.get(&id).unwrap().kind, ClientKind::Provider);
    assert_eq!(paired.status, VaultStatus::Unlocked);
    assert_eq!(paired.credentials.secret.len(), 32);
    assert!(
        !format!("{:?}", paired.credentials).contains(&b64u::encode(&paired.credentials.secret))
    );

    // Reconnecting with the stored secret works, with a wrong one it does not.
    Client::connect(connect_stream(&s), &options, &paired.credentials)
        .await
        .expect("reconnect");
    let wrong = Credentials {
        client_id: id.clone(),
        secret: Zeroizing::new(vec![1; 32]),
    };
    let refused = Client::connect(connect_stream(&s), &options, &wrong).await;
    assert!(matches!(refused, Err(ClientError::Unpaired)), "{refused:?}");

    // The user removes the client in the app: it has to pair again.
    s.host.clients.remove(&id).unwrap();
    let gone = Client::connect(connect_stream(&s), &options, &paired.credentials).await;
    assert!(matches!(gone, Err(ClientError::Unpaired)));
    pair(&s, ClientKind::Provider).await;
}

#[tokio::test]
async fn a_denied_pairing_is_reported_and_stores_nothing() {
    let s = setup(false, false);
    s.host.approve.store(false, Ordering::SeqCst);
    let mut shown = None;
    let outcome = Client::pair(connect_stream(&s), &provider_options(), |code| {
        shown = Some(code.to_string())
    })
    .await;
    assert!(matches!(outcome, Err(ClientError::Denied)));
    assert!(shown.is_some(), "the code was shown before the answer");
    assert!(s.host.clients.list().is_empty());
}

#[tokio::test]
async fn a_second_pairing_while_one_waits_is_busy() {
    let s = setup(true, false);
    let first = tokio::spawn({
        let stream = connect_stream(&s);
        async move { Client::pair(stream, &provider_options(), |_| {}).await }
    });
    eventually("the first request to wait", || {
        s.host.prompts.lock().unwrap().len() == 1
    })
    .await;

    let second = Client::pair(connect_stream(&s), &provider_options(), |_| {}).await;
    assert!(matches!(second, Err(ClientError::Busy)), "{second:?}");

    s.host.approval_gate.as_ref().unwrap().notify_one();
    let (_client, paired) = first.await.unwrap().expect("the first one is approved");
    assert!(s.host.clients.get(&paired.credentials.client_id).is_some());
}

#[tokio::test]
async fn a_server_that_cannot_prove_the_secret_is_rejected() {
    let options = provider_options();
    // The fake is only trustworthy if it can also pass.
    let good = Client::connect(fake_server(Fake::Honest, "id-1"), &options, &creds("id-1")).await;
    let good = good.expect("a server that knows the secret is accepted");
    assert_eq!(good.status(), VaultStatus::Locked);
    assert_eq!(good.app_version(), "9.9.9");

    let impostor = Client::connect(
        fake_server(Fake::WrongProof, "id-2"),
        &options,
        &creds("id-2"),
    )
    .await;
    assert!(
        matches!(impostor, Err(ClientError::ServerProofMismatch)),
        "{impostor:?}"
    );
}

#[tokio::test]
async fn silence_and_hang_ups_are_errors_not_hangs() {
    let mut options = provider_options();
    options.timeouts.step = Duration::from_millis(150);
    options.timeouts.request = Duration::from_millis(200);

    let silent = Client::connect(fake_server(Fake::Silent, "id-3"), &options, &creds("id-3")).await;
    assert!(
        matches!(silent, Err(ClientError::Timeout("challenge"))),
        "{silent:?}"
    );

    let hung_up =
        Client::connect(fake_server(Fake::HangUp, "id-4"), &options, &creds("id-4")).await;
    assert!(matches!(hung_up, Err(ClientError::Closed)), "{hung_up:?}");

    // A request that is never answered times out, and the connection is not reused after that
    // (a late answer could be taken for the next request's).
    let mut c = Client::connect(
        fake_server(Fake::SilentAfterReady, "id-5"),
        &options,
        &creds("id-5"),
    )
    .await
    .unwrap();
    let late = c.request(&Request::Status).await;
    assert!(
        matches!(late, Err(ClientError::Timeout("request"))),
        "{late:?}"
    );
    assert!(matches!(
        c.request(&Request::Status).await,
        Err(ClientError::Closed)
    ));
}

#[tokio::test]
async fn provider_clients_are_refused_everything_but_passkey_requests() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    let mut c = login(&s, &paired.credentials).await;
    for request in [
        Request::FindLogins {
            origin: "https://example.com".into(),
        },
        Request::SearchItems {
            query: String::new(),
            limit: None,
        },
        Request::Lock,
        Request::GeneratePassword { options: None },
    ] {
        let refused = c.request(&request).await;
        assert!(
            matches!(&refused, Err(ClientError::Remote { code, .. }) if code == "forbidden"),
            "{request:?} -> {refused:?}"
        );
    }
    // The connection is fine afterwards, and these are allowed.
    assert_eq!(ok(c.request(&Request::Status).await)["status"], "unlocked");
    assert_eq!(ok(c.request(&Request::Unlock).await)["status"], "unlocked");
    assert!(
        ok(c.request(&Request::PasskeyListAll).await)
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn listing_every_passkey_needs_an_unlocked_vault_and_never_prompts() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    let mut c = login(&s, &paired.credentials).await;
    s.host.engine.lock();
    let listed = c.request(&Request::PasskeyListAll).await;
    assert!(matches!(listed, Err(ClientError::Locked)), "{listed:?}");
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 0, "no unlock prompt");
    // Status works while locked, and asking to unlock does prompt.
    assert_eq!(ok(c.request(&Request::Status).await)["status"], "locked");
    assert_eq!(ok(c.request(&Request::Unlock).await)["status"], "unlocked");
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert!(ok(c.request(&Request::PasskeyListAll).await).is_array());
}

#[tokio::test]
async fn typed_passkey_requests_and_results_round_trip() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    let mut c = login(&s, &paired.credentials).await;
    let origin = "https://example.com";

    let created: PasskeyCreated = c
        .request_as(&Request::PasskeyCreate {
            request: PasskeyCreateRequest {
                origin: origin.into(),
                rp_id: "example.com".into(),
                rp_name: "Example".into(),
                user_handle: b"alice-handle".to_vec(),
                user_name: "alice".into(),
                user_display_name: "Alice".into(),
                algorithms: vec![-7],
                exclude: vec![],
                // The app decides this from its own user check, whatever the client claims.
                user_verified: true,
            },
            uv: UserVerification::Required,
        })
        .await
        .expect("create");
    let auth = parse_authenticator_data(&created.auth_data).unwrap();
    let public = auth.credential.unwrap().1;

    let all: Vec<PasskeyCandidate> = c.request_as(&Request::PasskeyListAll).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].credential_id, created.credential_id);
    assert_eq!(all[0].item_id, created.item_id);
    assert_eq!(all[0].user_handle, b"alice-handle");
    assert_eq!(all[0].rp_name, "Example");

    let found: Vec<PasskeyCandidate> = c
        .request_as(&Request::PasskeyCandidates {
            origin: origin.into(),
            rp_id: "example.com".into(),
            allow: vec![created.credential_id.clone()],
        })
        .await
        .unwrap();
    assert_eq!(found.len(), 1);

    // Excluding the credential we hold is refused with the app's stable code.
    let again = c
        .request(&Request::PasskeyCreate {
            request: PasskeyCreateRequest {
                origin: origin.into(),
                rp_id: "example.com".into(),
                rp_name: "Example".into(),
                user_handle: b"someone-else".to_vec(),
                user_name: "bob".into(),
                user_display_name: String::new(),
                algorithms: vec![-7],
                exclude: vec![created.credential_id.clone()],
                user_verified: false,
            },
            uv: UserVerification::Required,
        })
        .await;
    assert!(
        matches!(&again, Err(ClientError::Remote { code, .. }) if code == "passkeyExists"),
        "{again:?}"
    );

    let data = br#"{"type":"webauthn.get"}"#;
    let assertion: PasskeyAssertion = c
        .request_as(&Request::PasskeyAssert {
            request: PasskeyAssertRequest {
                origin: origin.into(),
                rp_id: "example.com".into(),
                item_id: found[0].item_id.clone(),
                client_data_hash: sha256(data).to_vec(),
                user_verified: false,
            },
            uv: UserVerification::Required,
        })
        .await
        .expect("assert");
    let mut signed = assertion.auth_data.clone();
    signed.extend_from_slice(&sha256(data));
    public
        .verify(&signed, &assertion.signature)
        .expect("the signature verifies");
    assert_eq!(assertion.user_handle, b"alice-handle");

    // The user check happened in the app, so the flag is the app's to claim.
    let flags = parse_authenticator_data(&assertion.auth_data)
        .unwrap()
        .flags;
    assert_eq!(flags & lp_core::passkey::FLAG_UV, lp_core::passkey::FLAG_UV);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 2);

    // A result of the wrong shape is a protocol error, not a panic.
    let wrong: Result<PasskeyCreated, _> = c.request_as(&Request::Status).await;
    assert!(matches!(wrong, Err(ClientError::Protocol(_))), "{wrong:?}");
}

#[tokio::test]
async fn requests_must_be_objects() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let bad = c.request(&json!([1, 2, 3])).await;
    assert!(matches!(bad, Err(ClientError::Protocol(_))), "{bad:?}");
    // That was caught before anything was sent, so the connection is still good.
    assert_eq!(ok(c.request(&Request::Status).await)["status"], "unlocked");
}
