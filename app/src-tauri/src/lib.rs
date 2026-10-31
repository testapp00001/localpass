//! LocalPass app shell: wires the engine into Tauri (commands, events, tray,
//! auto-lock). Sync starts at launch and keeps running while locked.

#[cfg(target_os = "android")]
mod android;
#[cfg(desktop)]
mod applog;
mod broker;
mod clipboard;
mod commands;
#[cfg(windows)]
mod focus;
#[cfg(windows)]
mod hello;
#[cfg(target_os = "android")]
mod mobile;
pub mod native_host;
#[cfg(any(windows, target_os = "linux"))]
mod native_messaging;
mod settings;
mod windows_passkeys;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lp_engine::{Engine, EngineEvent, PairingRequest, VaultStatus};
use lp_sync::service::SyncEvent;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tracing::{info, warn};

pub use settings::Settings;

pub struct AppState {
    pub engine: Engine,
    pub data_dir: PathBuf,
    pub settings: Mutex<Settings>,
    pub last_activity: Mutex<Instant>,
}

impl AppState {
    pub fn touch(&self) {
        *self.last_activity.lock().expect("activity") = Instant::now();
    }

    pub fn settings(&self) -> Settings {
        self.settings.lock().expect("settings").clone()
    }
}

/// Events pushed to the UI on the `lp-event` channel.
#[derive(Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum UiEvent {
    Status {
        status: VaultStatus,
    },
    VaultChanged,
    SyncCompleted {
        peer: String,
        received: usize,
        sent: usize,
        clock_skew_ms: i64,
    },
    SyncFailed {
        addr: String,
        error: String,
    },
    PeerDiscovered {
        device: Option<String>,
    },
    NetworkChanged,
    PairingRequest(PairingRequest),
    PairingFinished {
        device: Option<String>,
        error: Option<String>,
    },
    InviteWrongCode {
        attempts_left: u32,
    },
    ForkDetected,
    /// A browser extension (or the Windows passkey provider) wants to connect.
    ExtensionPairing {
        request_id: u64,
        name: String,
        code: String,
        /// `extension` or `provider`, so the dialog can say who is asking.
        kind: lp_broker::ClientKind,
    },
    /// The client withdrew that request (it disconnected): close the dialog.
    ExtensionPairingCancelled {
        request_id: u64,
    },
    /// A browser or the Windows provider wants to replace something saved (a login's password, a
    /// passkey for the same account). Nothing changes unless the user agrees.
    ConfirmOverwrite {
        request_id: u64,
        kind: lp_broker::OverwriteKind,
        title: String,
        username: String,
        /// The site the request comes from.
        site: String,
        /// For a login: the address it is saved for.
        saved_for: Option<String>,
        /// For a login: the request comes from that very host (otherwise another address of
        /// the same site).
        same_host: bool,
    },
    /// The client withdrew that question: close the dialog.
    ConfirmOverwriteCancelled {
        request_id: u64,
    },
    /// A browser asks the user to prove it is them (Linux: type the master password). `reason`
    /// is the cleaned text of what for, for example "Sign in to example.com". Windows uses Hello instead.
    #[cfg(not(windows))]
    VerifyUser {
        request_id: u64,
        reason: String,
    },
    /// The client withdrew that check (or it ended): close the dialog.
    #[cfg(not(windows))]
    VerifyUserCancelled {
        request_id: u64,
    },
    /// The user opened the window (tray icon or starting the app again). The unlock screen
    /// then offers Windows Hello if it had held back while the window was hidden.
    #[cfg(desktop)]
    WindowShown,
}

fn ui_event(ev: EngineEvent) -> Option<UiEvent> {
    Some(match ev {
        EngineEvent::Status(status) => UiEvent::Status { status },
        EngineEvent::VaultChanged => UiEvent::VaultChanged,
        EngineEvent::PairingRequest(r) => UiEvent::PairingRequest(r),
        EngineEvent::PairingFinished { device, error } => {
            UiEvent::PairingFinished { device, error }
        }
        EngineEvent::Sync(s) => match s {
            SyncEvent::SyncCompleted {
                peer,
                received,
                sent,
                clock_skew_ms,
                ..
            } => UiEvent::SyncCompleted {
                peer: peer.to_hex(),
                received,
                sent,
                clock_skew_ms,
            },
            SyncEvent::SyncFailed { addr, error, .. } => UiEvent::SyncFailed {
                addr: addr.to_string(),
                error,
            },
            SyncEvent::PeerDiscovered { device, .. } => UiEvent::PeerDiscovered {
                device: device.map(|d| d.to_hex()),
            },
            SyncEvent::NetworkChanged { .. } => UiEvent::NetworkChanged,
            SyncEvent::InviteWrongCode { attempts_left } => {
                UiEvent::InviteWrongCode { attempts_left }
            }
            SyncEvent::ForkDetected { .. } => UiEvent::ForkDetected,
            SyncEvent::Listening { .. } | SyncEvent::InviteFinished { .. } => return None,
        },
    })
}

