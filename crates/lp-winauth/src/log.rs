//! The provider's log: `%LOCALAPPDATA%\LocalPass\provider\provider.log` plus `OutputDebugStringW`
//! (DebugView), and standard output in the command-line modes.
//!
//! Every line has the time, process id, thread id and the package family name, so a COM-started server
//! can be told apart from a `status` run. Nothing secret is ever logged: the pairing secret and request
//! bodies stay out; credential ids are shortened with [`short_id`]; user names and ids are not logged.
//!
//! Nothing is written until [`init`] is called (the binary does; tests do not, so they never touch the
//! real log).

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::core::{HRESULT, PCWSTR};

use crate::sys::wide;

/// Past this size the log is moved to `provider.log.1` at start-up.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

static LOG_FILE: OnceLock<PathBuf> = OnceLock::new();
static LOCK: Mutex<()> = Mutex::new(());
static TO_STDOUT: AtomicBool = AtomicBool::new(false);
static PFN: OnceLock<String> = OnceLock::new();

/// Starts logging to `dir\provider.log`. The first call wins.
pub fn init(dir: &Path, to_stdout: bool) {
    TO_STDOUT.store(to_stdout, Ordering::SeqCst);
    let _ = std::fs::create_dir_all(dir);
    let file = dir.join("provider.log");
    if std::fs::metadata(&file).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&file, dir.join("provider.log.1"));
    }
    let _ = LOG_FILE.set(file);
}

/// Where the log goes (None before [`init`]).
pub fn path() -> Option<&'static Path> {
    LOG_FILE.get().map(PathBuf::as_path)
}

/// The package family name of this process, `<none>` when it has no package identity.
pub fn package_family_name() -> &'static str {
    PFN.get_or_init(|| {
        let mut len = 0u32;
        // SAFETY: size query with a null buffer, then a read into a buffer of the reported size.
        let rc = unsafe { GetCurrentPackageFamilyName(&mut len, None) };
        // 122 = ERROR_INSUFFICIENT_BUFFER; 15700 = APPMODEL_ERROR_NO_PACKAGE (no identity).
        if rc.0 != 122 || len == 0 {
            return "<none>".to_owned();
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: `buf` has the length the size query reported.
        let rc = unsafe {
            GetCurrentPackageFamilyName(&mut len, Some(windows::core::PWSTR(buf.as_mut_ptr())))
        };
        if rc.0 != 0 {
            return format!("<error {}>", rc.0);
        }
        String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)])
    })
}

/// Writes one line. Prefer the [`plog!`](crate::plog) macro.
pub fn write(message: &str) {
    let Some(file) = LOG_FILE.get() else {
        return;
    };
    // SAFETY: plain Win32 calls without pointers.
    let (t, pid, tid) = unsafe { (GetLocalTime(), GetCurrentProcessId(), GetCurrentThreadId()) };
    let line = format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} pid={pid} tid={tid} pfn={} {message}\n",
        t.wYear,
        t.wMonth,
        t.wDay,
        t.wHour,
        t.wMinute,
        t.wSecond,
        t.wMilliseconds,
        package_family_name(),
    );
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
    {
        let _ = f.write_all(line.as_bytes());
    }
    let wide_line = wide(&line);
    // SAFETY: `wide_line` is NUL terminated and alive for the call.
    unsafe { OutputDebugStringW(PCWSTR(wide_line.as_ptr())) };
    if TO_STDOUT.load(Ordering::SeqCst) {
        let _ = std::io::stdout().write_all(line.as_bytes());
    }
}

/// Logs a formatted line (see [`write`]).
#[macro_export]
macro_rules! plog {
    ($($arg:tt)*) => { $crate::log::write(&format!($($arg)*)) };
}

/// `0x80090036` style text for an HRESULT.
pub fn hr(h: HRESULT) -> String {
    format!("0x{:08X}", h.0 as u32)
}

/// The first 4 bytes of a credential id in hex, enough to correlate log lines without recording the id.
pub fn short_id(id: &[u8]) -> String {
    let head: String = id.iter().take(4).map(|b| format!("{b:02x}")).collect();
    format!("{head}..({}b)", id.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_ids_are_shortened() {
        assert_eq!(short_id(&[0xAB; 32]), "abababab..(32b)");
        assert_eq!(short_id(&[1]), "01..(1b)");
        assert_eq!(short_id(&[]), "..(0b)");
    }

    #[test]
    fn hresults_print_as_hex() {
        assert_eq!(hr(HRESULT(0x8009_0036_u32 as i32)), "0x80090036");
        assert_eq!(hr(HRESULT(0)), "0x00000000");
    }

    #[test]
    fn writing_before_init_does_nothing() {
        // No file is configured in the test process: this must neither panic nor create a log.
        write("not logged");
        assert!(path().is_none());
    }
}
