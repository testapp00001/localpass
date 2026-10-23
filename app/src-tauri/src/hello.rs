//! Windows Hello quick unlock.
//!
//! A Hello-protected RSA key (TPM-backed when available) signs a fixed
//! challenge. PKCS#1 v1.5 signatures are deterministic, so the signature is
//! stable key material: the quick-unlock key is HKDF(signature). The private
//! key never leaves Windows, and every signature needs a Hello gesture.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use lp_core::crypto::{Key32, hkdf_sha256};
use windows::Security::Credentials::{
    KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialRetrievalResult,
    KeyCredentialStatus,
};
use windows::Security::Cryptography::CryptographicBuffer;
use windows::core::{HSTRING, Interface, RuntimeType};
use windows_future::{IAsyncInfo, IAsyncOperation};

fn status_error(status: KeyCredentialStatus) -> String {
    match status {
        // The UI treats this exact text as "the user backed out" and stays quiet.
        KeyCredentialStatus::UserCanceled | KeyCredentialStatus::UserPrefersPassword => {
            "cancelled".into()
        }
        KeyCredentialStatus::NotFound => {
            "Windows Hello key not found; turn quick unlock off and on again".into()
        }
        KeyCredentialStatus::SecurityDeviceLocked => {
            "the security device is locked; try again later".into()
        }
        other => format!("Windows Hello failed ({})", other.0),
    }
}

pub fn available() -> bool {
    KeyCredentialManager::IsSupportedAsync()
        .and_then(|op| op.join())
        .unwrap_or(false)
}

fn credential_name(vault_id: &str) -> HSTRING {
    HSTRING::from(format!("LocalPass-{}", &vault_id[..vault_id.len().min(16)]))
}

/// The Hello dialog of the check that is running, once the watcher below has seen it (0 = none).
static OUR_DIALOG: AtomicIsize = AtomicIsize::new(0);

/// Watches for the Hello dialog after a check was started and makes sure it is in front of
/// everything (it sometimes opens behind the window the user was in, with only its taskbar
/// button flashing). Logs, for the prompt log, what had the foreground when the check started,
/// whether and when the dialog appeared, and what had the foreground 300 ms after the start.
/// The dialog is the first dialog window that was not open when the check started, so another
/// Windows Security window (the passkey chooser the user just used) is left alone.
/// Only one watcher runs at a time; a second call while one is active does nothing.
fn focus_hello_dialog(why: &'static str) {
    use std::time::{Duration, Instant};

    static WATCHING: AtomicBool = AtomicBool::new(false);
    if WATCHING.swap(true, Ordering::SeqCst) {
        return;
    }
    OUR_DIALOG.store(0, Ordering::SeqCst);
    let already_open = crate::focus::security_dialogs();
    let started = Instant::now();
    tracing::info!(
        target: "lp_prompt",
        "{why}: Windows Hello starting; foreground: {}; Windows Security windows already open: {}",
        crate::focus::foreground_description(),
        already_open.len()
    );
    std::thread::spawn(move || {
        let (mut raised, mut reported) = (false, false);
        while started.elapsed() < Duration::from_secs(5) && !(raised && reported) {
            std::thread::sleep(Duration::from_millis(25));
            let elapsed = started.elapsed();
            let dialog = crate::focus::security_dialogs()
                .into_iter()
                .find(|d| !already_open.contains(d));
            if let Some(dialog) = dialog
                && !raised
            {
                raised = true;
                OUR_DIALOG.store(dialog, Ordering::SeqCst);
                tracing::info!(
                    target: "lp_prompt",
                    "{why}: Hello dialog appeared after {} ms ({})",
                    elapsed.as_millis(),
                    crate::focus::describe_raw(dialog)
                );
                let in_front = crate::focus::bring_to_front(dialog, why);
                tracing::info!(target: "lp_prompt", "{why}: Hello dialog in front: {in_front}");
            }
            if !reported && elapsed >= Duration::from_millis(300) {
                reported = true;
                tracing::info!(
                    target: "lp_prompt",
                    "{why}: 300 ms after Windows Hello started: foreground {}; dialog found: {}",
                    crate::focus::foreground_description(),
                    raised
                );
            }
        }
        if !raised {
            tracing::info!(target: "lp_prompt", "{why}: no Hello dialog appeared within 5 s");
        }
        WATCHING.store(false, Ordering::SeqCst);
    });
}

/// Derives the quick-unlock key, prompting for Windows Hello. With `create`,
/// a new Hello key replaces any previous one for this vault. `why` says who asked (for the log).
pub fn derive_key(vault_id: &str, create: bool, why: &'static str) -> Result<Key32, String> {
    let name = credential_name(vault_id);
    focus_hello_dialog(why);
    let retrieved: KeyCredentialRetrievalResult = if create {
        KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::ReplaceExisting,
        )
    } else {
        KeyCredentialManager::OpenAsync(&name)
    }
    .and_then(|op| op.join())
    .map_err(|e| e.message().to_string())?;
    let status = retrieved.Status().map_err(|e| e.message().to_string())?;
    if status != KeyCredentialStatus::Success {
        return Err(status_error(status));
    }
    let credential = retrieved
        .Credential()
        .map_err(|e| e.message().to_string())?;
    let challenge = CryptographicBuffer::CreateFromByteArray(
        format!("localpass-hello-v1:{vault_id}").as_bytes(),
    )
    .map_err(|e| e.message().to_string())?;
    focus_hello_dialog(why);
    let signed = credential
        .RequestSignAsync(&challenge)
        .and_then(|op| op.join())
        .map_err(|e| e.message().to_string())?;
    let status = signed.Status().map_err(|e| e.message().to_string())?;
    if status != KeyCredentialStatus::Success {
        return Err(status_error(status));
    }
    let buffer = signed.Result().map_err(|e| e.message().to_string())?;
    let mut sig = windows::core::Array::<u8>::new();
    CryptographicBuffer::CopyToByteArray(&buffer, &mut sig).map_err(|e| e.message().to_string())?;
    let key = hkdf_sha256(&sig, b"localpass/hello", b"quick-unlock-v1");
    Ok(key)
}

