//! Phase 5.2 hardening of the broker: revoking a client, who may reveal what, resource limits
//! and oversize replies. Each test here failed before its fix (see docs/phase5-hardening-notes.md).

mod common;

use std::sync::atomic::Ordering;

use common::*;
use lp_broker::ClientKind;
use lp_broker::ClientStore;
use lp_broker::client::ClientError;
use lp_engine::{ItemQuery, VaultStatus};
use serde_json::json;

// ---- V4: forgetting a client ends its open sessions ----

#[tokio::test]
async fn a_forgotten_client_cannot_use_its_open_connection() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let mut c = login(&s, &paired.credentials).await;
    let fill = json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"});
    assert_eq!(ok(c.request(&fill).await)["password"], "pw");

    // The user presses "Forget" in Settings: only the store changes.
    s.host
        .clients
        .remove(&paired.credentials.client_id)
        .unwrap();

    let after = c.request(&fill).await;
    assert!(
        after.is_err(),
        "a forgotten client's open connection still works: {after:?}"
    );
}

#[tokio::test]
async fn forgetting_a_client_while_it_waits_for_the_user_reveals_nothing() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let mut c = login(&s, &paired.credentials).await;
    s.host.engine.lock();

    let fill = json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"});
    let pending = c.send_request(&fill).await.unwrap();
    eventually("the unlock prompt", || {
        s.host.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;

    s.host
        .clients
        .remove(&paired.credentials.client_id)
        .unwrap();
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    let answer = c.response(pending).await;
    assert!(
        answer.is_err(),
        "the password reached a client that was forgotten while it waited: {answer:?}"
    );
}

#[tokio::test]
async fn revoking_a_client_closes_its_connection_and_withdraws_its_prompts() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let mut c = login(&s, &paired.credentials).await;
    let mut bystander = {
        let (_p, other) = pair(&s, ClientKind::Extension).await;
        login(&s, &other.credentials).await
    };
    s.host.engine.lock();

    let pending = c
        .send_request(&json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"}))
        .await
        .unwrap();
    eventually("the unlock prompt", || {
        s.host.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;

    let forgotten = &paired.credentials.client_id;
    s.host.clients.remove(forgotten).unwrap();
    s.broker.revoke(forgotten);

    // The prompt it was waiting on is withdrawn at once, and its connection is closed.
    eventually("the unlock prompt to be withdrawn", || {
        s.host.cancelled.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;
    assert!(matches!(
        c.response(pending).await,
        Err(ClientError::Closed)
    ));
    assert_eq!(s.host.engine.status(), VaultStatus::Locked);

    // Other clients are not touched.
    assert_eq!(
        ok(bystander.request(&json!({"op": "status"})).await)["status"],
        "locked"
    );
    // The forgotten client has to pair again.
    let again = lp_broker::client::Client::connect(
        connect_stream(&s),
        &options(ClientKind::Extension),
        &paired.credentials,
    )
    .await;
    assert!(matches!(again, Err(ClientError::Unpaired)));
}

#[tokio::test]
async fn revoking_an_idle_client_closes_its_connection() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let mut c = login(&s, &paired.credentials).await;
    ok(c.request(&json!({"op": "status"})).await);

    s.host
        .clients
        .remove(&paired.credentials.client_id)
        .unwrap();
    s.broker.revoke(&paired.credentials.client_id);
    // The next read finds the connection closed (not an error answer to one request).
    let next = c.request(&json!({"op": "status"})).await;
    assert!(
        matches!(next, Err(ClientError::Closed | ClientError::Io(_))),
        "{next:?}"
    );
}

// ---- V5: reaching a login saved for another site needs the user, every time ----

fn force_fill(id: &str, origin: &str) -> serde_json::Value {
    json!({"op": "fillLogin", "itemId": id, "origin": origin, "force": true})
}

#[tokio::test]
async fn searching_all_items_asks_the_user_every_time() {
    let s = setup(false, false);
    s.host
        .engine
        .save_item(&login_item(
            "Bank",
            "https://bank.example",
            "alice",
            "bank-pw",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;
    let search = json!({"op": "searchItems", "query": "bank"});

    for round in 1..=2 {
        let hits = ok(c.request(&search).await);
        assert_eq!(hits[0]["title"], "Bank");
        assert_eq!(
            s.host.verifies.load(Ordering::SeqCst),
            round,
            "asked each time"
        );
    }
    assert!(
        s.host
            .reasons
            .lock()
            .unwrap()
            .iter()
            .all(|r| !r.is_empty() && !r.contains("bank-pw")),
        "the user is told what for"
    );

    // Declined: nothing comes back.
    s.host.verify.store(false, Ordering::SeqCst);
    assert_eq!(error_code(c.request(&search).await), "denied");
    // Even the policy that relaxes passkey checks does not apply here.
    s.host.verify_every.store(false, Ordering::SeqCst);
    assert_eq!(error_code(c.request(&search).await), "denied");
}

#[tokio::test]
async fn searching_all_items_lists_only_live_logins() {
    let s = setup(false, false);
    let engine = &s.host.engine;
    engine
        .save_item(&login_item("Bank", "https://bank.example", "alice", "pw"))
        .unwrap();
    let trashed = engine
        .save_item(&login_item(
            "Old bank",
            "https://old.example",
            "alice",
            "pw",
        ))
        .unwrap();
    engine.trash_item(&trashed).unwrap();
    engine
        .save_item(&lp_engine::ItemInput {
            kind: "note".into(),
            title: "Bank note".into(),
            notes: "the code is 1234".into(),
            ..Default::default()
        })
        .unwrap();
    let mut c = paired_login(&s).await;
    let hits = ok(c
        .request(&json!({"op": "searchItems", "query": "bank"}))
        .await);
    let titles: Vec<&str> = hits
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Bank"]);
}

#[tokio::test]
async fn forced_fill_asks_the_user_and_names_the_login() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Bank",
            "https://bank.example",
            "alice",
            "bank-pw",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;

    // The normal path for the right site stays silent.
    let same_site = json!({"op": "fillLogin", "itemId": id, "origin": "https://bank.example"});
    assert_eq!(ok(c.request(&same_site).await)["password"], "bank-pw");
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 0);

    // A login for another site: Windows Hello, with the login and the page named.
    let forced = ok(c.request(&force_fill(&id, "https://other.example")).await);
    assert_eq!(forced["password"], "bank-pw");
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 1);
    let reason = s.host.reasons.lock().unwrap().last().unwrap().clone();
    assert!(reason.contains("Bank"), "{reason}");
    assert!(reason.contains("other.example"), "{reason}");
    assert!(!reason.contains("bank-pw"), "{reason}");

    // Every time, also when the vault was just unlocked for this very request.
    s.host.engine.lock();
    ok(c.request(&force_fill(&id, "https://other.example")).await);
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 1);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 2);
    ok(c.request(&force_fill(&id, "https://other.example")).await);
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), 3);

    // Declined: no password. The policy that relaxes passkey checks does not apply.
    s.host.verify.store(false, Ordering::SeqCst);
    s.host.verify_every.store(false, Ordering::SeqCst);
    let refused = c.request(&force_fill(&id, "https://other.example")).await;
    assert_eq!(error_code(refused), "denied");
}

