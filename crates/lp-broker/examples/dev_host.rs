//! A broker with a throwaway vault, for trying the browser extension without the full app and
//! for the end-to-end test in `extension/e2e`. It approves every client, unlocks without asking
//! and treats every user check (and every "may this be replaced") as passed, so it must never be used with a real vault.
//!
//! ```sh
//! LOCALPASS_PROFILE=dev cargo run -p lp-broker --example dev_host -- \
//!     --login "Example|https://example.com|alice|secret"
//! ```
//!
//! Prints `READY <pipe>` when listening and a `VAULT <json>` line after every change. Reads
//! `lock` and `quit` from standard input, and for tests of a slow Unlock: `hold-unlock` (from then
//! on an unlock prints `UNLOCK?` and waits), `unlock yes` / `unlock no` (answers the waiting one)
//! and `auto-unlock` (back to unlocking at once).

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lp_broker::server::BoxFuture;
use lp_broker::{
    Broker, ClientStore, Host, MemoryClientStore, OverwritePrompt, PairingPrompt, Policy,
};
use lp_core::crypto::KdfParams;
use lp_engine::{Engine, EngineConfig, EngineEvent, FileSecretStore, ItemInput, ItemQuery};

const MASTER: &str = "dev host master password";

struct DevHost {
    engine: Engine,
    clients: MemoryClientStore,
    /// `hold-unlock`: an unlock waits for `unlock yes|no` (as the app waits for the person).
    hold_unlock: AtomicBool,
    unlock_answer: Mutex<Option<tokio::sync::oneshot::Sender<bool>>>,
}

impl Host for DevHost {
    fn engine(&self) -> &Engine {
        &self.engine
    }
    fn clients(&self) -> &dyn ClientStore {
        &self.clients
    }
    fn policy(&self) -> Policy {
        Policy::default()
    }
    fn approve_pairing<'a>(&'a self, prompt: &'a PairingPrompt) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            println!("PAIRED {} code={}", prompt.name, prompt.code);
            true
        })
    }
    fn unlock<'a>(&'a self, _client: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if self.hold_unlock.load(Ordering::SeqCst) {
                let (answer, answered) = tokio::sync::oneshot::channel();
                *self.unlock_answer.lock().expect("unlock answer") = Some(answer);
                println!("UNLOCK?");
                if !answered.await.unwrap_or(false) {
                    return false;
                }
            }
            self.engine.unlock(MASTER).is_ok()
        })
    }
    fn verify_user<'a>(&'a self, _reason: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
    fn confirm_overwrite<'a>(&'a self, _prompt: &'a OverwritePrompt) -> BoxFuture<'a, bool> {
        Box::pin(async { true })
    }
}

fn print_vault(engine: &Engine) {
    let Ok(items) = engine.list_items(&ItemQuery::default()) else {
        return;
    };
    let views: Vec<_> = items
        .iter()
        .filter_map(|i| engine.get_item(&i.id).ok())
        .collect();
    println!(
        "VAULT {}",
        serde_json::to_string(&views).expect("serializes")
    );
}

#[tokio::main]
async fn main() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = EngineConfig::new(dir.path(), "dev");
    config.kdf = KdfParams::insecure_for_tests();
    config.sync.mdns = false;
    let engine = Engine::open(
        config,
        Arc::new(FileSecretStore::new(dir.path().join("device.key"))),
    )
    .expect("engine");
    engine.create_vault(MASTER, "Dev host").expect("vault");

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--login" {
            let spec = args.next().expect("--login needs a value");
            let parts: Vec<&str> = spec.splitn(4, '|').collect();
            let [title, url, user, password] = parts[..] else {
                panic!("--login takes title|url|user|password");
            };
            engine
                .save_item(&ItemInput {
                    title: title.into(),
                    urls: vec![url.into()],
                    username: user.into(),
                    password: password.into(),
                    ..Default::default()
                })
                .expect("save login");
        }
    }

    let host = Arc::new(DevHost {
        engine: engine.clone(),
        clients: MemoryClientStore::default(),
        hold_unlock: AtomicBool::new(false),
        unlock_answer: Mutex::new(None),
    });
    let control = host.clone();
    let (stop, stopped) = tokio::sync::watch::channel(false);
    #[cfg(any(windows, target_os = "linux"))]
    let server = tokio::spawn(lp_broker::pipe::serve(Broker::new(host), stopped));
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (host, stopped, Broker::<DevHost>::new);
        panic!("the dev host needs Windows named pipes or Linux sockets");
    }

    let mut events = engine.subscribe();
    let watcher = engine.clone();
    tokio::spawn(async move {
        while let Ok(event) = events.recv().await {
            if matches!(event, EngineEvent::VaultChanged) {
                print_vault(&watcher);
            }
        }
    });

    println!("READY {}", lp_broker::pipe::pipe_name());
    print_vault(&engine);

    let commands = engine.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match line.trim() {
                "lock" => {
                    commands.lock();
                    println!("LOCKED");
                }
                "vault" => print_vault(&commands),
                "hold-unlock" => control.hold_unlock.store(true, Ordering::SeqCst),
                "auto-unlock" => control.hold_unlock.store(false, Ordering::SeqCst),
                answer @ ("unlock yes" | "unlock no") => {
                    let waiting = control.unlock_answer.lock().expect("unlock answer").take();
                    if let Some(waiting) = waiting {
                        let _ = waiting.send(answer == "unlock yes");
                    }
                }
                "quit" => std::process::exit(0),
                _ => {}
            }
        }
        // Standard input closed: the test is over.
        std::process::exit(0);
    });

    #[cfg(any(windows, target_os = "linux"))]
    {
        let _ = server.await;
        drop(stop);
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    drop(stop);
}
