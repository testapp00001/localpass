//! The main window around prompts that a browser (a broker client) raises (Linux only).
//!
//! The user is in the browser when it asks for an unlock, a "Confirm it is you", a "Replace?" or
//! a pairing approval, so the app shows its window for the prompt. When the last such prompt has
//! ended, the window goes back to how it was: hidden again (so the browser gets the focus back),
//! minimized again, or left alone when it was on screen. Nothing is restored when the window was
//! never raised (the quick unlock path), or when the user opened the window themselves meanwhile
//! (tray icon, starting the app again).
//!
//! The window is brought up by re-mapping it (see [`Raise`]): GNOME on Wayland gives the focus
//! to a freshly mapped window, but not to a mapped one that asks for it. A window that has the
//! focus already is left alone.
//!
//! Prompts nest (a pairing first asks for an unlock) and overlap (another connection's check), so
//! the state is taken when the first one opens and restored when the last one ends. Each prompt
//! holds a [`Prompt`] guard: the broker drops a prompt's future when its client leaves or its
//! time runs out, and the guard's `Drop` counts that as an end too.
//!
//! Lock rule: `spans` may be held while asking the window for its state, which waits for the main
//! thread. The main thread (tray, window events) therefore never takes `spans`: it only touches
//! the atomics and the `opened` mutex, which nothing holds while waiting.
//!
//! Every step is logged to the prompt log (target `lp_prompt`): the kind of prompt, never a site,
//! user or client name.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tracing::info;

use crate::broker::PromptKind;

/// What the window looked like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub visible: bool,
    /// GTK reports this only where the desktop tells it (X11). On Wayland it stays false.
    pub minimized: bool,
    pub focused: bool,
}

/// How the window is brought up for a prompt.
///
/// Measured on GNOME (Wayland, assisted run 1 of 8.4, 2026-11-13): a window hidden in the tray
/// got the focus with every method; a window open behind the browser did not with `present`
/// (GNOME showed "LocalPass is ready" instead) and did with `remap`. So `remap` is the default,
/// in release builds too; the others can be chosen with `LOCALPASS_RAISE` in debug builds only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Raise {
    /// Unminimize, show, focus (GTK's `present`).
    Present,
    /// A window on screen without the focus is hidden first and, after a short pause, shown
    /// and focused: a freshly mapped window gets the focus where a `present` of a mapped one
    /// does not. A hidden window is just shown and focused (it is unmapped already).
    Remap,
    /// Like `Remap`, but without the final `set_focus` (debug builds only): tells whether the
    /// "LocalPass is ready" notification that came with a re-map comes from that call.
    RemapNoFocus,
}

impl Raise {
    fn name(self) -> &'static str {
        match self {
            Raise::Present => "present",
            Raise::Remap => "remap",
            Raise::RemapNoFocus => "remap-no-focus",
        }
    }
}

/// What one raise does, chosen from the method and the window's state at that moment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Steps {
    /// The window is on screen with the focus already: nothing (re-mapping it would make it
    /// blink, as the pairing dialog after its unlock did in the measurement).
    Nothing,
    /// Unminimize, show, and focus when `focus`.
    Show { focus: bool },
    /// Hide, pause, then unminimize, show, and focus when `focus`.
    Remap { focus: bool },
}

/// The steps of one raise.
fn steps(how: Raise, seen: Seen) -> Steps {
    // `visible` counts too: right after a hide GTK may still call the window active (seen in
    // the measurement: `visible=false focused=true`).
    if seen.focused && seen.visible && !seen.minimized {
        return Steps::Nothing;
    }
    let focus = how != Raise::RemapNoFocus;
    match how {
        Raise::Present => Steps::Show { focus },
        // A hidden window is unmapped already: showing it is the fresh map, and a hide first
        // would only add the pause (it got the focus at once with every method).
        Raise::Remap | Raise::RemapNoFocus if !seen.visible => Steps::Show { focus },
        Raise::Remap | Raise::RemapNoFocus => Steps::Remap { focus },
    }
}

