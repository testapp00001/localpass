//! Status, set up, pair, remove and the cache sync, written against the traits in `model` so the order of
//! the steps can be tested with fakes. Errors are plain sentences for the Settings screen.

use std::io;
use std::time::Duration;

use super::Status;
use super::model::{
    ClientBook, Command, CommandRunner, Layout, Output, PackageInfo, RunError, Step, System,
    format_version, has_plugin_api, parse_packages, parse_report, query_script, register_script,
    remove_script, same_path,
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const REGISTER_TIMEOUT: Duration = Duration::from_secs(120);
const REMOVE_TIMEOUT: Duration = Duration::from_secs(120);
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(45);
/// The provider waits up to 3 minutes for the user to approve the code.
const PAIR_TIMEOUT: Duration = Duration::from_secs(200);
const SYNC_TIMEOUT: Duration = Duration::from_secs(60);
const ALIAS_WAIT: Duration = Duration::from_secs(10);

pub const UNSUPPORTED: &str = "This version of Windows cannot add passkey providers. It needs Windows 11 24H2 with the September 2025 update or later (build 26100.6725).";
pub const DEVELOPER_MODE_OFF: &str = "Developer Mode is off. Turn it on in Windows Settings > System > For developers, then try again. LocalPass is not signed with a certificate that Windows trusts, so Windows only installs its passkey provider in that mode.";
pub const FILES_MISSING: &str = "The passkey provider is not part of this copy of LocalPass (the windows-passkeys folder next to localpass.exe is missing or empty). Install LocalPass with its installer.";
pub const MANIFEST_MISMATCH: &str = "The package manifest in the windows-passkeys folder is for a different package than this copy of LocalPass installs. Install LocalPass with its installer.";
const NOT_SET_UP: &str = "Set up Windows passkeys first.";

/// Everything the flows touch.
pub struct Ctx<'a> {
    pub layout: &'a Layout,
    pub runner: &'a dyn CommandRunner,
    pub system: &'a dyn System,
    pub clients: &'a dyn ClientBook,
}

impl Ctx<'_> {
    fn run(&self, command: &Command) -> Result<Output, String> {
        tracing::debug!(step = %command.step.label(), "windows passkeys: running");
        self.runner.run(command).map_err(|e| match e {
            RunError::TimedOut => {
                format!("Windows did not answer in time ({}).", command.step.label())
            }
            RunError::Spawn(why) => format!("Could not run {}: {why}", command.step.label()),
        })
    }

    fn provider(
        &self,
        mode: &'static str,
        extra: &[&str],
        timeout: Duration,
    ) -> Result<Output, String> {
        self.run(&self.layout.provider_command(mode, extra, timeout))
    }

    fn powershell(&self, step: Step, script: String, timeout: Duration) -> Result<Output, String> {
        self.run(&self.layout.powershell_command(step, script, timeout))
    }

    fn alias_exists(&self) -> bool {
        self.system.exists(&self.layout.alias)
    }
}

/// What can be known without running anything.
pub fn precheck(ctx: &Ctx<'_>) -> Status {
    let version = ctx.system.webauthn_version();
    Status {
        available: true,
        supported: version.is_some_and(has_plugin_api),
        windows_version: version.map(format_version),
        developer_mode: ctx.system.developer_mode(),
        provider_exe_found: ctx.system.exists(&ctx.layout.provider_exe())
            && ctx.system.exists(&ctx.layout.manifest()),
        app_path_ok: ctx
            .system
            .app_path()
            .is_some_and(|p| same_path(&p, &ctx.layout.app_exe)),
        ..Status::default()
    }
}

/// The whole status: the package through PowerShell, the rest through the provider's own `status` mode.
pub fn status(ctx: &Ctx<'_>) -> Status {
    let mut status = precheck(ctx);
    if !status.supported {
        return status;
    }
    let package = match query_package(ctx) {
        Ok(package) => package,
        Err(why) => {
            status.problem = Some(why);
            return status;
        }
    };
    let Some(package) = package else {
        return status;
    };
    status.package_registered = true;
    status.package_in_bundle = same_path(&package.install_location, &ctx.layout.bundle_dir);
    if !ctx.alias_exists() {
        status.problem = Some("Windows has no command for the installed passkey provider. Remove it and set it up again.".into());
        return status;
    }
    match ctx.provider("status", &[], PROVIDER_TIMEOUT) {
        Ok(out) if out.success() => {
            let report = parse_report(&out.stdout);
            if report.identity {
                status.registered_in_windows = report.registered == Some(true);
                status.enabled_in_windows = status.registered_in_windows && report.enabled;
                status.paired = report.paired == Some(true) && ctx.clients.has_provider();
            } else {
                status.problem =
                    Some("The passkey provider started without its package identity.".into());
            }
        }
        Ok(out) => {
            status.problem = Some(format!(
                "The passkey provider did not answer: {}",
                out.message()
            ));
        }
        Err(why) => status.problem = Some(why),
    }
    status
}