#[tokio::test]
async fn forced_fill_reaches_only_live_logins_and_fails_before_asking() {
    let s = setup(false, false);
    let engine = &s.host.engine;
    let trashed = engine
        .save_item(&login_item("Old", "https://old.example", "a", "old-pw"))
        .unwrap();
    engine.trash_item(&trashed).unwrap();
    let note = engine
        .save_item(&lp_engine::ItemInput {
            kind: "note".into(),
            title: "Note".into(),
            notes: "n".into(),
            ..Default::default()
        })
        .unwrap();
    let live = engine
        .save_item(&login_item("Live", "https://live.example", "a", "live-pw"))
        .unwrap();
    let mut c = paired_login(&s).await;

    for id in [trashed.as_str(), note.as_str(), "not-an-id"] {
        let answer = c.request(&force_fill(id, "https://other.example")).await;
        assert!(answer.is_err(), "{id}: {answer:?}");
    }
    assert_eq!(
        error_code(
            c.request(&force_fill(&trashed, "https://other.example"))
                .await
        ),
        "notFound"
    );
    // A bad page is refused before the user is asked.
    assert_eq!(
        error_code(c.request(&force_fill(&live, "http://other.example")).await),
        "invalid"
    );
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        0,
        "nobody was asked about requests that cannot succeed"
    );
}

