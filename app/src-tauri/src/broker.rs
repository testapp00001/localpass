//! Connects the broker (`lp-broker`) to the app. Approving a new browser, unlocking and the
//! Windows Hello check all go through the UI, and paired clients are kept encrypted next to
//! the vault. The broker runs on Windows (named pipe) and Linux (Unix socket).

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_broker::server::BoxFuture;
use lp_broker::{Broker, ClientRecord, ClientStore, Host, OverwritePrompt, PairingPrompt, Policy};
use lp_engine::{Engine, LocalProtect, VaultStatus};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::oneshot;
use tracing::{info, warn};

use crate::AppState;

const CLIENTS_PURPOSE: &[u8] = b"localpass-broker-clients-v1";

/// Paired clients (browser extensions, the Windows passkey provider), one file protected by the
/// platform (DPAPI on Windows, the system keyring on Linux; see `lp_engine::LocalProtect`).
pub struct ClientFile {
    path: PathBuf,
    protector: Arc<dyn LocalProtect>,
    clients: Mutex<BTreeMap<String, ClientRecord>>,
    /// The file existed but could not be read: the first save keeps a copy of it (RA-5).
    unreadable: AtomicBool,
}

impl ClientFile {
    pub fn load(path: PathBuf, protector: Arc<dyn LocalProtect>) -> Self {
        let mut unreadable = false;
        let clients = match std::fs::read(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => BTreeMap::new(),
            Err(e) => {
                warn!(error = %e, "could not read the paired clients; starting empty");
                unreadable = true;
                BTreeMap::new()
            }
            Ok(blob) => {
                let list = protector
                    .unprotect(CLIENTS_PURPOSE, &blob)
                    .map_err(|e| e.to_string())
                    .and_then(|plain| {
                        serde_json::from_slice::<Vec<ClientRecord>>(&plain)
                            .map_err(|e| e.to_string())
                    });
                match list {
                    Ok(list) => list.into_iter().map(|c| (c.id.clone(), c)).collect(),
                    Err(e) => {
                        warn!(error = %e, "could not read the paired clients; starting empty");
                        unreadable = true;
                        BTreeMap::new()
                    }
                }
            }
        };
        Self {
            path,
            protector,
            clients: Mutex::new(clients),
            unreadable: AtomicBool::new(unreadable),
        }
    }

    fn save(&self, clients: &BTreeMap<String, ClientRecord>) -> io::Result<()> {
        let list: Vec<&ClientRecord> = clients.values().collect();
        let plain = serde_json::to_vec(&list).map_err(io::Error::other)?;
        let blob = self
            .protector
            .protect(CLIENTS_PURPOSE, &plain)
            .map_err(io::Error::other)?;
        if self.unreadable.swap(false, Ordering::SeqCst) {
            // A file we could not read is about to be replaced (the next pairing). A later start, or a
            // person, may still be able to use it, so keep the bytes.
            let _ = std::fs::copy(&self.path, self.path.with_extension("unreadable"));
        }
        let tmp = self.path.with_extension("tmp");
        std::fs::write(&tmp, blob)?;
        std::fs::rename(&tmp, &self.path)
    }
}

impl ClientStore for ClientFile {
    fn get(&self, id: &str) -> Option<ClientRecord> {
        self.clients.lock().expect("clients").get(id).cloned()
    }

    fn insert(&self, record: ClientRecord) -> io::Result<()> {
        let mut clients = self.clients.lock().expect("clients");
        clients.insert(record.id.clone(), record);
        self.save(&clients)
    }

    fn remove(&self, id: &str) -> io::Result<()> {
        let mut clients = self.clients.lock().expect("clients");
        clients.remove(id);
        self.save(&clients)
    }

    fn list(&self) -> Vec<ClientRecord> {
        self.clients
            .lock()
            .expect("clients")
            .values()
            .cloned()
            .collect()
    }

    fn touch(&self, id: &str, now_ms: u64) {
        let mut clients = self.clients.lock().expect("clients");
        let Some(client) = clients.get_mut(id) else {
            return;
        };
        // Written at most once a minute: the timestamp is only for the settings list.
        if now_ms.saturating_sub(client.last_used_ms) > 60_000 {
            client.last_used_ms = now_ms;
            let _ = self.save(&clients);
        }
    }
}

/// Questions waiting for the user's answer in the UI (pairing requests, "replace what is saved?").
#[derive(Default)]
pub struct Approvals {
    next: AtomicU64,
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
}

