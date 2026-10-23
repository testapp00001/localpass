//! The provider's handlers against a real broker: the `dev_host` example (a throwaway vault behind the
//! real named pipe, started as a child process on a private pipe profile) is paired with, created in,
//! listed and signed with through `RealRelay` + `Handler`, and the responses are encoded with the real
//! `webauthn.dll` and parsed again. Nothing here touches the user's LocalPass: the pipe name carries a
//! profile unique to each test, no app path is configured (so nothing can be started), and the pairing
//! secret lives in a temporary directory.
//!
//! `dev_host` approves every pairing and every Windows Hello check, so these tests need no gestures.
//! The tests build it first (`cargo build -p lp-broker --example dev_host`) so a stale copy cannot hide
//! a protocol change.

#![cfg(windows)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Once};
use std::time::{Duration, Instant};

use lp_broker::client::Timeouts;
use lp_core::passkey::{FLAG_BE, FLAG_BS, FLAG_UP, FLAG_UV, parse_authenticator_data, rp_id_hash};
use lp_core::status::VaultStatus;
use lp_winauth::cache::{self, SyncOutcome};
use lp_winauth::handler::{Handler, OpCall};
use lp_winauth::relay::{CancelToken, Probe, RealRelay, Relay, RelayConfig, RelayError};
use lp_winauth::store::Store;
use lp_winauth::sys::{
    NTE_DEVICE_NOT_FOUND, NTE_NOT_FOUND, PLUGIN_LOCKED, PLUGIN_UNLOCKED, REQUEST_TYPE_CTAP2_CBOR,
};
use lp_winauth::testing::{
    self, FakeSink, FakeVerifier, GOOD_SIGNATURE, MemoryCache, WEBAUTHN_IO_MAKE_CREDENTIAL,
};
use windows::Win32::Foundation::S_OK;
use windows::core::GUID;

fn release_build() -> bool {
    std::env::current_exe()
        .unwrap()
        .components()
        .any(|c| c.as_os_str() == "release")
}

fn dev_host_path() -> PathBuf {
    // target/<profile>/deps/<test>.exe -> target/<profile>/examples/dev_host.exe
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.join("examples").join("dev_host.exe")
}

fn build_dev_host() {
    static BUILT: Once = Once::new();
    BUILT.call_once(|| {
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let mut command = Command::new(cargo);
        command.args(["build", "-q", "-p", "lp-broker", "--example", "dev_host"]);
        if release_build() {
            command.arg("--release");
        }
        let status = command.status().expect("run cargo");
        assert!(
            status.success(),
            "could not build the dev_host example (cargo build -p lp-broker --example dev_host)"
        );
        assert!(dev_host_path().exists(), "{:?} is missing", dev_host_path());
    });
}

/// `dev_host` on a private pipe profile.
struct DevHost {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    profile: String,
}

impl DevHost {
    fn start() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        build_dev_host();
        let profile = format!(
            "lpwa-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        );
        let mut child = Command::new(dev_host_path())
            .env("LOCALPASS_PROFILE", &profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start dev_host");
        // A stuck child must not outlive the test run.
        let pid = child.id();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(180));
            let _ = Command::new("taskkill")
                .args(["/F", "/PID", &pid.to_string()])
                .output();
        });
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let host = Self {
            child,
            stdin,
            lines,
            profile,
        };
        host.wait_for("READY");
        host.wait_for_pipe();
        host
    }

    /// `dev_host` prints READY as soon as it has spawned the pipe server, which may not have created
    /// the pipe yet: knock until it exists (the server just sees a client that hangs up).
    fn wait_for_pipe(&self) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let knock = runtime.block_on(lp_broker::pipe::connect_profile(Some(
                self.profile.as_str(),
            )));
            if knock.is_ok() {
                return;
            }
            assert!(Instant::now() < deadline, "the pipe never appeared");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The next stdout line starting with `prefix` (20 s at most); other lines are skipped.
    fn wait_for(&self, prefix: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.starts_with(prefix) => return line,
                Ok(_) => {}
                Err(_) => panic!("dev_host never printed {prefix:?}"),
            }
        }
    }

    fn lock(&mut self) {
        writeln!(self.stdin, "lock").unwrap();
        self.stdin.flush().unwrap();
        self.wait_for("LOCKED");
    }

    fn quit(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

impl Drop for DevHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn relay_for(host: &DevHost, store_dir: &std::path::Path) -> Arc<RealRelay> {
    let mut config = RelayConfig::new(Some(host.profile.clone()), None);
    config.timeouts = Timeouts {
        step: Duration::from_secs(5),
        pairing: Duration::from_secs(20),
        request: Duration::from_secs(30),
    };
    config.launch_wait = Duration::from_secs(1);
    Arc::new(RealRelay::new(Store::new(store_dir), config).expect("relay"))
}

fn call<'a>(tx: u128, encoded: &'a [u8]) -> OpCall<'a> {
    OpCall {
        transaction_id: GUID::from_u128(tx),
        hwnd: 0,
        request_type: REQUEST_TYPE_CTAP2_CBOR,
        encoded,
        signature: GOOD_SIGNATURE,
    }
}

