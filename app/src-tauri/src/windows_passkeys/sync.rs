//! Keeps Windows' credential cache in step with the vault. Windows only offers (and signs in with)
//! passkeys it has in its cache, so passkeys that arrive from the phone, or are deleted, must reach it.
//! The provider does the actual work (`sync-cache`, it has the package identity); this module decides
//! when: a few seconds after the last vault change, one run at a time, never while the vault is locked.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lp_engine::{Engine, EngineEvent, VaultStatus};
use tokio::sync::{Notify, broadcast::error::RecvError};
use tracing::{debug, info, warn};

use super::flows::{self, CacheSync, Ctx};
use super::manager::Manager;

/// How long the vault must be quiet before a run (a sync from the phone sends a burst of changes).
pub const QUIET: Duration = Duration::from_secs(3);

/// The timing rules, without clocks or tasks so they can be tested: a run is due `quiet` after the
/// last event, and only one run is ever in progress.
#[derive(Debug)]
pub struct Debounce {
    quiet: Duration,
    last_event: Option<Instant>,
    running: bool,
}

impl Debounce {
    pub fn new(quiet: Duration) -> Self {
        Self {
            quiet,
            last_event: None,
            running: false,
        }
    }

    /// Something changed; (re)start the quiet period.
    pub fn event(&mut self, now: Instant) {
        self.last_event = Some(now);
    }

    /// How long to wait before asking again: `None` when nothing is waiting (or a run is in progress,
    /// whose end will ask).
    pub fn wait(&self, now: Instant) -> Option<Duration> {
        if self.running {
            return None;
        }
        self.last_event
            .map(|at| (at + self.quiet).saturating_duration_since(now))
    }

    /// True once if a run is due now; the caller must call [`Debounce::finish`] afterwards.
    pub fn take(&mut self, now: Instant) -> bool {
        if self.running || self.wait(now) != Some(Duration::ZERO) {
            return false;
        }
        self.last_event = None;
        self.running = true;
        true
    }

    pub fn finish(&mut self) {
        self.running = false;
    }

    /// Forgets a pending run (the vault is locked: unlocking raises a new event).
    pub fn clear(&mut self) {
        self.last_event = None;
    }
}

/// Lets the commands ask for a sync (after setting up or pairing) without waiting for a vault change.
#[derive(Clone, Default)]
pub struct Nudge(Arc<Notify>);

impl Nudge {
    pub fn now(&self) {
        self.0.notify_one();
    }
}

/// Starts the task that follows the engine's events. Called once, from the app's setup.
pub fn spawn(manager: Arc<Manager>, engine: Engine, nudge: Nudge) {
    let mut events = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        let mut debounce = Debounce::new(QUIET);
        loop {
            let wait = debounce.wait(Instant::now());
            tokio::select! {
                event = events.recv() => match event {
                    Ok(EngineEvent::VaultChanged | EngineEvent::Status(VaultStatus::Unlocked))
                    | Err(RecvError::Lagged(_)) => debounce.event(Instant::now()),
                    Ok(_) => {}
                    Err(RecvError::Closed) => break,
                },
                () = nudge.0.notified() => debounce.event(Instant::now()),
                () = async { tokio::time::sleep(wait.unwrap_or_default()).await }, if wait.is_some() => {
                    if !debounce.take(Instant::now()) {
                        continue;
                    }
                    if engine.status() == VaultStatus::Unlocked {
                        let manager = manager.clone();
                        let result = tauri::async_runtime::spawn_blocking(move || manager.with_ctx(run_once)).await;
                        match result {
                            Ok(Ok(CacheSync::Done(summary))) => info!(%summary, "windows passkeys: credential cache synced"),
                            Ok(Ok(CacheSync::Skipped(why))) => debug!(why, "windows passkeys: credential cache not synced"),
                            Ok(Err(why)) => warn!(%why, "windows passkeys: credential cache sync failed"),
                            Err(e) => warn!(error = %e, "windows passkeys: credential cache sync task failed"),
                        }
                    } else {
                        debounce.clear();
                    }
                    debounce.finish();
                }
            }
        }
    });
}

fn run_once(ctx: &Ctx<'_>) -> Result<CacheSync, String> {
    flows::sync_cache(ctx)
}
