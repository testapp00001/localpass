//! Getting a prompt in front of the program the user is working in (Windows only).
//!
//! A browser or Windows asks LocalPass, which sits in the tray, for a user check. Windows does
//! not let a background program take the foreground, so without help the Windows Hello dialog
//! (or our own window) only flashes its taskbar button and the user has to click it first.
//!
//! * [`Anchor`]: a 1x1 transparent tool window (no taskbar button) that is brought to the front
//!   before Hello is started, so that the process owns the foreground while Hello opens.
//! * [`bring_to_front`]: the steps that win the foreground back, tried one after the other and
//!   logged. The same steps raise the Hello dialog and the main window.
//!
//! Everything is logged at info level with the target `lp_prompt` (which is what the app log
//! file `prompts.log` keeps): which window had the foreground, the result of each step, and the
//! foreground a moment later. Only the program's file name, window class, ids and handles are
//! logged, never window titles, paths or anything from the vault.

use std::ffi::c_void;
use std::mem;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tracing::info;
use windows::Win32::Foundation::{COLORREF, CloseHandle, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput, VK_LMENU, VK_MENU,
};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows,
    GetClassNameW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId, IsIconic, IsWindow,
    IsWindowVisible, LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SW_SHOWNA,
    SetForegroundWindow, SetLayeredWindowAttributes, ShowWindow, SwitchToThisWindow,
    TranslateMessage, WM_CLOSE, WM_DESTROY, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

/// Log line for the prompt log file.
macro_rules! plog {
    ($($arg:tt)*) => { info!(target: "lp_prompt", $($arg)*) };
}

/// One way of winning the foreground. Each one is tried only if the one before did not work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Plain `SetForegroundWindow`: works when this process may take the foreground.
    Plain,
    /// Share the input state of the foreground window's thread for a moment, then ask again.
    AttachInput,
    /// The undocumented `SwitchToThisWindow`, the way Alt+Tab switches.
    SwitchTo,
    /// A synthetic Alt key press counts as "the last input event", which lets the next
    /// `SetForegroundWindow` through. Last, because the foreground program sees the key.
    AltKey,
}

/// What [`bring_to_front`] tries, in this order. Measured on Windows 11 with another program in
/// front: `Plain` is refused, and `AttachInput`, `SwitchTo` and `AltKey` each work on their own.
pub const DEFAULT_STEPS: &[Step] = &[Step::Plain, Step::AttachInput, Step::SwitchTo, Step::AltKey];

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut c_void)
}

fn raw(h: HWND) -> isize {
    h.0 as isize
}

/// The program's file name for a process id, or `?` when it cannot be opened (elevated).
fn exe_name(pid: u32) -> String {
    // SAFETY: plain Win32 calls; the handle is closed again and the buffer outlives the call.
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return "?".into();
        };
        let mut buf = [0u16; 520];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return "?".into();
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit(['\\', '/']).next().unwrap_or("?").to_string()
    }
}

fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 128];
    // SAFETY: the buffer is valid for the call.
    let n = unsafe { GetClassNameW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// "hwnd=0x.. pid=.. exe=.. class=..": which window this is, without its title.
pub fn describe(h: HWND) -> String {
    if h.0.is_null() {
        return "none".into();
    }
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer.
    let tid = unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    format!(
        "hwnd={:#x} pid={pid} tid={tid} exe={} class={}",
        raw(h),
        exe_name(pid),
        class_name(h)
    )
}

/// [`describe`] for a raw handle.
pub fn describe_raw(h: isize) -> String {
    describe(hwnd(h))
}

/// Class of the windows Windows Security uses for Windows Hello and the passkey chooser.
const SECURITY_DIALOG_CLASS: &str = "Credential Dialog Xaml Host";

/// The Windows Security dialog windows that are open (visible) right now.
pub fn security_dialogs() -> Vec<isize> {
    unsafe extern "system" fn each(h: HWND, found: LPARAM) -> BOOL {
        // SAFETY: `found` is the `Vec` passed to `EnumWindows` below, alive for the call.
        let found = unsafe { &mut *(found.0 as *mut Vec<isize>) };
        // SAFETY: plain window queries.
        if unsafe { IsWindowVisible(h).as_bool() } && class_name(h) == SECURITY_DIALOG_CLASS {
            found.push(raw(h));
        }
        BOOL(1)
    }
    let mut found: Vec<isize> = Vec::new();
    // SAFETY: the callback only uses the pointer during this call.
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut Vec<isize> as isize));
    }
    found
}

