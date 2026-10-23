//! How the provider reaches the running LocalPass app.
//!
//! [`Relay`] is what the request handlers call: plain blocking methods, so they (and their tests) need no
//! async code. [`RealRelay`] implements it over [`lp_broker::client`], opening a NEW pipe connection for
//! every operation: dropping that connection is how a pending Windows Hello prompt in the app is
//! withdrawn (the app aborts the prompts of a connection that went away), which is what a cancelled
//! operation needs. The tests use `crate::testing::FakeRelay` instead.
//!
//! Which pipe (profile) and which exe to start are given explicitly in [`RelayConfig`]: only the binary
//! reads `profile.txt` and the registry, so tests can never reach, or start, the user's real app.

use std::future::Future;
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lp_broker::ClientKind;
use lp_broker::client::{Client, ClientError, Credentials, Options, Paired, Timeouts};
use lp_broker::pipe;
use lp_broker::protocol::{AssertedPasskey, Request, UserVerification};
use lp_core::passkey::{PasskeyCandidate, PasskeyCreateRequest, PasskeyCreated};
use lp_core::status::VaultStatus;
use tokio::net::windows::named_pipe::NamedPipeClient;
use tokio::runtime::Runtime;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

use crate::plog;
use crate::store::Store;

/// Why a relayed operation failed, in the terms the HRESULT table needs (see
/// [`crate::translate::hresult_for`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    /// The app cannot be reached: not running (and not startable), the pipe is not LocalPass, no vault.
    Unreachable(String),
    /// This provider never paired, or the app forgot it.
    NotPaired,
    /// The user said no, Windows Hello failed or was dismissed, the vault stayed locked, or the
    /// operation was cancelled by Windows.
    Cancelled,
    /// No such passkey.
    NotFound,
    /// The exclude list names a passkey the vault holds.
    Exists,
    Unsupported(String),
    Invalid(String),
    Other(String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(why) => write!(f, "LocalPass is not reachable: {why}"),
            Self::NotPaired => write!(f, "the provider is not paired with LocalPass"),
            Self::Cancelled => write!(f, "cancelled or not approved"),
            Self::NotFound => write!(f, "no matching passkey"),
            Self::Exists => write!(f, "a passkey from the exclude list already exists"),
            Self::Unsupported(why) => write!(f, "not supported: {why}"),
            Self::Invalid(why) => write!(f, "invalid request: {why}"),
            Self::Other(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for RelayError {}

/// Cancels one operation from another thread (Windows' `CancelOperation` arrives on a different RPC
/// thread than the call it cancels). Cloning shares the token.
#[derive(Clone)]
pub struct CancelToken(Arc<TokenInner>);

struct TokenInner {
    flag: Mutex<bool>,
    wake: Condvar,
    async_flag: watch::Sender<bool>,
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(TokenInner {
            flag: Mutex::new(false),
            wake: Condvar::new(),
            async_flag: watch::channel(false).0,
        }))
    }

    pub fn cancel(&self) {
        *self.0.flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.0.wake.notify_all();
        self.0.async_flag.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.flag.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Blocks up to `wait` for a cancel; true if the token is cancelled. For synchronous code and fakes.
    pub fn wait_cancelled(&self, wait: Duration) -> bool {
        let guard = self.0.flag.lock().unwrap_or_else(|e| e.into_inner());
        let (guard, _) = self
            .0
            .wake
            .wait_timeout_while(guard, wait, |cancelled| !*cancelled)
            .unwrap_or_else(|e| e.into_inner());
        *guard
    }

    /// Resolves once cancelled (immediately if it already is).
    pub async fn cancelled(&self) {
        let mut seen = self.0.async_flag.subscribe();
        let _ = seen.wait_for(|cancelled| *cancelled).await;
    }
}

/// The app, as the request handlers see it. Each method is one operation on its own connection;
/// cancelling `cancel` withdraws a prompt the app is showing for it.
pub trait Relay: Send + Sync {
    /// The vault state. Never starts the app, never prompts.
    fn status(&self) -> Result<VaultStatus, RelayError>;
    /// Creates a passkey. The app checks the user (Windows Hello) before it does.
    fn create(
        &self,
        cancel: &CancelToken,
        request: PasskeyCreateRequest,
    ) -> Result<PasskeyCreated, RelayError>;
    /// Signs in with the one passkey for `rp_id` (limited to the credential ids in `allow` when it is not
    /// empty), in ONE request: starts the app if needed, and the app unlocks the vault when it is locked
    /// (that unlock is the user check) or asks for Windows Hello when it is already open. No match or
    /// several matches (no picker yet) is `NotFound`.
    fn assert_any(
        &self,
        cancel: &CancelToken,
        origin: &str,
        rp_id: &str,
        allow: &[Vec<u8>],
        client_data_hash: &[u8],
    ) -> Result<AssertedPasskey, RelayError>;
    /// Metadata of every passkey, for the credential cache. Never starts the app, never prompts.
    fn list_all(&self) -> Result<Vec<PasskeyCandidate>, RelayError>;
}

/// What a `status` command can tell about the app without pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// There is no pipe: the app is not running.
    NotRunning,
    /// The pipe answers as LocalPass, but this provider is not paired.
    ReachableUnpaired,
    /// Paired and connected; the vault's state.
    Paired(VaultStatus),
    /// Something else: `why`.
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// Broker pipe profile (`None` = the real app's pipe).
    pub profile: Option<String>,
    /// `localpass.exe`, started with `--minimized` when its pipe is missing. `None` = never start it.
    pub app_path: Option<PathBuf>,
    /// How long to wait for a started app to open its pipe (25 s, like the browser helper).
    pub launch_wait: Duration,
    pub timeouts: Timeouts,
}