fn handler_for(relay: Arc<RealRelay>) -> (Handler, Arc<FakeSink>) {
    let sink = Arc::new(FakeSink::default());
    let handler = Handler::new(
        testing::api(),
        relay,
        Arc::new(FakeVerifier::default()),
        sink.clone(),
    );
    (handler, sink)
}

#[test]
fn pair_create_list_assert_end_to_end() {
    let host = DevHost::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = relay_for(&host, dir.path());

    // Before pairing: the app answers, but this provider is unknown.
    assert_eq!(relay.probe(), Probe::ReachableUnpaired);
    assert_eq!(relay.status().unwrap_err(), RelayError::NotPaired);

    // Pair: the code shown to the user is the code the app was asked to approve.
    let mut shown = None;
    relay
        .pair(|code| shown = Some(code.to_string()))
        .expect("pairing works");
    let shown = shown.expect("the code was shown");
    assert_eq!(shown.len(), 6);
    let approved = host.wait_for("PAIRED");
    assert!(approved.contains(&format!("code={shown}")), "{approved}");
    assert!(approved.contains("Windows passkey provider"), "{approved}");
    assert!(dir.path().join("client.bin").exists());
    assert_eq!(relay.probe(), Probe::Paired(VaultStatus::Unlocked));
    assert_eq!(relay.status().unwrap(), VaultStatus::Unlocked);

    let (handler, sink) = handler_for(relay.clone());
    assert_eq!(handler.lock_status(), PLUGIN_UNLOCKED);

    // MakeCredential for the captured webauthn.io request.
    let request = testing::unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
    let created = handler.make_credential(&call(1, &request));
    assert_eq!(created.hr, S_OK, "{created:?}");
    assert_eq!(testing::attestation_format(&created.response), "none");
    let auth = testing::attestation_auth_data(&created.response);
    let parsed = parse_authenticator_data(&auth).expect("authenticator data");
    assert_eq!(parsed.rp_id_hash, rp_id_hash("webauthn.io"));
    assert_eq!(parsed.flags, FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS | 0x40);
    assert_eq!(parsed.sign_count, 0);
    assert_eq!(&auth[37..53], &[0u8; 16], "zero AAGUID");
    let (credential_id, public) = parsed.credential.expect("attested credential");
    assert_eq!(sink.rows().len(), 1, "added to the Windows cache");
    assert_eq!(sink.rows()[0].credential_id, credential_id);

    // The same transaction again: the same answer, still one credential in the vault.
    assert_eq!(handler.make_credential(&call(1, &request)), created);

    // A second site, then list everything the cache would be told about.
    let other =
        testing::make_credential_request("example.com", "Example", b"alice", "alice", &[-7], &[]);
    assert_eq!(handler.make_credential(&call(2, &other)).hr, S_OK);
    let all = relay.list_all().expect("list");
    assert_eq!(all.len(), 2, "one credential per transaction, not per call");
    let first = all.iter().find(|c| c.rp_id == "webauthn.io").unwrap();
    assert_eq!(first.credential_id, credential_id);
    assert_eq!(first.user_name, "test0101");
    assert_eq!(first.user_handle, b"webauthnio-test0101");

    // The cache sync mirrors both into an empty cache, and again changes nothing.
    let cache_api = MemoryCache::default();
    assert!(matches!(
        cache::sync(relay.as_ref(), &cache_api),
        SyncOutcome::Done {
            added: 2,
            removed: 0,
            failed: 0,
            ..
        }
    ));
    assert!(matches!(
        cache::sync(relay.as_ref(), &cache_api),
        SyncOutcome::Done {
            added: 0,
            removed: 0,
            unchanged: 2,
            failed: 0
        }
    ));

    // GetAssertion with the allow list Windows narrows to: the response parses and the signature
    // verifies against the public key from the registration.
    let hash = [0x42u8; 32];
    let get =
        testing::get_assertion_request("webauthn.io", &hash, std::slice::from_ref(&credential_id));
    let asserted = handler.get_assertion(&call(3, &get));
    assert_eq!(asserted.hr, S_OK, "{asserted:?}");
    let assertion = testing::parse_assertion(&asserted.response);
    assert_eq!(assertion.credential_id, credential_id);
    assert_eq!(assertion.user_id, b"webauthnio-test0101");
    assert_eq!(assertion.user_name, "test0101");
    assert!(assertion.user_selected);
    let parsed = parse_authenticator_data(&assertion.auth_data).unwrap();
    assert_eq!(parsed.rp_id_hash, rp_id_hash("webauthn.io"));
    assert_eq!(parsed.flags, FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS);
    assert_eq!(parsed.sign_count, 0);
    let mut signed = assertion.auth_data.clone();
    signed.extend_from_slice(&hash);
    public
        .verify(&signed, &assertion.signature)
        .expect("the assertion verifies against the registered public key");

    // No allow list: one passkey for the site, so it is chosen.
    let bare = testing::get_assertion_request("example.com", &[7; 32], &[]);
    assert_eq!(handler.get_assertion(&call(4, &bare)).hr, S_OK);
    // Nothing for a site we have no passkey for.
    let none = testing::get_assertion_request("nothing.example", &[7; 32], &[]);
    assert_eq!(handler.get_assertion(&call(5, &none)).hr, NTE_NOT_FOUND);

    // Exclude list: the engine refuses a second passkey for a credential it holds.
    let exclude = testing::make_credential_request(
        "webauthn.io",
        "webauthn.io",
        b"someone-else",
        "bob",
        &[-7],
        std::slice::from_ref(&credential_id),
    );
    assert_eq!(
        handler.make_credential(&call(6, &exclude)).hr,
        lp_winauth::sys::NTE_EXISTS
    );
    assert_eq!(relay.list_all().unwrap().len(), 2);

    // A second account at example.com: the app reports "several match" in the one sign-in request, which
    // Windows hears as not found (no picker yet), and nothing is signed; an allow list narrows it again.
    let bob = testing::make_credential_request(
        "example.com",
        "Example",
        b"bob-handle",
        "bob",
        &[-7],
        &[],
    );
    assert_eq!(handler.make_credential(&call(7, &bob)).hr, S_OK);
    let both = testing::get_assertion_request("example.com", &[7; 32], &[]);
    assert_eq!(handler.get_assertion(&call(8, &both)).hr, NTE_NOT_FOUND);
    let bobs_id = relay
        .list_all()
        .unwrap()
        .into_iter()
        .find(|c| c.user_name == "bob")
        .unwrap()
        .credential_id;
    let narrowed =
        testing::get_assertion_request("example.com", &[7; 32], std::slice::from_ref(&bobs_id));
    let signed = handler.get_assertion(&call(9, &narrowed));
    assert_eq!(signed.hr, S_OK, "{signed:?}");
    let signed = testing::parse_assertion(&signed.response);
    assert_eq!(signed.credential_id, bobs_id);
    assert_eq!(signed.user_name, "bob");
    assert_eq!(signed.user_id, b"bob-handle");
}