/// The window operations the guard needs (a fake in the tests). None of them may wait for the
/// main thread except `seen`; they only post requests, which the main thread handles in order.
pub trait MainWindow: Send + Sync {
    /// Reads the state. Waits for the main thread: never call it on the main thread.
    fn seen(&self) -> Seen;
    /// Unminimize, show, and focus when `focus`.
    fn show(&self, focus: bool);
    fn hide(&self);
    fn minimize(&self);
}

/// How long a re-map leaves the window unmapped, so the compositor sees the unmap before the
/// window maps again.
const REMAP_PAUSE: Duration = Duration::from_millis(100);

/// How the window was before the first prompt opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Before {
    Hidden,
    Minimized,
    Showing,
}

impl Before {
    fn of(seen: Seen) -> Self {
        if !seen.visible {
            Before::Hidden
        } else if seen.minimized {
            Before::Minimized
        } else {
            Before::Showing
        }
    }
}

/// What is done when the last prompt ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Restore {
    Hide,
    Minimize,
    /// Left as it is, and why.
    Nothing(&'static str),
}

/// From the first prompt's opening to the last one's end.
struct Span {
    id: u64,
    open: usize,
    before: Before,
    raised: bool,
    /// A re-map hid the window and has not shown it again yet (its prompt may be dropped
    /// during the pause).
    unmapped: bool,
}

pub struct PromptWindows<W: MainWindow> {
    window: W,
    how: Raise,
    pause: Duration,
    spans: Mutex<Option<Span>>,
    next_span: AtomicU64,
    /// The id of the open span (0: none). Read on the main thread.
    active: AtomicU64,
    /// The span during which the user opened the window themselves.
    user_opened_in: AtomicU64,
    /// When the open span started, for the focus log. Never held while waiting.
    opened: Mutex<Option<Instant>>,
}

impl<W: MainWindow> PromptWindows<W> {
    pub fn new(window: W, how: Raise) -> Self {
        Self {
            window,
            how,
            pause: REMAP_PAUSE,
            spans: Mutex::new(None),
            next_span: AtomicU64::new(1),
            active: AtomicU64::new(0),
            user_opened_in: AtomicU64::new(0),
            opened: Mutex::new(None),
        }
    }

