//! Unit tests with a fake machine (no process is started, no registry or file is touched), plus one
//! `#[ignore]`d run against the real OS with a throwaway package (see `real_os` at the end).

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::flows::{
    self, CacheSync, Ctx, DEVELOPER_MODE_OFF, FILES_MISSING, MANIFEST_MISMATCH, Refresh,
    UNSUPPORTED,
};
use super::model::{
    ClientBook, Command, CommandRunner, Identity, Layout, Output, PackageInfo, RunError, Step,
    System, clean_path, has_plugin_api, parse_manifest_package, parse_packages, parse_report,
    query_script, register_script, remove_script, same_path,
};
use super::sync::Debounce;

const BUNDLE: &str = r"C:\Apps\LocalPass\windows-passkeys";
const APP: &str = r"C:\Apps\LocalPass\localpass.exe";
const ALIAS: &str = r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\lp-winauth-provider.exe";
const PAIRING: &str = r"C:\Users\u\AppData\Local\LocalPass\provider\client.bin";
const GOOD_WINDOWS: [u32; 4] = [10, 0, 26100, 7171];

fn layout() -> Layout {
    Layout {
        identity: Identity::shipped(),
        bundle_dir: BUNDLE.into(),
        app_exe: APP.into(),
        alias: ALIAS.into(),
        powershell: r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
        data_dir: r"C:\Users\u\AppData\Local\LocalPass\provider".into(),
    }
}

/// A whole fake PC: PowerShell's package list, the provider behind its alias, the registry, the files,
/// and the app's client list. `ops` records everything that happened, in order.
struct Fake {
    ops: Mutex<Vec<String>>,
    webauthn: Option<[u32; 4]>,
    developer_mode: bool,
    files: Mutex<HashSet<PathBuf>>,
    app_path: Mutex<Option<PathBuf>>,
    package: Mutex<Option<PackageInfo>>,
    registered: Mutex<bool>,
    enabled: bool,
    provider_paired: Mutex<bool>,
    app_clients: Mutex<usize>,
    /// Something other than the app answers the provider's pairing (the app never sees it).
    squatter: Mutex<bool>,
    /// Steps that fail, with what they print.
    failing: Mutex<Vec<(Step, Output)>>,
    /// The `Identity Name` in the bundled manifest.
    manifest_name: String,
}

impl Fake {
    /// A PC with a good Windows, Developer Mode on and the provider files in the bundle folder.
    fn new() -> Self {
        let files = [
            format!(r"{BUNDLE}\lp-winauth-provider.exe"),
            format!(r"{BUNDLE}\AppxManifest.xml"),
        ]
        .map(PathBuf::from)
        .into_iter()
        .collect();
        Self {
            ops: Mutex::default(),
            webauthn: Some(GOOD_WINDOWS),
            developer_mode: true,
            files: Mutex::new(files),
            app_path: Mutex::default(),
            package: Mutex::default(),
            registered: Mutex::default(),
            enabled: false,
            provider_paired: Mutex::default(),
            squatter: Mutex::default(),
            app_clients: Mutex::default(),
            failing: Mutex::default(),
            manifest_name: "LocalPass.Passkeys".into(),
        }
    }

    fn ctx<'a>(&'a self, layout: &'a Layout) -> Ctx<'a> {
        Ctx {
            layout,
            runner: self,
            system: self,
            clients: self,
        }
    }

    fn log(&self, op: &str) {
        self.ops.lock().unwrap().push(op.to_owned());
    }

    fn ops(&self) -> Vec<String> {
        self.ops.lock().unwrap().clone()
    }

    fn position(&self, op: &str) -> usize {
        self.ops()
            .iter()
            .position(|o| o == op)
            .unwrap_or_else(|| panic!("{op} did not run; ran {:?}", self.ops()))
    }

    fn fail(&self, step: Step, message: &str) {
        self.failing.lock().unwrap().push((
            step,
            Output {
                code: Some(1),
                stdout: format!("error: {message}\n"),
                stderr: String::new(),
            },
        ));
    }

    fn registered_package(&self, location: &str) {
        *self.package.lock().unwrap() = Some(PackageInfo {
            full_name: "LocalPass.Passkeys_0.1.0.0_x64__rwhsg7s1qxd2y".into(),
            version: "0.1.0.0".into(),
            install_location: location.into(),
            status: "Ok".into(),
        });
        self.files.lock().unwrap().insert(ALIAS.into());
    }

    /// Set up, turned on, paired on both sides.
    fn ready() -> Self {
        let mut fake = Self::new();
        fake.enabled = true;
        fake.registered_package(BUNDLE);
        *fake.registered.lock().unwrap() = true;
        *fake.provider_paired.lock().unwrap() = true;
        *fake.app_clients.lock().unwrap() = 1;
        *fake.app_path.lock().unwrap() = Some(APP.into());
        fake
    }