impl Approvals {
    fn open(&self) -> (u64, oneshot::Receiver<bool>) {
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("approvals").insert(id, tx);
        (id, rx)
    }

    /// Forgets a request nobody is waiting for any more.
    fn forget(&self, id: u64) {
        self.pending.lock().expect("approvals").remove(&id);
    }

    /// Delivers the user's answer. False if the request is gone (it timed out).
    pub fn resolve(&self, id: u64, approve: bool) -> bool {
        self.pending
            .lock()
            .expect("approvals")
            .remove(&id)
            .is_some_and(|tx| tx.send(approve).is_ok())
    }
}

pub struct AppHost {
    app: AppHandle,
    engine: Engine,
    clients: ClientFile,
    approvals: Approvals,
    /// Open master-password checks (request id -> wrong tries so far).
    #[cfg(not(windows))]
    verifying: Mutex<HashMap<u64, u32>>,
}

/// How a master-password try for a browser's check went.
#[cfg(not(windows))]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VerifyOutcome {
    /// The password was right; the browser's request goes on.
    Accepted,
    /// Wrong; the dialog stays open.
    Wrong,
    /// Too many wrong tries; the request is refused and the dialog closes.
    Refused,
}

/// Wrong tries after which a check is refused (the browser may ask again, which opens a new one).
#[cfg(not(windows))]
const MAX_VERIFY_TRIES: u32 = 5;

impl AppHost {
    pub fn new(
        app: AppHandle,
        engine: Engine,
        data_dir: &std::path::Path,
        protector: Arc<dyn LocalProtect>,
    ) -> Self {
        Self {
            app,
            engine,
            clients: ClientFile::load(data_dir.join("broker-clients.bin"), protector),
            approvals: Approvals::default(),
            #[cfg(not(windows))]
            verifying: Mutex::new(HashMap::new()),
        }
    }

    /// Checks a master password typed into the dialog of an open check. Runs Argon2: call off
    /// the UI thread. Only requests opened by `verify_user` count, never a pairing or overwrite
    /// question. The try is counted before the check, so parallel tries can't get extra chances.
    #[cfg(not(windows))]
    pub fn verify_password(
        &self,
        request_id: u64,
        password: &str,
    ) -> Result<VerifyOutcome, String> {
        {
            let mut open = self.verifying.lock().expect("verifying");
            let tries = open.get_mut(&request_id).ok_or("that check is closed")?;
            *tries += 1;
            if *tries > MAX_VERIFY_TRIES {
                drop(open);
                self.approvals.resolve(request_id, false);
                return Ok(VerifyOutcome::Refused);
            }
        }
        let ok = self
            .engine
            .verify_password(password)
            .map_err(|e| e.to_string())?;
        if ok {
            self.approvals.resolve(request_id, true);
            return Ok(VerifyOutcome::Accepted);
        }
        let tries = self
            .verifying
            .lock()
            .expect("verifying")
            .get(&request_id)
            .copied()
            .unwrap_or(MAX_VERIFY_TRIES);
        if tries >= MAX_VERIFY_TRIES {
            self.approvals.resolve(request_id, false);
            Ok(VerifyOutcome::Refused)
        } else {
            Ok(VerifyOutcome::Wrong)
        }
    }

    /// The user's answer from the UI to a pairing, overwrite or "is it you?" question. A master
    /// password check can only be answered yes by `verify_password`, never by this call.
    pub fn respond(&self, request_id: u64, approve: bool) -> bool {
        #[cfg(not(windows))]
        if approve
            && self
                .verifying
                .lock()
                .expect("verifying")
                .contains_key(&request_id)
        {
            return false;
        }
        self.approvals.resolve(request_id, approve)
    }

    pub fn clients(&self) -> &ClientFile {
        &self.clients
    }
}

impl Host for AppHost {
    fn engine(&self) -> &Engine {
        &self.engine
    }

    fn clients(&self) -> &dyn ClientStore {
        &self.clients
    }

    fn policy(&self) -> Policy {
        let settings = self.app.state::<AppState>().settings();
        Policy {
            verify_every_passkey: settings.passkey_verify_every_time,
        }
    }