impl RelayConfig {
    pub fn new(profile: Option<String>, app_path: Option<PathBuf>) -> Self {
        Self {
            profile,
            app_path,
            launch_wait: Duration::from_secs(25),
            timeouts: Timeouts::default(),
        }
    }
}

/// The real thing: [`Relay`] over the broker's named pipe.
pub struct RealRelay {
    runtime: Runtime,
    store: Store,
    config: RelayConfig,
    options: Options,
    /// Set once the app was started, so one provider process does not start it over and over.
    launched_at: Mutex<Option<Instant>>,
}

impl RealRelay {
    pub fn new(store: Store, config: RelayConfig) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("lp-relay")
            .enable_all()
            .build()?;
        let mut options = Options::new("Windows passkey provider", ClientKind::Provider);
        options.timeouts = config.timeouts;
        Ok(Self {
            runtime,
            store,
            config,
            options,
            launched_at: Mutex::new(None),
        })
    }

    pub fn config(&self) -> &RelayConfig {
        &self.config
    }

    /// Runs `work` to completion; when `cancel` fires first the future is dropped, which drops its pipe
    /// connection, which makes the app withdraw its prompt.
    fn run<T>(
        &self,
        cancel: Option<&CancelToken>,
        work: impl Future<Output = Result<T, RelayError>>,
    ) -> Result<T, RelayError> {
        self.runtime.block_on(async {
            match cancel {
                Some(token) => {
                    tokio::select! {
                        biased;
                        () = token.cancelled() => Err(RelayError::Cancelled),
                        result = work => result,
                    }
                }
                None => work.await,
            }
        })
    }

    fn credentials(&self) -> Result<Credentials, RelayError> {
        match self.store.load_credentials() {
            Ok(Some(credentials)) => Ok(credentials),
            Ok(None) => Err(RelayError::NotPaired),
            Err(e) => {
                plog!("stored pairing cannot be used: {e}");
                Err(RelayError::NotPaired)
            }
        }
    }

    /// Opens the pipe, starting the app first when `launch` is set and it is not running.
    async fn open_pipe(&self, launch: bool) -> Result<NamedPipeClient, RelayError> {
        let deadline = Instant::now() + self.config.launch_wait;
        let mut started = false;
        loop {
            let attempt = timeout(
                self.config.timeouts.step,
                pipe::connect_profile(self.config.profile.as_deref()),
            )
            .await;
            match attempt {
                Ok(Ok(pipe)) => return Ok(pipe),
                Ok(Err(e)) if pipe::app_not_running(&e) => {
                    let path = self.config.app_path.as_deref().filter(|_| launch);
                    let Some(path) = path else {
                        return Err(RelayError::Unreachable("LocalPass is not running".into()));
                    };
                    if !started {
                        started = true;
                        self.start_app(path)?;
                    }
                    if Instant::now() >= deadline {
                        return Err(RelayError::Unreachable(
                            "LocalPass did not open its pipe in time".into(),
                        ));
                    }
                    sleep(Duration::from_millis(300)).await;
                }
                Ok(Err(e)) => {
                    return Err(RelayError::Unreachable(format!(
                        "cannot open the pipe: {e}"
                    )));
                }
                Err(_) => {
                    return Err(RelayError::Unreachable("timed out opening the pipe".into()));
                }
            }
        }
    }

    fn start_app(&self, path: &Path) -> Result<(), RelayError> {
        // At most one start per 20 s per process: a second operation that finds the pipe missing
        // while the first start is still booting just waits.
        {
            let mut last = self.launched_at.lock().unwrap_or_else(|e| e.into_inner());
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
                return Ok(());
            }
            *last = Some(Instant::now());
        }
        plog!("LocalPass is not running: starting {}", path.display());
        launch_app(path).map(|_| ()).map_err(|e| {
            plog!("could not start LocalPass: {e}");
            RelayError::Unreachable(format!("could not start LocalPass: {e}"))
        })
    }

    /// A logged-in connection for one operation.
    async fn connect(&self, launch: bool) -> Result<Client<NamedPipeClient>, RelayError> {
        let credentials = self.credentials()?;
        let pipe = self.open_pipe(launch).await?;
        allow_foreground(&pipe);
        Client::connect(pipe, &self.options, &credentials)
            .await
            .map_err(|e| map_client_error(e, Phase::Connect))
    }

    /// Pairs with the app (the user approves the code in the app). `on_code` is called with the six
    /// digits as soon as the app shows them. Starts the app if needed. Saves the credentials.
    pub fn pair(&self, on_code: impl FnOnce(&str)) -> Result<Paired, RelayError> {
        self.run(None, async {
            let pipe = self.open_pipe(true).await?;
            allow_foreground(&pipe);
            let (_client, paired) = Client::pair(pipe, &self.options, on_code)
                .await
                .map_err(|e| map_client_error(e, Phase::Pairing))?;
            self.store
                .save_credentials(&paired.credentials)
                .map_err(|e| RelayError::Other(format!("could not save the pairing: {e}")))?;
            Ok(paired)
        })
    }

    /// What is on the other end of the pipe, for the `status` command. Never starts the app and never
    /// asks the app to pair (a Hello without a known client id only gets "unpaired" back).
    pub fn probe(&self) -> Probe {
        let result = self.run(None, async {
            let paired = self.store.load_credentials().ok().flatten();
            let pipe = match self.open_pipe(false).await {
                Ok(pipe) => pipe,
                Err(RelayError::Unreachable(why)) if why == "LocalPass is not running" => {
                    return Ok(Probe::NotRunning);
                }
                Err(e) => return Ok(Probe::Failed(e.to_string())),
            };
            let stranger = Credentials {
                client_id: "provider-probe".into(),
                secret: zeroize::Zeroizing::new(vec![0; 32]),
            };
            let credentials = paired.as_ref().unwrap_or(&stranger);
            match Client::connect(pipe, &self.options, credentials).await {
                Ok(client) if paired.is_some() => Ok(Probe::Paired(client.status())),
                Ok(_) => Ok(Probe::Failed("an unknown client was accepted".into())),
                Err(ClientError::Unpaired) if paired.is_none() => Ok(Probe::ReachableUnpaired),
                Err(ClientError::Unpaired) => Ok(Probe::Failed(
                    "the app no longer knows this provider (pair again)".into(),
                )),
                Err(e) => Ok(Probe::Failed(e.to_string())),
            }
        });
        result.unwrap_or_else(|e| Probe::Failed(e.to_string()))
    }
}