    fn status_text(&self) -> String {
        let registered = *self.registered.lock().unwrap();
        let state = match (registered, self.enabled) {
            (false, _) => "no",
            (true, true) => "yes, enabled",
            (true, false) => "yes, disabled",
        };
        let paired = if *self.provider_paired.lock().unwrap() {
            "yes"
        } else {
            "no"
        };
        format!(
            "LocalPass passkey provider, class {{91E9966B-58AE-455A-AB60-4B2FDB66A730}}\npackage identity: LocalPass.Passkeys_rwhsg7s1qxd2y\nregistered: {state}\ndata dir: C:\\x\napp path: {APP}\npaired: {paired}\napp reachable: yes\nvault: Unlocked\ncredential cache: 2 row(s)\n"
        )
    }
}

fn ok(stdout: &str) -> Output {
    Output {
        code: Some(0),
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

impl CommandRunner for Fake {
    fn run(&self, command: &Command) -> Result<Output, RunError> {
        let step = command.step;
        // Whatever starts must be a fixed program with fixed arguments.
        match step {
            Step::Provider(_) => assert_eq!(command.program, Path::new(ALIAS)),
            _ => assert!(command.program.ends_with("powershell.exe")),
        }
        let tag = match step {
            Step::PackageQuery => "ps:query".to_owned(),
            Step::PackageRegister => "ps:register".to_owned(),
            Step::PackageRemove => "ps:remove".to_owned(),
            Step::Provider(mode) => format!("alias:{mode}"),
        };
        self.log(&tag);
        if let Some((_, out)) = self
            .failing
            .lock()
            .unwrap()
            .iter()
            .find(|(s, _)| *s == step)
        {
            return Ok(out.clone());
        }
        Ok(match step {
            Step::PackageQuery => match &*self.package.lock().unwrap() {
                Some(p) => ok(&format!(
                    "package|{}|{}|{}|{}\n",
                    p.full_name,
                    p.version,
                    p.install_location.display(),
                    p.status
                )),
                None => ok(""),
            },
            Step::PackageRegister => {
                // The manifest path travels in the environment, not in the command line.
                let manifest = command
                    .env
                    .iter()
                    .find(|(k, _)| k == "LP_MANIFEST")
                    .unwrap();
                assert_eq!(manifest.1, format!(r"{BUNDLE}\AppxManifest.xml"));
                self.registered_package(BUNDLE);
                ok("ok\n")
            }
            Step::PackageRemove => {
                *self.package.lock().unwrap() = None;
                self.files.lock().unwrap().remove(Path::new(ALIAS));
                ok("ok\n")
            }
            Step::Provider("status") => ok(&self.status_text()),
            Step::Provider("register") => {
                *self.registered.lock().unwrap() = true;
                ok("registered LocalPass (disabled until you enable it in Settings)\n")
            }
            Step::Provider("unregister") => {
                *self.registered.lock().unwrap() = false;
                ok("unregistered\n")
            }
            Step::Provider("pair") => {
                assert_eq!(command.args, ["pair", "--force"]);
                *self.provider_paired.lock().unwrap() = true;
                if !*self.squatter.lock().unwrap() {
                    *self.app_clients.lock().unwrap() = 1;
                }
                ok("paired\n")
            }
            Step::Provider("sync-cache") => {
                ok("cache synced: 1 added, 0 removed, 2 unchanged, 0 failed\n")
            }
            Step::Provider(other) => panic!("unexpected provider mode {other}"),
        })
    }
}

impl System for Fake {
    fn webauthn_version(&self) -> Option<[u32; 4]> {
        self.webauthn
    }

    fn developer_mode(&self) -> bool {
        self.developer_mode
    }

    fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains(path)
    }

    fn manifest_package(&self, _path: &Path) -> Option<String> {
        Some(self.manifest_name.clone())
    }

    fn app_path(&self) -> Option<PathBuf> {
        self.app_path.lock().unwrap().clone()
    }

    fn set_app_path(&self, path: &Path) -> io::Result<()> {
        self.log("reg:set_app_path");
        *self.app_path.lock().unwrap() = Some(path.to_owned());
        Ok(())
    }

    fn clear_app_path(&self) -> io::Result<()> {
        self.log("reg:clear_app_path");
        *self.app_path.lock().unwrap() = None;
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.log(&format!("file:remove {}", path.display()));
        if self.files.lock().unwrap().remove(path) {
            Ok(())
        } else {
            Err(io::ErrorKind::NotFound.into())
        }
    }

    fn sleep(&self, _duration: Duration) {
        self.log("sleep");
    }
}

impl ClientBook for Fake {
    fn has_provider(&self) -> bool {
        *self.app_clients.lock().unwrap() > 0
    }

    fn forget_providers(&self) -> io::Result<usize> {
        self.log("clients:forget");
        Ok(std::mem::take(&mut *self.app_clients.lock().unwrap()))
    }
}

// ---- parsing ----

#[test]
fn the_provider_status_text_is_understood() {
    let text = "LocalPass passkey provider, class {91E9966B-58AE-455A-AB60-4B2FDB66A730}\npackage identity: LocalPass.Passkeys_rwhsg7s1qxd2y\nregistered: yes, enabled\noperation-signing key: ECDSA P-384 (104 bytes)\ndata dir: C:\\Users\\admin\\AppData\\Local\\LocalPass\\provider\npipe profile: none (the real app)\napp path: C:\\app\\target\\release\\localpass.exe\npaired: yes\napp reachable: no (LocalPass is not running)\ncredential cache: 2 row(s)\n";
    let report = parse_report(text);
    assert!(report.identity);
    assert_eq!(report.registered, Some(true));
    assert!(report.enabled);
    assert_eq!(report.paired, Some(true));
    assert_eq!(
        report.app_path.as_deref(),
        Some(r"C:\app\target\release\localpass.exe")
    );

    let off = parse_report(
        "package identity: none (run it through the execution alias to see the registration)\nregistered: unknown (webauthn.dll: x)\napp path: not set\npaired: stored pairing is unreadable (pair again with --force)\n",
    );
    assert!(!off.identity);
    assert_eq!(off.registered, None);
    assert!(!off.enabled);
    assert_eq!(off.paired, None);
    assert_eq!(off.app_path, None);

    let disabled = parse_report("registered: yes, disabled\npaired: no\n");
    assert_eq!(disabled.registered, Some(true));
    assert!(!disabled.enabled);
    assert_eq!(disabled.paired, Some(false));
    assert_eq!(parse_report("registered: no\n").registered, Some(false));
}

#[test]
fn the_package_list_is_parsed() {
    let text = "package|LocalPass.Passkeys_0.1.0.0_x64__rwhsg7s1qxd2y|0.1.0.0|C:\\Users\\O'Brien\\LocalPass\\windows-passkeys|Ok\r\nnoise\r\n";
    let packages = parse_packages(text);
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].version, "0.1.0.0");
    assert_eq!(
        packages[0].install_location,
        Path::new(r"C:\Users\O'Brien\LocalPass\windows-passkeys")
    );
    assert_eq!(packages[0].status, "Ok");
    assert!(parse_packages("").is_empty());
    assert!(parse_packages("error: nope\n").is_empty());
}

