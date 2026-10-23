//! JNI entry points called from Kotlin (gen/android/.../NativeBridge.kt).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use jni::JNIEnv;
use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jlong, jstring};
use lp_core::crypto::Key32;
use lp_engine::{Engine, EngineConfig, ProvidedSecret};

/// Data directory and device secret handed over by `MainActivity` before Tauri starts.
static BOOT: OnceLock<(PathBuf, [u8; 32])> = OnceLock::new();
/// The UI's engine, so a background sync in the same process reuses it.
static ENGINE: OnceLock<Engine> = OnceLock::new();
/// Phones lock this long after LocalPass leaves the screen.
pub const BACKGROUND_LOCK: Duration = Duration::from_secs(60);

/// False when the user chose "never lock automatically" in Settings.
static LOCK_WHEN_AWAY: AtomicBool = AtomicBool::new(true);

/// True once the Tauri UI is running in this process (it has its own auto-lock loop).
static UI_STARTED: AtomicBool = AtomicBool::new(false);

/// How long a vault unlocked for a credential service stays unlocked when the app isn't open.
const PROVIDER_LOCK_AFTER: Duration = Duration::from_secs(60);

/// When the credential services last used the vault, and whether a thread is waiting to lock it.
static PROVIDER_USED: Mutex<Option<SystemTime>> = Mutex::new(None);
static PROVIDER_WATCHER: AtomicBool = AtomicBool::new(false);

pub fn set_lock_when_away(enabled: bool) {
    LOCK_WHEN_AWAY.store(enabled, Ordering::Relaxed);
}

/// When the app left the screen. This is a wall-clock time because the monotonic clock
/// stops while the phone sleeps, which would let a long-idle app stay unlocked.
static BACKGROUNDED: Mutex<Option<SystemTime>> = Mutex::new(None);

pub fn boot() -> Option<(PathBuf, Key32)> {
    BOOT.get()
        .map(|(dir, secret)| (dir.clone(), Key32::from_bytes(*secret)))
}

