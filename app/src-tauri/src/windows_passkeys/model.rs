//! Identity of the provider package, the file layout, the traits that touch the machine, and parsers for
//! what PowerShell and the provider print. No I/O happens here.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The package name in `crates/lp-winauth/package/AppxManifest.xml` (`Identity Name`).
pub const PACKAGE_NAME: &str = "LocalPass.Passkeys";
/// The provider's exe in the package; its execution alias has the same name.
pub const PROVIDER_EXE: &str = "lp-winauth-provider.exe";
/// The folder next to `localpass.exe` that `tauri build` fills (`tauri.windows.conf.json` resources).
pub const BUNDLE_FOLDER: &str = "windows-passkeys";
pub const MANIFEST_FILE: &str = "AppxManifest.xml";
/// `HKCU` key whose `AppPath` value tells the provider where the app is (`lp-winauth` `store.rs`).
pub const REGISTRY_SUBKEY: &str = r"Software\LocalPass\Provider";
/// The provider's data folder under `%LOCALAPPDATA%\LocalPass` (`client.bin` = its pairing secret).
pub const DATA_FOLDER: &str = "provider";
/// The pairing file in the data folder.
pub const PAIRING_FILE: &str = "client.bin";
/// The first webauthn.dll with the plug-in authenticator API (Windows 11 24H2 + the 2025-09 update): the
/// (build, revision) of its file version. Only these two numbers count: a program without a Windows 10
/// manifest is told 6.2 as major.minor for system files (measured: `6.2.26100.7462` in a test binary where
/// PowerShell shows `10.0.26100.7462`).
pub const MIN_WEBAUTHN: (u32, u32) = (26100, 6725);

/// Which package, exe, registry key and data folder to work with. Release builds always use the shipped
/// values; debug builds let environment variables swap them for a throwaway package (see `Identity::current`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub package: String,
    /// The exe name in the package; the execution alias has the same name.
    pub exe: String,
    pub registry_subkey: String,
    pub data_folder: String,
}

impl Identity {
    pub fn shipped() -> Self {
        Self {
            package: PACKAGE_NAME.into(),
            exe: PROVIDER_EXE.into(),
            registry_subkey: REGISTRY_SUBKEY.into(),
            data_folder: DATA_FOLDER.into(),
        }
    }

    /// The shipped identity. In a debug build `LOCALPASS_PROVIDER_PACKAGE`, `_EXE`, `_REGKEY` and `_DATA`
    /// replace the matching part (for a real-OS run of the set-up code against a throwaway package, with a
    /// provider built `--features test-identity`). Values with odd characters are ignored.
    pub fn current() -> Self {
        #[allow(unused_mut)]
        let mut identity = Self::shipped();
        #[cfg(debug_assertions)]
        {
            let var = |name: &str| std::env::var(name).ok().filter(|v| plain_name(v));
            if let Some(v) = var("LOCALPASS_PROVIDER_PACKAGE") {
                identity.package = v;
            }
            if let Some(v) = var("LOCALPASS_PROVIDER_EXE") {
                identity.exe = v;
            }
            if let Some(v) = var("LOCALPASS_PROVIDER_REGKEY") {
                identity.registry_subkey = v;
            }
            if let Some(v) = var("LOCALPASS_PROVIDER_DATA") {
                identity.data_folder = v;
            }
        }
        identity
    }

    pub fn is_shipped(&self) -> bool {
        *self == Self::shipped()
    }

    /// The process name Windows shows for the exe (no `.exe`).
    pub fn exe_stem(&self) -> &str {
        self.exe.strip_suffix(".exe").unwrap_or(&self.exe)
    }
}

/// Letters, digits and `. _ - \`; the values reach PowerShell as environment variables, never inside a
/// command line, but nothing odd gets that far either.
#[cfg(debug_assertions)]
fn plain_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 120
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '\\'))
}

/// Where everything is.
#[derive(Clone, Debug)]
pub struct Layout {
    pub identity: Identity,
    /// `windows-passkeys` next to this app: the provider exe, `AppxManifest.xml` and `Assets`.
    pub bundle_dir: PathBuf,
    /// This app's exe (what the provider starts when the app is not running).
    pub app_exe: PathBuf,
    /// `%LOCALAPPDATA%\Microsoft\WindowsApps\<exe>`: runs the provider with package identity.
    pub alias: PathBuf,
    pub powershell: PathBuf,
    /// `%LOCALAPPDATA%\LocalPass\<data folder>`.
    pub data_dir: PathBuf,
}