#[test]
fn the_windows_version_gate() {
    assert!(has_plugin_api(GOOD_WINDOWS));
    assert!(has_plugin_api([10, 0, 26100, 6725]));
    assert!(!has_plugin_api([10, 0, 26100, 6724]));
    assert!(!has_plugin_api([10, 0, 22631, 4602]));
    assert!(has_plugin_api([10, 0, 27000, 1]));
    // A test binary without a Windows 10 manifest is told 6.2 as major.minor (measured on this PC).
    assert!(has_plugin_api([6, 2, 26100, 7462]));
}

#[test]
fn paths_compare_like_windows_does() {
    assert!(same_path(
        Path::new(r"C:\Apps\LocalPass\"),
        Path::new(r"c:\apps\localpass")
    ));
    assert!(same_path(
        Path::new(r"\\?\C:\Apps\LocalPass"),
        Path::new(r"C:\Apps\LocalPass")
    ));
    assert!(!same_path(
        Path::new(r"C:\Apps\LocalPass"),
        Path::new(r"C:\Apps\LocalPass2")
    ));
    assert_eq!(clean_path(Path::new(r"\\?\C:\x")), Path::new(r"C:\x"));
    assert_eq!(
        clean_path(Path::new(r"\\?\UNC\srv\share")),
        Path::new(r"\\?\UNC\srv\share")
    );
}

#[test]
fn the_message_of_a_failed_program_is_short_and_clean() {
    let out = Output {
        code: Some(1),
        stdout: "something\nerror: Deployment failed with HRESULT: 0x80073CFF\u{7}\n".into(),
        stderr: "noise".into(),
    };
    assert_eq!(out.message(), "Deployment failed with HRESULT: 0x80073CFF");
    let long = Output {
        code: Some(1),
        stdout: "x".repeat(1000),
        stderr: String::new(),
    };
    assert_eq!(long.message().len(), 300);
}

#[test]
fn the_powershell_text_has_no_quotes_and_no_values_spliced_in() {
    let layout = layout();
    for script in [query_script(), register_script(), remove_script()] {
        assert!(
            !script.contains('"'),
            "double quotes would need escaping: {script}"
        );
        assert!(!script.contains("C:\\"), "no path may be part of the text");
        assert!(
            !script.contains("LocalPass.Passkeys"),
            "the name arrives through the environment"
        );
    }
    let command = layout.powershell_command(
        Step::PackageRegister,
        register_script(),
        Duration::from_secs(1),
    );
    assert_eq!(
        command.args[..4],
        ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
    );
    assert_eq!(command.args.len(), 5);
    let env: Vec<&str> = command.env.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(env, ["LP_PACKAGE", "LP_EXE_STEM", "LP_MANIFEST"]);
    assert_eq!(command.env[1].1, "lp-winauth-provider");
    // The modes of the provider are fixed words.
    let status = layout.provider_command("status", &[], Duration::from_secs(1));
    assert_eq!(status.args, ["status"]);
    assert_eq!(status.program, Path::new(ALIAS));
}

#[test]
fn the_shipped_manifest_matches_the_constants() {
    let manifest = include_str!("../../../../crates/lp-winauth/package/AppxManifest.xml");
    let identity = Identity::shipped();
    assert!(manifest.contains(&format!("<Identity Name=\"{}\"", identity.package)));
    assert!(manifest.contains(&format!("Executable=\"{}\"", identity.exe)));
    assert!(manifest.contains(&format!("Alias=\"{}\"", identity.exe)));
    // The class id is a constant of lp-winauth (`provider::CLSID`), which has its own test against this file.
    assert!(manifest.contains("<com:Class Id=\"91E9966B-58AE-455A-AB60-4B2FDB66A730\""));
    assert!(manifest.contains("ProcessorArchitecture=\"x64\""));
    assert!(!manifest.contains('@'));
}

// ---- status ----

#[test]
fn nothing_is_asked_on_a_windows_without_the_plugin_api() {
    let mut fake = Fake::new();
    fake.webauthn = Some([10, 0, 22631, 4602]);
    let layout = layout();
    let status = flows::status(&fake.ctx(&layout));
    assert!(status.available && !status.supported);
    assert_eq!(status.windows_version.as_deref(), Some("22631.4602"));
    assert!(
        fake.ops().is_empty(),
        "no command may run: {:?}",
        fake.ops()
    );
}

#[test]
fn a_fresh_pc_needs_setup() {
    let fake = Fake::new();
    let layout = layout();
    let status = flows::status(&fake.ctx(&layout));
    assert!(status.supported && status.developer_mode && status.provider_exe_found);
    assert!(
        !status.package_registered && !status.registered_in_windows && !status.enabled_in_windows
    );
    assert!(!status.paired && !status.app_path_ok);
    assert_eq!(status.problem, None);
    assert_eq!(
        fake.ops(),
        ["ps:query"],
        "only the package list is asked when there is no package"
    );
}

#[test]
fn a_ready_pc_reports_everything() {
    let fake = Fake::ready();
    let layout = layout();
    let status = flows::status(&fake.ctx(&layout));
    assert!(status.package_registered && status.package_in_bundle);
    assert!(status.registered_in_windows && status.enabled_in_windows);
    assert!(status.paired && status.app_path_ok);
    assert_eq!(status.problem, None);
}

#[test]
fn paired_needs_both_sides() {
    let fake = Fake::ready();
    *fake.app_clients.lock().unwrap() = 0; // forgotten in LocalPass
    let layout = layout();
    assert!(!flows::status(&fake.ctx(&layout)).paired);
    let fake = Fake::ready();
    *fake.provider_paired.lock().unwrap() = false; // the provider lost its secret
    assert!(!flows::status(&fake.ctx(&layout)).paired);
}

#[test]
fn a_package_from_another_folder_is_not_ours() {
    let fake = Fake::ready();
    fake.registered_package(r"C:\dev\target\provider\pkg");
    let layout = layout();
    let status = flows::status(&fake.ctx(&layout));
    assert!(status.package_registered && !status.package_in_bundle);
}

#[test]
fn a_failing_package_list_is_reported_not_hidden() {
    let fake = Fake::new();
    fake.fail(
        Step::PackageQuery,
        "The term 'Get-AppxPackage' is not recognized",
    );
    let layout = layout();
    let status = flows::status(&fake.ctx(&layout));
    assert!(status.problem.unwrap().contains("Get-AppxPackage"));
    assert!(!status.package_registered);
}

// ---- set up ----

#[test]
fn setup_writes_the_app_path_before_it_registers_and_registers_before_the_alias() {
    let fake = Fake::new();
    let layout = layout();
    let status = flows::setup(&fake.ctx(&layout)).unwrap();
    assert!(fake.position("reg:set_app_path") < fake.position("ps:register"));
    assert!(fake.position("ps:register") < fake.position("alias:register"));
    assert_eq!(fake.app_path().as_deref(), Some(Path::new(APP)));
    assert!(status.package_registered && status.package_in_bundle && status.registered_in_windows);
    assert!(!status.enabled_in_windows, "only the user can turn it on");
    assert!(status.app_path_ok);
}

#[test]
fn setup_with_developer_mode_off_runs_nothing() {
    let mut fake = Fake::new();
    fake.developer_mode = false;
    let layout = layout();
    assert_eq!(
        flows::setup(&fake.ctx(&layout)).unwrap_err(),
        DEVELOPER_MODE_OFF
    );
    assert!(
        fake.ops().is_empty(),
        "no command and no registry write may happen: {:?}",
        fake.ops()
    );
}

#[test]
fn setup_refuses_an_unsupported_windows_and_missing_files_without_running_anything() {
    let mut fake = Fake::new();
    fake.webauthn = Some([10, 0, 19045, 1]);
    let layout = layout();
    assert_eq!(flows::setup(&fake.ctx(&layout)).unwrap_err(), UNSUPPORTED);
    assert!(fake.ops().is_empty());

    let fake = Fake::new();
    fake.files.lock().unwrap().clear();
    assert_eq!(flows::setup(&fake.ctx(&layout)).unwrap_err(), FILES_MISSING);
    assert!(fake.ops().is_empty());
}

#[test]
fn setup_refuses_a_manifest_that_is_not_for_this_identity() {
    let mut fake = Fake::new();
    fake.manifest_name = "LocalPass.PasskeysTest".into();
    let layout = layout();
    assert_eq!(
        flows::setup(&fake.ctx(&layout)).unwrap_err(),
        MANIFEST_MISMATCH
    );
    assert!(
        fake.ops().is_empty(),
        "nothing may run or be written: {:?}",
        fake.ops()
    );
}

#[test]
fn the_package_name_is_read_from_the_manifest() {
    let manifest = include_str!("../../../../crates/lp-winauth/package/AppxManifest.xml");
    assert_eq!(
        parse_manifest_package(manifest).as_deref(),
        Some("LocalPass.Passkeys")
    );
    assert_eq!(
        parse_manifest_package(
            "<Package><Identity Publisher=\"x\" Name=\"A.B\" Version=\"1\"/></Package>"
        )
        .as_deref(),
        Some("A.B")
    );
    assert_eq!(parse_manifest_package("<Package></Package>"), None);
    assert_eq!(
        parse_manifest_package("<Package><Identity Version=\"1\"/></Package>"),
        None
    );
}

#[test]
fn setup_twice_does_not_register_the_package_twice() {
    let fake = Fake::new();
    let layout = layout();
    flows::setup(&fake.ctx(&layout)).unwrap();
    fake.ops.lock().unwrap().clear();
    flows::setup(&fake.ctx(&layout)).unwrap();
    assert!(
        !fake.ops().contains(&"ps:register".to_owned()),
        "{:?}",
        fake.ops()
    );
    assert!(
        fake.ops().contains(&"alias:register".to_owned()),
        "details are refreshed"
    );
}

#[test]
fn setup_replaces_a_registration_from_another_folder_unregistering_first() {
    let fake = Fake::ready();
    fake.registered_package(r"C:\old\LocalPass\windows-passkeys");
    let layout = layout();
    flows::setup(&fake.ctx(&layout)).unwrap();
    let order = [
        "alias:unregister",
        "ps:remove",
        "reg:set_app_path",
        "ps:register",
        "alias:register",
    ];
    let positions: Vec<usize> = order.iter().map(|op| fake.position(op)).collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "{:?}",
        fake.ops()
    );
}

