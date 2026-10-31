//! `localpass.exe` doubles as the browser's native-messaging helper: when a browser starts it
//! for the extension, it relays between the browser and the running app instead of opening a
//! window. See `lp_broker::nmhost`.

use std::io;

/// Whether the command line is a browser starting the native-messaging helper.
pub fn wanted(args: &[String]) -> bool {
    lp_broker::nmhost::is_native_host_invocation(args)
}

/// Runs the helper and returns the process exit code.
pub fn run(args: &[String]) -> i32 {
    lp_broker::nmhost::run(args, &launch_app)
}

/// Starts LocalPass in the tray, separate from this helper (and from the browser's process
/// group), so it keeps running after the browser closes.
fn launch_app() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = std::process::Command::new(exe);
    command.arg("--minimized");
    // This helper's standard streams are the browser's native-messaging pipes: the app must not inherit them,
    // or its log lines would be read by the browser as frames and the browser would never see the helper end.
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
        // Leave the browser's job object if it allows that; otherwise start normally.
        if command
            .creation_flags(flags | CREATE_BREAKAWAY_FROM_JOB)
            .spawn()
            .is_ok()
        {
            return Ok(());
        }
        command.creation_flags(flags);
    }
    #[cfg(unix)]
    {
        // Its own process group: the browser ending the helper's group does not take the app with it.
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command.spawn().map(|_| ())
}