    fn approve_pairing<'a>(&'a self, prompt: &'a PairingPrompt) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Connecting a browser needs the vault unlocked, so a passer-by can't approve one.
            if self.engine.status() != VaultStatus::Unlocked && !self.unlock().await {
                return false;
            }
            let (request_id, answer) = self.approvals.open();
            // If the client gives up while the dialog is open, the broker drops this future:
            // the guard then closes the dialog.
            let mut open = OpenApproval {
                host: self,
                request_id,
                answered: false,
                withdrawn: |request_id| crate::UiEvent::ExtensionPairingCancelled { request_id },
            };
            let _ = self.app.emit(
                "lp-event",
                crate::UiEvent::ExtensionPairing {
                    request_id,
                    name: prompt.name.clone(),
                    code: prompt.code.clone(),
                    kind: prompt.kind,
                },
            );
            crate::show_main(&self.app);
            let approved = answer.await.unwrap_or(false);
            open.answered = true;
            approved
        })
    }

    fn confirm_overwrite<'a>(&'a self, prompt: &'a OverwritePrompt) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let (request_id, answer) = self.approvals.open();
            // If the client leaves while the dialog is open the broker drops this future, and the
            // guard closes the dialog.
            let mut open = OpenApproval {
                host: self,
                request_id,
                answered: false,
                withdrawn: |request_id| crate::UiEvent::ConfirmOverwriteCancelled { request_id },
            };
            let _ = self.app.emit(
                "lp-event",
                crate::UiEvent::ConfirmOverwrite {
                    request_id,
                    kind: prompt.kind,
                    title: prompt.title.clone(),
                    username: prompt.username.clone(),
                    site: prompt.site.clone(),
                    saved_for: prompt.saved_for.clone(),
                    same_host: prompt.same_host,
                },
            );
            // Same steps as the pairing dialog: the user is in the browser, so the window
            // has to come to the front for them to see the question.
            crate::show_main(&self.app);
            let agreed = answer.await.unwrap_or(false);
            open.answered = true;
            agreed
        })
    }

    fn unlock<'a>(&'a self) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            if self.engine.status() == VaultStatus::Unlocked {
                return true;
            }
            if self.engine.quick_unlock_enabled()
                && crate::commands::quick_unlock(&self.engine, true)
                    .await
                    .is_ok()
            {
                return true;
            }
            // Show the unlock screen and wait for the user to use it.
            crate::show_main(&self.app);
            let deadline = Instant::now() + Duration::from_secs(110);
            while Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if self.engine.status() == VaultStatus::Unlocked {
                    return true;
                }
            }
            false
        })
    }

    fn verify_user<'a>(&'a self, reason: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Windows Hello's key-signing API has no text field for what the check is about,
            // so the reason (which site, already cleaned up by the broker) is only logged.
            info!(reason, "asking for Windows Hello");
            #[cfg(windows)]
            {
                // If the client goes away the broker drops this future, and the guard asks
                // Windows to cancel the pending Hello dialog instead of leaving it open.
                let cancel = Arc::new(crate::hello::CancelHandle::default());
                let _on_drop = CancelOnDrop(cancel.clone());
                tokio::task::spawn_blocking(move || {
                    // The user is in the browser, not in this app: bring up the focus window so
                    // the Hello dialog can come to the front (dropped when the check is over).
                    let why = "check for a browser or Windows";
                    let _focus = crate::focus::Anchor::acquire(why);
                    match crate::hello::verify_user(&cancel, why) {
                        Ok(()) => true,
                        Err(e) => {
                            warn!(error = %e, "user verification failed");
                            false
                        }
                    }
                })
                .await
                .unwrap_or(false)
            }
            #[cfg(not(windows))]
            {
                // No Hello off Windows: the master password is the proof (decision 2026-10-28).
                // The dialog shows the site; a client that leaves takes it down.
                // Registered as a master-password check in the same step as it is opened: `respond` takes the
                // `verifying` lock first, so it can never see the request open but not yet marked.
                let (request_id, answer) = {
                    let mut verifying = self.verifying.lock().expect("verifying");
                    let opened = self.approvals.open();
                    verifying.insert(opened.0, 0);
                    opened
                };
                let mut open = OpenApproval {
                    host: self,
                    request_id,
                    answered: false,
                    withdrawn: |request_id| crate::UiEvent::VerifyUserCancelled { request_id },
                };
                let _ = self.app.emit(
                    "lp-event",
                    crate::UiEvent::VerifyUser {
                        request_id,
                        reason: reason.to_string(),
                    },
                );
                crate::show_main(&self.app);
                // The user has two minutes, like the unlock screen.
                let verified = tokio::time::timeout(Duration::from_secs(110), answer)
                    .await
                    .map(|a| a.unwrap_or(false))
                    .unwrap_or(false);
                open.answered = verified;
                verified
            }
        })
    }
}

