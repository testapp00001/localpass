//! Commands invoked by the UI. Secrets are copied from Rust; list views never
//! receive passwords. Argon2-heavy calls run on blocking threads.

use std::time::Duration;

use lp_core::passgen::PasswordOptions;
use lp_engine::{
    DeviceInfo, Engine, FolderView, ImportReport, InviteInfo, ItemInput, ItemQuery, ItemSummary,
    ItemView, SyncStatus, TotpCode, VaultStatus,
};
use serde::Serialize;
use tauri::{AppHandle, State};
use zeroize::Zeroizing;

use crate::{AppState, Settings, clipboard, settings};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> CmdResult<T> + Send + 'static,
) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickUnlockInfo {
    /// "windowsHello", "biometric" or "none".
    kind: &'static str,
    available: bool,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    status: VaultStatus,
    device_name: Option<String>,
    device_id: Option<String>,
    suggested_device_name: String,
    platform: &'static str,
    version: &'static str,
    quick_unlock: QuickUnlockInfo,
    settings: Settings,
}

fn quick_unlock_info(app: &AppHandle, engine: &Engine) -> QuickUnlockInfo {
    let enabled = engine.quick_unlock_enabled();
    #[cfg(windows)]
    {
        let _ = app;
        QuickUnlockInfo {
            kind: "windowsHello",
            available: crate::hello::available(),
            enabled,
        }
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let available = app
            .state::<crate::mobile::MobileBridge<tauri::Wry>>()
            .biometric_available();
        QuickUnlockInfo {
            kind: "biometric",
            available,
            enabled,
        }
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = app;
        QuickUnlockInfo {
            kind: "none",
            available: false,
            enabled,
        }
    }
}

#[cfg(target_os = "android")]
fn biometric_blob_path(state: &AppState) -> std::path::PathBuf {
    state.data_dir.join("biometric.blob")
}

fn suggested_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .map(|n| {
            let lower = n.to_lowercase();
            let mut c = lower.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_default()
        })
        .unwrap_or_else(|_| "My device".into())
}

/// Settings with `start_at_login` reflecting the real OS autostart state.
fn effective_settings(app: &AppHandle, state: &AppState) -> Settings {
    #[allow(unused_mut)]
    let mut s = state.settings();
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        if let Ok(enabled) = app.autolaunch().is_enabled() {
            s.start_at_login = enabled;
        }
    }
    #[cfg(mobile)]
    let _ = app;
    s
}

#[tauri::command]
pub async fn app_info(app: AppHandle, state: State<'_, AppState>) -> CmdResult<AppInfo> {
    let engine = state.engine.clone();
    let handle = app.clone();
    let quick_unlock = blocking(move || Ok(quick_unlock_info(&handle, &engine))).await?;
    Ok(AppInfo {
        status: state.engine.status(),
        device_name: state.engine.device_name(),
        device_id: state.engine.device_id(),
        suggested_device_name: suggested_name(),
        platform: if cfg!(target_os = "android") {
            "android"
        } else {
            std::env::consts::OS
        },
        version: env!("CARGO_PKG_VERSION"),
        quick_unlock,
        settings: effective_settings(&app, &state),
    })
}