#[tokio::test]
async fn a_forced_fill_waiting_for_the_user_is_withdrawn_with_the_connection() {
    let s = setup_with(false, false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Bank",
            "https://bank.example",
            "alice",
            "bank-pw",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;
    c.send_request(&force_fill(&id, "https://other.example"))
        .await
        .unwrap();
    eventually("the user check", || {
        s.host.verifies.load(Ordering::SeqCst) == 1
    })
    .await;
    drop(c);
    eventually("the user check to be dropped", || {
        s.host.cancelled.verifies.load(Ordering::SeqCst) == 1
    })
    .await;
}

// ---- V8: resource limits ----

use std::time::Duration;

use lp_broker::framing::{read_message, write_message};
use tokio::io::DuplexStream;

/// A connection that does nothing, like a program that connects and then goes quiet.
fn idle_socket(s: &Setup) -> DuplexStream {
    connect_stream(s)
}

#[tokio::test(start_paused = true)]
async fn idle_unauthenticated_sockets_leave_room_for_a_login() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    // More idle sockets than the number of sessions the app serves.
    let idle: Vec<DuplexStream> = (0..12).map(|_| idle_socket(&s)).collect();
    tokio::task::yield_now().await;
    let mut c = login(&s, &paired.credentials).await;
    assert_eq!(
        ok(c.request(&json!({"op": "status"})).await)["status"],
        "unlocked"
    );
    drop(idle);
}

#[tokio::test(start_paused = true)]
async fn idle_sockets_are_let_go_after_the_handshake_time_limit() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Extension).await;
    let idle: Vec<DuplexStream> = (0..64).map(|_| idle_socket(&s)).collect();
    tokio::time::sleep(Duration::from_secs(40)).await;
    let mut c = login(&s, &paired.credentials).await;
    assert_eq!(
        ok(c.request(&json!({"op": "status"})).await)["status"],
        "unlocked"
    );
    drop(idle);
}

#[tokio::test(start_paused = true)]
async fn an_idle_authenticated_connection_is_closed() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    ok(c.request(&json!({"op": "status"})).await);
    tokio::time::sleep(Duration::from_secs(2 * 3600)).await;
    let late = c.request(&json!({"op": "status"})).await;
    assert!(
        matches!(late, Err(ClientError::Closed | ClientError::Io(_))),
        "an idle connection stayed open for two hours: {late:?}"
    );
    // The client can simply connect again.
    let mut again = paired_login(&s).await;
    ok(again.request(&json!({"op": "status"})).await);
}

#[tokio::test]
async fn a_connection_cannot_have_unlimited_requests_in_flight() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let mut c = paired_login(&s).await;
    s.host.engine.lock();

    // Each of these waits for the unlock prompt.
    let fill = json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"});
    let mut ids = Vec::new();
    for _ in 0..40 {
        ids.push(c.send_request(&fill).await.unwrap());
    }
    // The ones over the limit are answered at once, without waiting for the user.
    let last = *ids.last().unwrap();
    let answer = tokio::time::timeout(Duration::from_secs(3), c.response(last))
        .await
        .expect("a request over the limit waited instead of being refused");
    assert_eq!(error_code(answer), "tooMany");

    // The ones within the limit are still served.
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    assert_eq!(ok(c.response(ids[0]).await)["password"], "pw");
}

