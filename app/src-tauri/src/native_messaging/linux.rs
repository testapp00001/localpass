//! Registers the native-messaging helper with the browsers of this Linux user: one host manifest
//! file per browser, `app.localpass.host.json`, naming the program to start and the one extension
//! allowed to start it. Nothing else is written (no system folders). The real folders are only
//! used by the app: everything here takes a [`Places`], and tests pass a throwaway one.
//!
//! - Firefox reads `~/.mozilla/native-messaging-hosts`, also when it keeps its profiles in
//!   `~/.config/mozilla` (XDG). Its source forces the legacy folder for these manifests:
//!   `toolkit/xre/nsXREDirProvider.cpp`, `XRE_USER_NATIVE_MANIFESTS` ("Keep forcing the legacy path
//!   for compatibility", `GetUserDataDirectoryHome(.., aForceLegacy = true)` + `.mozilla`), and
//!   `toolkit/components/extensions/NativeManifests.sys.mjs` reads only that folder and the system
//!   one (mozilla-central, checked 2026-11-12). So there is no second Firefox location to write.
//! - Chrome and the Chromium browsers read `<user data dir>/NativeMessagingHosts`
//!   (`chrome/common/chrome_paths.cc`, `DIR_USER_NATIVE_MESSAGING`), and the default user data dir
//!   is `<config>/<browser folder>` with `<config>` = `$XDG_CONFIG_HOME` or `~/.config`
//!   (`chrome/common/chrome_paths_linux.cc`). Chrome also honours `$CHROME_CONFIG_HOME`; that rare
//!   override is not followed here.
//!
//! Flatpak and Snap browsers live in their own sandboxed folders and cannot start a host program
//! outside the sandbox, so they are not served.

use std::io;
use std::path::{Path, PathBuf};

use lp_broker::nmhost::{FIREFOX_EXTENSION_ID, HOST_NAME, allowed_origin};

/// Which manifest format a browser reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// `allowed_extensions` with the add-on id.
    Firefox,
    /// `allowed_origins` with the extension's origin.
    Chromium,
}

#[derive(Clone, Copy, Debug)]
pub struct Browser {
    /// The name shown to the user.
    pub name: &'static str,
    pub family: Family,
    /// Chromium family: the browser's folder under `<config>` (its default user data dir).
    config_folder: &'static str,
}

/// Every browser we register with. Firefox first: its file is always written (as before), the
/// others only when the browser's own config folder exists.
pub static BROWSERS: [Browser; 6] = [
    Browser {
        name: "Firefox",
        family: Family::Firefox,
        config_folder: "",
    },
    Browser {
        name: "Google Chrome",
        family: Family::Chromium,
        config_folder: "google-chrome",
    },
    Browser {
        name: "Chromium",
        family: Family::Chromium,
        config_folder: "chromium",
    },
    Browser {
        name: "Brave",
        family: Family::Chromium,
        config_folder: "BraveSoftware/Brave-Browser",
    },
    Browser {
        name: "Microsoft Edge",
        family: Family::Chromium,
        config_folder: "microsoft-edge",
    },
    Browser {
        name: "Vivaldi",
        family: Family::Chromium,
        config_folder: "vivaldi",
    },
];

/// The user's home and config folders. Only [`Places::from_env`] reads the environment.
#[derive(Clone, Debug)]
pub struct Places {
    pub home: PathBuf,
    /// `$XDG_CONFIG_HOME` when it is an absolute path, otherwise `<home>/.config`.
    pub config: PathBuf,
}

impl Places {
    pub fn new(home: impl Into<PathBuf>, xdg_config_home: Option<&Path>) -> Self {
        let home = home.into();
        let config = match xdg_config_home {
            Some(dir) if dir.is_absolute() => dir.to_path_buf(),
            _ => home.join(".config"),
        };
        Places { home, config }
    }