/// Called by the UI on user activity; resets the auto-lock timer.
#[tauri::command]
pub fn touch(state: State<'_, AppState>) {
    state.touch();
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Strength {
    /// 0 (too guessable) … 4 (very unguessable).
    score: u8,
    guesses_log10: f64,
    crack_time: String,
    warning: Option<String>,
    suggestions: Vec<String>,
}

#[tauri::command]
pub fn password_strength(password: String) -> Strength {
    let password = Zeroizing::new(password);
    let e = zxcvbn::zxcvbn(&password, &[]);
    Strength {
        score: e.score() as u8,
        guesses_log10: e.guesses_log10(),
        crack_time: e
            .crack_times()
            .offline_slow_hashing_1e4_per_second()
            .to_string(),
        warning: e
            .feedback()
            .and_then(|f| f.warning())
            .map(|w| w.to_string()),
        suggestions: e
            .feedback()
            .map(|f| f.suggestions().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
    }
}

#[tauri::command]
pub async fn create_vault(
    state: State<'_, AppState>,
    password: String,
    device_name: String,
) -> CmdResult<()> {
    let engine = state.engine.clone();
    let password = Zeroizing::new(password);
    blocking(move || {
        engine
            .create_vault(&password, device_name.trim())
            .map_err(err)
    })
    .await?;
    state.touch();
    state.engine.start_sync().await.map_err(err)
}

#[tauri::command]
pub async fn join_vault(
    state: State<'_, AppState>,
    code: String,
    addrs: Vec<String>,
    password: String,
    device_name: String,
) -> CmdResult<()> {
    let password = Zeroizing::new(password);
    state
        .engine
        .join(&code, &addrs, &password, device_name.trim())
        .await
        .map_err(err)?;
    state.touch();
    Ok(())
}

#[tauri::command]
pub async fn unlock(state: State<'_, AppState>, password: String) -> CmdResult<()> {
    let engine = state.engine.clone();
    let password = Zeroizing::new(password);
    blocking(move || engine.unlock(&password).map_err(err)).await?;
    state.touch();
    // Sync may not have started yet if the device secret needed repair.
    if !state.engine.sync_running() {
        let _ = state.engine.start_sync().await;
    }
    Ok(())
}

/// Unlocks with Windows Hello. Also used when a browser or Windows asks for something while
/// the vault is locked (`for_client`): the user is then in another program, so a focus window
/// is brought up first to let the Hello dialog come to the front (see `focus.rs`).
#[cfg(windows)]
pub(crate) async fn quick_unlock(engine: &Engine, for_client: bool) -> CmdResult<()> {
    let engine = engine.clone();
    let id = engine.device_id().ok_or("no vault")?;
    blocking(move || {
        let why = if for_client {
            "unlock for a browser or Windows"
        } else {
            "unlock screen"
        };
        let _focus = for_client
            .then(|| crate::focus::Anchor::acquire(why))
            .flatten();
        let key = crate::hello::derive_key(&id, false, why)?;
        engine.unlock_quick(&key).map_err(err)
    })
    .await
}

#[cfg(not(windows))]
pub(crate) async fn quick_unlock(_engine: &Engine, _for_client: bool) -> CmdResult<()> {
    Err("quick unlock is not available here".into())
}

/// `auto`: the unlock screen offers Windows Hello by itself when it appears. That is refused
/// (`"hidden"`) while the window is not on screen: the app is then running in the tray for a
/// browser or Windows, whose request owns the prompt, and a second Hello would appear.
#[tauri::command]
pub async fn unlock_quick(
    app: AppHandle,
    state: State<'_, AppState>,
    auto: Option<bool>,
) -> CmdResult<()> {
    #[cfg(desktop)]
    if auto == Some(true) && !crate::main_window_showing(&app) {
        tracing::info!(
            target: "lp_prompt",
            "unlock screen: not offering Windows Hello by itself, the window is hidden"
        );
        return Err("hidden".into());
    }
    let _ = auto;
    let engine = state.engine.clone();
    #[cfg(windows)]
    {
        let _ = &app;
        quick_unlock(&engine, false).await?;
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let blob = std::fs::read_to_string(biometric_blob_path(&state))
            .map_err(|_| "biometric unlock is not set up")?;
        blocking(move || {
            let secret = app
                .state::<crate::mobile::MobileBridge<tauri::Wry>>()
                .biometric_decrypt(blob)?;
            let bytes = Zeroizing::new(
                data_encoding::BASE64
                    .decode(secret.as_bytes())
                    .map_err(err)?,
            );
            let key = lp_core::crypto::Key32::from_slice(&bytes).map_err(err)?;
            engine.unlock_quick(&key).map_err(err)
        })
        .await?;
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (app, engine);
        return Err("quick unlock is not available on this device".into());
    }
    #[allow(unreachable_code)]
    {
        state.touch();
        Ok(())
    }
}

#[tauri::command]
pub fn lock(state: State<'_, AppState>) {
    state.engine.lock();
}

#[tauri::command]
pub async fn quick_unlock_enable(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let engine = state.engine.clone();
    #[cfg(windows)]
    {
        let _ = &app;
        let id = engine.device_id().ok_or("no vault")?;
        blocking(move || {
            let key = crate::hello::derive_key(&id, true, "turn on quick unlock")?;
            engine.quick_unlock_enable(&key).map_err(err)
        })
        .await
    }
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        let path = biometric_blob_path(&state);
        blocking(move || {
            let key = lp_core::crypto::Key32::random().map_err(err)?;
            let b64 = Zeroizing::new(data_encoding::BASE64.encode(key.as_bytes()));
            let blob = app
                .state::<crate::mobile::MobileBridge<tauri::Wry>>()
                .biometric_enroll(b64.to_string())?;
            std::fs::write(&path, blob).map_err(err)?;
            engine.quick_unlock_enable(&key).map_err(err)
        })
        .await
    }
    #[cfg(not(any(windows, target_os = "android")))]
    {
        let _ = (app, engine);
        Err("quick unlock is not available on this device".into())
    }
}

#[tauri::command]
pub fn quick_unlock_disable(state: State<'_, AppState>) -> CmdResult<()> {
    state.engine.quick_unlock_disable().map_err(err)?;
    #[cfg(windows)]
    if let Some(id) = state.engine.device_id() {
        crate::hello::delete(&id);
    }
    #[cfg(target_os = "android")]
    let _ = std::fs::remove_file(biometric_blob_path(&state));
    Ok(())
}

#[tauri::command]
pub async fn change_password(
    state: State<'_, AppState>,
    old: String,
    new: String,
) -> CmdResult<()> {
    let engine = state.engine.clone();
    let (old, new) = (Zeroizing::new(old), Zeroizing::new(new));
    blocking(move || engine.change_master_password(&old, &new).map_err(err)).await
}

#[tauri::command]
pub fn list_items(state: State<'_, AppState>, query: ItemQuery) -> CmdResult<Vec<ItemSummary>> {
    state.engine.list_items(&query).map_err(err)
}

#[tauri::command]
pub fn get_item(state: State<'_, AppState>, id: String) -> CmdResult<ItemView> {
    state.touch();
    state.engine.get_item(&id).map_err(err)
}

#[tauri::command]
pub fn save_item(state: State<'_, AppState>, input: ItemInput) -> CmdResult<String> {
    state.touch();
    state.engine.save_item(&input).map_err(err)
}

#[tauri::command]
pub fn trash_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.engine.trash_item(&id).map_err(err)
}