impl Relay for RealRelay {
    fn status(&self) -> Result<VaultStatus, RelayError> {
        self.run(None, async {
            let client = self.connect(false).await?;
            Ok(client.status())
        })
    }

    fn create(
        &self,
        cancel: &CancelToken,
        request: PasskeyCreateRequest,
    ) -> Result<PasskeyCreated, RelayError> {
        // `user_verified` is the app's to decide: it runs its own Windows Hello check.
        let request = Request::PasskeyCreate {
            request: PasskeyCreateRequest {
                user_verified: false,
                ..request
            },
            uv: UserVerification::Required,
        };
        self.run(Some(cancel), async {
            let mut client = self.connect(true).await?;
            client
                .request_as(&request)
                .await
                .map_err(|e| map_client_error(e, Phase::Request))
        })
    }

    fn assert_any(
        &self,
        cancel: &CancelToken,
        origin: &str,
        rp_id: &str,
        allow: &[Vec<u8>],
        client_data_hash: &[u8],
    ) -> Result<AssertedPasskey, RelayError> {
        // The app decides who is verified: its unlock or its Windows Hello check, never a flag from us.
        let request = Request::PasskeyAssertAny {
            origin: origin.to_owned(),
            rp_id: rp_id.to_owned(),
            allow: allow.to_vec(),
            client_data_hash: client_data_hash.to_vec(),
            uv: UserVerification::Required,
        };
        self.run(Some(cancel), async {
            let mut client = self.connect(true).await?;
            client
                .request_as(&request)
                .await
                .map_err(|e| map_client_error(e, Phase::Request))
        })
    }