fn query_package(ctx: &Ctx<'_>) -> Result<Option<PackageInfo>, String> {
    let out = ctx.powershell(Step::PackageQuery, query_script(), QUERY_TIMEOUT)?;
    if !out.success() {
        return Err(format!(
            "Windows would not list the installed packages: {}",
            out.message()
        ));
    }
    Ok(parse_packages(&out.stdout).into_iter().next())
}

/// Registers LocalPass in Windows' list of passkey providers (switched off: the user turns it on in
/// Windows settings, with Windows Hello). Repeating it is fine.
pub fn setup(ctx: &Ctx<'_>) -> Result<Status, String> {
    // Checks first, before any command runs.
    let pre = precheck(ctx);
    if !pre.supported {
        return Err(UNSUPPORTED.into());
    }
    if !pre.developer_mode {
        return Err(DEVELOPER_MODE_OFF.into());
    }
    if !pre.provider_exe_found {
        return Err(FILES_MISSING.into());
    }
    // Registering a manifest for another identity than the one this app manages would leave a package
    // the app can neither see nor remove (or replace a registration that is not its own).
    if ctx
        .system
        .manifest_package(&ctx.layout.manifest())
        .as_deref()
        != Some(ctx.layout.identity.package.as_str())
    {
        return Err(MANIFEST_MISMATCH.into());
    }

    let mut package = query_package(ctx)?;
    if let Some(found) = &package
        && !same_path(&found.install_location, &ctx.layout.bundle_dir)
    {
        // Registered from somewhere else (the app was moved or reinstalled): one identity cannot be
        // registered from two folders, so the old registration goes first. Windows' switch resets.
        tracing::info!(from = %found.install_location.display(), "windows passkeys: replacing a registration from another folder");
        unregister_and_remove(ctx)?;
        package = None;
    }

    // Where the app is, before anything can ask the provider to start it.
    ctx.system
        .set_app_path(&ctx.layout.app_exe)
        .map_err(|e| format!("Could not save where LocalPass is: {e}"))?;

    if package.is_none() {
        let out = ctx.powershell(Step::PackageRegister, register_script(), REGISTER_TIMEOUT)?;
        if !out.success() {
            return Err(register_failure(&out));
        }
    }
    wait_for_alias(ctx)?;

    let out = ctx.provider("register", &[], PROVIDER_TIMEOUT)?;
    if !out.success() {
        return Err(format!(
            "Windows would not add LocalPass to its passkey providers: {}",
            out.message()
        ));
    }
    tracing::info!("windows passkeys: set up");
    Ok(status(ctx))
}

fn register_failure(out: &Output) -> String {
    let message = out.message();
    let lower = message.to_ascii_lowercase();
    // 0x80073CFF: sideloading is off / needs a developer licence.
    if lower.contains("0x80073cff")
        || lower.contains("developer mode")
        || lower.contains("developer license")
    {
        return DEVELOPER_MODE_OFF.into();
    }
    format!("Windows would not register the passkey provider: {message}")
}

fn wait_for_alias(ctx: &Ctx<'_>) -> Result<(), String> {
    let step = Duration::from_millis(250);
    let mut waited = Duration::ZERO;
    while !ctx.alias_exists() {
        if waited >= ALIAS_WAIT {
            return Err("The passkey provider was registered, but Windows did not create its command in time. Try again in a moment.".into());
        }
        ctx.system.sleep(step);
        waited += step;
    }
    Ok(())
}

/// Takes LocalPass out of Windows' passkey list first (that needs the package's identity), then removes
/// the package. If the first step fails nothing is removed, so it can be tried again.
fn unregister_and_remove(ctx: &Ctx<'_>) -> Result<(), String> {
    if !ctx.alias_exists() {
        return Err("Windows has no command for the installed passkey provider, so LocalPass cannot take itself out of the passkey list. Nothing was removed.".into());
    }
    let out = ctx.provider("unregister", &[], PROVIDER_TIMEOUT)?;
    if !out.success() {
        return Err(format!(
            "LocalPass could not leave Windows' passkey list ({}). The package was not removed, so you can try again.",
            out.message()
        ));
    }
    let out = ctx.powershell(Step::PackageRemove, remove_script(), REMOVE_TIMEOUT)?;
    if !out.success() {
        return Err(format!(
            "Windows would not remove the passkey provider package: {}",
            out.message()
        ));
    }
    Ok(())
}

/// Removes everything set up: the entry in Windows' passkey list, the package, the app path. With
/// `forget_pairing` also the pairing on both sides.
pub fn remove(ctx: &Ctx<'_>, forget_pairing: bool) -> Result<Status, String> {
    if query_package(ctx)?.is_some() {
        unregister_and_remove(ctx)?;
    }
    ctx.system
        .clear_app_path()
        .map_err(|e| format!("Could not clear where LocalPass is: {e}"))?;
    if forget_pairing {
        forget_pairing_files(ctx)?;
    }
    tracing::info!(forget_pairing, "windows passkeys: removed");
    Ok(status(ctx))
}