#[tokio::test]
async fn user_checks_are_asked_one_at_a_time() {
    let s = setup_with(false, false, true);
    let mut c = paired_login(&s).await;
    let create = |handle: &[u8]| {
        json!({
            "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
            "rpName": "Example", "userHandle": lp_core::passkey::b64u::encode(handle),
            "userName": "alice", "algorithms": [-7], "uv": "required",
        })
    };
    let mut ids = Vec::new();
    for handle in [b"1", b"2", b"3"] {
        ids.push(c.send_request(&create(handle)).await.unwrap());
    }
    eventually("the first user check", || {
        s.host.verifies.load(Ordering::SeqCst) >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        s.host.verifies.load(Ordering::SeqCst),
        1,
        "three Windows Hello prompts were opened at once"
    );
    for round in 2..=3 {
        s.host.verify_gate.as_ref().unwrap().notify_one();
        eventually("the next user check", || {
            s.host.verifies.load(Ordering::SeqCst) == round
        })
        .await;
    }
    s.host.verify_gate.as_ref().unwrap().notify_one();
    for id in ids {
        ok(c.response(id).await);
    }
}

// ---- V10: a reply that does not fit in a frame is an error, not silence ----

#[tokio::test]
async fn a_reply_over_the_frame_limit_is_an_error_and_the_connection_lives_on() {
    let s = setup(false, false);
    for n in 0..40 {
        let mut item = login_item(
            &format!("{n:02}{}", "x".repeat(40_000)),
            "https://example.com",
            "alice",
            "pw",
        );
        item.notes = String::new();
        s.host.engine.save_item(&item).unwrap();
    }
    let mut c = paired_login(&s).await;

    let huge = json!({"op": "findLogins", "origin": "https://example.com"});
    let answer = tokio::time::timeout(Duration::from_secs(5), c.request(&huge))
        .await
        .expect("no answer at all to a request whose reply was too large");
    assert_eq!(error_code(answer), "tooLarge");

    // The same connection still works.
    let status = tokio::time::timeout(Duration::from_secs(5), c.request(&json!({"op": "status"})))
        .await
        .expect("the connection stopped answering after a too-large reply");
    assert_eq!(ok(status)["status"], "unlocked");
}

#[allow(dead_code)]
async fn _raw(_: &mut DuplexStream) {
    let _ = (read_message::<DuplexStream>, write_message::<DuplexStream>);
}

#[tokio::test]
async fn a_queue_of_requests_does_not_prompt_again_after_a_declined_unlock() {
    let s = setup(false, true);
    let id = s
        .host
        .engine
        .save_item(&login_item("Example", "https://example.com", "alice", "pw"))
        .unwrap();
    let mut c = paired_login(&s).await;
    s.host.engine.lock();
    s.host.unlock_works.store(false, Ordering::SeqCst);

    let fill = json!({"op": "fillLogin", "itemId": id, "origin": "https://example.com"});
    let mut ids = Vec::new();
    for _ in 0..4 {
        ids.push(c.send_request(&fill).await.unwrap());
    }
    eventually("the unlock prompt", || {
        s.host.unlocks.load(Ordering::SeqCst) == 1
    })
    .await;
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    for id in ids {
        assert_eq!(error_code(c.response(id).await), "locked");
    }
    assert_eq!(
        s.host.unlocks.load(Ordering::SeqCst),
        1,
        "the user was asked again for requests that had queued behind the refused prompt"
    );

    // A fresh request afterwards asks again: the user may have changed their mind.
    s.host.unlock_works.store(true, Ordering::SeqCst);
    s.host.unlock_gate.as_ref().unwrap().notify_one();
    assert_eq!(ok(c.request(&fill).await)["password"], "pw");
    assert_eq!(s.host.unlocks.load(Ordering::SeqCst), 2);
}

// ---- V11: nothing the user already has is replaced without their yes ----

use lp_broker::OverwriteKind;

fn create_request(handle: &[u8]) -> serde_json::Value {
    json!({
        "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
        "rpName": "Example", "userHandle": lp_core::passkey::b64u::encode(handle),
        "userName": String::from_utf8_lossy(handle).trim_end_matches("-handle").to_string(),
        "algorithms": [-7], "uv": "required",
    })
}

fn save(origin: &str, user: &str, password: &str) -> serde_json::Value {
    json!({"op": "saveLogin", "origin": origin, "username": user, "password": password})
}

#[tokio::test]
async fn a_new_login_is_saved_without_asking() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let saved = ok(c
        .request(&save("https://example.com", "alice", "pw1"))
        .await);
    assert_eq!(saved["status"], "created");
    // The same login again changes nothing and asks nothing.
    let again = ok(c
        .request(&save("https://example.com", "alice", "pw1"))
        .await);
    assert_eq!(again["status"], "unchanged");
    // Another account, and another site, are new logins.
    ok(c.request(&save("https://example.com", "bob", "pw2")).await);
    ok(c.request(&save("https://example.org", "alice", "pw3"))
        .await);
    assert!(s.host.overwrites.lock().unwrap().is_empty());
}