#[test]
fn setup_reports_a_developer_mode_failure_in_plain_words() {
    let fake = Fake::new();
    fake.fail(Step::PackageRegister, "Deployment failed with HRESULT: 0x80073CFF, To install this application you need either a Windows developer license or a sideloading-enabled system.");
    let layout = layout();
    assert_eq!(
        flows::setup(&fake.ctx(&layout)).unwrap_err(),
        DEVELOPER_MODE_OFF
    );
    assert!(!fake.ops().contains(&"alias:register".to_owned()));

    let fake = Fake::new();
    fake.fail(
        Step::PackageRegister,
        "Deployment failed with HRESULT: 0x80073D02, the package is in use",
    );
    let message = flows::setup(&fake.ctx(&layout)).unwrap_err();
    assert!(message.contains("0x80073D02"), "{message}");
}

#[test]
fn setup_can_be_resumed_after_the_authenticator_step_failed() {
    let fake = Fake::new();
    fake.fail(Step::Provider("register"), "register failed, hr=0x80090027");
    let layout = layout();
    assert!(
        flows::setup(&fake.ctx(&layout))
            .unwrap_err()
            .contains("0x80090027")
    );
    fake.failing.lock().unwrap().clear();
    fake.ops.lock().unwrap().clear();
    let status = flows::setup(&fake.ctx(&layout)).unwrap();
    assert!(status.registered_in_windows);
    assert!(
        !fake.ops().contains(&"ps:register".to_owned()),
        "the package was already there"
    );
}