    /// A prompt opens. Hold the guard until it has ended; dropping it is the end.
    pub fn begin(&self, kind: PromptKind) -> Prompt<'_, W> {
        let mut spans = self.spans.lock().expect("prompt spans");
        match spans.as_mut() {
            Some(span) => {
                span.open += 1;
                info!(
                    target: "lp_prompt",
                    kind = kind.name(),
                    open = span.open,
                    "browser prompt opened while another is open; the window state of the first counts"
                );
            }
            None => {
                let seen = self.window.seen();
                let id = self.next_span.fetch_add(1, Ordering::SeqCst);
                let before = Before::of(seen);
                *spans = Some(Span {
                    id,
                    open: 1,
                    before,
                    raised: false,
                    unmapped: false,
                });
                *self.opened.lock().expect("prompt opened") = Some(Instant::now());
                self.active.store(id, Ordering::SeqCst);
                info!(
                    target: "lp_prompt",
                    kind = kind.name(),
                    visible = seen.visible,
                    minimized = seen.minimized,
                    focused = seen.focused,
                    before = ?before,
                    "browser prompt opened; window state found"
                );
            }
        }
        Prompt {
            owner: self,
            kind,
            outcome: "withdrawn",
        }
    }

    /// The user opened the window (tray icon, starting the app again): it stays open after the
    /// prompts. Safe on the main thread.
    pub fn user_opened(&self) {
        let span = self.active.load(Ordering::SeqCst);
        if span != 0 {
            self.user_opened_in.store(span, Ordering::SeqCst);
            info!(target: "lp_prompt", "the user opened the window while a browser prompt is open; it stays open afterwards");
        }
    }

    /// The main window gained or lost the focus. Logged while a prompt is open. Safe on the main
    /// thread.
    pub fn focus_changed(&self, focused: bool) {
        let since = *self.opened.lock().expect("prompt opened");
        if let Some(since) = since {
            info!(
                target: "lp_prompt",
                focused,
                ms = since.elapsed().as_millis() as u64,
                "main window focus changed during a browser prompt"
            );
        }
    }

    /// Brings the window up. A re-map waits its pause here, without blocking a runtime thread;
    /// if the prompt's future is dropped meanwhile, `end` shows the window again where needed.
    async fn raise(&self, kind: PromptKind) {
        let seen = self.window.seen();
        let steps = steps(self.how, seen);
        let focus = {
            let mut spans = self.spans.lock().expect("prompt spans");
            if let Some(span) = spans.as_mut() {
                span.raised = true;
                if matches!(steps, Steps::Remap { .. }) {
                    span.unmapped = true;
                }
            }
            info!(
                target: "lp_prompt",
                kind = kind.name(),
                how = self.how.name(),
                steps = ?steps,
                visible = seen.visible,
                minimized = seen.minimized,
                focused = seen.focused,
                "raising the main window"
            );
            // Under the lock, so the flag and the step agree for `end`. Both only post a
            // request to the main thread.
            match steps {
                Steps::Nothing => return,
                Steps::Show { focus } => {
                    self.window.show(focus);
                    return;
                }
                Steps::Remap { focus } => {
                    self.window.hide();
                    focus
                }
            }
        };
        tokio::time::sleep(self.pause).await;
        let mut spans = self.spans.lock().expect("prompt spans");
        if let Some(span) = spans.as_mut() {
            span.unmapped = false;
        }
        self.window.show(focus);
        info!(target: "lp_prompt", kind = kind.name(), focus, "main window shown again after the re-map");
    }

    fn end(&self, kind: PromptKind, outcome: &'static str) {
        let mut spans = self.spans.lock().expect("prompt spans");
        let Some(span) = spans.as_mut() else {
            return;
        };
        span.open -= 1;
        if span.open > 0 {
            info!(
                target: "lp_prompt",
                kind = kind.name(),
                outcome,
                open = span.open,
                "browser prompt ended; another is still open"
            );
            return;
        }
        let user_opened = self.user_opened_in.load(Ordering::SeqCst) == span.id;
        let restore = decide(span.before, span.raised, user_opened);
        let ms = self
            .opened
            .lock()
            .expect("prompt opened")
            .take()
            .map(|t| t.elapsed().as_millis() as u64)
            .unwrap_or(0);
        self.active.store(0, Ordering::SeqCst);
        // A re-map was cut off in its pause (its prompt was dropped): the window is hidden
        // although it may have been on screen. Show it again (without taking the focus) unless
        // it is to be hidden anyway.
        let unmapped = span.unmapped;
        *spans = None;
        // Under the lock, so a prompt that opens right now reads the window after this step.
        // These calls only post a request to the main thread.
        if unmapped && restore != Restore::Hide {
            self.window.show(false);
        }
        match restore {
            Restore::Hide => self.window.hide(),
            Restore::Minimize => self.window.minimize(),
            Restore::Nothing(_) => {}
        }
        info!(
            target: "lp_prompt",
            kind = kind.name(),
            outcome,
            ms,
            restore = ?restore,
            unmapped,
            "browser prompt ended; window restored"
        );
    }
}

/// What to do with the window when the last prompt has ended.
fn decide(before: Before, raised: bool, user_opened: bool) -> Restore {
    if !raised {
        return Restore::Nothing("never raised");
    }
    if user_opened {
        return Restore::Nothing("the user opened it");
    }
    match before {
        Before::Hidden => Restore::Hide,
        Before::Minimized => Restore::Minimize,
        Before::Showing => Restore::Nothing("it was on screen"),
    }
}

/// One open prompt. Dropping it ends the prompt.
pub struct Prompt<'a, W: MainWindow> {
    owner: &'a PromptWindows<W>,
    kind: PromptKind,
    outcome: &'static str,
}

impl<W: MainWindow> Prompt<'_, W> {
    /// Brings the window up for this prompt.
    pub async fn raise(&self) {
        self.owner.raise(self.kind).await;
    }

    /// How the prompt ended, for the log (default: withdrawn, the future was dropped).
    pub fn ended(&mut self, outcome: &'static str) {
        self.outcome = outcome;
    }
}

impl<W: MainWindow> Drop for Prompt<'_, W> {
    fn drop(&mut self) {
        self.owner.end(self.kind, self.outcome);
    }
}