/// A dialog (pairing, or "replace what is saved?") that is open in the UI. Dropped unanswered (the
/// client left), it takes the dialog down.
struct OpenApproval<'a> {
    host: &'a AppHost,
    request_id: u64,
    answered: bool,
    /// The event that tells the UI to close the dialog.
    withdrawn: fn(u64) -> crate::UiEvent,
}

impl Drop for OpenApproval<'_> {
    fn drop(&mut self) {
        #[cfg(not(windows))]
        self.host
            .verifying
            .lock()
            .expect("verifying")
            .remove(&self.request_id);
        if !self.answered {
            self.host.approvals.forget(self.request_id);
            let _ = self
                .host
                .app
                .emit("lp-event", (self.withdrawn)(self.request_id));
        }
    }
}

/// Cancels a pending Windows Hello check when dropped (also after it finished: harmless).
#[cfg(windows)]
struct CancelOnDrop(Arc<crate::hello::CancelHandle>);

#[cfg(windows)]
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// What the commands need from the running broker.
pub struct BrokerState {
    pub host: Arc<AppHost>,
    pub broker: Arc<Broker<AppHost>>,
    /// Dropping this stops the pipe listener.
    _stop: tokio::sync::watch::Sender<bool>,
}

/// Starts listening for browsers and OS providers (Windows and Linux).
pub fn start(
    app: &AppHandle,
    engine: Engine,
    data_dir: &std::path::Path,
    protector: Arc<dyn LocalProtect>,
) {
    let host = Arc::new(AppHost::new(app.clone(), engine, data_dir, protector));
    let broker = Broker::new(host.clone());
    let (stop, stopped) = tokio::sync::watch::channel(false);
    #[cfg(any(windows, target_os = "linux"))]
    {
        let broker = broker.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = lp_broker::pipe::serve(broker, stopped).await {
                warn!(error = %e, "the browser connection is unavailable");
            }
        });
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = stopped;
    app.manage(BrokerState {
        host,
        broker,
        _stop: stop,
    });
}

impl BrokerState {
    /// Forgets a paired client and ends its open connections at once: a browser that was
    /// removed must not keep using the session it already had.
    pub fn forget_client(&self, id: &str) -> io::Result<()> {
        self.host.clients().remove(id)?;
        self.broker.revoke(id);
        Ok(())
    }
}

/// A paired client as the settings page shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub id: String,
    pub name: String,
    pub kind: lp_broker::ClientKind,
    pub created_ms: u64,
    pub last_used_ms: u64,
}

impl From<ClientRecord> for ClientInfo {
    fn from(c: ClientRecord) -> Self {
        Self {
            id: c.id,
            name: c.name,
            kind: c.kind,
            created_ms: c.created_ms,
            last_used_ms: c.last_used_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_broker::ClientKind;

    fn record(id: &str) -> ClientRecord {
        ClientRecord {
            id: id.into(),
            name: "Chrome extension".into(),
            kind: ClientKind::Extension,
            secret: vec![7; 32],
            created_ms: 1,
            last_used_ms: 1,
        }
    }

    /// RA-5: pairing after a failed read must not destroy the unreadable file.
    #[test]
    fn an_unreadable_clients_file_is_kept_before_the_next_pairing_replaces_it() {
        let dir = std::env::temp_dir().join(format!("lp-clients-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("clients.bin");
        std::fs::write(&path, b"not a DPAPI blob").unwrap();

        let file = ClientFile::load(path.clone(), lp_engine::platform_protector());
        assert!(file.list().is_empty(), "starts empty, failing closed");
        file.insert(record("c1")).unwrap();
        assert_eq!(
            std::fs::read(path.with_extension("unreadable")).unwrap(),
            b"not a DPAPI blob"
        );
        // The new file reads back, and a second pairing does not copy anything over the kept bytes.
        file.insert(record("c2")).unwrap();
        assert_eq!(
            std::fs::read(path.with_extension("unreadable")).unwrap(),
            b"not a DPAPI blob"
        );
        assert_eq!(
            ClientFile::load(path, lp_engine::platform_protector())
                .list()
                .len(),
            2
        );

        // A missing file is just a fresh start: nothing to keep.
        let fresh = ClientFile::load(dir.join("other.bin"), lp_engine::platform_protector());
        fresh.insert(record("c3")).unwrap();
        assert!(!dir.join("other.unreadable").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