/// Asks a Windows Security dialog to close, if that window is still open and still a dialog of
/// that kind (a handle can be reused). Returns whether it was there.
pub fn close_security_dialog(dialog: isize) -> bool {
    let h = hwnd(dialog);
    // SAFETY: plain window calls on a handle that may have died.
    unsafe {
        if !IsWindow(Some(h)).as_bool()
            || !IsWindowVisible(h).as_bool()
            || class_name(h) != SECURITY_DIALOG_CLASS
        {
            return false;
        }
        let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    true
}

/// The window that has the foreground right now, described as in [`describe`].
pub fn foreground_description() -> String {
    // SAFETY: no arguments.
    describe(unsafe { GetForegroundWindow() })
}

/// Runs one step and says whether the call itself reported success.
fn run_step(step: Step, target: HWND) -> String {
    // SAFETY: plain Win32 calls with handles that may have died (the calls then just fail).
    unsafe {
        match step {
            Step::Plain => format!("called={}", SetForegroundWindow(target).as_bool()),
            Step::AttachInput => {
                let me = GetCurrentThreadId();
                let front = GetForegroundWindow();
                let front_thread = if front.0.is_null() {
                    0
                } else {
                    GetWindowThreadProcessId(front, None)
                };
                let attached = front_thread != 0
                    && front_thread != me
                    && AttachThreadInput(me, front_thread, true).as_bool();
                let _ = BringWindowToTop(target);
                let called = SetForegroundWindow(target).as_bool();
                if attached {
                    let _ = AttachThreadInput(me, front_thread, false);
                }
                format!("attached={attached} called={called}")
            }
            Step::AltKey => {
                let scan = MapVirtualKeyW(u32::from(VK_MENU.0), MAPVK_VK_TO_VSC) as u16;
                let key = |flags| INPUT {
                    r#type: INPUT_KEYBOARD,
                    Anonymous: INPUT_0 {
                        ki: KEYBDINPUT {
                            wVk: VK_LMENU,
                            wScan: scan,
                            dwFlags: flags,
                            time: 0,
                            dwExtraInfo: 0,
                        },
                    },
                };
                let inputs = [
                    key(KEYEVENTF_EXTENDEDKEY),
                    key(KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
                ];
                let sent = SendInput(&inputs, mem::size_of::<INPUT>() as i32);
                let called = SetForegroundWindow(target).as_bool();
                format!("sent={sent} called={called}")
            }
            Step::SwitchTo => {
                SwitchToThisWindow(target, true);
                "called".into()
            }
        }
    }
}

/// Brings `target` (a window of any process) to the front with [`DEFAULT_STEPS`].
pub fn bring_to_front(target: isize, why: &str) -> bool {
    bring_to_front_with(target, why, DEFAULT_STEPS)
}

/// Tries `steps` in order until `target` is the foreground window; logs the result of each.
pub fn bring_to_front_with(target: isize, why: &str, steps: &[Step]) -> bool {
    let target = hwnd(target);
    // SAFETY: no arguments.
    if unsafe { GetForegroundWindow() } == target {
        plog!("{why}: window {:#x} is already in front", raw(target));
        return true;
    }
    for &step in steps {
        let detail = run_step(step, target);
        // A window that was only just created is not in front the instant the call returns:
        // give the switch a moment before judging the step.
        // SAFETY: no arguments.
        let mut now = unsafe { GetForegroundWindow() };
        for _ in 0..3 {
            if now == target {
                break;
            }
            thread::sleep(Duration::from_millis(20));
            // SAFETY: no arguments.
            now = unsafe { GetForegroundWindow() };
        }
        let in_front = now == target;
        plog!(
            "{why}: step {step:?} {detail} -> {}; foreground is now {}",
            if in_front { "in front" } else { "not in front" },
            describe(now)
        );
        if in_front {
            return true;
        }
    }
    false
}

/// Raises the app's own window once it is on screen (showing it is asynchronous in Tauri).
/// Runs in the background and returns at once.
pub fn raise_when_shown(window: isize, why: &'static str) {
    thread::spawn(move || {
        let target = hwnd(window);
        let started = Instant::now();
        // SAFETY: plain Win32 queries.
        while unsafe { !IsWindowVisible(target).as_bool() || IsIconic(target).as_bool() } {
            if started.elapsed() > Duration::from_secs(2) {
                plog!("{why}: the window never became visible; not raising it");
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        plog!("{why}: foreground before: {}", foreground_description());
        let ok = bring_to_front(window, why);
        plog!("{why}: window in front: {ok}");
    });
}

// ---- the anchor window ----

unsafe extern "system" fn anchor_proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if msg == WM_DESTROY {
        // SAFETY: ends the message loop of the anchor's own thread.
        unsafe { PostQuitMessage(0) };
        return LRESULT(0);
    }
    // SAFETY: forwards the message we were given.
    unsafe { DefWindowProcW(h, msg, w, l) }
}

fn create_anchor_window() -> Option<HWND> {
    static CLASS: std::sync::Once = std::sync::Once::new();
    let class = w!("LocalPassFocusAnchor");
    // SAFETY: standard window class and window creation on the calling thread.
    unsafe {
        let module = GetModuleHandleW(PCWSTR::null()).ok()?;
        let instance = HINSTANCE(module.0);
        CLASS.call_once(|| {
            let class_info = WNDCLASSEXW {
                cbSize: mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(anchor_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassExW(&class_info);
        });
        // A popup tool window (no taskbar button, not in Alt+Tab), 1x1 pixel, fully transparent.
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            class,
            w!("LocalPass"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(instance),
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 0, LWA_ALPHA);
        Some(h)
    }
}

/// An invisible window that owns the foreground while a Windows Hello check runs for a browser
/// or for Windows. Drop it when the check is over: the window goes away and the foreground
/// returns to the program the user was in.
pub struct Anchor {
    window: isize,
    /// Who had the foreground before, to give it back.
    previous: isize,
    thread: Option<JoinHandle<()>>,
    why: &'static str,
}

impl Anchor {
    /// Creates the window and brings it to the front. Blocks for a few tens of milliseconds
    /// (the window lives on its own thread). `None` if the window could not be created; the
    /// check then simply runs without help.
    pub fn acquire(why: &'static str) -> Option<Self> {
        Self::acquire_with(why, DEFAULT_STEPS)
    }

    pub fn acquire_with(why: &'static str, steps: &'static [Step]) -> Option<Self> {
        let (tx, rx) = mpsc::channel::<Option<(isize, isize)>>();
        let thread = thread::Builder::new()
            .name("lp-focus-anchor".into())
            .spawn(move || anchor_thread(why, steps, tx))
            .ok()?;
        match rx.recv_timeout(Duration::from_secs(3)) {
            Ok(Some((window, previous))) => Some(Self {
                window,
                previous,
                thread: Some(thread),
                why,
            }),
            _ => {
                plog!("{why}: could not set up the focus window; continuing without it");
                None
            }
        }
    }
}

fn anchor_thread(
    why: &'static str,
    steps: &'static [Step],
    tx: mpsc::Sender<Option<(isize, isize)>>,
) {
    // SAFETY: no arguments.
    let previous = unsafe { GetForegroundWindow() };
    plog!("{why}: foreground before: {}", describe(previous));
    let Some(window) = create_anchor_window() else {
        let _ = tx.send(None);
        return;
    };
    // SAFETY: the window belongs to this thread.
    unsafe {
        let _ = ShowWindow(window, SW_SHOWNA);
    }
    let in_front = bring_to_front_with(raw(window), why, steps);
    plog!("{why}: focus window in front: {in_front}");
    let _ = tx.send(Some((raw(window), raw(previous))));
    let mut msg = MSG::default();
    // SAFETY: the usual message loop; it ends when the window is destroyed (WM_QUIT).
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

impl Drop for Anchor {
    fn drop(&mut self) {
        let window = hwnd(self.window);
        let previous = hwnd(self.previous);
        // If nothing else took the foreground, hand it back to where the user was; otherwise
        // closing the window lets Windows pick the next one, which is usually the same.
        // SAFETY: plain Win32 calls on handles that may have died.
        unsafe {
            if GetForegroundWindow() == window
                && !previous.0.is_null()
                && IsWindow(Some(previous)).as_bool()
            {
                let back = SetForegroundWindow(previous).as_bool();
                plog!("{}: gave the foreground back: {back}", self.why);
            }
            let _ = PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
