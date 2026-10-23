//! The named pipe the app listens on. Only the user who runs the app can open it, and remote
//! connections are refused. Clients prove they know their secret, and the app proves the same
//! back, so a process that grabs the pipe name first still can't pass itself off as the app.

use std::io;

/// The profile named by `LOCALPASS_PROFILE`, if any. A profile gives a second instance of the
/// app (and its test clients) its own pipe.
pub fn profile_from_env() -> Option<String> {
    std::env::var("LOCALPASS_PROFILE")
        .ok()
        .and_then(|p| clean_profile(&p))
}

fn clean_profile(profile: &str) -> Option<String> {
    let cleaned: String = profile
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The pipe name for the current Windows user and an explicit profile (`None` is the default
/// pipe of a normal install). Clients that don't inherit the app's environment, such as the
/// Windows passkey provider, pass the profile themselves.
pub fn pipe_name_for(profile: Option<&str>) -> String {
    let user = platform::user_id();
    match profile.and_then(clean_profile) {
        Some(p) => format!(r"\\.\pipe\localpass-broker-v1-{user}-{p}"),
        None => format!(r"\\.\pipe\localpass-broker-v1-{user}"),
    }
}

/// The pipe name for this process: the default, or the one `LOCALPASS_PROFILE` selects.
pub fn pipe_name() -> String {
    pipe_name_for(profile_from_env().as_deref())
}

#[cfg(all(windows, feature = "server"))]
pub use platform::serve;
#[cfg(windows)]
pub use platform::{connect, connect_profile};

#[cfg(windows)]
mod platform {
    use std::io;
    use std::time::Duration;

    use std::os::windows::io::AsRawHandle;

    use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
    use tracing::debug;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    /// `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`: whoever serves the pipe may learn who is
    /// connecting but cannot act as that user.
    const IDENTIFY_ONLY: u32 = 0x0010_0000 | 0x0001_0000;

    /// Windows' ERROR_PIPE_BUSY: all instances are in use for a moment.
    const PIPE_BUSY: i32 = 231;

    /// The current user's SID as text, for example `S-1-5-21-...`.
    fn user_sid() -> io::Result<String> {
        // SAFETY: plain Win32 calls; the token is opened for this process and closed by `token_sid`.
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(io::Error::last_os_error());
            }
            token_sid(token)
        }
    }

    /// The user SID, as text, of the process that serves `pipe` (a client handle).
    fn pipe_server_sid(pipe: HANDLE) -> io::Result<String> {
        // SAFETY: plain Win32 calls; `pid` is a valid out pointer, and the process and token
        // handles are closed on every path.
        unsafe {
            let mut pid = 0u32;
            if GetNamedPipeServerProcessId(pipe, &mut pid) == 0 || pid == 0 {
                return Err(io::Error::last_os_error());
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut token: HANDLE = std::ptr::null_mut();
            let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token);
            CloseHandle(process);
            if opened == 0 {
                return Err(io::Error::last_os_error());
            }
            token_sid(token)
        }
    }

    /// The user in `token`, as text. Closes the token.
    ///
    /// # Safety
    /// `token` must be an open token handle with `TOKEN_QUERY`.
    unsafe fn token_sid(token: HANDLE) -> io::Result<String> {
        // SAFETY: every buffer outlives the call that fills it, the token handle is closed, and
        // the string from ConvertSidToStringSidW is freed with LocalFree.
        unsafe {
            let mut len = 0u32;
            GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
            let mut buf = vec![0u8; len as usize];
            let ok = GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len);
            CloseHandle(token);
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            let user = &*(buf.as_ptr() as *const TOKEN_USER);
            let mut wide: *mut u16 = std::ptr::null_mut();
            if ConvertSidToStringSidW(user.User.Sid, &mut wide) == 0 {
                return Err(io::Error::last_os_error());
            }
            let mut n = 0;
            while *wide.add(n) != 0 {
                n += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(wide, n));
            LocalFree(wide.cast());
            Ok(text)
        }
    }

    pub fn user_id() -> String {
        user_sid().unwrap_or_else(|_| "unknown".into())
    }

    /// Refuses a pipe that another account serves. The name is machine-wide and built from this user's
    /// SID, which anyone can list, so a process of another user could create it first and answer the
    /// first pairing before there is a secret to prove (phase 5.3, RB-5/RC-1). A process of this user
    /// is the known same-user case: the secret check covers it once paired.
    fn check_server_owner(client: &NamedPipeClient) -> io::Result<()> {
        check_server_is(client, &user_sid()?)
    }

    fn check_server_is(client: &NamedPipeClient, expected_sid: &str) -> io::Result<()> {
        let theirs = pipe_server_sid(client.as_raw_handle())?;
        if theirs == expected_sid {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the LocalPass pipe is served by another account",
            ))
        }
    }

    /// The listening side, which only the app (the `server` feature) needs.
    #[cfg(feature = "server")]
    mod listener {
        use std::io;
        use std::sync::Arc;
        use std::time::Duration;

        use tokio::net::windows::named_pipe::{PipeMode, ServerOptions};
        use tokio::sync::watch;
        use tracing::warn;
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};

        use super::user_sid;
        use crate::server::{Broker, Host};

        /// A security descriptor that lets only the current user use the pipe.
        struct UserOnly {
            descriptor: PSECURITY_DESCRIPTOR,
            attributes: SECURITY_ATTRIBUTES,
        }

        // SAFETY: the descriptor is private to this value and only read through the attributes.
        unsafe impl Send for UserOnly {}

        impl UserOnly {
            fn new() -> io::Result<Self> {
                let sddl: Vec<u16> = format!("D:P(A;;GA;;;{})", user_sid()?)
                    .encode_utf16()
                    .chain(std::iter::once(0))
                    .collect();
                let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
                // SAFETY: `sddl` is NUL terminated and lives through the call.
                let ok = unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        sddl.as_ptr(),
                        1, // SDDL_REVISION_1
                        &mut descriptor,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Self {
                    descriptor,
                    attributes: SECURITY_ATTRIBUTES {
                        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                        lpSecurityDescriptor: descriptor,
                        bInheritHandle: 0,
                    },
                })
            }

            fn as_ptr(&mut self) -> *mut std::ffi::c_void {
                (&mut self.attributes as *mut SECURITY_ATTRIBUTES).cast()
            }
        }

        impl Drop for UserOnly {
            fn drop(&mut self) {
                // SAFETY: allocated by ConvertStringSecurityDescriptorToSecurityDescriptorW.
                unsafe { LocalFree(self.descriptor.cast()) };
            }
        }

        /// Accepts connections until `stop` changes. Returns an error if the pipe can't be created
        /// (for example another process already owns the name).
        pub async fn serve<H: Host>(
            broker: Arc<Broker<H>>,
            mut stop: watch::Receiver<bool>,
        ) -> io::Result<()> {
            let name = crate::pipe::pipe_name();
            let mut security = UserOnly::new()?;
            let mut first = true;
            loop {
                let mut options = ServerOptions::new();
                options
                    .first_pipe_instance(first)
                    .reject_remote_clients(true)
                    .pipe_mode(PipeMode::Byte);
                // SAFETY: the attributes point at a descriptor that lives as long as `security`.
                let server = unsafe {
                    options.create_with_security_attributes_raw(&name, security.as_ptr())
                }?;
                first = false;
                tokio::select! {
                    connected = server.connect() => {
                        if let Err(e) = connected {
                            warn!(error = %e, "broker pipe accept failed");
                            tokio::time::sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                    }
                    _ = stop.changed() => return Ok(()),
                }
                let broker = broker.clone();
                tokio::spawn(async move { broker.serve(server).await });
            }
        }
    }

    #[cfg(feature = "server")]
    pub use listener::serve;

    /// Connects to the app's pipe, waiting briefly if it is busy. The pipe is the one
    /// `LOCALPASS_PROFILE` selects (or the default).
    pub async fn connect() -> io::Result<NamedPipeClient> {
        open(super::pipe_name()).await
    }

    /// Like [`connect`], but for an explicit profile (`None` is the default pipe). For clients
    /// that don't inherit the app's environment.
    pub async fn connect_profile(profile: Option<&str>) -> io::Result<NamedPipeClient> {
        open(super::pipe_name_for(profile)).await
    }

    async fn open(name: String) -> io::Result<NamedPipeClient> {
        let mut attempts = 0;
        loop {
            match ClientOptions::new()
                .security_qos_flags(IDENTIFY_ONLY)
                .open(&name)
            {
                Ok(client) => {
                    check_server_owner(&client)?;
                    return Ok(client);
                }
                Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && attempts < 50 => {
                    attempts += 1;
                    debug!("broker pipe busy; retrying");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use tokio::net::windows::named_pipe::ServerOptions;

        /// RB-5/RC-1: a pipe is only used when this account's process serves it. Another account cannot
        /// be set up in a test, so the other half is checked with a SID that is not ours.
        #[tokio::test]
        async fn a_pipe_served_by_another_account_is_refused() {
            let name = format!(r"\\.\pipe\localpass-owner-test-{}", std::process::id());
            let _server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&name)
                .unwrap();
            let client = ClientOptions::new()
                .security_qos_flags(IDENTIFY_ONLY)
                .open(&name)
                .unwrap();
            assert_eq!(
                pipe_server_sid(client.as_raw_handle()).unwrap(),
                user_sid().unwrap()
            );
            check_server_owner(&client).expect("our own process serves it");
            let refused = check_server_is(&client, "S-1-5-18").unwrap_err();
            assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn user_id() -> String {
        std::env::var("USER").unwrap_or_else(|_| "user".into())
    }
}

/// Whether an error from [`connect`] means the app is not running.
pub fn app_not_running(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}