#[tokio::test]
async fn changing_a_saved_password_needs_the_users_yes() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Example",
            "https://example.com",
            "alice",
            "pw1",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;

    // Declined: the answer is "denied" and the saved password stays.
    s.host.confirm.store(false, Ordering::SeqCst);
    let declined = c
        .request(&save("https://example.com", "alice", "pw2"))
        .await;
    assert_eq!(error_code(declined), "denied");
    assert_eq!(s.host.engine.get_item(&id).unwrap().password, "pw1");
    {
        let asked = s.host.overwrites.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].kind, OverwriteKind::Login);
        assert_eq!(asked[0].title, "Example");
        assert_eq!(asked[0].username, "alice");
        assert_eq!(asked[0].site, "example.com");
        assert_eq!(asked[0].saved_for.as_deref(), Some("https://example.com"));
        assert!(asked[0].same_host);
    }

    // Agreed: it is changed in place, and the old password goes to the history.
    s.host.confirm.store(true, Ordering::SeqCst);
    let done = ok(c
        .request(&save("https://example.com", "alice", "pw2"))
        .await);
    assert_eq!(
        (done["status"].as_str(), done["id"].as_str()),
        (Some("updated"), Some(id.as_str()))
    );
    let item = s.host.engine.get_item(&id).unwrap();
    assert_eq!(item.password, "pw2");
    assert_eq!(item.password_history[0].value, "pw1");
    assert_eq!(s.host.overwrites.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn a_save_from_another_address_of_the_site_asks_and_says_so() {
    let s = setup(false, false);
    let id = s
        .host
        .engine
        .save_item(&login_item(
            "Example",
            "https://example.com",
            "alice",
            "pw1",
        ))
        .unwrap();
    let mut c = paired_login(&s).await;

    s.host.confirm.store(false, Ordering::SeqCst);
    let sub = save("https://login.example.com", "alice", "pw2");
    assert_eq!(error_code(c.request(&sub).await), "denied");
    assert_eq!(s.host.engine.get_item(&id).unwrap().password, "pw1");
    assert_eq!(
        s.host
            .engine
            .list_items(&ItemQuery::default())
            .unwrap()
            .len(),
        1,
        "a declined save leaves nothing behind"
    );
    {
        let asked = s.host.overwrites.lock().unwrap();
        assert_eq!(asked[0].site, "login.example.com");
        assert_eq!(asked[0].saved_for.as_deref(), Some("https://example.com"));
        assert!(
            !asked[0].same_host,
            "the dialog must say it is another address"
        );
    }

    s.host.confirm.store(true, Ordering::SeqCst);
    assert_eq!(ok(c.request(&sub).await)["status"], "updated");
    assert_eq!(s.host.engine.get_item(&id).unwrap().password, "pw2");
}

#[tokio::test]
async fn a_new_passkey_for_an_account_that_has_one_needs_the_users_yes() {
    let s = setup(false, false);
    let mut c = paired_login(&s).await;
    let first = ok(c.request(&create_request(b"alice-handle")).await);
    assert!(
        s.host.overwrites.lock().unwrap().is_empty(),
        "a first passkey asks nothing"
    );
    // Another account on the site is a new passkey too.
    ok(c.request(&create_request(b"bob-handle")).await);
    assert!(s.host.overwrites.lock().unwrap().is_empty());

    // The same account again would replace the first: declined, nothing changes, and the user
    // is not also asked for Windows Hello.
    let verifies = s.host.verifies.load(Ordering::SeqCst);
    s.host.confirm.store(false, Ordering::SeqCst);
    let declined = c.request(&create_request(b"alice-handle")).await;
    assert_eq!(error_code(declined), "denied");
    assert_eq!(s.host.verifies.load(Ordering::SeqCst), verifies);
    {
        let asked = s.host.overwrites.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].kind, OverwriteKind::Passkey);
        assert_eq!(asked[0].site, "example.com");
        assert_eq!(asked[0].username, "alice");
    }
    let candidates = ok(c
        .request(&json!({"op": "passkeyCandidates", "origin": "https://example.com", "rpId": "example.com"}))
        .await);
    let alice = candidates
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["userName"] == "alice")
        .unwrap();
    assert_eq!(
        alice["credentialId"], first["credentialId"],
        "the old passkey is still there"
    );

    // Agreed: it is replaced.
    s.host.confirm.store(true, Ordering::SeqCst);
    let second = ok(c.request(&create_request(b"alice-handle")).await);
    assert_ne!(second["credentialId"], first["credentialId"]);
    assert_eq!(s.host.overwrites.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn the_windows_provider_is_asked_before_it_replaces_a_passkey_too() {
    let s = setup(false, false);
    let (_first, paired) = pair(&s, ClientKind::Provider).await;
    let mut c = login(&s, &paired.credentials).await;
    ok(c.request(&create_request(b"alice-handle")).await);
    s.host.confirm.store(false, Ordering::SeqCst);
    assert_eq!(
        error_code(c.request(&create_request(b"alice-handle")).await),
        "denied"
    );
}