impl Layout {
    pub fn new(identity: Identity, bundle_dir: PathBuf, app_exe: PathBuf) -> Self {
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let system_root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        Self {
            alias: local
                .join("Microsoft")
                .join("WindowsApps")
                .join(&identity.exe),
            powershell: system_root
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe"),
            data_dir: local.join("LocalPass").join(&identity.data_folder),
            bundle_dir: clean_path(&bundle_dir),
            app_exe: clean_path(&app_exe),
            identity,
        }
    }

    pub fn provider_exe(&self) -> PathBuf {
        self.bundle_dir.join(&self.identity.exe)
    }

    pub fn manifest(&self) -> PathBuf {
        self.bundle_dir.join(MANIFEST_FILE)
    }

    pub fn pairing_file(&self) -> PathBuf {
        self.data_dir.join(PAIRING_FILE)
    }
}

/// Removes the `\\?\` prefix Windows APIs add (PowerShell and the registry value should not see it).
pub fn clean_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

/// Whether two paths are the same place (Windows: case-insensitive, trailing separators ignored).
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        clean_path(p)
            .to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

// ---- the things that touch the machine ----

/// Which of the fixed commands this is (the fakes in the tests tell them apart by it; real runs log it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    PackageQuery,
    PackageRegister,
    PackageRemove,
    /// The provider through its execution alias (`register`, `unregister`, `status`, `pair`, `sync-cache`).
    Provider(&'static str),
}

impl Step {
    pub fn label(self) -> String {
        match self {
            Self::PackageQuery => "package query".into(),
            Self::PackageRegister => "package register".into(),
            Self::PackageRemove => "package remove".into(),
            Self::Provider(mode) => format!("provider {mode}"),
        }
    }
}

/// A program to run: a fixed path, fixed arguments, values only through the environment, a time limit.
#[derive(Clone, Debug)]
pub struct Command {
    pub step: Step,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub timeout: Duration,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }

    /// The most useful line of what the program said: an `error:` line, else the last non-empty line.
    pub fn message(&self) -> String {
        let text = format!("{}\n{}", self.stdout, self.stderr);
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let line = lines
            .iter()
            .find(|l| l.to_ascii_lowercase().starts_with("error:"))
            .or(lines.last())
            .copied()
            .unwrap_or("");
        let line = line.strip_prefix("error:").map_or(line, str::trim);
        line.chars().filter(|c| !c.is_control()).take(300).collect()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunError {
    Spawn(String),
    TimedOut,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(why) => write!(f, "could not start the program: {why}"),
            Self::TimedOut => write!(f, "it did not finish in time"),
        }
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, command: &Command) -> Result<Output, RunError>;
}

/// Reads and writes of the machine's state that are not child processes.
pub trait System: Send + Sync {
    /// The file version of `webauthn.dll`.
    fn webauthn_version(&self) -> Option<[u32; 4]>;
    /// `AllowDevelopmentWithoutDevLicense` (Settings > System > For developers > Developer Mode).
    fn developer_mode(&self) -> bool;
    fn exists(&self, path: &Path) -> bool;
    /// The `Identity Name` of the package manifest at `path` (see [`parse_manifest_package`]).
    fn manifest_package(&self, path: &Path) -> Option<String>;
    /// `HKCU\<registry subkey>\AppPath`.
    fn app_path(&self) -> Option<PathBuf>;
    fn set_app_path(&self, path: &Path) -> io::Result<()>;
    /// Removes the value (and the key when it is empty).
    fn clear_app_path(&self) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    fn sleep(&self, duration: Duration);
}

/// The app's list of paired clients, for the provider's side of the pairing.
pub trait ClientBook: Send + Sync {
    fn has_provider(&self) -> bool;
    /// Forgets every paired Windows provider; returns how many.
    fn forget_providers(&self) -> io::Result<usize>;
}

// ---- the fixed PowerShell scripts ----
//
// Single quotes only, so the command line needs no escaping, and no value is spliced in: the package name,
// the manifest path and the exe name arrive as `LP_PACKAGE`, `LP_MANIFEST` and `LP_EXE_STEM`.

const PREAMBLE: &str = "$ErrorActionPreference = 'Stop'; [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ";
const FAIL: &str = "catch { Write-Output ('error: ' + $_.Exception.Message); exit 1 }";