#[tauri::command]
pub fn restore_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.engine.restore_item(&id).map_err(err)
}

#[tauri::command]
pub fn purge_item(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.engine.purge_item(&id).map_err(err)
}

#[tauri::command]
pub fn empty_trash(state: State<'_, AppState>) -> CmdResult<usize> {
    state.engine.empty_trash().map_err(err)
}

#[tauri::command]
pub fn set_favorite(state: State<'_, AppState>, id: String, favorite: bool) -> CmdResult<()> {
    state.engine.set_favorite(&id, favorite).map_err(err)
}

#[tauri::command]
pub fn resolve_conflict(
    state: State<'_, AppState>,
    id: String,
    field: String,
    op: String,
) -> CmdResult<()> {
    state.engine.resolve_conflict(&id, &field, &op).map_err(err)
}

/// Copies a field's value (for `totp`, the current code). Returns the clear delay.
/// Runs off the main thread because on Android it waits for the Kotlin plugin.
#[tauri::command(async)]
pub fn copy_field(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    field: String,
) -> CmdResult<u32> {
    state.touch();
    let secs = state.settings().clipboard_clear_seconds;
    let value = state.engine.field_secret(&id, &field).map_err(err)?;
    clipboard::copy_secret(&app, value, Duration::from_secs(u64::from(secs)))?;
    Ok(secs)
}

/// Copies a generated password.
#[tauri::command(async)]
pub fn copy_text(app: AppHandle, state: State<'_, AppState>, text: String) -> CmdResult<u32> {
    let secs = state.settings().clipboard_clear_seconds;
    clipboard::copy_secret(
        &app,
        Zeroizing::new(text),
        Duration::from_secs(u64::from(secs)),
    )?;
    Ok(secs)
}