#[test]
fn a_locked_vault_is_unlocked_by_the_app_for_a_sign_in_but_never_for_listing() {
    let mut host = DevHost::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = relay_for(&host, dir.path());
    relay.pair(|_| {}).expect("pairing works");
    let (handler, _sink) = handler_for(relay.clone());

    let request =
        testing::make_credential_request("example.com", "Example", b"alice", "alice", &[-7], &[]);
    assert_eq!(handler.make_credential(&call(1, &request)).hr, S_OK);

    host.lock();
    assert_eq!(relay.status().unwrap(), VaultStatus::Locked);
    assert_eq!(handler.lock_status(), PLUGIN_LOCKED);
    // Listing never prompts: a locked vault is "cancelled", and a cache sync then changes nothing.
    assert_eq!(relay.list_all().unwrap_err(), RelayError::Cancelled);
    let cache_api = MemoryCache::default();
    cache_api.push(testing::stale_row());
    assert!(matches!(
        cache::sync(relay.as_ref(), &cache_api),
        SyncOutcome::Skipped(_)
    ));
    assert_eq!(cache_api.rows().len(), 1, "the cache was left alone");
    assert_eq!(cache_api.calls(), Vec::<&str>::new(), "not even read");
    assert_eq!(
        relay.status().unwrap(),
        VaultStatus::Locked,
        "listing did not unlock"
    );

    // A sign-in is one request: the app unlocks (dev_host unlocks) and signs.
    let get = testing::get_assertion_request("example.com", &[9; 32], &[]);
    let outcome = handler.get_assertion(&call(2, &get));
    assert_eq!(outcome.hr, S_OK, "{outcome:?}");
    assert_eq!(relay.status().unwrap(), VaultStatus::Unlocked);
    assert_eq!(handler.lock_status(), PLUGIN_UNLOCKED);
}

