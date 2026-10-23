//! The LocalPass Windows passkey provider.
//!
//! Windows starts it as a COM server (`-ActivateAuthenticator -Embedding`); the other modes are for the
//! install scripts and for people, and need package identity, so run them through the execution alias
//! (`%LOCALAPPDATA%\Microsoft\WindowsApps\lp-winauth-provider.exe`):
//!
//! ```text
//! register      add LocalPass to Windows' passkey providers (disabled until enabled in Settings), or update it
//! unregister    remove it (do this before removing the package)
//! status        what is registered, paired and reachable
//! pair [--force]  pair with the running app: prints a 6-digit code to approve in LocalPass
//! sync-cache    mirror the vault's passkeys into Windows' credential cache
//! cache-selftest  add, read and remove one fake cache row (checks the OS cache calls; leaves nothing behind)
//! clsid         print the COM class id (works without identity)
//! ```
//!
//! It never enables itself (that is the user's Windows Hello check in Settings).

#[cfg(not(windows))]
fn main() {
    eprintln!("the LocalPass passkey provider only runs on Windows");
}

#[cfg(windows)]
fn main() -> std::process::ExitCode {
    imp::main()
}

#[cfg(windows)]
mod imp {
    use std::io::Write as _;
    use std::process::ExitCode;

    use lp_winauth::cache::{CacheApi, OsCache, SyncOutcome};
    use lp_winauth::com;
    use lp_winauth::log::{self, hr, package_family_name};
    use lp_winauth::plog;
    use lp_winauth::provider::{
        CLSID, CLSID_TEXT, Provider, Registered, data_dir, load_api, register, unregister,
    };
    use lp_winauth::relay::Probe;
    use lp_winauth::store::{self, Store};
    use lp_winauth::sys::{CredentialRow, NTE_NOT_FOUND, STATE_DISABLED, STATE_ENABLED};
    use lp_winauth::verify::curve_name;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

    pub fn main() -> ExitCode {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let serve = args.iter().any(|a| {
            a.eq_ignore_ascii_case("-ActivateAuthenticator") || a.eq_ignore_ascii_case("-Embedding")
        });
        let mode = if serve {
            "serve"
        } else {
            args.first().map_or("help", String::as_str)
        };
        if mode == "clsid" {
            println!("{CLSID_TEXT}");
            return ExitCode::SUCCESS;
        }
        log::init(&data_dir(), false);
        let exe = std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        plog!("start mode={mode} exe={exe}");
        // SAFETY: COM initialisation for the process, multithreaded as the plugin interface needs.
        let init = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        plog!("CoInitializeEx(MTA) hr={}", hr(init));
        match mode {
            "serve" => com::serve(build_provider, sync_at_start),
            "register" => register_mode(),
            "unregister" => unregister_mode(),
            "status" => status_mode(),
            "pair" => pair_mode(args.iter().any(|a| a == "--force")),
            "sync-cache" => sync_cache_mode(),
            "cache-selftest" => cache_selftest_mode(),
            "launch-test" => launch_test_mode(&args[1..]),
            _ => {
                println!(
                    "usage: lp-winauth-provider register | unregister | status | pair [--force] | sync-cache | cache-selftest | clsid"
                );
                ExitCode::from(2)
            }
        }
    }

    fn build_provider() -> Result<Provider, String> {
        Provider::open(Store::open_default())
    }

    /// Runs once when the COM server starts: Windows starts it on demand, and this is a cheap moment to
    /// bring its credential cache up to date. It never starts the app and never prompts.
    fn sync_at_start() {
        match com::provider() {
            Ok(provider) => {
                let outcome = provider.sync_cache();
                plog!("cache sync at start: {outcome:?}");
            }
            Err(why) => plog!("cache sync at start: provider unavailable: {why}"),
        }
    }

    fn register_mode() -> ExitCode {
        let api = match load_api() {
            Ok(api) => api,
            Err(why) => return fail(&format!("cannot load webauthn.dll: {why}")),
        };
        plog!(
            "register: clsid={CLSID_TEXT} identity={}",
            package_family_name()
        );
        match register(api) {
            Ok(Registered::Added { op_sign_key }) => {
                println!(
                    "registered LocalPass (disabled until you enable it in Settings); operation-signing key: {op_sign_key}"
                );
                print_state();
                ExitCode::SUCCESS
            }
            Ok(Registered::Updated) => {
                println!("LocalPass was already registered: details updated, key kept");
                print_state();
                ExitCode::SUCCESS
            }
            Err(code) => fail(&format!(
                "register failed, hr={} (package identity needed: run through the execution alias)",
                hr(code)
            )),
        }
    }