    fn list_all(&self) -> Result<Vec<PasskeyCandidate>, RelayError> {
        self.run(None, async {
            let mut client = self.connect(false).await?;
            client
                .request_as(&Request::PasskeyListAll)
                .await
                .map_err(|e| map_client_error(e, Phase::Request))
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Opening the connection and logging in.
    Connect,
    Pairing,
    /// A request on an established connection.
    Request,
}

/// Maps the broker client's errors to ours. Everything that means "the app is not there" is
/// `Unreachable`; "the user did not approve" (including a vault that stayed locked) is `Cancelled`.
fn map_client_error(error: ClientError, phase: Phase) -> RelayError {
    match error {
        ClientError::Unpaired => RelayError::NotPaired,
        ClientError::Denied | ClientError::Locked => RelayError::Cancelled,
        ClientError::ServerProofMismatch => {
            RelayError::Unreachable("the program on the pipe is not LocalPass".into())
        }
        ClientError::Busy => RelayError::Other("LocalPass is busy with another request".into()),
        // The request itself outlasted its timeout: not "unreachable", the app was there.
        ClientError::Timeout("request") if phase == Phase::Request => {
            RelayError::Other("LocalPass did not answer in time".into())
        }
        ClientError::Timeout(_) | ClientError::Io(_) | ClientError::Closed => {
            RelayError::Unreachable(error.to_string())
        }
        ClientError::Remote { code, message } => match code.as_str() {
            "notFound" => RelayError::NotFound,
            // Several passkeys match and there is no picker: the same answer to Windows as none.
            "ambiguous" => {
                plog!("the app found several passkeys for this sign-in: {message}");
                RelayError::NotFound
            }
            "passkeyExists" => RelayError::Exists,
            "unsupported" => RelayError::Unsupported(message),
            "invalid" => RelayError::Invalid(message),
            "noVault" => RelayError::Unreachable(message),
            // `locked` and `denied` are mapped by the client already.
            _ => RelayError::Other(format!("LocalPass said {code}: {message}")),
        },
        ClientError::Protocol(why) => RelayError::Other(why),
    }
}

/// Lets the app's process take the foreground, so its Windows Hello window can come in front of the
/// browser. Best effort: a COM-started process may not have the right to hand out.
fn allow_foreground(pipe: &NamedPipeClient) {
    let mut pid = 0u32;
    // SAFETY: the handle is the live pipe handle; `pid` is a valid out pointer.
    let found = unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle().cast(), &mut pid) };
    if found == 0 || pid == 0 {
        return;
    }
    // SAFETY: plain call with a process id.
    let allowed = unsafe { AllowSetForegroundWindow(pid) };
    plog!(
        "AllowSetForegroundWindow(app pid {pid}) -> {}",
        allowed != 0
    );
}

/// How [`launch_exe`] can start a program from this (packaged) process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchMethod {
    /// `CreateProcess` with no special flags and null standard handles.
    CreateProcess,
    /// `ShellExecuteExW`, verb "open", window shown minimized without taking focus.
    ShellExecute,
    /// What the browser helper does (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`, breakaway from the
    /// job first). In this packaged COM server it fails with `os error 50`. Only `launch-test` uses it,
    /// to show that; [`launch_exe`] never does.
    Legacy,
}