    /// The real places of this user, or an error when there is no home (nothing is invented).
    pub fn from_env() -> io::Result<Self> {
        let home = match std::env::var_os("HOME").map(PathBuf::from) {
            Some(home) if home.is_absolute() => home,
            _ => return Err(io::Error::other("HOME is not set")),
        };
        let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
        Ok(Places::new(home, xdg.as_deref()))
    }

    /// The folder whose existence says the browser is there (it has been run).
    fn browser_root(&self, browser: &Browser) -> PathBuf {
        match browser.family {
            Family::Firefox => self.home.join(".mozilla"),
            Family::Chromium => self.config.join(browser.config_folder),
        }
    }

    /// Where `browser` looks for our host manifest.
    pub fn manifest_path(&self, browser: &Browser) -> PathBuf {
        let dir = match browser.family {
            Family::Firefox => self.browser_root(browser).join("native-messaging-hosts"),
            Family::Chromium => self.browser_root(browser).join("NativeMessagingHosts"),
        };
        dir.join(format!("{HOST_NAME}.json"))
    }

    /// Whether `browser` has been run by this user. Chromium family: its config folder exists.
    /// Firefox: one of its profile roots exists, the legacy `~/.mozilla/firefox` or the XDG
    /// `<config>/mozilla/firefox` that new Firefox installs use. (`~/.mozilla` itself is not
    /// proof: it may hold only our own `native-messaging-hosts` folder.)
    pub fn is_present(&self, browser: &Browser) -> bool {
        match browser.family {
            Family::Firefox => {
                self.home.join(".mozilla").join("firefox").is_dir()
                    || self.config.join("mozilla").join("firefox").is_dir()
            }
            Family::Chromium => self.browser_root(browser).is_dir(),
        }
    }
}

/// The host manifest `family` reads: which program to start and which extension may use it.
pub fn manifest_json(family: Family, exe: &Path) -> String {
    let mut manifest = serde_json::json!({
        "name": HOST_NAME,
        "description": "LocalPass password manager",
        "path": exe.to_string_lossy(),
        "type": "stdio",
    });
    match family {
        Family::Firefox => {
            manifest["allowed_extensions"] = serde_json::json!([FIREFOX_EXTENSION_ID])
        }
        Family::Chromium => manifest["allowed_origins"] = serde_json::json!([allowed_origin()]),
    }
    serde_json::to_string_pretty(&manifest).expect("manifest serializes")
}

fn write_manifest(path: &Path, text: &str) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Written next to the target and renamed, so a browser never reads half a file.
    let partial = path.with_extension("json.part");
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}

/// Writes the manifest for Firefox and for every Chromium browser whose config folder exists.
/// Returns the names of the browsers written for. A missing browser gets no folder.
pub fn install(places: &Places, exe: &Path) -> io::Result<Vec<&'static str>> {
    let mut written = Vec::new();
    for browser in &BROWSERS {
        if browser.family == Family::Chromium && !places.is_present(browser) {
            continue;
        }
        write_manifest(
            &places.manifest_path(browser),
            &manifest_json(browser.family, exe),
        )?;
        written.push(browser.name);
    }
    Ok(written)
}