fn data_dir(app: &AppHandle) -> PathBuf {
    #[cfg(target_os = "android")]
    if let Some((dir, _)) = android::boot() {
        return dir;
    }
    let base = app.path().app_data_dir().expect("app data dir");
    match std::env::var("LOCALPASS_PROFILE") {
        Ok(p) if !p.trim().is_empty() => base.join("profiles").join(p.trim()),
        _ => base,
    }
}

/// How files that only this user may read back are protected: DPAPI on Windows, a key in the system
/// keyring on Linux (one entry per `LOCALPASS_PROFILE`), nothing extra elsewhere (Android hands the
/// engine its Keystore secret instead).
fn local_protector(dir: &std::path::Path) -> std::sync::Arc<dyn lp_engine::LocalProtect> {
    #[cfg(target_os = "linux")]
    {
        let profile = std::env::var("LOCALPASS_PROFILE")
            .ok()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty());
        std::sync::Arc::new(lp_engine::KeyedProtector::new(
            lp_engine::SecretServiceKeys::new(profile.as_deref())
                .guarding([dir.join("device.key"), dir.join("broker-clients.bin")]),
        ))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = dir;
        lp_engine::platform_protector()
    }
}

/// The engine's platform label on desktop (Android opens its engine in `android::open_engine`).
#[cfg(not(target_os = "android"))]
fn platform() -> &'static str {
    std::env::consts::OS
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        #[cfg(desktop)]
        let _ = w.unminimize();
        let _ = w.show();
        // Not implemented on Android (it logs a warning).
        #[cfg(desktop)]
        let _ = w.set_focus();
        // Windows refuses `set_focus` while another program has the foreground (a browser asked
        // us for something): the steps in `focus.rs` win it back, and log what happened.
        #[cfg(windows)]
        if let Ok(hwnd) = w.hwnd() {
            focus::raise_when_shown(hwnd.0 as isize, "open the LocalPass window");
        }
    }
}

/// Shows the window because the user asked to (tray icon, starting the app again), and tells
/// the unlock screen, which may have held back its Windows Hello offer while hidden.
#[cfg(desktop)]
fn show_main_for_user(app: &AppHandle) {
    show_main(app);
    let _ = app.emit("lp-event", UiEvent::WindowShown);
}

/// Whether the window is on screen (shown and not minimized).
#[cfg(desktop)]
pub(crate) fn main_window_showing(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false))
}

fn start_background(app: &AppHandle, engine: Engine) {
    // Forward engine events to the UI.
    let handle = app.clone();
    let mut rx = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if let Some(ui) = ui_event(ev) {
                        let _ = handle.emit("lp-event", ui);
                    }
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = handle.emit("lp-event", UiEvent::VaultChanged);
                }
                Err(RecvError::Closed) => break,
            }
        }
    });

    // Sync runs whenever a vault exists, locked or not.
    let sync_engine = engine.clone();
    tauri::async_runtime::spawn(async move {
        if sync_engine.status() != VaultStatus::NoVault
            && let Err(e) = sync_engine.start_sync().await
        {
            warn!(error = %e, "sync did not start");
        }
    });

    // Auto-lock after inactivity.
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let state = handle.state::<AppState>();
            let minutes = state.settings().auto_lock_minutes;
            if minutes == 0 || state.engine.status() != VaultStatus::Unlocked {
                continue;
            }
            let idle = state.last_activity.lock().expect("activity").elapsed();
            if idle >= Duration::from_secs(u64::from(minutes) * 60) {
                info!("auto-locking after inactivity");
                state.engine.lock();
            }
            #[cfg(target_os = "android")]
            if android::background_for().is_some_and(|d| d >= android::BACKGROUND_LOCK) {
                info!("locking: app is in the background");
                state.engine.lock();
            }
        }
    });
}