fn script(body: &str) -> String {
    format!("{PREAMBLE}try {{ {body} }} {FAIL}")
}

pub fn query_script() -> String {
    script(
        "Get-AppxPackage -Name $env:LP_PACKAGE | ForEach-Object { \
         Write-Output ('package|' + $_.PackageFullName + '|' + $_.Version + '|' + $_.InstallLocation + '|' + $_.Status) }",
    )
}

pub fn register_script() -> String {
    script("Add-AppxPackage -Register $env:LP_MANIFEST; Write-Output 'ok'")
}

/// Stops the provider's own processes (never the app), then removes the package.
pub fn remove_script() -> String {
    script(
        "Get-Process -Name $env:LP_EXE_STEM -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue; \
         $p = Get-AppxPackage -Name $env:LP_PACKAGE; if ($p) { $p | Remove-AppxPackage }; Write-Output 'ok'",
    )
}

impl Layout {
    pub fn powershell_command(&self, step: Step, body: String, timeout: Duration) -> Command {
        let mut env = vec![("LP_PACKAGE".to_owned(), self.identity.package.clone())];
        env.push((
            "LP_EXE_STEM".to_owned(),
            self.identity.exe_stem().to_owned(),
        ));
        env.push((
            "LP_MANIFEST".to_owned(),
            self.manifest().to_string_lossy().into_owned(),
        ));
        Command {
            step,
            program: self.powershell.clone(),
            args: ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
                .map(String::from)
                .into_iter()
                .chain([body])
                .collect(),
            env,
            timeout,
        }
    }

    /// The provider through its alias, in a mode with no free-form arguments.
    pub fn provider_command(
        &self,
        mode: &'static str,
        extra: &[&str],
        timeout: Duration,
    ) -> Command {
        Command {
            step: Step::Provider(mode),
            program: self.alias.clone(),
            args: std::iter::once(mode)
                .chain(extra.iter().copied())
                .map(String::from)
                .collect(),
            env: Vec::new(),
            timeout,
        }
    }
}

// ---- parsers ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageInfo {
    pub full_name: String,
    pub version: String,
    pub install_location: PathBuf,
    pub status: String,
}

/// `package|<full name>|<version>|<install location>|<status>` lines from [`query_script`].
pub fn parse_packages(stdout: &str) -> Vec<PackageInfo> {
    stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.trim().splitn(5, '|');
            (parts.next()? == "package").then_some(())?;
            Some(PackageInfo {
                full_name: parts.next()?.to_owned(),
                version: parts.next()?.to_owned(),
                install_location: PathBuf::from(parts.next()?),
                status: parts.next()?.to_owned(),
            })
        })
        .collect()
}

/// What `lp-winauth-provider status` says about itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The process had package identity.
    pub identity: bool,
    /// `None` when Windows could not say.
    pub registered: Option<bool>,
    pub enabled: bool,
    /// `None` when the stored pairing cannot be read.
    pub paired: Option<bool>,
    pub app_path: Option<String>,
}

pub fn parse_report(stdout: &str) -> Report {
    let mut report = Report::default();
    for line in stdout.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "package identity" => report.identity = !value.starts_with("none"),
            "registered" => {
                report.registered = if value.starts_with("yes") {
                    Some(true)
                } else if value == "no" {
                    Some(false)
                } else {
                    None
                };
                report.enabled = value == "yes, enabled";
            }
            "paired" => {
                report.paired = match value {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => None,
                }
            }
            "app path" => report.app_path = (value != "not set").then(|| value.to_owned()),
            _ => {}
        }
    }
    report
}

/// The package name in `<Identity Name="...">` of an `AppxManifest.xml`.
pub fn parse_manifest_package(xml: &str) -> Option<String> {
    let identity = &xml[xml.find("<Identity")?..];
    let tag = &identity[..identity.find('>')?];
    let name = &tag[tag.find("Name=\"")? + 6..];
    Some(name[..name.find('"')?].to_owned())
}

/// Whether a webauthn.dll file version (as read from the version resource) has the plug-in API.
pub fn has_plugin_api(version: [u32; 4]) -> bool {
    (version[2], version[3]) >= MIN_WEBAUTHN
}

/// The part of the version worth showing: `26100.7462` (build, revision).
pub fn format_version(version: [u32; 4]) -> String {
    format!("{}.{}", version[2], version[3])
}
