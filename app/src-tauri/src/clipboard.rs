//! Copying secrets. On Windows the value is excluded from clipboard history
//! and cloud clipboard, and it's cleared after a delay if still present.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use zeroize::Zeroizing;

/// Increments on every copy so an older clear timer doesn't wipe a newer value.
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[cfg(desktop)]
pub fn copy_secret(
    _app: &tauri::AppHandle,
    text: Zeroizing<String>,
    clear_after: Duration,
) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    #[cfg(windows)]
    {
        use arboard::SetExtWindows;
        cb.set()
            .exclude_from_history()
            .exclude_from_cloud()
            .exclude_from_monitoring()
            .text(text.as_str())
            .map_err(|e| e.to_string())?;
    }
    #[cfg(not(windows))]
    cb.set_text(text.as_str()).map_err(|e| e.to_string())?;

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !clear_after.is_zero() {
        std::thread::spawn(move || {
            std::thread::sleep(clear_after);
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            if let Ok(mut cb) = arboard::Clipboard::new() {
                // Only clear if the user hasn't copied something else since.
                let still_ours = cb
                    .get_text()
                    .map(|t| Zeroizing::new(t) == text)
                    .unwrap_or(false);
                if still_ours {
                    let _ = cb.clear();
                }
            }
        });
    }
    Ok(())
}

#[cfg(target_os = "android")]
pub fn copy_secret(
    app: &tauri::AppHandle,
    text: Zeroizing<String>,
    clear_after: Duration,
) -> Result<(), String> {
    use tauri::Manager;
    GENERATION.fetch_add(1, Ordering::SeqCst);
    app.state::<crate::mobile::MobileBridge<tauri::Wry>>()
        .copy_sensitive(&text, clear_after.as_millis() as u64)
}

#[cfg(target_os = "ios")]
pub fn copy_secret(
    _app: &tauri::AppHandle,
    _text: Zeroizing<String>,
    _clear_after: Duration,
) -> Result<(), String> {
    Err("copying is not supported on this platform yet".into())
}