// ---- pair ----

#[test]
fn pair_forgets_the_stale_record_then_pairs_with_force() {
    let fake = Fake::ready();
    *fake.provider_paired.lock().unwrap() = false;
    let layout = layout();
    let status = flows::pair(&fake.ctx(&layout)).unwrap();
    assert!(fake.position("clients:forget") < fake.position("alias:pair"));
    assert!(status.paired);
}

#[test]
fn pair_does_nothing_when_already_paired_and_needs_a_set_up_first() {
    let fake = Fake::ready();
    let layout = layout();
    flows::pair(&fake.ctx(&layout)).unwrap();
    assert!(!fake.ops().contains(&"alias:pair".to_owned()));

    let fake = Fake::new();
    assert_eq!(
        flows::pair(&fake.ctx(&layout)).unwrap_err(),
        "Set up Windows passkeys first."
    );
    assert!(!fake.ops().contains(&"alias:pair".to_owned()));
}

/// RC-1: the provider's `pair` exits 0 although the app never approved it.
#[test]
fn a_pairing_the_app_never_saw_is_an_error_and_is_undone() {
    let fake = Fake::ready();
    *fake.provider_paired.lock().unwrap() = false;
    *fake.squatter.lock().unwrap() = true;
    let layout = layout();
    let err = flows::pair(&fake.ctx(&layout)).unwrap_err();
    assert!(err.contains("did not see this pairing"), "{err}");
    assert!(
        fake.ops().iter().any(|op| op.starts_with("file:remove")),
        "the provider's secret is removed: {:?}",
        fake.ops()
    );
}

#[test]
fn a_refused_pairing_is_an_error() {
    let fake = Fake::ready();
    *fake.provider_paired.lock().unwrap() = false;
    fake.fail(Step::Provider("pair"), "pairing failed: denied");
    let layout = layout();
    assert!(
        flows::pair(&fake.ctx(&layout))
            .unwrap_err()
            .contains("denied")
    );
}

