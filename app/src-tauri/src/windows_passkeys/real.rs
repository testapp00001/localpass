//! The real machine: child processes with a time limit, the registry, file versions.

use std::ffi::c_void;
use std::io::{self, Read};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
};
use windows::core::{PCWSTR, w};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE};

use super::model::{Command, CommandRunner, Output, RunError, System, parse_manifest_package};

/// Keeps a console window from flashing up for a child process.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// More than this from a child is cut off (it only ever prints a few lines).
const MAX_OUTPUT: u64 = 1 << 20;

pub struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, command: &Command) -> Result<Output, RunError> {
        let mut child = std::process::Command::new(&command.program);
        child
            .args(&command.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        for (name, value) in &command.env {
            child.env(name, value);
        }
        let mut child = child.spawn().map_err(|e| RunError::Spawn(e.to_string()))?;
        let stdout = reader(child.stdout.take());
        let stderr = reader(child.stderr.take());
        let deadline = Instant::now() + command.timeout;
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    // The readers are left to finish by themselves: a grandchild may still hold the pipe.
                    return Err(RunError::TimedOut);
                }
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(e) => return Err(RunError::Spawn(e.to_string())),
            }
        };
        Ok(Output {
            code,
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

fn reader<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// The registry key and the file reads of this PC. `subkey` is `HKCU\<subkey>` where `AppPath` lives.
pub struct RealSystem {
    pub subkey: String,
}

const APP_PATH_VALUE: &str = "AppPath";

impl System for RealSystem {
    fn webauthn_version(&self) -> Option<[u32; 4]> {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
        file_version(&PathBuf::from(root).join("System32").join("webauthn.dll"))
    }

    fn developer_mode(&self) -> bool {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey_with_flags(
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock",
                KEY_READ,
            )
            .and_then(|key| key.get_value::<u32, _>("AllowDevelopmentWithoutDevLicense"))
            .is_ok_and(|value| value == 1)
    }

    fn exists(&self, path: &Path) -> bool {
        // The alias is a reparse point: `metadata` follows it, which can fail, `symlink_metadata` does not.
        std::fs::symlink_metadata(path).is_ok()
    }

    fn manifest_package(&self, path: &Path) -> Option<String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(MAX_OUTPUT)
            .read_to_string(&mut text)
            .ok()?;
        parse_manifest_package(&text)
    }

    fn app_path(&self) -> Option<PathBuf> {
        RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags(&self.subkey, KEY_READ)
            .and_then(|key| key.get_value::<String, _>(APP_PATH_VALUE))
            .ok()
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }

    fn set_app_path(&self, path: &Path) -> io::Result<()> {
        let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(&self.subkey)?;
        key.set_value(APP_PATH_VALUE, &path.to_string_lossy().as_ref())
    }

    fn clear_app_path(&self) -> io::Result<()> {
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let key = match hkcu.open_subkey_with_flags(&self.subkey, KEY_READ | KEY_WRITE) {
            Ok(key) => key,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        match key.delete_value(APP_PATH_VALUE) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        // Only an empty key goes: other values and keys under LocalPass are not ours to touch.
        if key.enum_values().next().is_none() && key.enum_keys().next().is_none() {
            drop(key);
            match hkcu.delete_subkey(&self.subkey) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_file(path)
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// The four-part file version (`10.0.26100.7171`) from the version resource.
fn file_version(path: &Path) -> Option<[u32; 4]> {
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let name = PCWSTR(wide.as_ptr());
    // SAFETY: `name` is NUL-terminated and outlives the calls; `data` holds `size` bytes, which is
    // what `GetFileVersionInfoW` may write; the pointer `VerQueryValueW` returns points into `data`
    // and is read (unaligned) before `data` is dropped.
    unsafe {
        let size = GetFileVersionInfoSizeW(name, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(name, None, size, data.as_mut_ptr().cast()).ok()?;
        let mut info: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut info, &mut len).as_bool()
            || info.is_null()
            || (len as usize) < size_of::<VS_FIXEDFILEINFO>()
        {
            return None;
        }
        let fixed = info.cast::<VS_FIXEDFILEINFO>().read_unaligned();
        Some([
            fixed.dwFileVersionMS >> 16,
            fixed.dwFileVersionMS & 0xFFFF,
            fixed.dwFileVersionLS >> 16,
            fixed.dwFileVersionLS & 0xFFFF,
        ])
    }
}