#[tauri::command]
pub fn totp_code(state: State<'_, AppState>, id: String) -> CmdResult<TotpCode> {
    state.engine.totp_code(&id).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Generated {
    password: String,
    entropy_bits: f64,
}

#[tauri::command]
pub fn generate_password(options: PasswordOptions) -> CmdResult<Generated> {
    let password = Engine::generate_password(&options).map_err(err)?;
    Ok(Generated {
        password: password.to_string(),
        entropy_bits: options.entropy_bits(),
    })
}

#[tauri::command]
pub fn folders(state: State<'_, AppState>) -> CmdResult<Vec<FolderView>> {
    state.engine.folders().map_err(err)
}

#[tauri::command]
pub fn save_folder(
    state: State<'_, AppState>,
    id: Option<String>,
    name: String,
) -> CmdResult<String> {
    state.engine.save_folder(id.as_deref(), &name).map_err(err)
}

#[tauri::command]
pub fn delete_folder(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.engine.delete_folder(&id).map_err(err)
}

#[tauri::command]
pub fn devices(state: State<'_, AppState>) -> CmdResult<Vec<DeviceInfo>> {
    state.engine.devices().map_err(err)
}

#[tauri::command]
pub fn rename_device(state: State<'_, AppState>, name: String) -> CmdResult<()> {
    state.engine.rename_this_device(&name).map_err(err)
}

#[tauri::command]
pub fn revoke_device(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.engine.revoke(&id).map_err(err)
}

#[tauri::command]
pub fn invite_start(state: State<'_, AppState>) -> CmdResult<InviteInfo> {
    state.touch();
    state.engine.invite_start().map_err(err)
}

#[tauri::command]
pub fn invite_cancel(state: State<'_, AppState>) {
    state.engine.invite_cancel();
}

#[tauri::command]
pub fn invite_respond(state: State<'_, AppState>, request_id: u64, approve: bool) -> CmdResult<()> {
    state
        .engine
        .invite_respond(request_id, approve)
        .map_err(err)
}

#[tauri::command]
pub fn sync_status(state: State<'_, AppState>) -> SyncStatus {
    state.engine.sync_status()
}

#[tauri::command]
pub async fn sync_now(state: State<'_, AppState>) -> CmdResult<usize> {
    let results = state.engine.sync_now().await.map_err(err)?;
    Ok(results.iter().filter(|r| r.result.is_ok()).count())
}

#[tauri::command]
pub async fn add_peer(state: State<'_, AppState>, addr: String) -> CmdResult<()> {
    state.engine.add_manual_peer(&addr, false).map_err(err)?;
    let target = lp_engine::devices::parse_addr(&addr).ok_or("expected an IP address")?;
    state.engine.sync_with(target, false).await.map_err(err)
}

#[tauri::command]
pub async fn import_file(
    state: State<'_, AppState>,
    path: String,
    password: Option<String>,
    keyfile: Option<String>,
) -> CmdResult<ImportReport> {
    let engine = state.engine.clone();
    let password = password.map(Zeroizing::new);
    blocking(move || {
        let bytes = Zeroizing::new(std::fs::read(&path).map_err(err)?);
        let keyfile = keyfile.map(std::fs::read).transpose().map_err(err)?;
        engine
            .import_file(
                &bytes,
                password.as_ref().map(|p| p.as_str()),
                keyfile.as_deref(),
                true,
            )
            .map_err(err)
    })
    .await
}

#[tauri::command]
pub async fn export_backup(
    state: State<'_, AppState>,
    path: String,
    password: String,
) -> CmdResult<()> {
    let engine = state.engine.clone();
    let password = Zeroizing::new(password);
    blocking(move || {
        let bytes = engine.export_backup(&password).map_err(err)?;
        std::fs::write(&path, bytes).map_err(err)
    })
    .await
}

#[tauri::command]
pub fn export_json(state: State<'_, AppState>, path: String) -> CmdResult<usize> {
    let json = Zeroizing::new(state.engine.export_json().map_err(err)?);
    std::fs::write(&path, json.as_bytes()).map_err(err)?;
    // Passkeys stay out of a plain file; the screen tells the user how many.
    state
        .engine
        .passkeys_left_out_of_plain_export()
        .map_err(err)
}

#[tauri::command]
pub fn get_settings(app: AppHandle, state: State<'_, AppState>) -> Settings {
    effective_settings(&app, &state)
}

#[tauri::command]
pub fn save_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> CmdResult<()> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        let enabled = autolaunch.is_enabled().unwrap_or(false);
        if settings.start_at_login && !enabled {
            autolaunch.enable().map_err(err)?;
        } else if !settings.start_at_login && enabled {
            autolaunch.disable().map_err(err)?;
        }
    }
    #[cfg(mobile)]
    let _ = app;
    settings::save(&state.data_dir, &settings).map_err(err)?;
    #[cfg(target_os = "android")]
    crate::android::set_lock_when_away(settings.auto_lock_minutes != 0);
    *state.settings.lock().expect("settings") = settings;
    Ok(())
}

#[tauri::command]
pub fn leave_vault(state: State<'_, AppState>) -> CmdResult<()> {
    #[cfg(windows)]
    if let Some(id) = state.engine.device_id() {
        crate::hello::delete(&id);
    }
    state.engine.leave_vault().map_err(err)
}