impl LaunchMethod {
    /// The methods [`launch_exe`] tries, in this order.
    pub const ORDER: [LaunchMethod; 2] = [Self::CreateProcess, Self::ShellExecute];

    pub fn name(self) -> &'static str {
        match self {
            Self::CreateProcess => "CreateProcess",
            Self::ShellExecute => "ShellExecuteExW",
            Self::Legacy => "legacy CreateProcess (detached, breakaway)",
        }
    }

    /// Starts `path` with `args` and returns the new process id.
    pub fn launch(self, path: &Path, args: &[&str]) -> io::Result<u32> {
        match self {
            Self::CreateProcess => create_process(path, args),
            Self::ShellExecute => shell_execute(path, args),
            Self::Legacy => legacy_create_process(path, args),
        }
    }
}

/// A program [`launch_exe`] started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Launched {
    pub method: LaunchMethod,
    pub pid: u32,
}

/// Starts LocalPass in the tray (`--minimized`) for a request that found the app not running.
pub fn launch_app(path: &Path) -> io::Result<Launched> {
    launch_exe(path, &["--minimized"])
}

/// Starts `path` with `args`, trying each of [`LaunchMethod::ORDER`] until one works and logging which.
///
/// A COM-activated package process cannot start the app the way the browser helper does (see
/// [`LaunchMethod::Legacy`]), so there are two ways to try before giving up.
pub fn launch_exe(path: &Path, args: &[&str]) -> io::Result<Launched> {
    let mut failure = None;
    for method in LaunchMethod::ORDER {
        match method.launch(path, args) {
            Ok(pid) => {
                plog!(
                    "started {} with {} (pid {pid})",
                    path.display(),
                    method.name()
                );
                return Ok(Launched { method, pid });
            }
            Err(e) => {
                plog!(
                    "starting {} with {} failed: {e}",
                    path.display(),
                    method.name()
                );
                failure = Some(e);
            }
        }
    }
    Err(failure.unwrap_or_else(|| io::Error::other("no way to start a program")))
}

fn create_process(path: &Path, args: &[&str]) -> io::Result<u32> {
    use std::process::{Command, Stdio};
    let mut command = Command::new(path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        command.current_dir(dir);
    }
    command.spawn().map(|child| child.id())
}

fn legacy_create_process(path: &Path, args: &[&str]) -> io::Result<u32> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mut command = std::process::Command::new(path);
    command.args(args);
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    if let Ok(child) = command
        .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
    {
        return Ok(child.id());
    }
    command
        .creation_flags(flags)
        .spawn()
        .map(|child| child.id())
}