#[cfg(desktop)]
fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let open = MenuItem::with_id(app, "open", "Open LocalPass", true, None::<&str>)?;
    let lock = MenuItem::with_id(app, "lock", "Lock", true, None::<&str>)?;
    let sync = MenuItem::with_id(app, "sync", "Sync now", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit LocalPass", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &lock, &sync, &sep, &quit])?;
    let icon = app.default_window_icon().cloned().expect("bundled icon");
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("LocalPass")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let state = app.state::<AppState>();
            match event.id.as_ref() {
                "open" => show_main_for_user(app),
                "lock" => state.engine.lock(),
                "sync" => {
                    let engine = state.engine.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = engine.sync_now().await;
                    });
                }
                "quit" => {
                    state.engine.stop_sync();
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_for_user(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let console = tracing_subscriber::fmt::layer()
            .with_target(false)
            // logcat shows escape codes as junk.
            .with_ansi(!cfg!(target_os = "android"))
            .with_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            );
        let registry = tracing_subscriber::registry().with(console);
        // The release app has no console, so the prompt log also goes to a file (see applog.rs).
        #[cfg(desktop)]
        let registry = registry.with(applog::layer());
        registry.init();
    }

    let profile = std::env::var("LOCALPASS_PROFILE")
        .ok()
        .filter(|p| !p.trim().is_empty());
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        // Separate dev profiles may run side by side; the real app is single-instance.
        if profile.is_none() {
            builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
                show_main_for_user(app)
            }));
        }
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ));
    }
    let _ = &profile;
    #[cfg(target_os = "android")]
    {
        builder = builder.plugin(mobile::init());
    }
    #[cfg(mobile)]
    {
        builder = builder.plugin(tauri_plugin_barcode_scanner::init());
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let dir = data_dir(&handle);
            std::fs::create_dir_all(&dir)?;
            // The vault file holds the wrapped keys and the KDF salt: on Linux other local users must not read the folder
            // (Windows keeps it under the user's profile ACL).
            #[cfg(all(unix, not(target_os = "android")))]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
            }
            #[cfg(desktop)]
            applog::open(&dir);
            let protector = local_protector(&dir);
            // On Android the process may already have opened the engine for a credential
            // service or the sync worker; share it.
            #[cfg(target_os = "android")]
            let engine = {
                let (_, key) =
                    android::boot().ok_or("MainActivity did not provide the device secret")?;
                android::mark_ui_started();
                android::open_engine(&dir, key)?
            };
            #[cfg(not(target_os = "android"))]
            let engine = {
                let config = lp_engine::EngineConfig::new(&dir, platform());
                let secrets: std::sync::Arc<dyn lp_engine::DeviceSecretStore> =
                    std::sync::Arc::new(lp_engine::FileSecretStore::with_protector(
                        dir.join("device.key"),
                        protector.clone(),
                    ));
                Engine::open(config, secrets)?
            };
            info!(data_dir = %dir.display(), status = ?engine.status(), "LocalPass starting");
            let loaded = settings::load(&dir);
            #[cfg(target_os = "android")]
            android::set_lock_when_away(loaded.auto_lock_minutes != 0);
            let broker_dir = dir.clone();
            app.manage(AppState {
                engine: engine.clone(),
                settings: Mutex::new(loaded),
                data_dir: dir,
                last_activity: Mutex::new(Instant::now()),
            });
            broker::start(&handle, engine.clone(), &broker_dir, protector);
            #[cfg(windows)]
            windows_passkeys::start(&handle, engine.clone());
            start_background(&handle, engine);
            #[cfg(desktop)]
            setup_tray(app)?;
            if !std::env::args().any(|a| a == "--minimized") {
                #[cfg(desktop)]
                show_main_for_user(&handle);
                #[cfg(not(desktop))]
                show_main(&handle);
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let state = window.state::<AppState>();
                if cfg!(desktop) && state.settings().close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::touch,
            commands::password_strength,
            commands::create_vault,
            commands::join_vault,
            commands::unlock,
            commands::unlock_quick,
            commands::lock,
            commands::quick_unlock_enable,
            commands::quick_unlock_disable,
            commands::change_password,
            commands::list_items,
            commands::get_item,
            commands::save_item,
            commands::trash_item,
            commands::restore_item,
            commands::purge_item,
            commands::empty_trash,
            commands::set_favorite,
            commands::resolve_conflict,
            commands::copy_field,
            commands::copy_text,
            commands::totp_code,
            commands::generate_password,
            commands::folders,
            commands::save_folder,
            commands::delete_folder,
            commands::devices,
            commands::rename_device,
            commands::revoke_device,
            commands::invite_start,
            commands::invite_cancel,
            commands::invite_respond,
            commands::sync_status,
            commands::sync_now,
            commands::add_peer,
            commands::import_file,
            commands::export_backup,
            commands::export_json,
            commands::get_settings,
            commands::save_settings,
            commands::leave_vault,
            commands::browser_status,
            commands::browser_install,
            commands::browser_remove,
            commands::browser_client_remove,
            commands::broker_respond,
            #[cfg(all(desktop, not(windows)))]
            commands::broker_verify,
            commands::open_extension_folder,
            commands::windows_passkeys_status,
            commands::windows_passkeys_setup,
            commands::windows_passkeys_pair,
            commands::windows_passkeys_remove,
            commands::windows_passkeys_open_settings,
            commands::android_setup_status,
            commands::android_open_setup,
        ])
        .build(tauri::generate_context!())
        .expect("error while building LocalPass")
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, code, .. } = event {
                // Keep running in the tray when the last window closes; explicit quit passes a code.
                if code.is_none()
                    && app.state::<AppState>().settings().close_to_tray
                    && cfg!(desktop)
                {
                    api.prevent_exit();
                }
            }
        });
}
