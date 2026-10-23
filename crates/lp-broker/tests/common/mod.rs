//! Shared by the broker's integration tests: a real engine behind a scripted `Host`, and
//! helpers that connect to the in-process broker with the library client over in-memory pipes.

#![allow(dead_code)]

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lp_broker::client::{Client, ClientError, Credentials, Options, Paired};
use lp_broker::server::BoxFuture;
use lp_broker::{
    Broker, ClientKind, ClientStore, Host, MemoryClientStore, OverwritePrompt, PairingPrompt,
    Policy,
};
use lp_core::crypto::KdfParams;
use lp_engine::{Engine, EngineConfig, FileSecretStore, ItemInput};
use serde_json::Value;
use tokio::io::DuplexStream;
use tokio::sync::Notify;

pub const MASTER: &str = "test master pw";

/// Counts prompts that were dropped before they finished (the connection went away).
#[derive(Default)]
pub struct Cancelled {
    pub approvals: AtomicUsize,
    pub unlocks: AtomicUsize,
    pub verifies: AtomicUsize,
}

/// Bumps its counter when dropped before `finish`.
struct Watch<'a> {
    counter: &'a AtomicUsize,
    done: bool,
}

impl<'a> Watch<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        Self {
            counter,
            done: false,
        }
    }

    fn finish(mut self) {
        self.done = true;
    }
}

impl Drop for Watch<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.counter.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub struct TestHost {
    pub engine: Engine,
    pub clients: MemoryClientStore,
    pub approve: AtomicBool,
    pub approval_gate: Option<Arc<Notify>>,
    pub unlock_gate: Option<Arc<Notify>>,
    pub verify_gate: Option<Arc<Notify>>,
    pub unlock_works: AtomicBool,
    pub verify: AtomicBool,
    /// What the user answers when asked whether something may be overwritten.
    pub confirm: AtomicBool,
    pub verify_every: AtomicBool,
    pub prompts: Mutex<Vec<PairingPrompt>>,
    /// The `reason` of every user check, in order.
    pub reasons: Mutex<Vec<String>>,
    /// Every "may this be overwritten" question, in order.
    pub overwrites: Mutex<Vec<OverwritePrompt>>,
    pub unlocks: AtomicUsize,
    pub verifies: AtomicUsize,
    pub cancelled: Cancelled,
}

impl Host for TestHost {
    fn engine(&self) -> &Engine {
        &self.engine
    }
    fn clients(&self) -> &dyn ClientStore {
        &self.clients
    }
    fn policy(&self) -> Policy {
        Policy {
            verify_every_passkey: self.verify_every.load(Ordering::SeqCst),
        }
    }
    fn approve_pairing<'a>(&'a self, prompt: &'a PairingPrompt) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let watch = Watch::new(&self.cancelled.approvals);
            self.prompts.lock().unwrap().push(prompt.clone());
            if let Some(gate) = &self.approval_gate {
                gate.notified().await;
            }
            watch.finish();
            self.approve.load(Ordering::SeqCst)
        })
    }
    fn unlock<'a>(&'a self) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let watch = Watch::new(&self.cancelled.unlocks);
            self.unlocks.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.unlock_gate {
                gate.notified().await;
            }
            watch.finish();
            self.unlock_works.load(Ordering::SeqCst) && self.engine.unlock(MASTER).is_ok()
        })
    }
    fn verify_user<'a>(&'a self, reason: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let watch = Watch::new(&self.cancelled.verifies);
            self.verifies.fetch_add(1, Ordering::SeqCst);
            self.reasons.lock().unwrap().push(reason.to_string());
            if let Some(gate) = &self.verify_gate {
                gate.notified().await;
            }
            watch.finish();
            self.verify.load(Ordering::SeqCst)
        })
    }
    fn confirm_overwrite<'a>(&'a self, prompt: &'a OverwritePrompt) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            self.overwrites.lock().unwrap().push(prompt.clone());
            self.confirm.load(Ordering::SeqCst)
        })
    }
}

pub struct Setup {
    pub host: Arc<TestHost>,
    pub broker: Arc<Broker<TestHost>>,
    _dir: tempfile::TempDir,
}