fn forget_pairing_files(ctx: &Ctx<'_>) -> Result<(), String> {
    ctx.clients
        .forget_providers()
        .map_err(|e| format!("Could not forget the pairing: {e}"))?;
    match ctx.system.remove_file(&ctx.layout.pairing_file()) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => {
            Err(format!("Could not delete the provider's pairing file: {e}"))
        }
        _ => Ok(()),
    }
}

/// Pairs the provider with this app: the provider's `pair` mode asks the app, which shows its usual
/// approval dialog. Returns when the user answered (or after the provider's 3 minute wait).
pub fn pair(ctx: &Ctx<'_>) -> Result<Status, String> {
    let current = status(ctx);
    if let Some(problem) = current.problem {
        return Err(problem);
    }
    if !current.package_registered || !current.registered_in_windows {
        return Err(NOT_SET_UP.into());
    }
    if current.paired {
        return Ok(current);
    }
    // An old record of the provider (its own copy of the secret is gone or unreadable) would only be
    // clutter next to the new one.
    ctx.clients
        .forget_providers()
        .map_err(|e| format!("Could not forget the old pairing: {e}"))?;
    let out = ctx.provider("pair", &["--force"], PAIR_TIMEOUT)?;
    if !out.success() {
        return Err(format!("Pairing did not finish: {}", out.message()));
    }
    if !ctx.clients.has_provider() {
        // The provider says it paired, but this app never approved it: whatever answered on the
        // pipe was not this app (RC-1). Drop the provider's secret so it cannot be used.
        let _ = forget_pairing_files(ctx);
        tracing::warn!("windows passkeys: the provider paired but this app did not see it");
        return Err(
            "LocalPass did not see this pairing, so something else answered the provider. \
             The pairing was removed. Restart LocalPass and try again."
                .into(),
        );
    }
    tracing::info!("windows passkeys: paired");
    Ok(status(ctx))
}

/// What a start-up refresh of the app path did.
#[derive(Debug, PartialEq, Eq)]
pub enum Refresh {
    /// No package, or it is registered from another folder (a development copy): leave the path alone.
    NotOurs,
    Unchanged,
    Updated,
}

/// When the package registered from this app's own folder is there, makes sure the registry tells the
/// provider where this app is now (the app may have been moved or reinstalled). A running copy that
/// is not the one the package belongs to never rewrites it.
pub fn refresh_app_path(ctx: &Ctx<'_>) -> Result<Refresh, String> {
    if !ctx.alias_exists() {
        return Ok(Refresh::NotOurs);
    }
    let Some(package) = query_package(ctx)? else {
        return Ok(Refresh::NotOurs);
    };
    if !same_path(&package.install_location, &ctx.layout.bundle_dir) {
        return Ok(Refresh::NotOurs);
    }
    if ctx
        .system
        .app_path()
        .is_some_and(|p| same_path(&p, &ctx.layout.app_exe))
    {
        return Ok(Refresh::Unchanged);
    }
    ctx.system
        .set_app_path(&ctx.layout.app_exe)
        .map_err(|e| format!("Could not save where LocalPass is: {e}"))?;
    Ok(Refresh::Updated)
}

/// What one cache sync did.
#[derive(Debug, PartialEq, Eq)]
pub enum CacheSync {
    /// Not applicable now (not set up, not turned on, not paired); the reason, for the log.
    Skipped(&'static str),
    /// The provider's summary line.
    Done(String),
}

/// Asks the provider to mirror the vault's passkeys into Windows' credential cache. Needs the vault
/// unlocked (the caller checks): the provider lists them through the app. Only runs when LocalPass is
/// registered, turned on in Windows and paired.
pub fn sync_cache(ctx: &Ctx<'_>) -> Result<CacheSync, String> {
    if !ctx.alias_exists() {
        return Ok(CacheSync::Skipped("the passkey provider is not set up"));
    }
    let out = ctx.provider("status", &[], PROVIDER_TIMEOUT)?;
    if !out.success() {
        return Err(format!("status failed: {}", out.message()));
    }
    let report = parse_report(&out.stdout);
    if !report.identity || report.registered != Some(true) {
        return Ok(CacheSync::Skipped(
            "LocalPass is not in Windows' passkey list",
        ));
    }
    if !report.enabled {
        return Ok(CacheSync::Skipped("LocalPass is not turned on in Windows"));
    }
    if report.paired != Some(true) || !ctx.clients.has_provider() {
        return Ok(CacheSync::Skipped("the provider is not paired"));
    }
    let out = ctx.provider("sync-cache", &[], SYNC_TIMEOUT)?;
    if out.success() {
        Ok(CacheSync::Done(out.message()))
    } else {
        Err(format!("sync-cache failed: {}", out.message()))
    }
}
