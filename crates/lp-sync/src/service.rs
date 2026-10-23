//! Background sync: TCP listener, mDNS, scheduler and address book.
//!
//! The service runs whether the vault is locked or not; it only moves signed
//! ciphertext. Peers are dialed when they appear on the network, after local
//! edits, after we learn something new (so changes hop onward), and
//! periodically. Known addresses are retried after network changes, so a
//! device arriving home can reach the others even if mDNS is flaky.

use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lp_core::crypto::{Key32, NoiseKeypair};
use lp_core::hlc::now_ms;
use lp_core::ids::{DeviceId, OpRef};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore, broadcast, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::discovery::{
    Discovery, DiscoveryEvent, Found, SYNC_SERVICE, device_tag, epoch_of, nearby_epochs, vault_tag,
};
use crate::netutil::{self, is_local_ip, parse_dialable};
use crate::pairing::{self, InviteFailure, InviteHandler, InviterParams, MAX_FAILED_ATTEMPTS};
use crate::protocol::{AddrHint, MAX_HINTS, PREAMBLE_PAIR, PREAMBLE_SYNC};
use crate::replica::Replica;
use crate::session::{self, SessionOutcome, SessionParams};
use crate::{Result, SyncError};

pub const DEFAULT_PORT: u16 = 47821;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(1500);
const PREAMBLE_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_INBOUND: usize = 16;
/// Inbound connections one address may hold at once, so a host that opens sockets and sends nothing
/// cannot use up every slot and lock out the user's other devices (a peer needs one or two).
const MAX_INBOUND_PER_IP: usize = 4;
const LOCAL_CHANGE_DEBOUNCE: Duration = Duration::from_millis(1500);
const BASE_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// An mDNS sighting counts as "visible" for this long unless removed.
const VISIBLE_FOR: Duration = Duration::from_secs(15 * 60);
/// Address-book hints older than this aren't passed on.
const HINT_MAX_AGE_MS: u64 = 14 * 24 * 60 * 60 * 1000;
/// Clock differences above this are reported.
pub const SKEW_WARN_MS: i64 = 60_000;

#[derive(Clone, Debug)]
pub struct SyncConfig {
    /// Preferred TCP port; falls back to an ephemeral port if taken. 0 = ephemeral.
    pub port: u16,
    /// Interface to listen on. `0.0.0.0` normally; loopback for tests.
    pub bind: Ipv4Addr,
    pub mdns: bool,
    pub tick: Duration,
    /// Re-sync a visible peer at least this often even without changes.
    pub resync_every: Duration,
    /// Retry stored addresses of not-visible peers this often.
    pub addrbook_every: Duration,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            bind: Ipv4Addr::UNSPECIFIED,
            mdns: true,
            tick: Duration::from_secs(10),
            resync_every: Duration::from_secs(300),
            addrbook_every: Duration::from_secs(600),
        }
    }
}

pub struct NetKeys {
    pub noise: NoiseKeypair,
    pub nk: Key32,
}