/// The app's main window through Tauri.
pub struct TauriWindow(pub tauri::AppHandle);

impl TauriWindow {
    fn main(&self) -> Option<tauri::WebviewWindow> {
        use tauri::Manager;
        self.0.get_webview_window("main")
    }
}

impl MainWindow for TauriWindow {
    fn seen(&self) -> Seen {
        let Some(w) = self.main() else {
            return Seen {
                visible: false,
                minimized: false,
                focused: false,
            };
        };
        Seen {
            visible: w.is_visible().unwrap_or(false),
            minimized: w.is_minimized().unwrap_or(false),
            focused: w.is_focused().unwrap_or(false),
        }
    }

    // Each call off the main thread posts one message to tao's event channel (a FIFO queue,
    // tauri-runtime-wry `send_user_message`); tao turns each into a GTK request on its own FIFO
    // channel, so a hide, the show after it and the focus after that are handled in that order.
    fn show(&self, focus: bool) {
        let Some(w) = self.main() else { return };
        let _ = w.unminimize();
        let _ = w.show();
        if focus {
            let _ = w.set_focus();
        }
    }

    fn hide(&self) {
        if let Some(w) = self.main() {
            let _ = w.hide();
        }
    }

    fn minimize(&self) {
        if let Some(w) = self.main() {
            let _ = w.minimize();
        }
    }
}

/// How prompts raise the window: `remap`, or in debug builds what `LOCALPASS_RAISE` names
/// (`present`, `remap`, `remap-no-focus`).
pub fn raise_mode() -> Raise {
    let asked = std::env::var("LOCALPASS_RAISE").ok();
    let how = mode_from(asked.as_deref(), cfg!(debug_assertions));
    info!(
        target: "lp_prompt",
        how = how.name(),
        asked = asked.as_deref().unwrap_or(""),
        debug_build = cfg!(debug_assertions),
        "browser prompts raise the window with this method (LOCALPASS_RAISE is read in debug builds only)"
    );
    how
}

/// The method for a `LOCALPASS_RAISE` value; release builds always re-map.
fn mode_from(asked: Option<&str>, debug_build: bool) -> Raise {
    if !debug_build {
        return Raise::Remap;
    }
    match asked.map(str::trim) {
        Some("present") => Raise::Present,
        Some("remap-no-focus") => Raise::RemapNoFocus,
        _ => Raise::Remap,
    }
}