// ---- remove ----

#[test]
fn remove_unregisters_from_windows_before_removing_the_package() {
    let fake = Fake::ready();
    let layout = layout();
    let status = flows::remove(&fake.ctx(&layout), false).unwrap();
    assert!(fake.position("alias:unregister") < fake.position("ps:remove"));
    assert!(fake.position("ps:remove") < fake.position("reg:clear_app_path"));
    assert!(!status.package_registered && !status.registered_in_windows && !status.app_path_ok);
    assert!(
        !fake.ops().iter().any(|op| op.starts_with("file:remove")),
        "the pairing stays by default"
    );
    assert!(!fake.ops().contains(&"clients:forget".to_owned()));
}

#[test]
fn remove_leaves_the_package_when_windows_would_not_unregister() {
    let fake = Fake::ready();
    fake.fail(
        Step::Provider("unregister"),
        "unregister failed, hr=0x80070005",
    );
    let layout = layout();
    let error = flows::remove(&fake.ctx(&layout), true).unwrap_err();
    assert!(
        error.contains("not removed") && error.contains("0x80070005"),
        "{error}"
    );
    assert!(
        !fake.ops().contains(&"ps:remove".to_owned()),
        "never leave the Windows entry behind"
    );
    assert!(fake.package.lock().unwrap().is_some());
    assert!(fake.app_path().is_some(), "nothing else is undone either");
}

#[test]
fn remove_without_the_alias_removes_nothing() {
    let fake = Fake::ready();
    fake.files.lock().unwrap().remove(Path::new(ALIAS));
    let layout = layout();
    assert!(
        flows::remove(&fake.ctx(&layout), false)
            .unwrap_err()
            .contains("Nothing was removed")
    );
    assert!(!fake.ops().contains(&"ps:remove".to_owned()));
}

#[test]
fn remove_can_forget_the_pairing_on_both_sides() {
    let fake = Fake::ready();
    fake.files.lock().unwrap().insert(PAIRING.into());
    let layout = layout();
    let status = flows::remove(&fake.ctx(&layout), true).unwrap();
    assert!(fake.ops().contains(&"clients:forget".to_owned()));
    assert!(fake.ops().contains(&format!("file:remove {PAIRING}")));
    assert!(!status.paired);
}

#[test]
fn remove_when_nothing_is_set_up_is_fine() {
    let fake = Fake::new();
    let layout = layout();
    flows::remove(&fake.ctx(&layout), true).unwrap();
    assert!(!fake.ops().contains(&"alias:unregister".to_owned()));
    assert!(!fake.ops().contains(&"ps:remove".to_owned()));
}

// ---- start-up refresh of the app path ----

#[test]
fn the_app_path_follows_the_app_when_the_package_is_ours() {
    let fake = Fake::ready();
    *fake.app_path.lock().unwrap() = Some(r"C:\Old\localpass.exe".into());
    let layout = layout();
    assert_eq!(
        flows::refresh_app_path(&fake.ctx(&layout)),
        Ok(Refresh::Updated)
    );
    assert_eq!(fake.app_path().as_deref(), Some(Path::new(APP)));
    assert_eq!(
        flows::refresh_app_path(&fake.ctx(&layout)),
        Ok(Refresh::Unchanged)
    );
}

#[test]
fn the_app_path_is_left_alone_for_a_development_copy_and_when_never_set_up() {
    // A package registered from the repository's script, and this (other) copy of the app running.
    let fake = Fake::ready();
    fake.registered_package(r"C:\dev\target\provider\pkg");
    *fake.app_path.lock().unwrap() = Some(r"C:\dev\target\release\localpass.exe".into());
    let layout = layout();
    assert_eq!(
        flows::refresh_app_path(&fake.ctx(&layout)),
        Ok(Refresh::NotOurs)
    );
    assert_eq!(
        fake.app_path().as_deref(),
        Some(Path::new(r"C:\dev\target\release\localpass.exe"))
    );

    let fake = Fake::new();
    assert_eq!(
        flows::refresh_app_path(&fake.ctx(&layout)),
        Ok(Refresh::NotOurs)
    );
    assert!(
        fake.ops().is_empty(),
        "no alias, so not even PowerShell starts: {:?}",
        fake.ops()
    );
}

// ---- cache sync ----

#[test]
fn the_cache_sync_runs_only_when_registered_enabled_and_paired() {
    let layout = layout();
    let fake = Fake::ready();
    assert_eq!(
        flows::sync_cache(&fake.ctx(&layout)),
        Ok(CacheSync::Done(
            "cache synced: 1 added, 0 removed, 2 unchanged, 0 failed".into()
        ))
    );
    assert!(fake.position("alias:status") < fake.position("alias:sync-cache"));

    let mut fake = Fake::ready();
    fake.enabled = false;
    assert!(matches!(
        flows::sync_cache(&fake.ctx(&layout)),
        Ok(CacheSync::Skipped(_))
    ));
    assert!(!fake.ops().contains(&"alias:sync-cache".to_owned()));

    let fake = Fake::ready();
    *fake.provider_paired.lock().unwrap() = false;
    assert!(matches!(
        flows::sync_cache(&fake.ctx(&layout)),
        Ok(CacheSync::Skipped(_))
    ));

    let fake = Fake::ready();
    *fake.app_clients.lock().unwrap() = 0;
    assert!(matches!(
        flows::sync_cache(&fake.ctx(&layout)),
        Ok(CacheSync::Skipped(_))
    ));

    let fake = Fake::new();
    assert!(matches!(
        flows::sync_cache(&fake.ctx(&layout)),
        Ok(CacheSync::Skipped(_))
    ));
    assert!(fake.ops().is_empty(), "no alias, nothing runs");
}

