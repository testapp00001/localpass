//! The native-messaging helper as a real child process: frames go in on its stdin and come out
//! of its stdout exactly as a browser would use it, with the app on the other side of a real
//! named pipe (on Linux, a Unix socket in a runtime folder of the test's own).

#![cfg(any(windows, target_os = "linux"))]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use lp_broker::nmhost::allowed_origin;
use lp_broker::server::BoxFuture;
use lp_broker::{
    Broker, ClientStore, Host, MemoryClientStore, OverwritePrompt, PairingPrompt, Policy,
};
use lp_core::crypto::KdfParams;
use lp_engine::{Engine, EngineConfig, FileSecretStore};
use serde_json::{Value, json};

struct SimpleHost {
    engine: Engine,
    clients: MemoryClientStore,
}

impl Host for SimpleHost {
    fn engine(&self) -> &Engine {
        &self.engine
    }
    fn clients(&self) -> &dyn ClientStore {
        &self.clients
    }
    fn policy(&self) -> Policy {
        Policy::default()
    }
    fn approve_pairing<'a>(&'a self, _prompt: &'a PairingPrompt) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn unlock<'a>(&'a self) -> BoxFuture<'a, bool> {
        Box::pin(async { false })
    }
    fn verify_user<'a>(&'a self, _reason: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn confirm_overwrite<'a>(&'a self, _prompt: &'a OverwritePrompt) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
}

fn helper_path() -> PathBuf {
    // target/debug/deps/<test>.exe -> target/debug/examples/nmhost.exe
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    path.pop();
    path.join("examples")
        .join(format!("nmhost{}", std::env::consts::EXE_SUFFIX))
}

/// The runtime folder the tests' sockets live in (Linux), so no test ever touches the user's own.
#[cfg(target_os = "linux")]
fn runtime_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().unwrap()).path()
}

fn kill(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
    #[cfg(not(windows))]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

struct Helper {
    child: Child,
}

impl Helper {
    fn start(origin: &str, profile: &str) -> Self {
        let mut command = Command::new(helper_path());
        command
            .arg(origin)
            .env("LOCALPASS_PROFILE", profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        #[cfg(target_os = "linux")]
        command.env("XDG_RUNTIME_DIR", runtime_dir());
        let child = command
            .spawn()
            .expect("start the helper (is the nmhost example built?)");
        // A stuck helper must not hang the test run.
        let pid = child.id();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(30));
            kill(pid);
        });
        Self { child }
    }

    fn send(&mut self, value: &Value) {
        let bytes = serde_json::to_vec(value).unwrap();
        let stdin = self.child.stdin.as_mut().unwrap();
        stdin
            .write_all(&(bytes.len() as u32).to_le_bytes())
            .unwrap();
        stdin.write_all(&bytes).unwrap();
        stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Option<Value> {
        let stdout = self.child.stdout.as_mut().unwrap();
        let mut len = [0u8; 4];
        stdout.read_exact(&mut len).ok()?;
        let mut body = vec![0u8; u32::from_le_bytes(len) as usize];
        stdout.read_exact(&mut body).ok()?;
        Some(serde_json::from_slice(&body).unwrap())
    }

    fn finish(mut self) -> i32 {
        drop(self.child.stdin.take());
        self.child.wait().unwrap().code().unwrap_or(-1)
    }
}

fn start_app(
    profile: &str,
) -> (
    tokio::sync::watch::Sender<bool>,
    tokio::task::JoinHandle<std::io::Result<()>>,
    tempfile::TempDir,
) {
    // SAFETY: only this test binary reads the variables, before the pipe is created.
    unsafe { std::env::set_var("LOCALPASS_PROFILE", profile) };
    #[cfg(target_os = "linux")]
    unsafe {
        std::env::set_var("XDG_RUNTIME_DIR", runtime_dir())
    };
    let dir = tempfile::tempdir().unwrap();
    let mut config = EngineConfig::new(dir.path(), "test");
    config.kdf = KdfParams::insecure_for_tests();
    config.sync.mdns = false;
    let engine = Engine::open(
        config,
        Arc::new(FileSecretStore::new(dir.path().join("device.key"))),
    )
    .unwrap();
    engine.create_vault("test master pw", "Test PC").unwrap();
    let host = Arc::new(SimpleHost {
        engine,
        clients: MemoryClientStore::default(),
    });
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(lp_broker::pipe::serve(Broker::new(host), stopped));
    (stop, server, dir)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn frames_travel_between_stdio_and_the_pipe() {
    let profile = format!("nmhost-{}", std::process::id());
    let (stop, server, _dir) = start_app(&profile);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let result = tokio::task::spawn_blocking(move || {
        let mut helper = Helper::start(&allowed_origin(), &profile);
        helper.send(
            &json!({"type": "hello", "v": 1, "name": "Chrome extension", "kind": "extension"}),
        );
        assert_eq!(helper.recv().unwrap()["type"], "unpaired");
        helper.send(&json!({"type": "pair"}));
        assert_eq!(helper.recv().unwrap()["type"], "pairing");
        assert_eq!(helper.recv().unwrap()["type"], "paired");
        helper.send(&json!({"id": 1, "op": "status"}));
        let reply = helper.recv().unwrap();
        assert_eq!(reply["id"], 1);
        assert_eq!(reply["result"]["status"], "unlocked");
        // The browser closing the port ends the helper cleanly.
        helper.finish()
    })
    .await
    .unwrap();
    assert_eq!(result, 0);
    stop.send(true).unwrap();
    server.await.unwrap().unwrap();
}

#[test]
fn the_helper_refuses_other_extensions() {
    let mut helper = Helper::start("chrome-extension://someotherextensionid/", "nmhost-refuse");
    let reply = helper.recv().expect("a refusal message");
    assert_eq!(reply["type"], "hostError");
    assert_eq!(reply["code"], "forbidden");
    assert_eq!(helper.finish(), 1);
}

#[test]
fn the_helper_reports_that_the_app_is_not_running() {
    let mut helper = Helper::start(&allowed_origin(), "nmhost-no-app");
    let reply = helper.recv().expect("an error message");
    assert_eq!(reply["type"], "hostError");
    assert_eq!(reply["code"], "appNotRunning");
    assert_eq!(helper.finish(), 0);
}