/// Lets another thread abandon a user check that is waiting for the Windows Hello dialog (the
/// client that asked for it went away). Cancelling asks Windows to cancel the pending
/// operation, which should close the dialog; the check then fails with "cancelled".
#[derive(Default)]
pub struct CancelHandle {
    state: Mutex<CancelState>,
}

#[derive(Default)]
struct CancelState {
    cancelled: bool,
    /// The Windows operation currently being waited for.
    current: Option<Pending>,
}

/// Cancelling the Windows operation does not always take its dialog down. Measured: after
/// `IAsyncInfo::Cancel()` on a pending Hello check the dialog stayed on screen (even after the
/// whole app had been killed) until it was closed by hand. So, a moment after cancelling, the
/// dialog of that check (the one the watcher saw) is closed, if it is still there.
fn close_dialog_if_left_open() {
    let dialog = OUR_DIALOG.swap(0, Ordering::SeqCst);
    if dialog == 0 {
        return;
    }
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        let closed = crate::focus::close_security_dialog(dialog);
        tracing::info!(
            target: "lp_prompt",
            "Hello dialog still open 400 ms after Cancel(): {closed}{}",
            if closed { " (closed it)" } else { "" }
        );
    });
}

/// A running Windows async operation, seen as `IAsyncInfo` so it can be cancelled.
struct Pending(IAsyncInfo);

// SAFETY: `windows-future` marks `IAsyncOperation<T>` Send and Sync because WinRT async
// operations are agile objects; this is the same object seen through another of its interfaces,
// and `Cancel` may be called from any thread.
unsafe impl Send for Pending {}

impl CancelHandle {
    /// Cancels the check now, or before it starts if it has not yet.
    pub fn cancel(&self) {
        let mut state = self.state.lock().expect("hello cancel");
        state.cancelled = true;
        // Only a check that is still waiting for the user is really cancelled; this is also
        // called after a finished one, when there is nothing to do.
        if let Some(op) = state.current.take() {
            let result = op.0.Cancel();
            tracing::info!(
                target: "lp_prompt",
                "Windows Hello check cancelled by the caller; Cancel() -> {}",
                if result.is_ok() { "ok" } else { "failed" }
            );
            drop(state);
            close_dialog_if_left_open();
        }
    }

    /// Waits for a Windows operation, unless the check was cancelled, in which case the
    /// operation is cancelled as well.
    fn wait<T: RuntimeType + 'static>(
        &self,
        op: windows::core::Result<IAsyncOperation<T>>,
    ) -> Result<T, String> {
        let op = op.map_err(|e| e.message().to_string())?;
        let info: Option<IAsyncInfo> = op.cast().ok();
        {
            let mut state = self.state.lock().expect("hello cancel");
            if state.cancelled {
                if let Some(info) = &info {
                    let _ = info.Cancel();
                }
                return Err("cancelled".into());
            }
            state.current = info.map(Pending);
        }
        let result = op.join();
        let cancelled = {
            let mut state = self.state.lock().expect("hello cancel");
            state.current = None;
            state.cancelled
        };
        if cancelled {
            return Err("cancelled".into());
        }
        result.map_err(|e| e.message().to_string())
    }
}

/// Asks the user to confirm with Windows Hello (PIN, fingerprint or face) without unlocking or
/// deriving anything. Used before signing in with a passkey. Succeeds only if the user passed
/// the check. The first use creates a Hello-protected key just for this.
pub fn verify_user(cancel: &CancelHandle, why: &'static str) -> Result<(), String> {
    let name = HSTRING::from("LocalPass-verify");
    focus_hello_dialog(why);
    let opened = cancel.wait(KeyCredentialManager::OpenAsync(&name))?;
    let retrieved = match opened.Status().map_err(|e| e.message().to_string())? {
        KeyCredentialStatus::NotFound => cancel.wait(KeyCredentialManager::RequestCreateAsync(
            &name,
            KeyCredentialCreationOption::ReplaceExisting,
        ))?,
        _ => opened,
    };
    let status = retrieved.Status().map_err(|e| e.message().to_string())?;
    if status != KeyCredentialStatus::Success {
        return Err(status_error(status));
    }
    let credential = retrieved
        .Credential()
        .map_err(|e| e.message().to_string())?;
    let challenge = CryptographicBuffer::CreateFromByteArray(b"localpass-verify-v1")
        .map_err(|e| e.message().to_string())?;
    focus_hello_dialog(why);
    let signed = cancel.wait(credential.RequestSignAsync(&challenge))?;
    match signed.Status().map_err(|e| e.message().to_string())? {
        KeyCredentialStatus::Success => Ok(()),
        other => Err(status_error(other)),
    }
}

pub fn delete(vault_id: &str) {
    let _ = KeyCredentialManager::DeleteAsync(&credential_name(vault_id)).and_then(|op| op.join());
}