/// The one parameter string `ShellExecuteExW` takes: arguments with spaces are quoted (ours are plain
/// words), and a quote inside one is refused.
fn shell_parameters(args: &[&str]) -> io::Result<String> {
    let mut parameters = String::new();
    for arg in args {
        if arg.contains('"') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "quotes in an argument",
            ));
        }
        if !parameters.is_empty() {
            parameters.push(' ');
        }
        if arg.is_empty() || arg.contains(char::is_whitespace) {
            parameters.push_str(&format!("\"{arg}\""));
        } else {
            parameters.push_str(arg);
        }
    }
    Ok(parameters)
}

fn shell_execute(path: &Path, args: &[&str]) -> io::Result<u32> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::GetProcessId;
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
        ShellExecuteExW,
    };
    const SW_SHOWMINNOACTIVE: i32 = 7;

    fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
        text.encode_wide().chain(std::iter::once(0)).collect()
    }
    let parameters = shell_parameters(args)?;
    let verb = wide("open".as_ref());
    let file = wide(path.as_os_str());
    let parameters = wide(parameters.as_ref());
    let directory = path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| wide(dir.as_os_str()));

    // SAFETY: an all-zero SHELLEXECUTEINFOW is valid (null pointers, zero handles); the fields that
    // matter are set below, and every pointer stays alive until the call returns.
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = parameters.as_ptr();
    info.lpDirectory = directory
        .as_ref()
        .map_or(std::ptr::null(), |dir| dir.as_ptr());
    info.nShow = SW_SHOWMINNOACTIVE;
    // SAFETY: `info` is fully initialised as above.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.hProcess.is_null() {
        // Started by an existing process (DDE): no handle to report.
        return Ok(0);
    }
    // SAFETY: a process handle the call returned to us (SEE_MASK_NOCLOSEPROCESS).
    let pid = unsafe { GetProcessId(info.hProcess) };
    // SAFETY: closing that same handle once.
    unsafe { CloseHandle(info.hProcess) };
    Ok(pid)
}

/// A flag the COM layer can share with background work (`sync-cache`) to stop it early.
pub fn shutdown_flag() -> &'static AtomicBool {
    static FLAG: AtomicBool = AtomicBool::new(false);
    &FLAG
}