#[derive(Clone, Debug)]
pub enum SyncEvent {
    Listening {
        port: u16,
    },
    PeerDiscovered {
        device: Option<DeviceId>,
        addrs: Vec<SocketAddr>,
    },
    SyncCompleted {
        peer: DeviceId,
        addr: SocketAddr,
        received: usize,
        sent: usize,
        roster_changed: bool,
        clock_skew_ms: i64,
    },
    SyncFailed {
        addr: SocketAddr,
        device: Option<DeviceId>,
        error: String,
    },
    ForkDetected {
        op: OpRef,
        reported_by: DeviceId,
    },
    NetworkChanged {
        ips: Vec<IpAddr>,
    },
    InviteWrongCode {
        attempts_left: u32,
    },
    InviteFinished {
        device: Option<DeviceId>,
        error: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum PeerKey {
    Device(DeviceId),
    Addr(SocketAddr),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerSource {
    Mdns,
    Manual,
    AddressBook,
    Inbound,
}

#[derive(Clone, Debug)]
struct PeerEntry {
    device: Option<DeviceId>,
    addrs: Vec<SocketAddr>,
    allow_public: bool,
    source: PeerSource,
    seen: Option<Instant>,
    attempt: Option<Instant>,
    ok: Option<Instant>,
    failures: u32,
    busy: bool,
    synced_counter: u64,
    addrbook_due: bool,
    last_error: Option<String>,
}

impl PeerEntry {
    fn new(
        device: Option<DeviceId>,
        addrs: Vec<SocketAddr>,
        source: PeerSource,
        allow_public: bool,
    ) -> Self {
        Self {
            device,
            addrs,
            allow_public,
            source,
            seen: None,
            attempt: None,
            ok: None,
            failures: 0,
            busy: false,
            synced_counter: 0,
            addrbook_due: false,
            last_error: None,
        }
    }

    fn visible(&self, now: Instant) -> bool {
        self.seen
            .is_some_and(|t| now.duration_since(t) < VISIBLE_FOR)
    }

    fn merge_addrs(&mut self, fresh: &[SocketAddr]) {
        let mut merged = fresh.to_vec();
        merged.extend(self.addrs.iter().filter(|a| !fresh.contains(a)));
        merged.truncate(8);
        self.addrs = merged;
    }
}

/// Status of one peer, for UIs and diagnostics.
#[derive(Clone, Debug)]
pub struct PeerSnapshot {
    pub device: Option<DeviceId>,
    pub addrs: Vec<SocketAddr>,
    pub source: PeerSource,
    pub visible: bool,
    pub busy: bool,
    pub last_ok_secs_ago: Option<u64>,
    pub failures: u32,
    pub last_error: Option<String>,
}

/// Result of one dial in [`SyncService::sync_now`].
#[derive(Clone, Debug)]
pub struct SyncAttempt {
    pub device: Option<DeviceId>,
    pub addr: SocketAddr,
    pub result: std::result::Result<(usize, usize), String>,
}

struct Invite {
    code: String,
    expires: Instant,
    failures: u32,
    handler: Arc<dyn InviteHandler>,
}

struct State {
    peers: BTreeMap<PeerKey, PeerEntry>,
    mdns_names: BTreeMap<String, PeerKey>,
    invite: Option<Invite>,
    ips: Vec<IpAddr>,
    addrbook_at: Option<Instant>,
    epoch: u64,
}

struct Inner {
    replica: Arc<Replica>,
    keys: NetKeys,
    config: SyncConfig,
    port: u16,
    events: broadcast::Sender<SyncEvent>,
    state: Mutex<State>,
    wake: Notify,
    counter: AtomicU64,
    inbound: Arc<Semaphore>,
    /// Inbound connections now open, by remote address (see `IpSlot`).
    inbound_by_ip: Arc<PerIp>,
    pairing: tokio::sync::Mutex<()>,
    discovery: Option<Discovery>,
    shutdown: watch::Sender<bool>,
    /// The runtime the service runs on. UI threads call into the service without being
    /// inside any runtime, so `tokio::spawn` there would panic.
    rt: tokio::runtime::Handle,
}

pub struct SyncService {
    inner: Arc<Inner>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl SyncService {
    pub async fn start(replica: Arc<Replica>, keys: NetKeys, config: SyncConfig) -> Result<Self> {
        let listener = bind(config.bind, config.port).await?;
        let port = listener.local_addr()?.port();
        let discovery = if config.mdns {
            match Discovery::start() {
                Ok(d) => Some(d),
                Err(e) => {
                    warn!(error = %e, "mDNS unavailable; only known and manual addresses will be used");
                    None
                }
            }
        } else {
            None
        };

        let mut peers = BTreeMap::new();
        for m in replica.store().manual_peers()? {
            if let Ok(addr) = m.addr.parse::<SocketAddr>() {
                peers.insert(
                    PeerKey::Addr(addr),
                    PeerEntry::new(None, vec![addr], PeerSource::Manual, m.allow_public),
                );
            }
        }
        let (shutdown, _) = watch::channel(false);
        let (events, _) = broadcast::channel(256);
        let inner = Arc::new(Inner {
            replica,
            keys,
            config,
            port,
            events,
            state: Mutex::new(State {
                peers,
                mdns_names: BTreeMap::new(),
                invite: None,
                ips: netutil::local_ips(),
                addrbook_at: None,
                epoch: 0,
            }),
            wake: Notify::new(),
            counter: AtomicU64::new(0),
            inbound: Arc::new(Semaphore::new(MAX_INBOUND)),
            inbound_by_ip: Arc::new(PerIp::default()),
            pairing: tokio::sync::Mutex::new(()),
            discovery,
            shutdown,
            rt: tokio::runtime::Handle::current(),
        });

        let mut tasks = vec![
            tokio::spawn(accept_loop(inner.clone(), listener)),
            tokio::spawn(scheduler_loop(inner.clone())),
        ];
        if let Some(d) = &inner.discovery {
            inner.advertise();
            match d.browse(SYNC_SERVICE) {
                Ok(rx) => tasks.push(tokio::spawn(mdns_loop(inner.clone(), rx))),
                Err(e) => warn!(error = %e, "mDNS browse failed"),
            }
        }
        info!(port, "sync service listening");
        let _ = inner.events.send(SyncEvent::Listening { port });
        Ok(Self {
            inner,
            tasks: Mutex::new(tasks),
        })
    }

    pub fn port(&self) -> u16 {
        self.inner.port
    }

    /// Local addresses peers can use to reach this device.
    pub fn listen_addrs(&self) -> Vec<SocketAddr> {
        self.inner.listen_addrs()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SyncEvent> {
        self.inner.events.subscribe()
    }

    pub fn replica(&self) -> &Arc<Replica> {
        &self.inner.replica
    }

    /// Call after writing local ops: peers are synced shortly (debounced).
    pub fn notify_local_change(&self) {
        self.inner.counter.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.clone();
        self.inner.rt.spawn(async move {
            tokio::time::sleep(LOCAL_CHANGE_DEBOUNCE).await;
            inner.wake.notify_one();
        });
    }

    /// Dials every reachable candidate now (visible, manual and stored
    /// addresses) and waits for all sessions to finish.
    pub async fn sync_now(&self) -> Vec<SyncAttempt> {
        self.inner.state.lock().expect("state").addrbook_at = None;
        self.inner.housekeeping();
        let now = Instant::now();
        let keys: Vec<PeerKey> = {
            let mut st = self.inner.state.lock().expect("state");
            st.peers
                .iter_mut()
                .filter(|(_, e)| {
                    !e.busy
                        && !e.addrs.is_empty()
                        && (e.visible(now) || e.source == PeerSource::Manual || e.addrbook_due)
                })
                .map(|(k, e)| {
                    e.busy = true;
                    e.attempt = Some(now);
                    e.addrbook_due = false;
                    *k
                })
                .collect()
        };
        let handles: Vec<_> = keys
            .into_iter()
            .map(|k| {
                let inner = self.inner.clone();
                tokio::spawn(async move { inner.dial(k).await })
            })
            .collect();
        let mut out = Vec::new();
        for h in handles {
            if let Ok(Some(a)) = h.await {
                out.push(a);
            }
        }
        out
    }

    /// One-off sync with an explicit address.
    pub async fn sync_with(&self, addr: SocketAddr, allow_public: bool) -> Result<SessionOutcome> {
        if !allow_public && !is_local_ip(&addr.ip()) {
            return Err(SyncError::Protocol(format!(
                "{addr} is not a local-network address"
            )));
        }
        let counter = self.inner.counter.load(Ordering::SeqCst);
        let res = self.inner.connect_and_sync(&[addr], allow_public).await;
        let (addr, res) = match res {
            Ok((a, o)) => (a, Ok(o)),
            Err((a, e)) => (a, Err(e)),
        };
        self.inner
            .finish_session(None, addr, counter, &res, PeerSource::Manual);
        res
    }

    pub fn add_manual_peer(&self, addr: SocketAddr, allow_public: bool) -> Result<()> {
        self.inner
            .replica
            .store()
            .add_manual_peer(&addr.to_string(), allow_public)?;
        self.inner.state.lock().expect("state").peers.insert(
            PeerKey::Addr(addr),
            PeerEntry::new(None, vec![addr], PeerSource::Manual, allow_public),
        );
        self.inner.wake.notify_one();
        Ok(())
    }

    pub fn remove_manual_peer(&self, addr: SocketAddr) -> Result<()> {
        self.inner
            .replica
            .store()
            .remove_manual_peer(&addr.to_string())?;
        self.inner
            .state
            .lock()
            .expect("state")
            .peers
            .remove(&PeerKey::Addr(addr));
        Ok(())
    }

    /// Opens an invite for `code` (see [`pairing`]). Replaces any open invite.
    pub fn start_invite(&self, code: String, handler: Arc<dyn InviteHandler>) -> Result<()> {
        if let Some(d) = &self.inner.discovery
            && let Err(e) = d.advertise_pairing(self.inner.port)
        {
            warn!(error = %e, "could not advertise the invite; joiner must use the address or QR code");
        }
        self.inner.state.lock().expect("state").invite = Some(Invite {
            code,
            expires: Instant::now() + pairing::INVITE_TTL,
            failures: 0,
            handler,
        });
        Ok(())
    }

    pub fn cancel_invite(&self) {
        self.inner.close_invite();
    }

    pub fn invite_active(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("state")
            .invite
            .as_ref()
            .is_some_and(|i| i.expires > Instant::now())
    }

    pub fn peers(&self) -> Vec<PeerSnapshot> {
        let now = Instant::now();
        let st = self.inner.state.lock().expect("state");
        st.peers
            .values()
            .map(|e| PeerSnapshot {
                device: e.device,
                addrs: e.addrs.clone(),
                source: e.source,
                visible: e.visible(now),
                busy: e.busy,
                last_ok_secs_ago: e.ok.map(|t| now.duration_since(t).as_secs()),
                failures: e.failures,
                last_error: e.last_error.clone(),
            })
            .collect()
    }

    pub fn shutdown(&self) {
        let _ = self.inner.shutdown.send(true);
        for t in self.tasks.lock().expect("tasks").drain(..) {
            t.abort();
        }
        if let Some(d) = &self.inner.discovery {
            d.shutdown();
        }
    }
}

impl Drop for SyncService {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn bind(ip: Ipv4Addr, port: u16) -> Result<TcpListener> {
    match TcpListener::bind((ip, port)).await {
        Ok(l) => Ok(l),
        Err(e) if port != 0 => {
            warn!(port, error = %e, "port unavailable; using an ephemeral port");
            Ok(TcpListener::bind((ip, 0)).await?)
        }
        Err(e) => Err(e.into()),
    }
}

/// How many inbound connections each remote address holds open.
#[derive(Default)]
struct PerIp {
    open: Mutex<HashMap<IpAddr, usize>>,
}

impl PerIp {
    fn take(self: &Arc<Self>, ip: IpAddr) -> Option<IpSlot> {
        let mut open = self.open.lock().expect("inbound");
        let count = open.entry(ip).or_insert(0);
        if *count >= MAX_INBOUND_PER_IP {
            return None;
        }
        *count += 1;
        Some(IpSlot {
            owner: self.clone(),
            ip,
        })
    }
}

/// One of the connections an address may hold open; given back when dropped.
struct IpSlot {
    owner: Arc<PerIp>,
    ip: IpAddr,
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut open = self.owner.open.lock().expect("inbound");
        if let Some(count) = open.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.ip);
            }
        }
    }
}

async fn accept_loop(inner: Arc<Inner>, listener: TcpListener) {
    let mut stop = inner.shutdown.subscribe();
    loop {
        tokio::select! {
            res = listener.accept() => match res {
                Ok((stream, remote)) => {
                    if inner.is_shutdown() {
                        break;
                    }
                    let Some(slot) = inner.inbound_by_ip.take(remote.ip()) else {
                        debug!(%remote, "too many inbound sessions from this address; dropping");
                        continue;
                    };
                    let Ok(permit) = inner.inbound.clone().try_acquire_owned() else {
                        debug!(%remote, "too many inbound sessions; dropping");
                        continue;
                    };
                    let inner = inner.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _slot = slot;
                        inner.handle_inbound(stream, remote).await;
                    });
                }
                Err(e) => {
                    warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
            _ = stop.changed() => break,
        }
    }
}

async fn scheduler_loop(inner: Arc<Inner>) {
    let mut stop = inner.shutdown.subscribe();
    loop {
        inner.housekeeping();
        for key in inner.due_peers() {
            let inner = inner.clone();
            tokio::spawn(async move {
                inner.dial(key).await;
            });
        }
        tokio::select! {
            _ = tokio::time::sleep(inner.config.tick) => {}
            _ = inner.wake.notified() => {}
            _ = stop.changed() => break,
        }
    }
}

async fn mdns_loop(inner: Arc<Inner>, mut rx: mpsc::UnboundedReceiver<DiscoveryEvent>) {
    let mut stop = inner.shutdown.subscribe();
    loop {
        let ev = tokio::select! {
            ev = rx.recv() => match ev { Some(e) => e, None => break },
            _ = stop.changed() => break,
        };
        match ev {
            DiscoveryEvent::Found(f) => inner.on_found(f),
            DiscoveryEvent::Removed(name) => inner.on_removed(&name),
        }
    }
}

impl Inner {
    fn listen_addrs(&self) -> Vec<SocketAddr> {
        if self.config.bind.is_loopback() {
            vec![SocketAddr::from((self.config.bind, self.port))]
        } else {
            netutil::local_addrs(self.port)
        }
    }

    /// Set synchronously by `shutdown()`; nothing new starts afterwards.
    fn is_shutdown(&self) -> bool {
        *self.shutdown.borrow()
    }

    fn emit(&self, ev: SyncEvent) {
        let _ = self.events.send(ev);
    }

    fn session_params(&self) -> SessionParams<'_> {
        SessionParams {
            replica: &self.replica,
            noise_secret: self.keys.noise.secret(),
            nk: &self.keys.nk,
            listen: self
                .listen_addrs()
                .iter()
                .map(ToString::to_string)
                .collect(),
            hints: self.hints(),
        }
    }

    /// Recently seen addresses of other members, shared in `Hello`.
    fn hints(&self) -> Vec<AddrHint> {
        let me = self.replica.me();
        let cutoff = now_ms().saturating_sub(HINT_MAX_AGE_MS);
        let mut rows = self.replica.store().peers().unwrap_or_default();
        rows.retain(|r| {
            r.device != me && !r.addrs.is_empty() && r.last_seen_ms.is_some_and(|t| t >= cutoff)
        });
        rows.sort_by_key(|r| std::cmp::Reverse(r.last_seen_ms));
        rows.into_iter()
            .take(MAX_HINTS)
            .map(|r| AddrHint {
                device: r.device,
                addrs: r.addrs,
                seen_ms: r.last_seen_ms.unwrap_or(0),
            })
            .collect()
    }

    fn advertise(&self) {
        let Some(d) = &self.discovery else { return };
        let epoch = epoch_of(now_ms());
        let vt = vault_tag(&self.keys.nk, epoch);
        let dt = device_tag(&self.keys.nk, &self.replica.me(), epoch);
        match d.advertise_sync(self.port, &vt, &dt) {
            Ok(()) => self.state.lock().expect("state").epoch = epoch,
            Err(e) => warn!(error = %e, "mDNS advertise failed"),
        }
    }

    fn on_found(&self, mut f: Found) {
        netutil::rank_dial_addrs(&mut f.addrs);
        if f.addrs.is_empty() {
            return;
        }
        let epochs = nearby_epochs(now_ms());
        let Some(vt) = f.vault_tag.as_deref() else {
            return;
        };
        if !epochs.iter().any(|e| vault_tag(&self.keys.nk, *e) == vt) {
            return; // another vault, or not a member
        }
        let view = self.replica.view();
        let device = f.device_tag.as_deref().and_then(|dt| {
            view.devices().map(|(id, _)| *id).find(|id| {
                epochs
                    .iter()
                    .any(|e| device_tag(&self.keys.nk, id, *e) == dt)
            })
        });
        if device == Some(self.replica.me()) {
            return;
        }
        if let Some(d) = device
            && !view.is_active(&d)
        {
            return;
        }
        let key = device
            .map(PeerKey::Device)
            .unwrap_or(PeerKey::Addr(f.addrs[0]));
        let now = Instant::now();
        let news = {
            let mut st = self.state.lock().expect("state");
            let entry = st
                .peers
                .entry(key)
                .or_insert_with(|| PeerEntry::new(device, Vec::new(), PeerSource::Mdns, false));
            let was_visible = entry.visible(now);
            let before = entry.addrs.clone();
            entry.merge_addrs(&f.addrs);
            netutil::rank_dial_addrs(&mut entry.addrs);
            entry.seen = Some(now);
            if entry.source != PeerSource::Manual {
                entry.source = PeerSource::Mdns;
            }
            let news = !was_visible || entry.addrs != before;
            st.mdns_names.insert(f.fullname, key);
            news
        };
        if news {
            debug!(?device, addrs = ?f.addrs, "peer discovered");
            self.emit(SyncEvent::PeerDiscovered {
                device,
                addrs: f.addrs,
            });
            self.wake.notify_one();
        }
    }

    fn on_removed(&self, fullname: &str) {
        let mut st = self.state.lock().expect("state");
        if let Some(key) = st.mdns_names.remove(fullname)
            && let Some(e) = st.peers.get_mut(&key)
        {
            e.seen = None;
        }
    }

    /// Periodic upkeep: network changes, tag rotation, invite expiry, address book.
    fn housekeeping(&self) {
        let now = Instant::now();
        let ips = netutil::local_ips();
        let (changed, epoch_stale, expired, addrbook_due) = {
            let mut st = self.state.lock().expect("state");
            let changed = ips != st.ips;
            if changed {
                st.ips = ips.clone();
                st.addrbook_at = None;
            }
            let epoch_stale = self.discovery.is_some() && st.epoch != epoch_of(now_ms());
            let expired = st.invite.as_ref().is_some_and(|i| i.expires <= now);
            let addrbook_due = st
                .addrbook_at
                .is_none_or(|t| now.duration_since(t) >= self.config.addrbook_every);
            if addrbook_due {
                st.addrbook_at = Some(now);
            }
            (changed, epoch_stale, expired, addrbook_due)
        };
        if changed {
            info!(?ips, "network changed");
            self.emit(SyncEvent::NetworkChanged { ips });
        }
        if epoch_stale || changed {
            self.advertise();
        }
        if expired {
            self.close_invite();
            self.emit(SyncEvent::InviteFinished {
                device: None,
                error: Some("invite expired".into()),
            });
        }
        if addrbook_due {
            self.load_address_book(now);
        }
    }

    fn load_address_book(&self, now: Instant) {
        let rows = self.replica.store().peers().unwrap_or_default();
        let view = self.replica.view();
        let me = self.replica.me();
        let mut st = self.state.lock().expect("state");
        for row in rows {
            if row.device == me || !view.is_active(&row.device) {
                continue;
            }
            let addrs = parse_dialable(&row.addrs, false);
            if addrs.is_empty() {
                continue;
            }
            let entry = st
                .peers
                .entry(PeerKey::Device(row.device))
                .or_insert_with(|| {
                    PeerEntry::new(Some(row.device), Vec::new(), PeerSource::AddressBook, false)
                });
            if !entry.visible(now) {
                let mut merged = entry.addrs.clone();
                merged.extend(addrs.into_iter().filter(|a| !entry.addrs.contains(a)));
                entry.addrs = merged;
                entry.addrbook_due = true;
            }
        }
    }

    /// Peers worth dialing now; marks them busy.
    fn due_peers(&self) -> Vec<PeerKey> {
        if self.is_shutdown() {
            return Vec::new();
        }
        let now = Instant::now();
        let counter = self.counter.load(Ordering::SeqCst);
        let mut st = self.state.lock().expect("state");
        let mut due = Vec::new();
        for (key, e) in st.peers.iter_mut() {
            if e.busy || e.addrs.is_empty() {
                continue;
            }
            if !(e.visible(now) || e.source == PeerSource::Manual || e.addrbook_due) {
                continue;
            }
            if e.failures > 0 {
                let backoff = BASE_BACKOFF
                    .saturating_mul(1 << (e.failures - 1).min(8))
                    .min(MAX_BACKOFF);
                if e.attempt.is_some_and(|t| now.duration_since(t) < backoff) {
                    continue;
                }
            }
            let dirty = e.synced_counter < counter;
            let stale =
                e.ok.is_none_or(|t| now.duration_since(t) >= self.config.resync_every);
            if dirty || stale || e.addrbook_due {
                e.busy = true;
                e.attempt = Some(now);
                e.addrbook_due = false;
                due.push(*key);
            }
        }
        due
    }

    async fn dial(self: &Arc<Self>, key: PeerKey) -> Option<SyncAttempt> {
        if self.is_shutdown() {
            return None;
        }
        let (addrs, allow_public, source) = {
            let st = self.state.lock().expect("state");
            let e = st.peers.get(&key)?;
            (e.addrs.clone(), e.allow_public, e.source)
        };
        let counter = self.counter.load(Ordering::SeqCst);
        let res = self.connect_and_sync(&addrs, allow_public).await;
        let (addr, res) = match res {
            Ok((a, o)) => (a, Ok(o)),
            Err((a, e)) => (a, Err(e)),
        };
        if let Some(e) = self.state.lock().expect("state").peers.get_mut(&key) {
            e.busy = false;
        }
        Some(self.finish_session(Some(key), addr, counter, &res, source))
    }

    async fn connect_and_sync(
        &self,
        addrs: &[SocketAddr],
        allow_public: bool,
    ) -> std::result::Result<(SocketAddr, SessionOutcome), (SocketAddr, SyncError)> {
        let mut last = None;
        let mut addrs = addrs.to_vec();
        netutil::rank_dial_addrs(&mut addrs);
        for addr in addrs
            .iter()
            .filter(|a| allow_public || is_local_ip(&a.ip()))
        {
            if self.is_shutdown() {
                return Err((*addr, SyncError::Protocol("sync is stopping".into())));
            }
            match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                Ok(Ok(mut stream)) => {
                    let _ = stream.set_nodelay(true);
                    if let Err(e) = stream.write_all(PREAMBLE_SYNC).await {
                        last = Some((*addr, SyncError::Io(e)));
                        continue;
                    }
                    let params = self.session_params();
                    return session::run_initiator(stream, &params)
                        .await
                        .map(|o| (*addr, o))
                        .map_err(|e| (*addr, e));
                }
                Ok(Err(e)) => last = Some((*addr, SyncError::Io(e))),
                Err(_) => last = Some((*addr, SyncError::Timeout)),
            }
        }
        Err(last.unwrap_or_else(|| {
            (
                SocketAddr::from(([0, 0, 0, 0], 0)),
                SyncError::Protocol("no dialable address".into()),
            )
        }))
    }

