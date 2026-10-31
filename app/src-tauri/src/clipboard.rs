//! Copying secrets. On Windows the value is excluded from clipboard history
//! and cloud clipboard; on Linux it carries the password-manager hint that clipboard managers
//! (KDE Klipper and others) honour. It's cleared after a delay if still present.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use zeroize::Zeroizing;

/// Increments on every copy so an older clear timer doesn't wipe a newer value.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// On X11 (and XWayland, which is what GNOME offers) the program that copied has to keep serving
/// the value for as long as it should be pasteable, so the one `Clipboard` of the process lives
/// as long as the process.
#[cfg(target_os = "linux")]
static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);

#[cfg(desktop)]
fn with_clipboard<T>(
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, String>,
) -> Result<T, String> {
    #[cfg(target_os = "linux")]
    {
        let mut slot = CLIPBOARD.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        f(slot.as_mut().expect("just created"))
    }
    #[cfg(not(target_os = "linux"))]
    f(&mut arboard::Clipboard::new().map_err(|e| e.to_string())?)
}

#[cfg(desktop)]
pub fn copy_secret(
    _app: &tauri::AppHandle,
    text: Zeroizing<String>,
    clear_after: Duration,
) -> Result<(), String> {
    with_clipboard(|cb| {
        #[cfg(windows)]
        {
            use arboard::SetExtWindows;
            cb.set()
                .exclude_from_history()
                .exclude_from_cloud()
                .exclude_from_monitoring()
                .text(text.as_str())
                .map_err(|e| e.to_string())
        }
        #[cfg(target_os = "linux")]
        {
            use arboard::SetExtLinux;
            cb.set()
                .exclude_from_history()
                .text(text.as_str())
                .map_err(|e| e.to_string())
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        cb.set_text(text.as_str()).map_err(|e| e.to_string())
    })?;

    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if !clear_after.is_zero() {
        std::thread::spawn(move || {
            std::thread::sleep(clear_after);
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }
            let _ = with_clipboard(|cb| {
                // Only clear if the user hasn't copied something else since.
                let still_ours = cb
                    .get_text()
                    .map(|t| Zeroizing::new(t) == text)
                    .unwrap_or(false);
                if still_ours {
                    let _ = cb.clear();
                }
                Ok(())
            });
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
