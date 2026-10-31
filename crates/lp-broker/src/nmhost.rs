//! The native-messaging helper. A browser starts it when the extension connects, and it does one
//! thing: copy framed JSON between the browser (stdin/stdout) and the app's named pipe. It
//! understands none of the messages, so the extension and the app handshake end to end.
//!
//! The helper is not a separate program: `localpass.exe` runs this when the browser starts it
//! (the browser passes the extension's origin as the first argument).

/// The fixed id of the LocalPass extension: Chrome and Edge derive it from the public key in
/// `extension/manifest.json`, so every unpacked copy gets the same one. Only this extension is
/// listed in the native-messaging manifest, and the helper checks it again.
pub const EXTENSION_ID: &str = "khlgdnhadbjfghhonaohnhdmedgnenlc";

/// The name of the native-messaging host the extension asks for.
pub const HOST_NAME: &str = "app.localpass.host";

/// The fixed id of the Firefox build of the extension (`browser_specific_settings.gecko.id` in
/// the generated Firefox manifest). Firefox only lets the add-ons listed in `allowed_extensions`
/// start the helper, and the helper checks it again.
pub const FIREFOX_EXTENSION_ID: &str = "localpass@localpass.local";

/// The file name of the host manifest. Firefox passes the manifest's path as the first argument.
fn manifest_file_name() -> String {
    format!("{HOST_NAME}.json")
}

/// Whether `args` (the full command line) are those of a browser starting a native host.
/// Chrome and Edge pass `<chrome-extension://id/>`; Firefox passes `<manifest path> <add-on id>`.
pub fn is_native_host_invocation(args: &[String]) -> bool {
    args.get(1).is_some_and(|a| {
        a.starts_with("chrome-extension://")
            || a.starts_with("moz-extension://")
            || (args.len() >= 3 && is_firefox_manifest(a))
    })
}

fn is_firefox_manifest(arg: &str) -> bool {
    std::path::Path::new(arg)
        .file_name()
        .is_some_and(|n| n.to_string_lossy() == manifest_file_name())
}

/// Whether the browser that started the helper is one we serve: Chrome/Edge with our extension's
/// origin, or Firefox with our add-on's id.
pub fn is_allowed_caller(args: &[String]) -> bool {
    match args.get(1).map(String::as_str) {
        Some(first) if first == allowed_origin() => true,
        Some(first) if is_firefox_manifest(first) => {
            args.get(2).map(String::as_str) == Some(FIREFOX_EXTENSION_ID)
        }
        _ => false,
    }
}

/// The one origin allowed to talk to the helper (Chrome and Edge).
pub fn allowed_origin() -> String {
    format!("chrome-extension://{EXTENSION_ID}/")
}

/// Starts the app when it isn't running. Returns once the process is spawned.
pub type LaunchApp<'a> = &'a (dyn Fn() -> std::io::Result<()> + Sync);

#[cfg(any(windows, target_os = "linux"))]
pub use platform::run;

#[cfg(any(windows, target_os = "linux"))]
mod platform {
    use std::io;
    use std::time::{Duration, Instant};

    use serde_json::json;
    use tokio::io::{AsyncRead, AsyncWrite};

    use super::{LaunchApp, is_allowed_caller};
    use crate::framing::{read_message, write_message};
    use crate::pipe;

    /// How long to wait for the app's pipe after launching it.
    const APP_START_WAIT: Duration = Duration::from_secs(25);

    async fn tell_browser<W: AsyncWrite + Unpin>(out: &mut W, code: &str, message: &str) {
        let frame = json!({"type": "hostError", "code": code, "message": message}).to_string();
        let _ = write_message(out, frame.as_bytes()).await;
    }

