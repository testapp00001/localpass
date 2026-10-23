//! A harmless stand-in for `localpass.exe` to test how the provider starts the app
//! (`lp-winauth-provider launch-test <this exe> [args]`, run through the execution alias so the starter has
//! package identity). Like the app it has no console window.
//!
//! It writes `%TEMP%\lp-launch-probe-<pid>.log` with its arguments, whether it has package identity,
//! whether it sits in a job object (and that job's limits), then one line a second for 12 seconds, and
//! exits. Nothing else.

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    use std::io::Write as _;
    use std::time::Duration;

    use windows::Win32::Foundation::{APPMODEL_ERROR_NO_PACKAGE, ERROR_INSUFFICIENT_BUFFER};
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFullName;
    use windows::Win32::System::JobObjects::{
        IsProcessInJob, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        QueryInformationJobObject,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    use windows::core::BOOL;

    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("lp-launch-probe-{pid}.log"));
    let Ok(mut log) = std::fs::File::create(&path) else {
        return;
    };
    let mut line = |text: String| {
        let _ = writeln!(log, "{text}");
        let _ = log.flush();
    };

    let args: Vec<String> = std::env::args().skip(1).collect();
    line(format!("start pid={pid} args={args:?}"));

    let mut length = 0u32;
    // SAFETY: asking for the size of the package name; a null buffer is allowed with length 0.
    let identity = unsafe { GetCurrentPackageFullName(&mut length, None) };
    let package = if identity == APPMODEL_ERROR_NO_PACKAGE {
        "none".to_owned()
    } else if identity == ERROR_INSUFFICIENT_BUFFER {
        "present".to_owned()
    } else {
        format!("unknown ({identity:?})")
    };
    line(format!("package identity: {package}"));

    let mut in_job = BOOL(0);
    // SAFETY: the current process handle is a pseudo handle; `in_job` is a valid out pointer.
    let asked = unsafe { IsProcessInJob(GetCurrentProcess(), None, &mut in_job) };
    line(format!("in a job object: {} ({asked:?})", in_job.as_bool()));
    if in_job.as_bool() {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: a null job handle means "the job of the calling process"; the buffer size is right.
        let queried = unsafe {
            QueryInformationJobObject(
                None,
                JobObjectExtendedLimitInformation,
                (&raw mut info).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                None,
            )
        };
        line(format!(
            "job limit flags: 0x{:X} (0x2000 kill on close, 0x800 breakaway ok, 0x1000 silent breakaway) ({queried:?})",
            info.BasicLimitInformation.LimitFlags.0
        ));
    }

    for second in 1..=12 {
        std::thread::sleep(Duration::from_secs(1));
        line(format!("alive {second}"));
    }
    line("exit".to_owned());
}