// ---- browser extension and passkey provider connection ----

/// Where the built extension is: next to the installed app, or in the repository when developing.
fn extension_folder(app: &AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    let has_manifest = |dir: &std::path::Path| dir.join("manifest.json").is_file();
    if let Ok(resources) = app.path().resource_dir() {
        let dir = resources.join("extension");
        if has_manifest(&dir) {
            return Some(dir);
        }
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .map(|a| a.join("extension").join("dist"))
        .find(|dir| has_manifest(dir))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    /// The connection to browsers exists on Windows only.
    available: bool,
    extension_id: &'static str,
    extension_folder: Option<String>,
    /// Chrome and Edge find the helper (the registration points at this copy of the app).
    chrome: bool,
    edge: bool,
    clients: Vec<crate::broker::ClientInfo>,
}

fn browser_status_of(app: &AppHandle, state: &AppState) -> BrowserStatus {
    use tauri::Manager;
    let clients = app
        .try_state::<crate::broker::BrokerState>()
        .map(|b| {
            use lp_broker::ClientStore;
            let mut list: Vec<_> = b
                .host
                .clients()
                .list()
                .into_iter()
                .map(Into::into)
                .collect();
            list.sort_by_key(|c: &crate::broker::ClientInfo| std::cmp::Reverse(c.last_used_ms));
            list
        })
        .unwrap_or_default();
    #[cfg(windows)]
    let (available, chrome, edge) = {
        let exe = std::env::current_exe().unwrap_or_default();
        let registered = |name: &str| {
            crate::native_messaging::BROWSERS
                .iter()
                .find(|(n, _)| *n == name)
                .is_some_and(|(_, key)| {
                    crate::native_messaging::is_registered(key, &state.data_dir, &exe)
                })
        };
        (true, registered("Chrome"), registered("Edge"))
    };
    #[cfg(not(windows))]
    let (available, chrome, edge) = {
        let _ = state;
        (false, false, false)
    };
    BrowserStatus {
        available,
        extension_id: lp_broker::nmhost::EXTENSION_ID,
        extension_folder: extension_folder(app).map(|p| p.display().to_string()),
        chrome,
        edge,
        clients,
    }
}

#[tauri::command]
pub fn browser_status(app: AppHandle, state: State<'_, AppState>) -> BrowserStatus {
    browser_status_of(&app, &state)
}

/// Lets Chrome and Edge start LocalPass as the extension's helper.
#[tauri::command]
pub fn browser_install(app: AppHandle, state: State<'_, AppState>) -> CmdResult<BrowserStatus> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe().map_err(err)?;
        crate::native_messaging::install(&state.data_dir, &exe).map_err(err)?;
        Ok(browser_status_of(&app, &state))
    }
    #[cfg(not(windows))]
    {
        let _ = (&app, &state);
        Err("the browser connection is only available on Windows".into())
    }
}

#[tauri::command]
pub fn browser_remove(app: AppHandle, state: State<'_, AppState>) -> CmdResult<BrowserStatus> {
    #[cfg(windows)]
    crate::native_messaging::remove(&state.data_dir).map_err(err)?;
    Ok(browser_status_of(&app, &state))
}

/// Forgets a paired browser or provider and ends its open connections; it has to be approved
/// again to reconnect.
#[tauri::command]
pub fn browser_client_remove(app: AppHandle, id: String) -> CmdResult<()> {
    use tauri::Manager;
    let broker = app
        .try_state::<crate::broker::BrokerState>()
        .ok_or("the browser connection is not running")?;
    broker.forget_client(&id).map_err(err)
}

/// The user's answer to "a browser wants to connect" and to "replace what is saved?".
#[tauri::command]
pub fn broker_respond(app: AppHandle, request_id: u64, approve: bool) {
    use tauri::Manager;
    if let Some(broker) = app.try_state::<crate::broker::BrokerState>() {
        broker.host.approvals().resolve(request_id, approve);
    }
}

/// Opens the folder with the built extension in Explorer, for "Load unpacked".
#[tauri::command]
pub fn open_extension_folder(app: AppHandle) -> CmdResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let folder = extension_folder(&app).ok_or(
        "the extension has not been built yet (run `bun run build` in the extension folder)",
    )?;
    app.opener()
        .open_path(folder.to_string_lossy(), None::<&str>)
        .map_err(err)
}

// ---- Windows passkey provider (Settings > Windows passkeys) ----