    /// Runs the helper until the browser or the app closes the connection. Returns the process
    /// exit code.
    pub fn run(args: &[String], launch_app: LaunchApp<'_>) -> i32 {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return 2;
        };
        runtime.block_on(async {
            let mut stdout = tokio::io::stdout();
            if !is_allowed_caller(args) {
                tell_browser(
                    &mut stdout,
                    "forbidden",
                    "this extension may not use LocalPass",
                )
                .await;
                return 1;
            }
            let pipe = match connect_or_start(launch_app).await {
                Ok(pipe) => pipe,
                Err(e) => {
                    tell_browser(
                        &mut stdout,
                        "appNotRunning",
                        &format!("LocalPass is not running and could not be started ({e})"),
                    )
                    .await;
                    return 0;
                }
            };
            relay(tokio::io::stdin(), stdout, pipe).await
        })
    }

    /// Connects to the app, starting it first if the pipe isn't there.
    async fn connect_or_start(launch_app: LaunchApp<'_>) -> io::Result<pipe::Stream> {
        match pipe::connect().await {
            Ok(client) => return Ok(client),
            Err(e) if !pipe::app_not_running(&e) => return Err(e),
            Err(_) => {}
        }
        launch_app()?;
        let started = Instant::now();
        loop {
            tokio::time::sleep(Duration::from_millis(300)).await;
            match pipe::connect().await {
                Ok(client) => return Ok(client),
                Err(e) if pipe::app_not_running(&e) && started.elapsed() < APP_START_WAIT => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Copies frames both ways until one side ends.
    async fn relay<I, O, P>(mut from_browser: I, mut to_browser: O, pipe: P) -> i32
    where
        I: AsyncRead + Unpin,
        O: AsyncWrite + Unpin,
        P: AsyncRead + AsyncWrite + Unpin,
    {
        let (mut from_app, mut to_app) = tokio::io::split(pipe);
        // True when the app is the side that went away (so the browser should be told).
        let app_closed = {
            let up = async {
                while let Ok(Some(frame)) = read_message(&mut from_browser).await {
                    if write_message(&mut to_app, &frame).await.is_err() {
                        return true;
                    }
                }
                false // the browser went away
            };
            let down = async {
                while let Ok(Some(frame)) = read_message(&mut from_app).await {
                    if write_message(&mut to_browser, &frame).await.is_err() {
                        return false; // the browser went away
                    }
                }
                true
            };
            tokio::pin!(up, down);
            tokio::select! {
                closed = &mut up => closed,
                closed = &mut down => closed,
            }
        };
        if app_closed {
            tell_browser(
                &mut to_browser,
                "appClosed",
                "LocalPass closed the connection",
            )
            .await;
        }
        0
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn run(_args: &[String], _launch_app: LaunchApp<'_>) -> i32 {
    eprintln!("the native-messaging helper is only available on Windows and Linux");
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_invocations_are_recognised() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(is_native_host_invocation(&args(&[
            "localpass.exe",
            "chrome-extension://abc/",
            "--parent-window=1"
        ])));
        assert!(is_native_host_invocation(&args(&[
            "localpass.exe",
            "moz-extension://abc/"
        ])));
        assert!(!is_native_host_invocation(&args(&["localpass.exe"])));
        assert!(!is_native_host_invocation(&args(&[
            "localpass.exe",
            "--minimized"
        ])));
        assert_eq!(
            allowed_origin(),
            format!("chrome-extension://{EXTENSION_ID}/")
        );
    }

    #[test]
    fn firefox_invocations_are_recognised_and_checked() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let manifest = "/home/me/.mozilla/native-messaging-hosts/app.localpass.host.json";
        let ours = args(&["localpass", manifest, FIREFOX_EXTENSION_ID]);
        assert!(is_native_host_invocation(&ours));
        assert!(is_allowed_caller(&ours));
        // Another add-on id, no id at all, or another manifest file is not ours.
        let other = args(&["localpass", manifest, "evil@example.com"]);
        assert!(is_native_host_invocation(&other));
        assert!(!is_allowed_caller(&other));
        assert!(!is_native_host_invocation(&args(&["localpass", manifest])));
        assert!(!is_allowed_caller(&args(&["localpass", manifest])));
        let wrong_file = args(&["localpass", "/tmp/other.json", FIREFOX_EXTENSION_ID]);
        assert!(!is_native_host_invocation(&wrong_file));
        assert!(!is_allowed_caller(&wrong_file));
        // Chrome's form still works, and only with our extension id.
        let chrome = args(&["localpass", &allowed_origin(), "--parent-window=0"]);
        assert!(is_allowed_caller(&chrome));
        assert!(!is_allowed_caller(&args(&[
            "localpass",
            "chrome-extension://abc/"
        ])));
        assert!(!is_allowed_caller(&args(&["localpass"])));
    }
}