/// Whether the process is shutting down (see [`shutdown_flag`]).
pub fn shutting_down() -> bool {
    shutdown_flag().load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_parameters_are_joined_and_quoted() {
        assert_eq!(shell_parameters(&[]).unwrap(), "");
        assert_eq!(shell_parameters(&["--minimized"]).unwrap(), "--minimized");
        assert_eq!(
            shell_parameters(&["--minimized", "a b", ""]).unwrap(),
            "--minimized \"a b\" \"\""
        );
        assert!(shell_parameters(&["say \"hi\""]).is_err());
    }

    #[test]
    fn programs_start_and_missing_ones_fail_with_every_method() {
        let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
        let started = LaunchMethod::CreateProcess
            .launch(cmd, &["/c", "exit", "0"])
            .unwrap();
        assert_ne!(started, 0);
        let missing = Path::new(
            r"C:
o\sucholder\localpass.exe",
        );
        assert!(launch_exe(missing, &["--minimized"]).is_err());
        for method in LaunchMethod::ORDER {
            assert!(method.launch(missing, &[]).is_err(), "{}", method.name());
        }
    }

    #[test]
    fn client_errors_map_to_relay_errors() {
        use ClientError as C;
        let m = |e| map_client_error(e, Phase::Request);
        assert_eq!(m(C::Unpaired), RelayError::NotPaired);
        assert_eq!(m(C::Denied), RelayError::Cancelled);
        assert_eq!(m(C::Locked), RelayError::Cancelled);
        assert!(matches!(
            m(C::ServerProofMismatch),
            RelayError::Unreachable(_)
        ));
        assert!(matches!(m(C::Closed), RelayError::Unreachable(_)));
        assert!(matches!(
            m(C::Io(io::Error::from(io::ErrorKind::BrokenPipe))),
            RelayError::Unreachable(_)
        ));
        assert!(matches!(m(C::Timeout("ready")), RelayError::Unreachable(_)));
        assert!(matches!(m(C::Timeout("request")), RelayError::Other(_)));
        assert!(matches!(m(C::Busy), RelayError::Other(_)));
        let remote = |code: &str| C::Remote {
            code: code.into(),
            message: "m".into(),
        };
        assert_eq!(m(remote("notFound")), RelayError::NotFound);
        assert_eq!(m(remote("ambiguous")), RelayError::NotFound);
        assert_eq!(m(remote("passkeyExists")), RelayError::Exists);
        assert!(matches!(
            m(remote("unsupported")),
            RelayError::Unsupported(_)
        ));
        assert!(matches!(m(remote("invalid")), RelayError::Invalid(_)));
        assert!(matches!(m(remote("noVault")), RelayError::Unreachable(_)));
        assert!(matches!(m(remote("internal")), RelayError::Other(_)));
        assert!(matches!(m(C::Protocol("x".into())), RelayError::Other(_)));
        // While connecting, the request timeout does not apply: any timeout means unreachable.
        assert!(matches!(
            map_client_error(C::Timeout("request"), Phase::Connect),
            RelayError::Unreachable(_)
        ));
    }

    #[test]
    fn a_cancel_token_wakes_sync_and_async_waiters() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
        assert!(!token.wait_cancelled(Duration::from_millis(20)));

        let waiter = {
            let token = token.clone();
            std::thread::spawn(move || token.wait_cancelled(Duration::from_secs(5)))
        };
        std::thread::sleep(Duration::from_millis(50));
        token.cancel();
        assert!(waiter.join().unwrap(), "the blocked thread saw the cancel");
        assert!(token.is_cancelled());
        assert!(token.wait_cancelled(Duration::ZERO), "stays cancelled");

        // An async waiter created after the cancel returns at once.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(2), token.cancelled())
                .await
                .expect("already cancelled");
        });
        // And one waiting when the cancel happens.
        let fresh = CancelToken::new();
        let later = fresh.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            later.cancel();
        });
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(5), fresh.cancelled())
                .await
                .expect("cancelled while waiting");
        });
    }

    #[test]
    fn without_credentials_nothing_is_attempted() {
        let dir = tempfile::tempdir().unwrap();
        let relay = RealRelay::new(
            Store::new(dir.path()),
            RelayConfig::new(Some("lp-test-never-exists".into()), None),
        )
        .unwrap();
        assert_eq!(relay.status().unwrap_err(), RelayError::NotPaired);
        assert_eq!(relay.list_all().unwrap_err(), RelayError::NotPaired);
        let created = relay.create(
            &CancelToken::new(),
            PasskeyCreateRequest {
                origin: "https://example.com".into(),
                rp_id: "example.com".into(),
                rp_name: String::new(),
                user_handle: vec![1],
                user_name: String::new(),
                user_display_name: String::new(),
                algorithms: vec![-7],
                exclude: vec![],
                user_verified: false,
            },
        );
        assert_eq!(created.unwrap_err(), RelayError::NotPaired);
    }

    #[test]
    fn a_missing_app_is_reported_not_started() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        store
            .save_credentials(&Credentials {
                client_id: "x".into(),
                secret: zeroize::Zeroizing::new(vec![1; 32]),
            })
            .unwrap();
        // A profile nobody listens on, and no app path: the pipe is missing, so it is Unreachable at once.
        let relay = RealRelay::new(
            store,
            RelayConfig::new(Some("lp-test-never-exists".into()), None),
        )
        .unwrap();
        let started = Instant::now();
        assert!(matches!(relay.status(), Err(RelayError::Unreachable(_))));
        assert!(matches!(
            relay.assert_any(&CancelToken::new(), "https://e.com", "e.com", &[], &[7; 32]),
            Err(RelayError::Unreachable(_))
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(relay.probe(), Probe::NotRunning);
    }
}