    async fn handle_inbound(self: &Arc<Self>, mut stream: TcpStream, remote: SocketAddr) {
        if self.is_shutdown() {
            return;
        }
        let _ = stream.set_nodelay(true);
        let mut pre = [0u8; 4];
        if !matches!(
            timeout(PREAMBLE_TIMEOUT, stream.read_exact(&mut pre)).await,
            Ok(Ok(_))
        ) {
            return;
        }
        if &pre == PREAMBLE_SYNC {
            let counter = self.counter.load(Ordering::SeqCst);
            let params = self.session_params();
            let res = session::run_responder(stream, &params).await;
            drop(params);
            self.finish_session(None, remote, counter, &res, PeerSource::Inbound);
        } else if &pre == PREAMBLE_PAIR {
            self.handle_pairing(stream).await;
        } else {
            debug!(%remote, "unknown preamble");
        }
    }

    /// Records the result of a session in memory, the address book and events.
    fn finish_session(
        &self,
        key: Option<PeerKey>,
        addr: SocketAddr,
        counter_at_start: u64,
        res: &Result<SessionOutcome>,
        source: PeerSource,
    ) -> SyncAttempt {
        let now = Instant::now();
        match res {
            Ok(out) => {
                let peer = out.peer.expect("set on success");
                let me = self.replica.me();
                let view = self.replica.view();
                let mut addrs: Vec<String> = parse_dialable(&out.peer_listen, false)
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                if source != PeerSource::Inbound {
                    let dialed = addr.to_string();
                    addrs.retain(|a| *a != dialed);
                    addrs.insert(0, dialed);
                }
                {
                    let store = self.replica.store();
                    let _ = store.peer_seen(&peer, &addrs, now_ms());
                    let _ = store.peer_sync_result(&peer, None, now_ms());
                    for h in &out.peer_hints {
                        if h.device != me && h.device != peer && view.is_known(&h.device) {
                            let hinted: Vec<String> = parse_dialable(&h.addrs, false)
                                .iter()
                                .map(ToString::to_string)
                                .collect();
                            let _ = store.peer_hint(&h.device, &hinted);
                        }
                    }
                }
                {
                    let mut st = self.state.lock().expect("state");
                    let dialable = parse_dialable(&addrs, false);
                    let entry = st
                        .peers
                        .entry(PeerKey::Device(peer))
                        .or_insert_with(|| PeerEntry::new(Some(peer), Vec::new(), source, false));
                    entry.merge_addrs(&dialable);
                    entry.ok = Some(now);
                    entry.failures = 0;
                    entry.last_error = None;
                    entry.synced_counter = entry.synced_counter.max(counter_at_start);
                    if let Some(k @ PeerKey::Addr(_)) = key
                        && let Some(e) = st.peers.get_mut(&k)
                    {
                        e.device = Some(peer);
                        e.ok = Some(now);
                        e.failures = 0;
                        e.last_error = None;
                        e.synced_counter = e.synced_counter.max(counter_at_start);
                    }
                }
                if out.received > 0 {
                    // New information: pass it on to everyone else soon.
                    self.counter.fetch_add(1, Ordering::SeqCst);
                    self.wake.notify_one();
                }
                for op in &out.forks {
                    warn!(?op, "fork detected");
                    self.emit(SyncEvent::ForkDetected {
                        op: *op,
                        reported_by: peer,
                    });
                }
                if out.clock_skew_ms.abs() > SKEW_WARN_MS {
                    warn!(?peer, skew_ms = out.clock_skew_ms, "peer clock differs");
                }
                info!(?peer, %addr, received = out.received, sent = out.sent, "sync completed");
                self.emit(SyncEvent::SyncCompleted {
                    peer,
                    addr,
                    received: out.received,
                    sent: out.sent,
                    roster_changed: out.roster_changed,
                    clock_skew_ms: out.clock_skew_ms,
                });
                SyncAttempt {
                    device: Some(peer),
                    addr,
                    result: Ok((out.received, out.sent)),
                }
            }
            Err(e) => {
                let msg = e.to_string();
                let device = match key {
                    Some(PeerKey::Device(d)) => Some(d),
                    _ => None,
                };
                {
                    let mut st = self.state.lock().expect("state");
                    if let Some(k) = key
                        && let Some(entry) = st.peers.get_mut(&k)
                    {
                        entry.failures = entry.failures.saturating_add(1);
                        entry.last_error = Some(msg.clone());
                    }
                }
                if let Some(d) = device {
                    let _ = self
                        .replica
                        .store()
                        .peer_sync_result(&d, Some(&msg), now_ms());
                }
                debug!(%addr, error = %msg, "sync failed");
                self.emit(SyncEvent::SyncFailed {
                    addr,
                    device,
                    error: msg.clone(),
                });
                SyncAttempt {
                    device,
                    addr,
                    result: Err(msg),
                }
            }
        }
    }