/// The process's one engine. The UI, the background sync worker and the credential services
/// can each be the first to need it (Android starts the process for whichever comes first), so
/// the first caller opens it and the rest share it. Two engines over one database would not see
/// each other's changes.
pub fn open_engine(dir: &std::path::Path, secret: Key32) -> Result<Engine, String> {
    static OPENING: Mutex<()> = Mutex::new(());
    if let Some(engine) = ENGINE.get() {
        return Ok(engine.clone());
    }
    let _guard = OPENING.lock().map_err(|_| "engine lock poisoned")?;
    if let Some(engine) = ENGINE.get() {
        return Ok(engine.clone());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let engine = Engine::open(
        EngineConfig::new(dir, "android"),
        Arc::new(ProvidedSecret(secret)),
    )
    .map_err(|e| e.to_string())?;
    let _ = ENGINE.set(engine.clone());
    Ok(engine)
}

/// Called when the Tauri UI starts: from then on the UI's own loop decides when to lock.
pub fn mark_ui_started() {
    UI_STARTED.store(true, Ordering::Relaxed);
}

/// How long the app has been in the background, if it is. A clock that went backwards
/// counts as "a long time" so the vault locks.
pub fn background_for() -> Option<Duration> {
    let since = (*BACKGROUNDED.lock().ok()?)?;
    Some(
        SystemTime::now()
            .duration_since(since)
            .unwrap_or(Duration::MAX),
    )
}

fn read_inputs(
    env: &mut JNIEnv<'_>,
    dir: &JString<'_>,
    secret: &JByteArray<'_>,
) -> Option<(PathBuf, [u8; 32])> {
    let dir: String = env.get_string(dir).ok()?.into();
    let bytes = env.convert_byte_array(secret).ok()?;
    let secret: [u8; 32] = bytes.as_slice().try_into().ok()?;
    Some((PathBuf::from(dir), secret))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_localpass_NativeBridge_init(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    secret: JByteArray<'_>,
) {
    if let Some(inputs) = read_inputs(&mut env, &data_dir, &secret) {
        let _ = BOOT.set(inputs);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_localpass_NativeBridge_onAppBackground(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    if let Ok(mut b) = BACKGROUNDED.lock() {
        *b = Some(SystemTime::now());
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_localpass_NativeBridge_onAppForeground(
    _env: JNIEnv<'_>,
    _class: JClass<'_>,
) {
    // The process may have been frozen while away, so the periodic check can't be relied
    // on to have run: decide here, before the vault is shown again.
    let away = background_for();
    if let Ok(mut b) = BACKGROUNDED.lock() {
        *b = None;
    }
    if LOCK_WHEN_AWAY.load(Ordering::Relaxed)
        && away.is_some_and(|d| d >= BACKGROUND_LOCK)
        && let Some(engine) = ENGINE.get()
    {
        engine.lock();
    }
}

/// One background round. Reuses the UI's engine if this process has one,
/// otherwise opens the vault (locked) just for this sync.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_localpass_NativeBridge_runSyncOnce(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    secret: JByteArray<'_>,
    timeout_ms: jlong,
) -> jstring {
    // A panic must not unwind into the JVM (that would take the whole app down).
    let summary = catch_unwind(AssertUnwindSafe(|| {
        match read_inputs(&mut env, &data_dir, &secret) {
            None => "bad arguments".to_string(),
            Some((dir, secret)) => run_sync_once(
                dir,
                Key32::from_bytes(secret),
                Duration::from_millis(timeout_ms.max(1000) as u64),
            ),
        }
    }))
    .unwrap_or_else(|_| "internal error".to_string());
    env.new_string(summary)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

fn run_sync_once(dir: PathBuf, secret: Key32, timeout: Duration) -> String {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return format!("no runtime: {e}"),
    };
    runtime.block_on(async move {
        let engine = match open_engine(&dir, secret) {
            Ok(e) => e,
            Err(e) => return format!("could not open vault: {e}"),
        };
        if engine.status() == lp_engine::VaultStatus::NoVault {
            return "no vault yet".to_string();
        }
        match tokio::time::timeout(timeout, engine.sync_once(Duration::from_secs(4))).await {
            Ok(Ok(attempts)) => {
                let ok = attempts.iter().filter(|a| a.result.is_ok()).count();
                format!("synced with {ok} of {} reachable devices", attempts.len())
            }
            Ok(Err(e)) => format!("sync failed: {e}"),
            Err(_) => "timed out".to_string(),
        }
    })
}

/// Locks the vault again a minute after the credential services last used it, unless the app's
/// UI is running (its own loop locks it then). One thread waits at a time.
fn lock_later(engine: &Engine) {
    if let Ok(mut used) = PROVIDER_USED.lock() {
        *used = Some(SystemTime::now());
    }
    if UI_STARTED.load(Ordering::Relaxed) || PROVIDER_WATCHER.swap(true, Ordering::AcqRel) {
        return;
    }
    let engine = engine.clone();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(5));
            if UI_STARTED.load(Ordering::Relaxed) {
                break;
            }
            let idle = PROVIDER_USED.lock().ok().and_then(|used| *used).map(|at| {
                SystemTime::now()
                    .duration_since(at)
                    .unwrap_or(Duration::MAX)
            });
            if idle.is_none_or(|d| d >= PROVIDER_LOCK_AFTER) {
                engine.lock();
                break;
            }
        }
        PROVIDER_WATCHER.store(false, Ordering::Release);
    });
}

fn credential_call(dir: PathBuf, secret: Key32, request: &str) -> String {
    // `biometricBlob` is app data (the Keystore-wrapped quick-unlock key), not engine state.
    if serde_json::from_str::<serde_json::Value>(request).is_ok_and(|v| v["op"] == "biometricBlob")
    {
        let blob = std::fs::read_to_string(dir.join("biometric.blob")).ok();
        return serde_json::json!({ "ok": { "blob": blob } }).to_string();
    }
    let engine = match open_engine(&dir, secret) {
        Ok(e) => e,
        Err(e) => {
            return serde_json::json!({
                "error": { "code": "error", "message": format!("could not open the vault: {e}") }
            })
            .to_string();
        }
    };
    let reply = lp_engine::provider::call(&engine, request);
    lock_later(&engine);
    reply
}

/// One request from the credential services (Credential Manager provider and autofill), as
/// JSON. See `lp_engine::provider` for the requests. Safe to call from any thread.
#[unsafe(no_mangle)]
pub extern "system" fn Java_app_localpass_NativeBridge_credentialCall(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    data_dir: JString<'_>,
    secret: JByteArray<'_>,
    request: JString<'_>,
) -> jstring {
    // A panic must not unwind into the JVM (that would take the whole app down).
    let reply = catch_unwind(AssertUnwindSafe(|| {
        let request: Option<String> = env.get_string(&request).ok().map(Into::into);
        match (read_inputs(&mut env, &data_dir, &secret), request) {
            (Some((dir, secret)), Some(request)) => {
                credential_call(dir, Key32::from_bytes(secret), &request)
            }
            _ => r#"{"error":{"code":"invalid","message":"bad arguments"}}"#.to_string(),
        }
    }))
    .unwrap_or_else(|_| r#"{"error":{"code":"error","message":"internal error"}}"#.to_string());
    env.new_string(reply)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}