pub use crate::windows_passkeys::Status as WindowsPasskeysStatus;

/// The manager exists on Windows (not in a development profile); elsewhere `None`.
#[cfg(windows)]
fn passkey_manager(app: &AppHandle) -> Option<std::sync::Arc<crate::windows_passkeys::Manager>> {
    use tauri::Manager;
    app.try_state::<std::sync::Arc<crate::windows_passkeys::Manager>>()
        .map(|state| state.inner().clone())
}

/// What the set-up looks like. `available` is false where the section should not show.
#[tauri::command]
pub async fn windows_passkeys_status(app: AppHandle) -> WindowsPasskeysStatus {
    #[cfg(windows)]
    if let Some(manager) = passkey_manager(&app) {
        return tauri::async_runtime::spawn_blocking(move || manager.status())
            .await
            .unwrap_or_default();
    }
    let _ = app;
    WindowsPasskeysStatus::default()
}

/// Registers the passkey provider with Windows (switched off until the user turns it on there).
#[tauri::command]
pub async fn windows_passkeys_setup(app: AppHandle) -> CmdResult<WindowsPasskeysStatus> {
    #[cfg(windows)]
    {
        let manager = passkey_manager(&app).ok_or(WINDOWS_ONLY)?;
        blocking(move || manager.setup()).await
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(WINDOWS_ONLY.into())
    }
}

/// Pairs the provider with this app (the usual approval dialog appears).
#[tauri::command]
pub async fn windows_passkeys_pair(app: AppHandle) -> CmdResult<WindowsPasskeysStatus> {
    #[cfg(windows)]
    {
        let manager = passkey_manager(&app).ok_or(WINDOWS_ONLY)?;
        blocking(move || manager.pair()).await
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(WINDOWS_ONLY.into())
    }
}

/// Takes LocalPass out of Windows' passkey list and removes the provider package.
#[tauri::command]
pub async fn windows_passkeys_remove(
    app: AppHandle,
    forget_pairing: bool,
) -> CmdResult<WindowsPasskeysStatus> {
    #[cfg(windows)]
    {
        let manager = passkey_manager(&app).ok_or(WINDOWS_ONLY)?;
        blocking(move || manager.remove(forget_pairing)).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, forget_pairing);
        Err(WINDOWS_ONLY.into())
    }
}

/// Opens Windows' page where passkey providers are turned on.
#[tauri::command]
pub fn windows_passkeys_open_settings(app: AppHandle) -> CmdResult<()> {
    #[cfg(windows)]
    {
        passkey_manager(&app).ok_or(WINDOWS_ONLY)?.open_settings()
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err(WINDOWS_ONLY.into())
    }
}

const WINDOWS_ONLY: &str = "Windows passkeys are only available in the Windows app";

// ---- Android: autofill and passkey provider set-up ----

/// Where the phone-level set-up stands. Each of `autofill`, `credential_provider`, `screen_lock`
/// and `local_network` is "unsupported", "on", "off" or "unknown" (Android does not tell apps).
/// `screen_lock` is "off" when the phone has no screen lock, which passkeys need.
#[derive(Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AndroidSetup {
    pub sdk: u32,
    pub autofill: String,
    pub credential_provider: String,
    pub screen_lock: String,
    pub local_network: String,
}

/// Reads the set-up state from Android. Only on Android; elsewhere everything is "unsupported".
#[tauri::command]
pub async fn android_setup_status(app: AppHandle) -> CmdResult<AndroidSetup> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        // The plugin call waits for Kotlin, so it must not run on the main thread.
        blocking(move || {
            app.state::<crate::mobile::MobileBridge<tauri::Wry>>()
                .setup_status()
        })
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        let unsupported = || "unsupported".to_string();
        Ok(AndroidSetup {
            sdk: 0,
            autofill: unsupported(),
            credential_provider: unsupported(),
            screen_lock: unsupported(),
            local_network: unsupported(),
        })
    }
}

/// Opens an Android settings page for one set-up step: "autofill", "credentials" or "app".
#[tauri::command]
pub async fn android_open_setup(app: AppHandle, target: String) -> CmdResult<()> {
    #[cfg(target_os = "android")]
    {
        use tauri::Manager;
        blocking(move || {
            app.state::<crate::mobile::MobileBridge<tauri::Wry>>()
                .open_setup(&target)
        })
        .await
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, target);
        Err("this is only available on Android".into())
    }
}