    fn unregister_mode() -> ExitCode {
        let api = match load_api() {
            Ok(api) => api,
            Err(why) => return fail(&format!("cannot load webauthn.dll: {why}")),
        };
        let result = unregister(api);
        let after = api.authenticator_state(&CLSID);
        plog!(
            "unregister: result={:?} state after: {:?}",
            result.as_ref().map_err(|c| hr(*c)),
            after.as_ref().map_err(|c| hr(*c))
        );
        match (result, after) {
            (Ok(()), Err(code)) if code == NTE_NOT_FOUND => {
                println!("unregistered");
                ExitCode::SUCCESS
            }
            (Err(code), _) => fail(&format!("unregister failed, hr={}", hr(code))),
            (Ok(()), _) => {
                fail("unregister returned success but the authenticator is still registered")
            }
        }
    }

    fn print_state() {
        if let Ok(api) = load_api() {
            match api.authenticator_state(&CLSID) {
                Ok(state) => println!("state: {}", state_name(state.raw())),
                Err(code) => println!("state: not registered (hr={})", hr(code)),
            }
        }
    }

    fn state_name(raw: i32) -> &'static str {
        match raw {
            STATE_DISABLED => "disabled",
            STATE_ENABLED => "enabled",
            _ => "unknown",
        }
    }

    fn status_mode() -> ExitCode {
        let store = Store::open_default();
        println!("LocalPass passkey provider, class {CLSID_TEXT}");
        let identity = package_family_name();
        println!(
            "package identity: {}",
            if identity == "<none>" {
                "none (run it through the execution alias to see the registration)"
            } else {
                identity
            }
        );
        match load_api() {
            Ok(api) => {
                match api.authenticator_state(&CLSID) {
                    Ok(state) => println!("registered: yes, {}", state_name(state.raw())),
                    Err(code) if code == NTE_NOT_FOUND => println!("registered: no"),
                    Err(code) => println!("registered: unknown (hr={})", hr(code)),
                }
                if let Ok(key) = api.operation_signing_public_key(&CLSID) {
                    println!(
                        "operation-signing key: {} ({} bytes)",
                        curve_name(key.as_slice()).unwrap_or("unknown"),
                        key.len()
                    );
                }
            }
            Err(why) => println!("registered: unknown (webauthn.dll: {why})"),
        }
        println!("data dir: {}", store.dir().display());
        println!(
            "log: {}",
            log::path().map_or("-".to_owned(), |p| p.display().to_string())
        );
        println!(
            "pipe profile: {}",
            store
                .read_profile()
                .unwrap_or_else(|| "none (the real app)".into())
        );
        println!(
            "app path: {}",
            store::app_path_from_registry()
                .map_or("not set".to_owned(), |p| p.display().to_string())
        );
        let paired = match store.load_credentials() {
            Ok(Some(_)) => "yes",
            Ok(None) => "no",
            Err(_) => "stored pairing is unreadable (pair again with --force)",
        };
        println!("paired: {paired}");
        match Provider::open(store) {
            Ok(provider) => {
                match provider.relay.probe() {
                    Probe::NotRunning => println!("app reachable: no (LocalPass is not running)"),
                    Probe::ReachableUnpaired => {
                        println!("app reachable: yes (this provider is not paired)");
                    }
                    Probe::Paired(vault) => {
                        println!("app reachable: yes");
                        println!("vault: {vault:?}");
                    }
                    Probe::Failed(why) => println!("app reachable: no ({why})"),
                }
                if let Ok(api) = load_api() {
                    let cache = OsCache { api, clsid: CLSID };
                    match cache.get_all() {
                        Ok(rows) => println!("credential cache: {} row(s)", rows.len()),
                        Err(code) => println!("credential cache: unreadable (hr={})", hr(code)),
                    }
                }
            }
            Err(why) => println!("app reachable: unknown ({why})"),
        }
        ExitCode::SUCCESS
    }

    fn pair_mode(force: bool) -> ExitCode {
        let store = Store::open_default();
        if !force && matches!(store.load_credentials(), Ok(Some(_))) {
            return fail("already paired; use `pair --force` to pair again");
        }
        let provider = match Provider::open(store) {
            Ok(provider) => provider,
            Err(why) => return fail(&why),
        };
        plog!("pair: connecting (force={force})");
        let result = provider.relay.pair(|code| {
            println!("Pairing code: {code}");
            println!(
                "Approve it in LocalPass (it shows the same code). Waiting up to 3 minutes..."
            );
            let _ = std::io::stdout().flush();
        });
        match result {
            Ok(_) => {
                println!("paired: this PC's Windows passkey provider can now use LocalPass");
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("pairing failed: {e}")),
        }
    }

    fn sync_cache_mode() -> ExitCode {
        let provider = match Provider::open(Store::open_default()) {
            Ok(provider) => provider,
            Err(why) => return fail(&why),
        };
        match provider.sync_cache() {
            SyncOutcome::Done {
                added,
                removed,
                unchanged,
                failed,
            } => {
                println!(
                    "cache synced: {added} added, {removed} removed, {unchanged} unchanged, {failed} failed"
                );
                if failed == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            SyncOutcome::Skipped(why) => fail(&format!("nothing changed: {why}")),
        }
    }

    /// Diagnostic: exercises the OS credential-cache calls with one fake row of our own authenticator
    /// (add twice, read, remove twice) and removes it again. Prints every HRESULT.
    fn cache_selftest_mode() -> ExitCode {
        let api = match load_api() {
            Ok(api) => api,
            Err(why) => return fail(&format!("cannot load webauthn.dll: {why}")),
        };
        let cache = OsCache { api, clsid: CLSID };
        let row = CredentialRow {
            credential_id: vec![0xCA; 32],
            rp_id: "selftest.invalid".into(),
            rp_name: "LocalPass self-test".into(),
            user_id: vec![1, 2, 3],
            user_name: "selftest".into(),
            user_display_name: "selftest".into(),
        };
        let show = |what: &str, result: Result<(), windows::core::HRESULT>| {
            let text = result.map_or_else(hr, |()| hr(windows::core::HRESULT(0)));
            println!("{what}: hr={text}");
        };
        let count = |cache: &OsCache<'_>| match cache.get_all() {
            Ok(rows) => format!("{} row(s)", rows.len()),
            Err(code) => format!("unreadable, hr={}", hr(code)),
        };
        println!("before: {}", count(&cache));
        show("add #1", cache.add(&row));
        show("add #2 (expect NTE_EXISTS 0x8009000F)", cache.add(&row));
        match cache.get_all() {
            Ok(rows) => {
                println!(
                    "read back: {} row(s), ours present: {}",
                    rows.len(),
                    rows.contains(&row)
                );
            }
            Err(code) => println!("read back failed, hr={}", hr(code)),
        }
        show("remove #1", cache.remove(&row));
        show(
            "remove #2 (expect NTE_NOT_FOUND 0x80090011)",
            cache.remove(&row),
        );
        println!("after: {}", count(&cache));
        ExitCode::SUCCESS
    }

    /// Diagnostic (not in the usage text): starts `<exe> [args]` exactly the way the provider starts the
    /// app (`relay::launch_exe`) and prints the method and the new process id. `--method
    /// createprocess|shell|legacy` tries one method alone. Use a harmless exe, for example the
    /// `launch_probe` example, never the app itself; stopping the process is up to the caller.
    fn launch_test_mode(args: &[String]) -> ExitCode {
        use lp_winauth::relay::{LaunchMethod, launch_exe};
        let mut args = args.iter().map(String::as_str).peekable();
        let mut method = None;
        if args.peek() == Some(&"--method") {
            args.next();
            method = match args.next() {
                Some("createprocess") => Some(LaunchMethod::CreateProcess),
                Some("shell") => Some(LaunchMethod::ShellExecute),
                Some("legacy") => Some(LaunchMethod::Legacy),
                _ => return fail("--method takes createprocess, shell or legacy"),
            };
        }
        let Some(exe) = args.next() else {
            return fail(
                "usage: launch-test [--method createprocess|shell|legacy] <exe> [args...]",
            );
        };
        let rest: Vec<&str> = args.collect();
        let path = std::path::Path::new(exe);
        let launched = match method {
            Some(method) => method
                .launch(path, &rest)
                .map(|pid| lp_winauth::relay::Launched { method, pid }),
            None => launch_exe(path, &rest),
        };
        match launched {
            Ok(launched) => {
                println!("started: {} pid={}", launched.method.name(), launched.pid);
                ExitCode::SUCCESS
            }
            Err(e) => fail(&format!("could not start {exe}: {e}")),
        }
    }

    fn fail(message: &str) -> ExitCode {
        plog!("error: {message}");
        println!("error: {message}");
        ExitCode::FAILURE
    }
}
