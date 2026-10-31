//! Registers the LocalPass native-messaging host with Chrome and Edge. It is per user (the
//! registry keys are under HKEY_CURRENT_USER), so no administrator rights are needed. The host
//! is `localpass.exe` itself: see `native_host`.

use std::io;
use std::path::{Path, PathBuf};

use lp_broker::nmhost::{HOST_NAME, allowed_origin};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

/// Browsers we register with: (name, registry path of its native-messaging hosts).
pub const BROWSERS: [(&str, &str); 2] = [
    ("Chrome", r"Software\Google\Chrome\NativeMessagingHosts"),
    ("Edge", r"Software\Microsoft\Edge\NativeMessagingHosts"),
];

pub fn manifest_path(data_dir: &Path) -> PathBuf {
    data_dir
        .join("native-messaging")
        .join(format!("{HOST_NAME}.json"))
}

/// The host manifest the browser reads: which program to start and which extension may use it.
pub fn manifest_json(exe: &Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "LocalPass password manager",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_origins": [allowed_origin()],
    }))
    .expect("manifest serializes")
}

fn host_key(browser_path: &str) -> String {
    format!(r"{browser_path}\{HOST_NAME}")
}

pub fn install(data_dir: &Path, exe: &Path) -> io::Result<()> {
    let manifest = manifest_path(data_dir);
    if let Some(dir) = manifest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&manifest, manifest_json(exe))?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for (_, browser_path) in BROWSERS {
        let (key, _) = hkcu.create_subkey(host_key(browser_path))?;
        key.set_value("", &manifest.to_string_lossy().as_ref())?;
    }
    Ok(())
}

pub fn remove(data_dir: &Path) -> io::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for (_, browser_path) in BROWSERS {
        match hkcu.delete_subkey(host_key(browser_path)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    match std::fs::remove_file(manifest_path(data_dir)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Whether `browser_path` has our host registered, pointing at this app's manifest and exe.
pub fn is_registered(browser_path: &str, data_dir: &Path, exe: &Path) -> bool {
    let manifest = manifest_path(data_dir);
    let registered = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(host_key(browser_path), KEY_READ)
        .and_then(|key| key.get_value::<String, _>(""))
        .ok();
    if registered.as_deref().map(Path::new) != Some(manifest.as_path()) {
        return false;
    }
    std::fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["path"].as_str().map(PathBuf::from))
        .is_some_and(|path| path == exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_names_the_exe_and_only_our_extension() {
        let json = manifest_json(Path::new(r"C:\Apps\LocalPass\localpass.exe"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "app.localpass.host");
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], r"C:\Apps\LocalPass\localpass.exe");
        let origins = value["allowed_origins"].as_array().unwrap();
        assert_eq!(origins.len(), 1);
        assert_eq!(
            origins[0],
            format!("chrome-extension://{}/", lp_broker::nmhost::EXTENSION_ID)
        );
    }

    #[test]
    fn manifest_lives_in_the_data_dir() {
        let path = manifest_path(Path::new(r"C:\Data\app.localpass"));
        assert!(path.ends_with(r"native-messaging\app.localpass.host.json"));
    }
}