/// Logs once per start which display backend GTK uses. GTK 3 tries Wayland first when
/// `WAYLAND_DISPLAY` is set, unless `GDK_BACKEND` says otherwise.
pub fn log_display_backend() {
    let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
    let gdk_backend = var("GDK_BACKEND");
    let wayland = var("WAYLAND_DISPLAY").is_some();
    let x11 = var("DISPLAY").is_some();
    let backend = match gdk_backend.as_deref().and_then(|b| b.split(',').next()) {
        Some(first) if first != "*" => first.to_string(),
        _ if wayland => "wayland".to_string(),
        _ if x11 => "x11".to_string(),
        _ => "unknown".to_string(),
    };
    info!(
        target: "lp_prompt",
        backend,
        gdk_backend = gdk_backend.as_deref().unwrap_or(""),
        session = var("XDG_SESSION_TYPE").as_deref().unwrap_or(""),
        desktop = var("XDG_CURRENT_DESKTOP").as_deref().unwrap_or(""),
        wayland_display = wayland,
        x11_display = x11,
        "display backend"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::Poll;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Show(bool),
        Hide,
        Minimize,
    }

    #[derive(Clone)]
    struct Fake {
        seen: Arc<Mutex<Seen>>,
        ops: Arc<Mutex<Vec<Op>>>,
    }

    impl Fake {
        fn new(visible: bool, minimized: bool, focused: bool) -> Self {
            Self {
                seen: Arc::new(Mutex::new(Seen {
                    visible,
                    minimized,
                    focused,
                })),
                ops: Arc::default(),
            }
        }
        fn ops(&self) -> Vec<Op> {
            self.ops.lock().unwrap().clone()
        }
    }

    impl MainWindow for Fake {
        fn seen(&self) -> Seen {
            *self.seen.lock().unwrap()
        }
        fn show(&self, focus: bool) {
            self.ops.lock().unwrap().push(Op::Show(focus));
            *self.seen.lock().unwrap() = Seen {
                visible: true,
                minimized: false,
                focused: focus,
            };
        }
        fn hide(&self) {
            self.ops.lock().unwrap().push(Op::Hide);
            let mut seen = self.seen.lock().unwrap();
            seen.visible = false;
            seen.focused = false;
        }
        fn minimize(&self) {
            self.ops.lock().unwrap().push(Op::Minimize);
            let mut seen = self.seen.lock().unwrap();
            seen.minimized = true;
            seen.focused = false;
        }
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
    }

    fn block<F: Future>(f: F) -> F::Output {
        rt().block_on(f)
    }

    /// Polls once inside the runtime (tokio's timer needs it); true when still pending.
    fn poll_once<F: Future>(rt: &tokio::runtime::Runtime, mut fut: Pin<&mut F>) -> bool {
        rt.block_on(std::future::poll_fn(|cx| {
            Poll::Ready(fut.as_mut().poll(cx).is_pending())
        }))
    }

    fn with(how: Raise, fake: &Fake, pause: Duration) -> PromptWindows<Fake> {
        let mut w = PromptWindows::new(fake.clone(), how);
        w.pause = pause;
        w
    }

    /// The default method (`remap`), no pause.
    fn windows(visible: bool, minimized: bool, focused: bool) -> (Fake, PromptWindows<Fake>) {
        let fake = Fake::new(visible, minimized, focused);
        let w = with(Raise::Remap, &fake, Duration::ZERO);
        (fake, w)
    }

    /// One raise and its end; the ops it caused.
    fn one_raise(how: Raise, visible: bool, focused: bool) -> Vec<Op> {
        let fake = Fake::new(visible, false, focused);
        let w = with(how, &fake, Duration::ZERO);
        let p = w.begin(PromptKind::Unlock);
        block(p.raise());
        fake.ops()
    }

    const S: Op = Op::Show(true);

    #[test]
    fn a_hidden_window_is_hidden_again_after_the_prompt() {
        let (fake, w) = windows(false, false, false);
        let mut p = w.begin(PromptKind::Unlock);
        block(p.raise());
        p.ended("unlocked");
        drop(p);
        assert_eq!(fake.ops(), [S, Op::Hide]);
        assert!(!fake.seen().visible);
    }

    #[test]
    fn a_window_that_was_on_screen_is_remapped_and_stays() {
        let (fake, w) = windows(true, false, false);
        let p = w.begin(PromptKind::Overwrite);
        block(p.raise());
        drop(p);
        assert_eq!(fake.ops(), [Op::Hide, S]);
        assert!(fake.seen().visible);
    }

    #[test]
    fn a_minimized_window_is_minimized_again_never_hidden() {
        let (fake, w) = windows(true, true, false);
        let p = w.begin(PromptKind::Verify);
        block(p.raise());
        drop(p);
        assert_eq!(fake.ops(), [Op::Hide, S, Op::Minimize]);
        assert!(fake.seen().visible, "still in the taskbar, not in the tray");
    }

    #[test]
    fn nested_prompts_restore_once_and_do_not_blink_the_focused_window() {
        // A pairing asks for an unlock first and then shows its own dialog.
        let (fake, w) = windows(false, false, false);
        let pairing = w.begin(PromptKind::Pairing);
        {
            let unlock = w.begin(PromptKind::Unlock);
            block(unlock.raise());
        }
        assert_eq!(fake.ops(), [S], "no hide between the unlock and the dialog");
        // The window has the focus from the unlock: the dialog's raise leaves it alone.
        block(pairing.raise());
        assert_eq!(fake.ops(), [S]);
        drop(pairing);
        assert_eq!(fake.ops(), [S, Op::Hide]);
    }

    #[test]
    fn overlapping_prompts_restore_after_the_last_whatever_the_order() {
        let (fake, w) = windows(false, false, false);
        let a = w.begin(PromptKind::Unlock);
        block(a.raise());
        // The second one finds the window up already: the first one's state still counts.
        let b = w.begin(PromptKind::Verify);
        // The user went back to the browser meanwhile: the second raise re-maps.
        fake.seen.lock().unwrap().focused = false;
        block(b.raise());
        drop(a);
        assert_eq!(fake.ops(), [S, Op::Hide, S]);
        drop(b);
        assert_eq!(fake.ops(), [S, Op::Hide, S, Op::Hide]);
    }

    #[test]
    fn a_dropped_prompt_restores_too() {
        let (fake, w) = windows(false, false, false);
        let dropped = async {
            let p = w.begin(PromptKind::Unlock);
            p.raise().await;
            std::future::pending::<()>().await;
            drop(p);
        };
        // Poll it once (it raises and waits), then drop it as the broker does when the client leaves.
        let rt = rt();
        let mut fut = Box::pin(dropped);
        assert!(poll_once(&rt, fut.as_mut()));
        assert_eq!(fake.ops(), [S]);
        drop(fut);
        assert_eq!(fake.ops(), [S, Op::Hide]);
    }

    #[test]
    fn a_window_the_user_opened_meanwhile_stays_open() {
        let (fake, w) = windows(false, false, false);
        let p = w.begin(PromptKind::Unlock);
        block(p.raise());
        w.user_opened();
        drop(p);
        assert_eq!(fake.ops(), [S]);

        // That was for that span only: the next prompt hides again.
        fake.hide();
        let p = w.begin(PromptKind::Unlock);
        block(p.raise());
        drop(p);
        assert_eq!(fake.ops(), [S, Op::Hide, S, Op::Hide]);
    }

    #[test]
    fn opening_the_window_with_no_prompt_open_changes_nothing_later() {
        let (fake, w) = windows(false, false, false);
        w.user_opened();
        let p = w.begin(PromptKind::Pairing);
        block(p.raise());
        drop(p);
        assert_eq!(fake.ops(), [S, Op::Hide]);
    }

    #[test]
    fn a_quick_unlock_that_never_raised_leaves_the_window_alone() {
        let (fake, w) = windows(false, false, false);
        let mut p = w.begin(PromptKind::Unlock);
        p.ended("quick unlock");
        drop(p);
        assert!(fake.ops().is_empty());

        // Nested under a pairing that does raise: the pairing's raise counts for the span.
        let pairing = w.begin(PromptKind::Pairing);
        drop(w.begin(PromptKind::Unlock));
        block(pairing.raise());
        drop(pairing);
        assert_eq!(fake.ops(), [S, Op::Hide]);
    }

    #[test]
    fn the_window_state_is_read_once_per_span() {
        let (fake, w) = windows(true, false, false);
        let a = w.begin(PromptKind::Unlock);
        block(a.raise());
        // The window is hidden by someone while the first prompt is open; the second prompt does
        // not take that as the state to go back to.
        fake.hide();
        let b = w.begin(PromptKind::Verify);
        block(b.raise());
        drop(b);
        drop(a);
        assert_eq!(fake.ops(), [Op::Hide, S, Op::Hide, S]);
    }

    #[test]
    fn a_focused_window_is_never_remapped_whatever_the_method() {
        for how in [Raise::Present, Raise::Remap, Raise::RemapNoFocus] {
            assert_eq!(one_raise(how, true, true), [], "{how:?}");
        }
    }

    #[test]
    fn a_window_behind_the_browser_is_remapped_in_order() {
        assert_eq!(one_raise(Raise::Remap, true, false), [Op::Hide, S]);
        assert_eq!(
            one_raise(Raise::RemapNoFocus, true, false),
            [Op::Hide, Op::Show(false)]
        );
        assert_eq!(one_raise(Raise::Present, true, false), [S]);
    }

    #[test]
    fn a_hidden_window_is_shown_without_a_hide_first() {
        assert_eq!(one_raise(Raise::Remap, false, false), [S]);
        assert_eq!(
            one_raise(Raise::RemapNoFocus, false, false),
            [Op::Show(false)]
        );
        assert_eq!(one_raise(Raise::Present, false, false), [S]);
        // Right after a hide GTK may still call the window active: it is shown all the same.
        assert_eq!(one_raise(Raise::Remap, false, true), [S]);
    }

    #[test]
    fn the_remap_pause_does_not_block_and_a_cut_off_remap_shows_the_window_again() {
        // A pause far longer than the test: polling must come back at once (no thread sleep).
        let fake = Fake::new(true, false, false);
        let w = with(Raise::Remap, &fake, Duration::from_secs(3600));
        let rt = rt();
        let prompt = async {
            let p = w.begin(PromptKind::Verify);
            p.raise().await;
        };
        let mut fut = Box::pin(prompt);
        assert!(poll_once(&rt, fut.as_mut()), "waits in the pause");
        assert_eq!(fake.ops(), [Op::Hide]);
        // The client leaves during the pause: the window was on screen, so it comes back,
        // without taking the focus.
        {
            let _in_rt = rt.enter();
            drop(fut);
        }
        assert_eq!(fake.ops(), [Op::Hide, Op::Show(false)]);
        assert!(fake.seen().visible);
    }

    #[test]
    fn a_cut_off_remap_of_a_window_that_was_hidden_stays_hidden() {
        let fake = Fake::new(false, false, false);
        let w = with(Raise::Remap, &fake, Duration::from_secs(3600));
        let rt = rt();
        let prompt = async {
            let p = w.begin(PromptKind::Verify);
            // Shown meanwhile (not by the user's opening), and not focused: the raise re-maps.
            *fake.seen.lock().unwrap() = Seen {
                visible: true,
                minimized: false,
                focused: false,
            };
            p.raise().await;
        };
        let mut fut = Box::pin(prompt);
        assert!(poll_once(&rt, fut.as_mut()));
        {
            let _in_rt = rt.enter();
            drop(fut);
        }
        assert_eq!(fake.ops(), [Op::Hide, Op::Hide]);
    }

    #[test]
    fn steps_table() {
        let seen = |visible, minimized, focused| Seen {
            visible,
            minimized,
            focused,
        };
        use Raise::*;
        for how in [Present, Remap, RemapNoFocus] {
            assert_eq!(steps(how, seen(true, false, true)), Steps::Nothing);
        }
        let behind = seen(true, false, false);
        let hidden = seen(false, false, false);
        let minimized = seen(true, true, false);
        assert_eq!(steps(Present, behind), Steps::Show { focus: true });
        assert_eq!(steps(Remap, behind), Steps::Remap { focus: true });
        assert_eq!(steps(RemapNoFocus, behind), Steps::Remap { focus: false });
        assert_eq!(steps(Remap, hidden), Steps::Show { focus: true });
        assert_eq!(steps(RemapNoFocus, hidden), Steps::Show { focus: false });
        assert_eq!(steps(Remap, minimized), Steps::Remap { focus: true });
        // Stale "focused" on a hidden or minimized window does not count.
        assert_eq!(
            steps(Remap, seen(false, false, true)),
            Steps::Show { focus: true }
        );
        assert_eq!(
            steps(Remap, seen(true, true, true)),
            Steps::Remap { focus: true }
        );
    }

    #[test]
    fn the_method_from_the_environment() {
        // Release builds always re-map, whatever is set.
        for asked in [None, Some("present"), Some("remap-no-focus"), Some("x")] {
            assert_eq!(mode_from(asked, false), Raise::Remap);
        }
        assert_eq!(mode_from(None, true), Raise::Remap);
        assert_eq!(mode_from(Some("remap"), true), Raise::Remap);
        assert_eq!(mode_from(Some(" present "), true), Raise::Present);
        assert_eq!(mode_from(Some("remap-no-focus"), true), Raise::RemapNoFocus);
        assert_eq!(mode_from(Some("other"), true), Raise::Remap);
    }

    #[test]
    fn decide_table() {
        use Before::*;
        assert_eq!(decide(Hidden, true, false), Restore::Hide);
        assert_eq!(decide(Minimized, true, false), Restore::Minimize);
        assert!(matches!(decide(Showing, true, false), Restore::Nothing(_)));
        for before in [Hidden, Minimized, Showing] {
            assert!(matches!(decide(before, false, false), Restore::Nothing(_)));
            assert!(matches!(decide(before, true, true), Restore::Nothing(_)));
        }
    }
}