/// Deletes every LocalPass host manifest (folders are left as they are).
pub fn remove(places: &Places) -> io::Result<()> {
    for browser in &BROWSERS {
        match std::fs::remove_file(places.manifest_path(browser)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// What a browser's host manifest says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Registration {
    /// No (readable) manifest.
    None,
    /// The manifest starts this very exe.
    This,
    /// The manifest starts another copy of LocalPass.
    Other(PathBuf),
}

pub fn registration(places: &Places, browser: &Browser, exe: &Path) -> Registration {
    let path = std::fs::read_to_string(places.manifest_path(browser))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v["path"].as_str().map(PathBuf::from));
    match path {
        None => Registration::None,
        Some(path) if path == exe => Registration::This,
        Some(path) => Registration::Other(path),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserState {
    pub name: &'static str,
    pub present: bool,
    pub registered: bool,
    /// The other exe a manifest of this browser names, if any.
    pub other_copy: Option<PathBuf>,
}

/// Where each browser stands, in the order of [`BROWSERS`].
pub fn status(places: &Places, exe: &Path) -> Vec<BrowserState> {
    BROWSERS
        .iter()
        .map(|browser| {
            let registration = registration(places, browser, exe);
            BrowserState {
                name: browser.name,
                present: places.is_present(browser),
                registered: registration == Registration::This,
                other_copy: match registration {
                    Registration::Other(path) => Some(path),
                    _ => None,
                },
            }
        })
        .collect()
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

    fn browser(name: &str) -> &'static Browser {
        BROWSERS.iter().find(|b| b.name == name).unwrap()
    }

    fn is_registered(places: &Places, browser: &Browser, exe: &Path) -> bool {
        registration(places, browser, exe) == Registration::This
    }

    #[test]
    fn the_firefox_manifest_names_the_exe_and_only_our_addon() {
        let json = manifest_json(Family::Firefox, Path::new("/usr/bin/localpass"));
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
    fn the_chromium_manifest_names_the_exe_and_only_our_extension() {
        let json = manifest_json(Family::Chromium, Path::new("/usr/bin/localpass"));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["name"], "app.localpass.host");
        assert_eq!(value["type"], "stdio");
        assert_eq!(value["path"], "/usr/bin/localpass");
        let origins = value["allowed_origins"].as_array().unwrap();
        assert_eq!(origins.len(), 1);
        assert_eq!(
            origins[0],
            format!("chrome-extension://{}/", lp_broker::nmhost::EXTENSION_ID)
        );
        assert!(value.get("allowed_extensions").is_none());
    }

    #[test]
    fn config_is_xdg_config_home_only_when_absolute() {
        let home = Path::new("/home/me");
        assert_eq!(Places::new(home, None).config, home.join(".config"));
        assert_eq!(
            Places::new(home, Some(Path::new("relative/cfg"))).config,
            home.join(".config")
        );
        assert_eq!(
            Places::new(home, Some(Path::new("/srv/cfg"))).config,
            Path::new("/srv/cfg")
        );
        let places = Places::new(home, Some(Path::new("/srv/cfg")));
        assert_eq!(
            places.manifest_path(browser("Brave")),
            Path::new(
                "/srv/cfg/BraveSoftware/Brave-Browser/NativeMessagingHosts/app.localpass.host.json"
            )
        );
        assert_eq!(
            places.manifest_path(browser("Firefox")),
            Path::new("/home/me/.mozilla/native-messaging-hosts/app.localpass.host.json")
        );
    }

    #[test]
    fn only_browsers_that_are_there_get_a_file() {
        let home = throwaway_home("present");
        let config = home.join("cfg");
        let places = Places::new(&home, Some(config.as_path()));
        std::fs::create_dir_all(config.join("google-chrome")).unwrap();
        std::fs::create_dir_all(config.join("BraveSoftware/Brave-Browser")).unwrap();
        let exe = Path::new("/usr/bin/localpass");

        let written = install(&places, exe).unwrap();
        assert_eq!(written, ["Firefox", "Google Chrome", "Brave"]);
        for name in ["Firefox", "Google Chrome", "Brave"] {
            assert!(places.manifest_path(browser(name)).is_file(), "{name}");
        }
        // No config folder was made for a browser that is not there.
        for folder in ["chromium", "microsoft-edge", "vivaldi"] {
            assert!(!config.join(folder).exists(), "{folder}");
        }
        assert!(!home.join(".config").exists());
        // No half-written file is left behind.
        let chrome_dir = config.join("google-chrome/NativeMessagingHosts");
        let names: Vec<_> = std::fs::read_dir(&chrome_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["app.localpass.host.json"]);
        let chrome: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(places.manifest_path(browser("Google Chrome"))).unwrap(),
        )
        .unwrap();
        assert!(chrome.get("allowed_origins").is_some());
        let firefox: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(places.manifest_path(browser("Firefox"))).unwrap(),
        )
        .unwrap();
        assert!(firefox.get("allowed_extensions").is_some());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn status_is_per_browser() {
        let home = throwaway_home("status");
        let places = Places::new(&home, None);
        let exe = Path::new("/usr/bin/localpass");
        let before = status(&places, exe);
        assert_eq!(before.len(), BROWSERS.len());
        assert!(
            before
                .iter()
                .all(|b| !b.present && !b.registered && b.other_copy.is_none())
        );

        // Firefox with its profiles in the XDG folder, and Vivaldi.
        std::fs::create_dir_all(home.join(".config/mozilla/firefox")).unwrap();
        std::fs::create_dir_all(home.join(".config/vivaldi")).unwrap();
        install(&places, exe).unwrap();
        let after = status(&places, exe);
        let get = |name: &str| after.iter().find(|b| b.name == name).unwrap().clone();
        assert!(get("Firefox").present && get("Firefox").registered);
        assert!(get("Vivaldi").present && get("Vivaldi").registered);
        let chrome = get("Google Chrome");
        assert!(!chrome.present && !chrome.registered);
        assert!(is_registered(&places, browser("Vivaldi"), exe));
        assert!(!is_registered(&places, browser("Chromium"), exe));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn firefox_counts_as_present_with_a_legacy_profile_root() {
        let home = throwaway_home("legacy");
        let places = Places::new(&home, None);
        // Only our own manifest folder in ~/.mozilla: Firefox is not proven to be there.
        install(&places, Path::new("/usr/bin/localpass")).unwrap();
        assert!(!places.is_present(browser("Firefox")));
        std::fs::create_dir_all(home.join(".mozilla/firefox")).unwrap();
        assert!(places.is_present(browser("Firefox")));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn another_copy_is_detected_and_this_copy_takes_over() {
        let home = throwaway_home("other");
        let places = Places::new(&home, None);
        std::fs::create_dir_all(home.join(".config/chromium")).unwrap();
        let installed = Path::new("/usr/bin/localpass");
        let dev = Path::new("/home/me/localpass/target/debug/localpass");
        install(&places, installed).unwrap();

        assert_eq!(
            registration(&places, browser("Chromium"), dev),
            Registration::Other(installed.to_path_buf())
        );
        let seen = status(&places, dev);
        let chromium = seen.iter().find(|b| b.name == "Chromium").unwrap();
        assert!(chromium.present && !chromium.registered);
        assert_eq!(chromium.other_copy.as_deref(), Some(installed));
        // A browser without a manifest names no other copy.
        let chrome = seen.iter().find(|b| b.name == "Google Chrome").unwrap();
        assert_eq!(chrome.other_copy, None);

        // "Use this copy" is the same install.
        install(&places, dev).unwrap();
        assert!(is_registered(&places, browser("Chromium"), dev));
        assert!(is_registered(&places, browser("Firefox"), dev));
        assert!(status(&places, dev).iter().all(|b| b.other_copy.is_none()));
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn remove_deletes_every_manifest_and_nothing_else() {
        let home = throwaway_home("remove");
        let config = home.join(".config");
        let places = Places::new(&home, None);
        for folder in [
            "google-chrome",
            "chromium",
            "BraveSoftware/Brave-Browser",
            "microsoft-edge",
            "vivaldi",
        ] {
            std::fs::create_dir_all(config.join(folder)).unwrap();
        }
        let exe = Path::new("/opt/localpass/localpass");
        assert_eq!(install(&places, exe).unwrap().len(), BROWSERS.len());
        assert!(BROWSERS.iter().all(|b| is_registered(&places, b, exe)));
        remove(&places).unwrap();
        for b in &BROWSERS {
            assert!(!places.manifest_path(b).exists(), "{}", b.name);
            assert_eq!(registration(&places, b, exe), Registration::None);
        }
        // The browsers' own folders stay.
        assert!(config.join("google-chrome").is_dir());
        // Removing again is fine.
        remove(&places).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }
}