#[test]
fn a_provider_the_app_does_not_know_is_refused() {
    let host = DevHost::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = relay_for(&host, dir.path());
    relay.pair(|_| {}).expect("pairing works");
    assert!(relay.status().is_ok());

    // A damaged secret cannot log in: the app answers "unknown client".
    let store = Store::new(dir.path());
    let mut credentials = store.load_credentials().unwrap().unwrap();
    credentials.secret = zeroize::Zeroizing::new(vec![0x55; 32]);
    store.save_credentials(&credentials).unwrap();
    assert_eq!(relay.status().unwrap_err(), RelayError::NotPaired);
    let (handler, _sink) = handler_for(relay);
    let request = testing::unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
    assert_eq!(
        handler.make_credential(&call(1, &request)).hr,
        NTE_DEVICE_NOT_FOUND
    );

    // No credentials at all.
    store.clear_credentials().unwrap();
    assert_eq!(
        relay_for(&host, dir.path()).status().unwrap_err(),
        RelayError::NotPaired
    );
}

#[test]
fn a_cancel_before_the_request_goes_out_creates_nothing() {
    let host = DevHost::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = relay_for(&host, dir.path());
    relay.pair(|_| {}).expect("pairing works");
    let token = CancelToken::new();
    token.cancel();
    let request = lp_core::passkey::PasskeyCreateRequest {
        origin: "https://example.com".into(),
        rp_id: "example.com".into(),
        rp_name: "Example".into(),
        user_handle: b"alice".to_vec(),
        user_name: "alice".into(),
        user_display_name: String::new(),
        algorithms: vec![-7],
        exclude: vec![],
        user_verified: false,
    };
    let started = Instant::now();
    assert_eq!(
        relay.create(&token, request).unwrap_err(),
        RelayError::Cancelled
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(
        relay.list_all().unwrap().is_empty(),
        "no credential was created"
    );
}

#[test]
fn when_the_app_goes_away_requests_fail_fast_as_device_not_found() {
    let mut host = DevHost::start();
    let dir = tempfile::tempdir().unwrap();
    let relay = relay_for(&host, dir.path());
    relay.pair(|_| {}).expect("pairing works");
    let (handler, sink) = handler_for(relay.clone());
    let request =
        testing::make_credential_request("example.com", "Example", b"alice", "alice", &[-7], &[]);
    assert_eq!(handler.make_credential(&call(1, &request)).hr, S_OK);

    host.quit();
    let started = Instant::now();
    assert!(matches!(relay.status(), Err(RelayError::Unreachable(_))));
    assert_eq!(handler.lock_status(), PLUGIN_LOCKED);
    assert_eq!(relay.probe(), Probe::NotRunning);
    let get = testing::get_assertion_request("example.com", &[9; 32], &[]);
    assert_eq!(
        handler.get_assertion(&call(2, &get)).hr,
        NTE_DEVICE_NOT_FOUND
    );
    let again =
        testing::make_credential_request("example.com", "Example", b"bob", "bob", &[-7], &[]);
    assert_eq!(
        handler.make_credential(&call(3, &again)).hr,
        NTE_DEVICE_NOT_FOUND
    );
    // The same transaction asked again gets the remembered answer without another attempt.
    assert_eq!(
        handler.make_credential(&call(3, &again)).hr,
        NTE_DEVICE_NOT_FOUND
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        sink.rows().len(),
        1,
        "only the first creation reached the cache"
    );
}
