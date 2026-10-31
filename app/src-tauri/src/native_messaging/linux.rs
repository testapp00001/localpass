//! Firefox looks for native-messaging hosts in `~/.mozilla/native-messaging-hosts/<name>.json`.
//! The file names the program to start and the one add-on allowed to start it. Nothing else is
//! written (no system folders), and the real folder is only used by the app: tests pass a
//! throwaway home.

use std::io;
use std::path::{Path, PathBuf};

use lp_broker::nmhost::{FIREFOX_EXTENSION_ID, HOST_NAME};

/// The folder Firefox reads (Fedora's Firefox RPM; Flatpak and Snap builds look elsewhere).
pub fn firefox_dir(home: &Path) -> PathBuf {
    home.join(".mozilla").join("native-messaging-hosts")
}

/// The user's home folder, or an error when there is none (nothing is invented).
pub fn home_dir() -> io::Result<PathBuf> {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if home.is_absolute() => Ok(home),
        _ => Err(io::Error::other("HOME is not set")),
    }
}

pub fn manifest_path(home: &Path) -> PathBuf {
    firefox_dir(home).join(format!("{HOST_NAME}.json"))
}

/// The host manifest Firefox reads: which program to start and which add-on may use it.
pub fn manifest_json(exe: &Path) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "name": HOST_NAME,
        "description": "LocalPass password manager",
        "path": exe.to_string_lossy(),
        "type": "stdio",
        "allowed_extensions": [FIREFOX_EXTENSION_ID],
    }))
    .expect("manifest serializes")
}

pub fn install(home: &Path, exe: &Path) -> io::Result<()> {
    let manifest = manifest_path(home);
    if let Some(dir) = manifest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written next to the target and renamed, so Firefox never reads half a file.
    let partial = manifest.with_extension("json.part");
    std::fs::write(&partial, manifest_json(exe))?;
    std::fs::rename(&partial, &manifest)
}

pub fn remove(home: &Path) -> io::Result<()> {
    match std::fs::remove_file(manifest_path(home)) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Whether Firefox's host manifest exists and starts this very exe.
pub fn is_registered(home: &Path, exe: &Path) -> bool {
    std::fs::read_to_string(manifest_path(home))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["path"].as_str().map(PathBuf::from))
        .is_some_and(|path| path == exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn throwaway_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lp-nm-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_manifest_names_the_exe_and_only_our_addon() {
        let json = manifest_json(Path::new("/usr/bin/localpass"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "app.localpass.host");
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], "/usr/bin/localpass");
        let allowed = value["allowed_extensions"].as_array().unwrap();
        assert_eq!(allowed.len(), 1);
        assert_eq!(allowed[0], FIREFOX_EXTENSION_ID);
        assert!(value.get("allowed_origins").is_none());
    }

    #[test]
    fn install_registers_and_remove_undoes_it() {
        let home = throwaway_home("install");
        let exe = Path::new("/opt/localpass/localpass");
        assert!(!is_registered(&home, exe));
        install(&home, exe).unwrap();
        assert!(
            manifest_path(&home)
                .ends_with(".mozilla/native-messaging-hosts/app.localpass.host.json")
        );
        assert!(is_registered(&home, exe));
        // Another copy of the app is not what Firefox would start.
        assert!(!is_registered(&home, Path::new("/tmp/other/localpass")));
        // Installing again (the app moved) points it at the new exe.
        let moved = Path::new("/usr/bin/localpass");
        install(&home, moved).unwrap();
        assert!(is_registered(&home, moved));
        assert!(!is_registered(&home, exe));
        remove(&home).unwrap();
        assert!(!manifest_path(&home).exists());
        remove(&home).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }
}
