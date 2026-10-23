//! What the Tauri commands and `lib.rs` use: the real machine behind the flows, one action at a time.

use std::io;
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use lp_broker::{ClientKind, ClientStore};
use lp_engine::Engine;
use tauri::{AppHandle, Manager as _};
use tracing::{debug, info, warn};

use super::Status;
use super::flows::{self, Ctx, Refresh};
use super::model::{BUNDLE_FOLDER, ClientBook, Identity, Layout};
use super::real::{RealRunner, RealSystem};
use super::sync;
use crate::broker::BrokerState;

/// Windows' own page that lists passkey providers (Accounts > Passkeys > Advanced options).
const SETTINGS_PAGE: &str = "ms-settings:passkeys-advancedoptions";
/// The Passkeys page, if the first one is not known to this Windows.
const SETTINGS_FALLBACK: &str = "ms-settings:savedpasskeys";
/// Start-up refresh of the app path waits this long, so it does not compete with the app starting.
const REFRESH_DELAY: Duration = Duration::from_secs(2);

/// The app's list of paired clients (browser extensions and the Windows provider).
struct AppClients(AppHandle);

impl ClientBook for AppClients {
    fn has_provider(&self) -> bool {
        self.0.try_state::<BrokerState>().is_some_and(|broker| {
            broker
                .host
                .clients()
                .list()
                .iter()
                .any(|client| client.kind == ClientKind::Provider)
        })
    }

    fn forget_providers(&self) -> io::Result<usize> {
        let Some(broker) = self.0.try_state::<BrokerState>() else {
            return Ok(0);
        };
        let providers: Vec<String> = broker
            .host
            .clients()
            .list()
            .into_iter()
            .filter(|client| client.kind == ClientKind::Provider)
            .map(|client| client.id)
            .collect();
        for id in &providers {
            broker.forget_client(id)?;
        }
        Ok(providers.len())
    }
}

pub struct Manager {
    app: AppHandle,
    layout: Layout,
    runner: RealRunner,
    system: RealSystem,
    clients: AppClients,
    /// One set-up, pairing or removal at a time.
    busy: Mutex<()>,
    nudge: sync::Nudge,
}

impl Manager {
    fn new(app: &AppHandle, layout: Layout) -> Self {
        Self {
            system: RealSystem {
                subkey: layout.identity.registry_subkey.clone(),
            },
            clients: AppClients(app.clone()),
            app: app.clone(),
            layout,
            runner: RealRunner,
            busy: Mutex::new(()),
            nudge: sync::Nudge::default(),
        }
    }

    pub fn with_ctx<T>(&self, f: impl FnOnce(&Ctx<'_>) -> T) -> T {
        f(&Ctx {
            layout: &self.layout,
            runner: &self.runner,
            system: &self.system,
            clients: &self.clients,
        })
    }

    pub fn status(&self) -> Status {
        self.with_ctx(flows::status)
    }

    fn exclusive<T>(&self, f: impl FnOnce(&Ctx<'_>) -> Result<T, String>) -> Result<T, String> {
        let _guard = match self.busy.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => {
                return Err(
                    "Another Windows passkeys action is still running. Wait for it to finish."
                        .into(),
                );
            }
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
        };
        self.with_ctx(f)
    }

    pub fn setup(&self) -> Result<Status, String> {
        let status = self.exclusive(flows::setup)?;
        self.nudge.now();
        Ok(status)
    }

    pub fn pair(&self) -> Result<Status, String> {
        let status = self.exclusive(flows::pair)?;
        self.nudge.now();
        Ok(status)
    }

    pub fn remove(&self, forget_pairing: bool) -> Result<Status, String> {
        self.exclusive(|ctx| flows::remove(ctx, forget_pairing))
    }

    /// Opens Windows' list of passkey providers, where the user turns LocalPass on.
    pub fn open_settings(&self) -> Result<(), String> {
        use tauri_plugin_opener::OpenerExt;
        let opener = self.app.opener();
        match opener.open_url(SETTINGS_PAGE, None::<&str>) {
            Ok(()) => Ok(()),
            Err(e) => {
                warn!(error = %e, "windows passkeys: the passkey providers page did not open; trying the passkeys page");
                opener
                    .open_url(SETTINGS_FALLBACK, None::<&str>)
                    .map_err(|e| format!("Could not open Windows settings: {e}"))
            }
        }
    }
}

/// Called once from the app's setup: makes the manager available to the commands, refreshes the app
/// path of an existing set-up, and starts following the vault for the credential cache.
pub fn start(app: &AppHandle, engine: Engine) {
    let identity = Identity::current();
    let profile = std::env::var("LOCALPASS_PROFILE")
        .ok()
        .filter(|p| !p.trim().is_empty());
    if profile.is_some() && identity.is_shipped() {
        // A development profile must not change the machine-wide registration.
        debug!("windows passkeys: not offered in a development profile");
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let bundle = app
        .path()
        .resource_dir()
        .unwrap_or_else(|_| exe.parent().map(ToOwned::to_owned).unwrap_or_default())
        .join(BUNDLE_FOLDER);
    let manager = Arc::new(Manager::new(app, Layout::new(identity, bundle, exe)));
    app.manage(manager.clone());

    let refresher = manager.clone();
    std::thread::spawn(move || {
        std::thread::sleep(REFRESH_DELAY);
        match refresher.with_ctx(flows::refresh_app_path) {
            Ok(Refresh::Updated) => {
                info!("windows passkeys: updated the app path for the passkey provider")
            }
            Ok(Refresh::Unchanged | Refresh::NotOurs) => {}
            Err(why) => warn!(%why, "windows passkeys: could not refresh the app path"),
        }
    });
    sync::spawn(manager.clone(), engine, manager.nudge.clone());
}