pub fn setup(approval_gate: bool, unlock_gate: bool) -> Setup {
    setup_with(approval_gate, unlock_gate, false)
}

pub fn setup_with(approval_gate: bool, unlock_gate: bool, verify_gate: bool) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let mut config = EngineConfig::new(Path::new(dir.path()), "test");
    config.kdf = KdfParams::insecure_for_tests();
    config.sync.mdns = false;
    let engine = Engine::open(
        config,
        Arc::new(FileSecretStore::new(dir.path().join("device.key"))),
    )
    .unwrap();
    engine.create_vault(MASTER, "Test PC").unwrap();
    let gate = |wanted: bool| wanted.then(|| Arc::new(Notify::new()));
    let host = Arc::new(TestHost {
        engine,
        clients: MemoryClientStore::default(),
        approve: AtomicBool::new(true),
        approval_gate: gate(approval_gate),
        unlock_gate: gate(unlock_gate),
        verify_gate: gate(verify_gate),
        unlock_works: AtomicBool::new(true),
        verify: AtomicBool::new(true),
        confirm: AtomicBool::new(true),
        verify_every: AtomicBool::new(true),
        prompts: Mutex::new(Vec::new()),
        reasons: Mutex::new(Vec::new()),
        overwrites: Mutex::new(Vec::new()),
        unlocks: AtomicUsize::new(0),
        verifies: AtomicUsize::new(0),
        cancelled: Cancelled::default(),
    });
    Setup {
        broker: Broker::new(host.clone()),
        host,
        _dir: dir,
    }
}

/// One end of an in-memory pipe whose other end is served by the broker.
pub fn connect_stream(s: &Setup) -> DuplexStream {
    let (client, server) = tokio::io::duplex(1 << 16);
    let broker = s.broker.clone();
    tokio::spawn(async move { broker.serve(server).await });
    client
}

pub fn options(kind: ClientKind) -> Options {
    Options::new("Chrome extension", kind)
}

/// Pairs with the library client, asserting that the code it was shown is the one the user is
/// asked to confirm in the app.
pub async fn pair(s: &Setup, kind: ClientKind) -> (Client<DuplexStream>, Paired) {
    let mut shown = None;
    let (client, paired) = Client::pair(connect_stream(s), &options(kind), |code| {
        shown = Some(code.to_string())
    })
    .await
    .expect("pairing works");
    let code = shown.expect("the client was shown a code");
    assert_eq!(code.len(), 6);
    assert_eq!(s.host.prompts.lock().unwrap().last().unwrap().code, code);
    (client, paired)
}

/// Opens an authenticated connection for a paired client. (The kind in the hello is ignored for
/// a known client: the app uses what it stored at pairing time.)
pub async fn login(s: &Setup, credentials: &Credentials) -> Client<DuplexStream> {
    Client::connect(
        connect_stream(s),
        &options(ClientKind::Extension),
        credentials,
    )
    .await
    .expect("login works")
}

pub async fn paired_login(s: &Setup) -> Client<DuplexStream> {
    let (_first, paired) = pair(s, ClientKind::Extension).await;
    login(s, &paired.credentials).await
}

/// The result of a request that should have succeeded.
pub fn ok(reply: Result<Value, ClientError>) -> Value {
    reply.unwrap_or_else(|e| panic!("request failed: {e:?}"))
}

/// The app's error code for a request that should have failed.
pub fn error_code(reply: Result<Value, ClientError>) -> String {
    match reply {
        Ok(value) => panic!("the request succeeded: {value}"),
        Err(ClientError::Locked) => "locked".into(),
        Err(ClientError::Denied) => "denied".into(),
        Err(ClientError::Remote { code, .. }) => code,
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

pub fn login_item(title: &str, url: &str, user: &str, password: &str) -> ItemInput {
    ItemInput {
        title: title.into(),
        username: user.into(),
        password: password.into(),
        urls: vec![url.into()],
        ..Default::default()
    }
}

/// Waits (up to 3 s) until `check` is true.
pub async fn eventually(what: &str, check: impl Fn() -> bool) {
    for _ in 0..60 {
        if check() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}