#[test]
fn a_failed_cache_sync_is_an_error_for_the_log() {
    let fake = Fake::ready();
    fake.fail(
        Step::Provider("sync-cache"),
        "nothing changed: the app is locked",
    );
    let layout = layout();
    assert!(
        flows::sync_cache(&fake.ctx(&layout))
            .unwrap_err()
            .contains("locked")
    );
}

// ---- debounce ----

#[test]
fn a_sync_is_due_a_quiet_period_after_the_last_change() {
    let quiet = Duration::from_secs(3);
    let t0 = Instant::now();
    let mut d = Debounce::new(quiet);
    assert_eq!(d.wait(t0), None, "nothing happened");
    assert!(!d.take(t0));

    d.event(t0);
    assert_eq!(d.wait(t0), Some(quiet));
    d.event(t0 + Duration::from_secs(2)); // a burst keeps pushing it back
    assert_eq!(d.wait(t0 + Duration::from_secs(2)), Some(quiet));
    assert!(
        !d.take(t0 + Duration::from_secs(4)),
        "only 2 s since the last change"
    );
    assert_eq!(
        d.wait(t0 + Duration::from_secs(4)),
        Some(Duration::from_secs(1))
    );
    assert!(d.take(t0 + Duration::from_secs(5)));
    assert_eq!(
        d.wait(t0 + Duration::from_secs(5)),
        None,
        "the change was taken"
    );
}

#[test]
fn only_one_sync_runs_at_a_time_and_a_change_during_a_run_gets_its_own_run() {
    let quiet = Duration::from_secs(3);
    let t0 = Instant::now();
    let mut d = Debounce::new(quiet);
    d.event(t0);
    assert!(d.take(t0 + quiet));
    // While it runs: more changes arrive, nothing else may start, and nobody is told to wait.
    d.event(t0 + quiet + Duration::from_secs(1));
    assert!(!d.take(t0 + Duration::from_secs(60)));
    assert_eq!(d.wait(t0 + Duration::from_secs(60)), None);
    d.finish();
    // Afterwards the new change is still due, after its own quiet period.
    assert_eq!(d.wait(t0 + Duration::from_secs(60)), Some(Duration::ZERO));
    assert!(d.take(t0 + Duration::from_secs(60)));
    d.finish();
    assert_eq!(d.wait(t0 + Duration::from_secs(61)), None);
}

#[test]
fn a_locked_vault_drops_the_pending_run() {
    let t0 = Instant::now();
    let mut d = Debounce::new(Duration::from_secs(3));
    d.event(t0);
    d.clear();
    assert_eq!(d.wait(t0 + Duration::from_secs(10)), None);
    assert!(!d.take(t0 + Duration::from_secs(10)));
}

// ---- the real OS, with a throwaway package ----