    fn close_invite(&self) {
        self.state.lock().expect("state").invite = None;
        if let Some(d) = &self.discovery {
            d.stop_pairing();
        }
    }

    async fn handle_pairing(self: &Arc<Self>, stream: TcpStream) {
        let Ok(_guard) = self.pairing.try_lock() else {
            return;
        };
        let (code, handler) = {
            let st = self.state.lock().expect("state");
            match &st.invite {
                Some(inv) if inv.expires > Instant::now() => {
                    (inv.code.clone(), inv.handler.clone())
                }
                _ => return,
            }
        };
        let params = InviterParams {
            code: &code,
            vault_id: self.replica.vault_id(),
            me: self.replica.me(),
            noise_secret: self.keys.noise.secret(),
            handler: handler.as_ref(),
        };
        match pairing::run_inviter(stream, &params).await {
            Ok(device) => {
                self.close_invite();
                info!(?device, "device joined");
                self.emit(SyncEvent::InviteFinished {
                    device: Some(device),
                    error: None,
                });
                self.counter.fetch_add(1, Ordering::SeqCst);
                self.wake.notify_one();
            }
            Err(InviteFailure::WrongCode) => {
                let left = {
                    let mut st = self.state.lock().expect("state");
                    match st.invite.as_mut() {
                        Some(inv) => {
                            inv.failures += 1;
                            MAX_FAILED_ATTEMPTS.saturating_sub(inv.failures)
                        }
                        None => 0,
                    }
                };
                if left == 0 {
                    self.close_invite();
                    self.emit(SyncEvent::InviteFinished {
                        device: None,
                        error: Some("too many wrong codes; start a new invite".into()),
                    });
                } else {
                    self.emit(SyncEvent::InviteWrongCode {
                        attempts_left: left,
                    });
                }
            }
            Err(InviteFailure::Declined) => {
                self.close_invite();
                self.emit(SyncEvent::InviteFinished {
                    device: None,
                    error: Some("declined".into()),
                });
            }
            Err(InviteFailure::Other(e)) => {
                debug!(error = %e, "pairing attempt failed");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RB-4: a host that opens sockets and sends nothing can hold only its own share of the slots.
    #[test]
    fn one_address_cannot_use_up_the_inbound_slots() {
        let counts = Arc::new(PerIp::default());
        let attacker: IpAddr = "192.168.1.66".parse().unwrap();
        let phone: IpAddr = "192.168.1.20".parse().unwrap();
        let held: Vec<_> = (0..MAX_INBOUND_PER_IP)
            .map(|_| counts.take(attacker).expect("within its share"))
            .collect();
        assert!(counts.take(attacker).is_none(), "its share is used up");
        assert!(
            counts.take(phone).is_some(),
            "other devices are still served"
        );
        // Slots come back when connections end, and the table does not grow without bound.
        drop(held);
        let live = counts.take(attacker).expect("a slot came back");
        assert_eq!(
            counts.open.lock().unwrap().len(),
            1,
            "only the live slot is counted"
        );
        drop(live);
        assert!(counts.open.lock().unwrap().is_empty());
        const _: () = assert!(MAX_INBOUND_PER_IP * 2 <= MAX_INBOUND);
    }
}
