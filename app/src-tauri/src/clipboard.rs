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

/// A picture on the clipboard, as RGBA. The pixels may show a 2FA secret, so they are wiped on drop.
#[cfg(desktop)]
pub struct ClipImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Zeroizing<Vec<u8>>,
}

/// The picture the user copied (a screenshot of a QR code), or a message saying there is none.
#[cfg(desktop)]
pub fn read_image() -> Result<ClipImage, String> {
    with_clipboard(|cb| {
        match cb.get_image() {
        Ok(image) => Ok(ClipImage {
            width: image.width,
            height: image.height,
            rgba: Zeroizing::new(image.bytes.into_owned()),
        }),
        Err(arboard::Error::ContentNotAvailable) => Err(
            "There is no picture on the clipboard. Copy a screenshot of the QR code first (for example PrtSc, Win+Shift+S, or \"Copy\" in your browser's screenshot tool), then paste it here."
                .into(),
        ),
        Err(e) => Err(format!("Could not read a picture from the clipboard: {e}")),
    }
    })
}

/// Empties the clipboard if it still holds exactly this picture, so the secret does not stay in it.
/// Returns whether it did. Something else the user copied in the meantime is left alone.
#[cfg(desktop)]
pub fn clear_if_image(image: &ClipImage) -> bool {
    with_clipboard(|cb| {
        let same = cb.get_image().is_ok_and(|now| {
            now.width == image.width
                && now.height == image.height
                && now.bytes[..] == image.rgba[..]
        });
        if same {
            // An older "clear the copied password" timer must not clear something newer than this.
            GENERATION.fetch_add(1, Ordering::SeqCst);
            // On GNOME Wayland an X11 client cannot drop a picture that a Wayland program (the
            // browser, the screenshot tool) put there: a bare `clear()` left it in place
            // (measured). Owning the clipboard with empty text replaces it, and a `clear()` a
            // moment later, once the desktop has taken that over, drops our own claim as well. With
            // no pause the second step ran first and the picture stayed.
            #[cfg(target_os = "linux")]
            if cb.set().text("").is_ok() {
                std::thread::sleep(Duration::from_millis(300));
            }
            cb.clear().map_err(|e| e.to_string())?;
        }
        Ok(same)
    })
    .unwrap_or(false)
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