/// Set up -> status -> remove through the REAL code against the REAL Windows, with a throwaway package
/// that cannot touch the real registration (`LocalPass.PasskeysTest`, its own exe name, class id, data
/// folder and registry key). Run by hand:
///
/// ```text
/// cargo build --release -p lp-winauth --bin lp-winauth-provider --features test-identity   (own target dir)
/// set LP_TEST_PROVIDER_EXE=<that exe>
/// set LOCALPASS_PROVIDER_PACKAGE=LocalPass.PasskeysTest  LOCALPASS_PROVIDER_EXE=lp-winauth-provider-test.exe
/// set LOCALPASS_PROVIDER_REGKEY=Software\LocalPass\ProviderTest  LOCALPASS_PROVIDER_DATA=provider-test
/// cargo test -p localpass --lib real_os -- --ignored --nocapture
/// ```
#[test]
#[ignore = "changes this PC's package registrations; see the doc comment"]
fn real_os() {
    use super::model::{BUNDLE_FOLDER, MANIFEST_FILE};
    use super::real::{RealRunner, RealSystem};

    let identity = Identity::current();
    assert!(
        !identity.is_shipped(),
        "refusing to run against the real package identity"
    );
    assert_ne!(identity.package, "LocalPass.Passkeys");
    assert_ne!(identity.exe, "lp-winauth-provider.exe");
    let source_exe =
        PathBuf::from(std::env::var_os("LP_TEST_PROVIDER_EXE").expect("LP_TEST_PROVIDER_EXE"));

    // The test bundle: the throwaway exe under its test name, the shipped manifest with the test
    // identity substituted, the logos.
    let root = std::env::temp_dir().join(format!("lp-wp-test-{}", std::process::id()));
    let bundle = root.join(BUNDLE_FOLDER);
    std::fs::create_dir_all(bundle.join("Assets")).unwrap();
    std::fs::copy(&source_exe, bundle.join(&identity.exe)).unwrap();
    let icons = Path::new(env!("CARGO_MANIFEST_DIR")).join("icons");
    for logo in ["StoreLogo", "Square44x44Logo", "Square150x150Logo"] {
        std::fs::copy(
            icons.join(format!("{logo}.png")),
            bundle.join("Assets").join(format!("{logo}.png")),
        )
        .unwrap();
    }
    let clsid = std::process::Command::new(&source_exe)
        .arg("clsid")
        .output()
        .unwrap();
    let clsid = String::from_utf8(clsid.stdout)
        .unwrap()
        .trim()
        .trim_matches(['{', '}'])
        .to_owned();
    assert_ne!(
        clsid, "91E9966B-58AE-455A-AB60-4B2FDB66A730",
        "that is the real class id"
    );
    let shipped = include_str!("../../../../crates/lp-winauth/package/AppxManifest.xml");
    let manifest = shipped
        .replace("91E9966B-58AE-455A-AB60-4B2FDB66A730", &clsid)
        .replace(
            "Name=\"LocalPass.Passkeys\"",
            &format!("Name=\"{}\"", identity.package),
        )
        .replace("lp-winauth-provider.exe", &identity.exe)
        .replace(
            "DisplayName=\"LocalPass\"",
            "DisplayName=\"LocalPass (test)\"",
        );
    std::fs::write(bundle.join(MANIFEST_FILE), manifest).unwrap();

    let layout = Layout::new(identity.clone(), bundle, std::env::current_exe().unwrap());
    assert_ne!(
        layout.alias,
        Layout::new(Identity::shipped(), PathBuf::new(), PathBuf::new()).alias
    );
    let system = RealSystem {
        subkey: identity.registry_subkey.clone(),
    };
    let clients = NoClients;
    let ctx = Ctx {
        layout: &layout,
        runner: &RealRunner,
        system: &system,
        clients: &clients,
    };

    let say = |label: &str, status: &super::Status| println!("{label}: {status:?}");
    let before = flows::status(&ctx);
    say("before", &before);
    assert!(before.supported, "this PC cannot run the test");
    assert!(before.developer_mode, "Developer Mode is off");
    assert!(
        !before.package_registered,
        "the throwaway package is already there: remove it first"
    );

    let result = flows::setup(&ctx);
    // Always clean up, also when the set-up failed half-way.
    let during = flows::status(&ctx);
    say("after setup", &during);
    println!(
        "Windows' passkey providers while the throwaway one is registered:\n{}",
        fido_plugins()
    );
    let removed = flows::remove(&ctx, true);
    let after = flows::status(&ctx);
    say("after remove", &after);
    let _ = std::fs::remove_dir_all(&root);
    // The throwaway provider's own log folder (never the real one).
    assert_ne!(layout.data_dir.file_name().unwrap(), "provider");
    let _ = std::fs::remove_dir_all(&layout.data_dir);

    let set_up = result.expect("setup");
    assert!(set_up.package_registered && set_up.package_in_bundle && set_up.registered_in_windows);
    assert!(!set_up.enabled_in_windows && set_up.app_path_ok);
    assert_eq!(during.problem, None);
    removed.expect("remove");
    assert!(!after.package_registered && !after.registered_in_windows && !after.app_path_ok);
    assert!(!layout.alias.exists() && std::fs::symlink_metadata(&layout.alias).is_err());
}

/// Read-only: the status of the REAL registration of this PC (it starts PowerShell for the package list and
/// the provider's `status` mode through its alias, and changes nothing). `paired` is false here because the
/// test has no app to ask for its client list.
///
/// ```text
/// cargo test -p localpass --lib real_status_readonly -- --ignored --nocapture
/// ```
#[test]
#[ignore = "reads this PC's real registration; read-only"]
fn real_status_readonly() {
    use super::real::{RealRunner, RealSystem};

    let identity = Identity::shipped();
    let layout = Layout::new(
        identity.clone(),
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("windows-passkeys"),
        std::env::current_exe().unwrap(),
    );
    let system = RealSystem {
        subkey: identity.registry_subkey,
    };
    let ctx = Ctx {
        layout: &layout,
        runner: &RealRunner,
        system: &system,
        clients: &NoClients,
    };
    let status = flows::status(&ctx);
    println!("{status:#?}");
    assert!(status.available && status.supported);
}

/// Name and State of every passkey provider Windows knows for this user (`reg query`, no admin rights).
fn fido_plugins() -> String {
    let query = |value: &str| {
        let out = std::process::Command::new("reg")
            .args([r"query", r"HKLM\SOFTWARE\Microsoft\FIDO", "/s", "/v", value])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let mut lines = Vec::new();
    for value in ["Name", "State"] {
        for line in query(value).lines().map(str::trim) {
            if line.starts_with(value) || line.contains(r"\Plugins\{") {
                lines.push(format!("  {line}"));
            }
        }
    }
    lines.join("\n")
}

/// The real-OS test has no app: no client list.
struct NoClients;

impl ClientBook for NoClients {
    fn has_provider(&self) -> bool {
        false
    }

    fn forget_providers(&self) -> io::Result<usize> {
        Ok(0)
    }
}
