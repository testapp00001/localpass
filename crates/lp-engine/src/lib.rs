//! The LocalPass engine: one API for the desktop app, the Android worker and
//! the CLI. It owns the store, the unlocked session and the sync service.
//!
//! Sync runs whenever a vault exists, locked or not (it only moves
//! ciphertext). Unlocking decrypts the log into memory; locking wipes it.

/// Public for the integration tests, which open a backup to look inside it.
#[doc(hidden)]
pub mod backup;
pub mod credentials;
pub mod devices;
pub mod error;
pub mod import_export;
pub mod items;
pub mod keys;
pub mod provider;
pub mod secret;
mod session;
pub mod webauthn_json;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use lp_core::cbor;
use lp_core::crypto::{KdfParams, Key32, NoiseKeypair, SigningKey, derive_kek, random_bytes};
use lp_core::hlc::{Hlc, now_ms};
use lp_core::ids::{DeviceId, FolderId, ItemId, VaultId};
use lp_core::model::{DataOp, DeviceKeys, FieldSet, FieldValue, RosterOp, keys as fkeys};
use lp_core::op::{SignedOp, create_data_op, create_roster_op};
use lp_core::passgen::{self, PasswordOptions};
use lp_core::totp::TotpSpec;
use lp_store::Store;
use lp_sync::Replica;
use lp_sync::pairing::{self, JoinAccept, JoinRequest};
use lp_sync::replica::META_JOIN_PROOF;
use lp_sync::service::{NetKeys, PeerSnapshot, SyncAttempt, SyncConfig, SyncEvent, SyncService};
use serde::Serialize;
use tokio::sync::{broadcast, oneshot};
use tracing::{info, warn};
use zeroize::Zeroizing;

pub use credentials::{
    LoginFill, LoginMatch, PasskeyAssertRequest, PasskeyAssertion, PasskeyCandidate,
    PasskeyCreateRequest, PasskeyCreated, SaveChange, SaveLoginOutcome, SavePlan, SaveStatus,
};
pub use devices::{DeviceInfo, InviteInfo, PairingRequest};
pub use error::{EngineError, Result};
pub use import_export::ImportReport;
pub use items::{FolderView, ItemInput, ItemQuery, ItemSummary, ItemView, PasskeyView, TotpCode};
#[cfg(all(target_os = "linux", feature = "keyring"))]
pub use secret::SecretServiceKeys;
pub use secret::{
    DeviceSecretStore, FileSecretStore, KeyedProtector, LocalKeySource, LocalProtect,
    PlatformProtect, ProvidedSecret, platform_protector, protect_local, unprotect_local,
};

use keys::{VaultKeys, meta};
use session::Unlocked;

const MIN_PASSWORD_LEN: usize = 8;

#[derive(Clone, Debug)]
pub struct EngineConfig {
    pub data_dir: PathBuf,
    /// "windows", "android", …
    pub platform: String,
    pub sync: SyncConfig,
    /// Argon2id cost for new vaults and password changes.
    pub kdf: KdfParams,
    /// Approve join requests without asking (CLI `invite --yes`, tests).
    pub auto_approve_joins: bool,
}

impl EngineConfig {
    pub fn new(data_dir: impl Into<PathBuf>, platform: impl Into<String>) -> Self {
        Self {
            data_dir: data_dir.into(),
            platform: platform.into(),
            sync: SyncConfig::default(),
            kdf: KdfParams::DEFAULT,
            auto_approve_joins: false,
        }
    }
}

/// Defined in `lp-core` so the broker's clients can name it without the engine.
pub use lp_core::status::VaultStatus;

#[derive(Clone, Debug)]
pub enum EngineEvent {
    Status(VaultStatus),
    /// Items, folders or devices changed (locally or from sync).
    VaultChanged,
    Sync(SyncEvent),
    PairingRequest(PairingRequest),
    PairingFinished {
        device: Option<String>,
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub running: bool,
    pub port: Option<u16>,
    pub listen_addrs: Vec<String>,
    pub peers: Vec<PeerStatus>,
    pub invite_active: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerStatus {
    pub device: Option<String>,
    pub name: Option<String>,
    pub addrs: Vec<String>,
    pub source: String,
    pub visible: bool,
    pub busy: bool,
    pub last_ok_secs_ago: Option<u64>,
    pub failures: u32,
    pub last_error: Option<String>,
}

struct VaultCtx {
    vault_id: VaultId,
    me: DeviceId,
    replica: Arc<Replica>,
    /// Network keys; missing if the device secret changed (repaired on unlock).
    net: Mutex<Option<(Key32, NoiseKeypair)>>,
}

pub(crate) struct Inner {
    config: EngineConfig,
    secrets: Arc<dyn DeviceSecretStore>,
    store: Arc<Mutex<Store>>,
    events: broadcast::Sender<EngineEvent>,
    vault: RwLock<Option<Arc<VaultCtx>>>,
    session: Mutex<Option<Unlocked>>,
    sync: Mutex<Option<Arc<SyncService>>>,
    /// Someone asked for sync to stay on (`start_sync`) rather than for one round (`sync_once`): the
    /// background round must then leave the service running when it ends.
    sync_wanted: AtomicBool,
    approvals: Mutex<BTreeMap<u64, oneshot::Sender<bool>>>,
    next_request: AtomicU64,
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) inner: Arc<Inner>,
}

impl Engine {
    pub fn open(config: EngineConfig, secrets: Arc<dyn DeviceSecretStore>) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let store = Store::open(&config.data_dir.join("vault.db"))?;
        let (events, _) = broadcast::channel(256);
        let engine = Engine {
            inner: Arc::new(Inner {
                config,
                secrets,
                store: Arc::new(Mutex::new(store)),
                events,
                vault: RwLock::new(None),
                session: Mutex::new(None),
                sync: Mutex::new(None),
                sync_wanted: AtomicBool::new(false),
                approvals: Mutex::new(BTreeMap::new()),
                next_request: AtomicU64::new(1),
            }),
        };
        engine.load_vault()?;
        Ok(engine)
    }

    fn load_vault(&self) -> Result<()> {
        let (vault_id, genesis, me, net_blob) = {
            let store = self.store();
            let Some(vault) = store.meta_get(meta::VAULT_ID)? else {
                return Ok(());
            };
            let genesis = store
                .meta_get(meta::GENESIS)?
                .ok_or_else(|| EngineError::Invalid("missing genesis".into()))?;
            let me = store
                .meta_get(meta::DEVICE_ID)?
                .ok_or_else(|| EngineError::Invalid("missing device id".into()))?;
            (
                VaultId::from_slice(&vault)?,
                DeviceId::from_slice(&genesis)?,
                DeviceId::from_slice(&me)?,
                store.meta_get(meta::KEYS_NET)?,
            )
        };
        let replica = Arc::new(Replica::open(
            self.inner.store.clone(),
            vault_id,
            genesis,
            me,
        )?);
        let net = match net_blob {
            Some(blob) => match self
                .inner
                .secrets
                .load_or_create()
                .and_then(|ds| keys::open_net(&ds, &vault_id, &me, &blob))
            {
                Ok(n) => Some(n),
                Err(e) => {
                    warn!(error = %e, "network keys unavailable until next unlock");
                    None
                }
            },
            None => None,
        };
        *self.inner.vault.write().expect("vault") = Some(Arc::new(VaultCtx {
            vault_id,
            me,
            replica,
            net: Mutex::new(net),
        }));
        Ok(())
    }

    fn store(&self) -> std::sync::MutexGuard<'_, Store> {
        self.inner.store.lock().expect("store")
    }

    fn ctx(&self) -> Result<Arc<VaultCtx>> {
        self.inner
            .vault
            .read()
            .expect("vault")
            .clone()
            .ok_or(EngineError::NoVault)
    }

    fn emit(&self, ev: EngineEvent) {
        let _ = self.inner.events.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.inner.events.subscribe()
    }

    pub fn config(&self) -> &EngineConfig {
        &self.inner.config
    }

    pub fn status(&self) -> VaultStatus {
        if self.inner.vault.read().expect("vault").is_none() {
            VaultStatus::NoVault
        } else if self.inner.session.lock().expect("session").is_some() {
            VaultStatus::Unlocked
        } else {
            VaultStatus::Locked
        }
    }

    /// This device's id (hex), if it belongs to a vault.
    pub fn device_id(&self) -> Option<String> {
        self.ctx().ok().map(|c| c.me.to_hex())
    }

    pub fn device_name(&self) -> Option<String> {
        self.store()
            .meta_get(meta::DEVICE_NAME)
            .ok()
            .flatten()
            .and_then(|b| String::from_utf8(b).ok())
    }

    // ---- vault lifecycle ----

    /// Creates a new vault with this device as its first member. Leaves it unlocked.
    pub fn create_vault(&self, password: &str, device_name: &str) -> Result<()> {
        if self.status() != VaultStatus::NoVault {
            return Err(EngineError::AlreadyHasVault);
        }
        check_password(password)?;
        let vault_id = VaultId::random()?;
        let sk = SigningKey::generate()?;
        let noise = NoiseKeypair::generate()?;
        let keys = VaultKeys {
            vk: Key32::random()?,
            nk: Key32::random()?,
            noise: noise.clone(),
            sk,
        };
        let device_keys = DeviceKeys::new(&keys.sk, &vault_id, noise.public());
        let genesis = create_roster_op(
            vault_id,
            &keys.sk,
            1,
            &RosterOp::Genesis {
                vault_id,
                keys: device_keys,
                created_ms: now_ms(),
            },
        )?;
        let me = keys.sk.device_id();
        self.persist_new_vault(vault_id, me, keys, password, device_name, &[genesis], None)?;
        self.write_device_meta(device_name)?;
        info!(device = %me.fingerprint(), "vault created");
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_new_vault(
        &self,
        vault_id: VaultId,
        genesis: DeviceId,
        keys: VaultKeys,
        password: &str,
        device_name: &str,
        initial_ops: &[SignedOp],
        join_proof: Option<&[SignedOp]>,
    ) -> Result<()> {
        let me = keys.sk.device_id();
        let salt: [u8; 16] = random_bytes()?;
        let params = self.inner.config.kdf;
        let kek = derive_kek(password, &salt, params)?;
        let vault_blob = keys::seal_vault_keys(&kek, b"vault", &vault_id, &keys)?;
        let ds = self.inner.secrets.load_or_create()?;
        let net_blob = keys::seal_net(&ds, &vault_id, &me, &keys.nk, &keys.noise)?;
        let params_cbor = cbor::to_vec(&params)?;
        let proof_cbor = join_proof.map(cbor::to_vec).transpose()?;
        {
            let mut store = self.store();
            store.append_batch(initial_ops, now_ms())?;
            let mut entries: Vec<(&str, &[u8])> = vec![
                (meta::GENESIS, genesis.as_bytes()),
                (meta::DEVICE_ID, me.as_bytes()),
                (meta::DEVICE_NAME, device_name.as_bytes()),
                (meta::KDF_PARAMS, &params_cbor),
                (meta::KDF_SALT, &salt),
                (meta::KEYS_VAULT, &vault_blob),
                (meta::KEYS_NET, &net_blob),
            ];
            if let Some(p) = &proof_cbor {
                entries.push((META_JOIN_PROOF, p));
            }
            // The vault id goes last: its presence marks a complete vault.
            entries.push((meta::VAULT_ID, vault_id.as_bytes()));
            store.meta_set_many(&entries)?;
        }
        let replica = Arc::new(Replica::open(
            self.inner.store.clone(),
            vault_id,
            genesis,
            me,
        )?);
        let ctx = Arc::new(VaultCtx {
            vault_id,
            me,
            replica: replica.clone(),
            net: Mutex::new(Some((keys.nk.clone(), keys.noise.clone()))),
        });
        *self.inner.vault.write().expect("vault") = Some(ctx);
        let session = Unlocked::load(keys.vk, keys.sk, &replica, Hlc::default())?;
        *self.inner.session.lock().expect("session") = Some(session);
        self.emit(EngineEvent::Status(VaultStatus::Unlocked));
        Ok(())
    }

    /// Unlocks with the master password (runs Argon2id: call off the UI thread).
    pub fn unlock(&self, password: &str) -> Result<()> {
        let ctx = self.ctx()?;
        if self.status() == VaultStatus::Unlocked {
            return Ok(());
        }
        let (salt, params, blob) = {
            let store = self.store();
            let salt = store
                .meta_get(meta::KDF_SALT)?
                .ok_or_else(|| EngineError::Invalid("missing salt".into()))?;
            let params: KdfParams = cbor::from_slice(
                &store
                    .meta_get(meta::KDF_PARAMS)?
                    .ok_or_else(|| EngineError::Invalid("missing KDF params".into()))?,
            )?;
            let blob = store
                .meta_get(meta::KEYS_VAULT)?
                .ok_or_else(|| EngineError::Invalid("missing keys".into()))?;
            (salt, params, blob)
        };
        let kek = derive_kek(password, &salt, params)?;
        let keys = keys::open_vault_keys(&kek, b"vault", &ctx.vault_id, &ctx.me, &blob)?;
        self.finish_unlock(&ctx, keys)
    }

    /// Checks the master password without changing any state (the vault stays locked or
    /// unlocked as it is). `Ok(false)` is a wrong password; runs Argon2id: call off the UI thread.
    pub fn verify_password(&self, password: &str) -> Result<bool> {
        let ctx = self.ctx()?;
        let (salt, params, blob) = {
            let store = self.store();
            let salt = store
                .meta_get(meta::KDF_SALT)?
                .ok_or_else(|| EngineError::Invalid("missing salt".into()))?;
            let params: KdfParams = cbor::from_slice(
                &store
                    .meta_get(meta::KDF_PARAMS)?
                    .ok_or_else(|| EngineError::Invalid("missing KDF params".into()))?,
            )?;
            let blob = store
                .meta_get(meta::KEYS_VAULT)?
                .ok_or_else(|| EngineError::Invalid("missing keys".into()))?;
            (salt, params, blob)
        };
        let kek = derive_kek(password, &salt, params)?;
        match keys::open_vault_keys(&kek, b"vault", &ctx.vault_id, &ctx.me, &blob) {
            Ok(_) => Ok(true),
            Err(EngineError::WrongPassword) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn finish_unlock(&self, ctx: &VaultCtx, keys: VaultKeys) -> Result<()> {
        {
            let mut net = ctx.net.lock().expect("net");
            if net.is_none() {
                let ds = self.inner.secrets.load_or_create()?;
                let blob = keys::seal_net(&ds, &ctx.vault_id, &ctx.me, &keys.nk, &keys.noise)?;
                self.store().meta_set(meta::KEYS_NET, &blob)?;
                *net = Some((keys.nk.clone(), keys.noise.clone()));
                info!("network keys repaired");
            }
        }
        let last_hlc: Hlc = self
            .store()
            .meta_get(meta::HLC_LAST)?
            .and_then(|b| cbor::from_slice(&b).ok())
            .unwrap_or_default();
        let session = Unlocked::load(keys.vk, keys.sk, &ctx.replica, last_hlc)?;
        *self.inner.session.lock().expect("session") = Some(session);
        self.emit(EngineEvent::Status(VaultStatus::Unlocked));
        self.emit(EngineEvent::VaultChanged);
        Ok(())
    }

    /// Wipes decrypted data from memory. Sync keeps running.
    pub fn lock(&self) {
        let was = self.inner.session.lock().expect("session").take();
        if was.is_some() {
            drop(was);
            self.emit(EngineEvent::Status(VaultStatus::Locked));
        }
    }

    pub fn change_master_password(&self, old: &str, new: &str) -> Result<()> {
        check_password(new)?;
        let ctx = self.ctx()?;
        let (salt, params, blob) = {
            let store = self.store();
            (
                store.meta_get(meta::KDF_SALT)?.unwrap_or_default(),
                cbor::from_slice::<KdfParams>(
                    &store.meta_get(meta::KDF_PARAMS)?.unwrap_or_default(),
                )?,
                store.meta_get(meta::KEYS_VAULT)?.unwrap_or_default(),
            )
        };
        let keys = keys::open_vault_keys(
            &derive_kek(old, &salt, params)?,
            b"vault",
            &ctx.vault_id,
            &ctx.me,
            &blob,
        )?;
        let new_salt: [u8; 16] = random_bytes()?;
        let new_params = self.inner.config.kdf;
        let new_blob = keys::seal_vault_keys(
            &derive_kek(new, &new_salt, new_params)?,
            b"vault",
            &ctx.vault_id,
            &keys,
        )?;
        let params_cbor = cbor::to_vec(&new_params)?;
        self.store().meta_set_many(&[
            (meta::KDF_SALT, &new_salt),
            (meta::KDF_PARAMS, &params_cbor),
            (meta::KEYS_VAULT, &new_blob),
        ])?;
        Ok(())
    }

    // ---- quick unlock (biometrics) ----

    /// Stores the vault keys sealed with a platform-protected quick-unlock key.
    pub fn quick_unlock_enable(&self, quick_key: &Key32) -> Result<()> {
        let ctx = self.ctx()?;
        let blob = self.with_session(|s, ctx| {
            let (nk, noise) = ctx
                .net
                .lock()
                .expect("net")
                .clone()
                .ok_or(EngineError::Locked)?;
            let keys = VaultKeys {
                vk: s.vk.clone(),
                sk: SigningKey::from_bytes(&s.sk.to_bytes()),
                nk,
                noise,
            };
            keys::seal_vault_keys(quick_key, b"quick", &ctx.vault_id, &keys)
        })?;
        drop(ctx);
        self.store().meta_set(meta::KEYS_QUICK, &blob)?;
        Ok(())
    }

    pub fn quick_unlock_disable(&self) -> Result<()> {
        Ok(self.store().meta_delete(meta::KEYS_QUICK)?)
    }

    pub fn quick_unlock_enabled(&self) -> bool {
        self.store()
            .meta_get(meta::KEYS_QUICK)
            .ok()
            .flatten()
            .is_some()
    }

    pub fn unlock_quick(&self, quick_key: &Key32) -> Result<()> {
        let ctx = self.ctx()?;
        let blob = self
            .store()
            .meta_get(meta::KEYS_QUICK)?
            .ok_or_else(|| EngineError::Invalid("quick unlock is off".into()))?;
        let keys = keys::open_vault_keys(quick_key, b"quick", &ctx.vault_id, &ctx.me, &blob)?;
        self.finish_unlock(&ctx, keys)
    }

    /// Removes this device from its vault locally: deletes all data and keys.
    pub fn leave_vault(&self) -> Result<()> {
        self.stop_sync();
        self.lock();
        *self.inner.vault.write().expect("vault") = None;
        self.store().wipe()?;
        self.inner.secrets.delete()?;
        self.emit(EngineEvent::Status(VaultStatus::NoVault));
        Ok(())
    }

    // ---- session helpers ----

    fn with_session<T>(&self, f: impl FnOnce(&mut Unlocked, &VaultCtx) -> Result<T>) -> Result<T> {
        let ctx = self.ctx()?;
        let mut guard = self.inner.session.lock().expect("session");
        let s = guard.as_mut().ok_or(EngineError::Locked)?;
        // Ops may have arrived through sync since we last looked.
        if s.seen_version != ctx.replica.version() {
            s.catch_up(&ctx.replica)?;
        }
        f(s, &ctx)
    }

    fn write_data(s: &mut Unlocked, ctx: &VaultCtx, op: DataOp) -> Result<()> {
        let signed = ctx
            .replica
            .append_local(|seq| create_data_op(ctx.vault_id, &s.vk, &s.sk, seq, &op))?;
        s.state.apply(ctx.me, signed.seq(), &op);
        s.applied.insert(ctx.me, signed.seq());
        Ok(())
    }

    /// Persists the clock, nudges sync and tells the UI.
    fn after_local_write(&self) {
        let hlc = self
            .inner
            .session
            .lock()
            .expect("session")
            .as_ref()
            .map(|s| s.clock.last());
        if let Some(h) = hlc
            && let Ok(bytes) = cbor::to_vec(&h)
        {
            let _ = self.store().meta_set(meta::HLC_LAST, &bytes);
        }
        if let Some(sync) = self.sync_service() {
            sync.notify_local_change();
        }
        self.emit(EngineEvent::VaultChanged);
    }

    fn write_device_meta(&self, name: &str) -> Result<()> {
        let platform = self.inner.config.platform.clone();
        self.with_session(|s, ctx| {
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::DeviceMetaSet {
                    device: ctx.me,
                    hlc,
                    name: name.to_string(),
                    platform,
                },
            )
        })?;
        self.after_local_write();
        Ok(())
    }

    /// Applies ops that arrived through sync to the unlocked session.
    pub fn catch_up(&self) -> Result<bool> {
        let ctx = self.ctx()?;
        let mut guard = self.inner.session.lock().expect("session");
        match guard.as_mut() {
            Some(s) => s.catch_up(&ctx.replica),
            None => Ok(false),
        }
    }

    // ---- items ----

    pub fn list_items(&self, query: &ItemQuery) -> Result<Vec<ItemSummary>> {
        self.with_session(|s, _| {
            let mut out: Vec<ItemSummary> = s
                .state
                .items
                .values()
                .filter(|i| items::matches(i, query))
                .map(items::summary)
                .collect();
            out.sort_by_key(|i| (!i.favorite, i.title.to_lowercase()));
            Ok(out)
        })
    }

    pub fn get_item(&self, id: &str) -> Result<ItemView> {
        let id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .ok_or_else(|| EngineError::NotFound("item".into()))?;
            Ok(items::view(&s.state, item))
        })
    }

    /// Creates (no id) or updates an item. Only changed fields are written.
    pub fn save_item(&self, input: &ItemInput) -> Result<String> {
        let desired = items::input_fields(input)?;
        let (id, changed) = self.with_session(|s, ctx| {
            let (id, sets) = match input.id.as_deref().filter(|i| !i.is_empty()) {
                Some(raw) => {
                    let id = ItemId::parse(raw)?;
                    let cur = s
                        .state
                        .items
                        .get(&id)
                        .ok_or_else(|| EngineError::NotFound("item".into()))?;
                    (id, items::diff(cur, desired))
                }
                None => (
                    ItemId::random()?,
                    desired
                        .into_iter()
                        .filter(|(_, v)| !v.is_null())
                        .map(|(key, value)| FieldSet {
                            key,
                            value,
                            base: None,
                        })
                        .collect(),
                ),
            };
            if sets.is_empty() {
                return Ok((id, false));
            }
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::ItemSet {
                    item: id,
                    hlc,
                    fields: sets,
                },
            )?;
            Ok((id, true))
        })?;
        if changed {
            self.after_local_write();
        }
        Ok(id.to_string())
    }

    fn set_flag(&self, id: &str, key: &str, value: bool) -> Result<()> {
        let id = ItemId::parse(id)?;
        self.with_session(|s, ctx| {
            let item = s
                .state
                .items
                .get(&id)
                .ok_or_else(|| EngineError::NotFound("item".into()))?;
            let base = item.winner(key).map(|w| w.stamp.op_ref());
            let hlc = s.clock.tick(now_ms());
            let fields = vec![FieldSet {
                key: key.to_string(),
                value: FieldValue::Bool(value),
                base,
            }];
            Self::write_data(
                s,
                ctx,
                DataOp::ItemSet {
                    item: id,
                    hlc,
                    fields,
                },
            )
        })?;
        self.after_local_write();
        Ok(())
    }

    /// Moves an item to the trash (recoverable).
    pub fn trash_item(&self, id: &str) -> Result<()> {
        self.set_flag(id, fkeys::TRASHED, true)
    }

    pub fn restore_item(&self, id: &str) -> Result<()> {
        self.set_flag(id, fkeys::TRASHED, false)
    }

    pub fn set_favorite(&self, id: &str, favorite: bool) -> Result<()> {
        self.set_flag(id, fkeys::FAVORITE, favorite)
    }

    /// Deletes an item permanently on every device.
    pub fn purge_item(&self, id: &str) -> Result<()> {
        let id = ItemId::parse(id)?;
        self.with_session(|s, ctx| {
            if !s.state.items.contains_key(&id) {
                return Err(EngineError::NotFound("item".into()));
            }
            let hlc = s.clock.tick(now_ms());
            Self::write_data(s, ctx, DataOp::ItemPurge { item: id, hlc })
        })?;
        self.after_local_write();
        Ok(())
    }

    pub fn empty_trash(&self) -> Result<usize> {
        let trashed: Vec<ItemId> = self.with_session(|s, _| {
            Ok(s.state
                .items
                .values()
                .filter(|i| i.is_trashed())
                .map(|i| i.id)
                .collect())
        })?;
        for id in &trashed {
            self.purge_item(&id.to_string())?;
        }
        Ok(trashed.len())
    }

    /// Keeps `chosen` (an op id from `ItemView.conflicts`) as the field's value.
    pub fn resolve_conflict(&self, id: &str, field: &str, chosen: &str) -> Result<()> {
        let id = ItemId::parse(id)?;
        let chosen = items::decode_op_ref(chosen)?;
        self.with_session(|s, ctx| {
            let item = s
                .state
                .items
                .get(&id)
                .ok_or_else(|| EngineError::NotFound("item".into()))?;
            let entries = item
                .fields
                .get(field)
                .ok_or_else(|| EngineError::NotFound("field".into()))?;
            let pick = entries
                .iter()
                .find(|e| e.stamp.op_ref() == chosen)
                .ok_or_else(|| EngineError::NotFound("value".into()))?;
            let winner = entries.last().expect("non-empty").stamp.op_ref();
            let fields = vec![FieldSet {
                key: field.to_string(),
                value: pick.value.clone(),
                base: Some(winner),
            }];
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::ItemSet {
                    item: id,
                    hlc,
                    fields,
                },
            )
        })?;
        self.after_local_write();
        Ok(())
    }

    /// Secret value of a field for copying (`totp` yields the current code).
    pub fn field_secret(&self, id: &str, field: &str) -> Result<Zeroizing<String>> {
        if field == fkeys::PASSKEY_PRIVATE_KEY {
            // Only a backup carries it (see import_export.rs); the interface never copies it.
            return Err(EngineError::Invalid(
                "a passkey's private key cannot be copied".into(),
            ));
        }
        let id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .ok_or_else(|| EngineError::NotFound("item".into()))?;
            let value = item
                .get(field)
                .ok_or_else(|| EngineError::NotFound("field".into()))?;
            let text = match (field, value) {
                (fkeys::TOTP, FieldValue::Text(uri)) => {
                    TotpSpec::parse(uri)?.code_at(now_ms() / 1000)
                }
                (_, FieldValue::Text(t)) => t.clone(),
                (_, FieldValue::Custom(c)) => c.value.clone(),
                (_, FieldValue::List(l)) => l.join("\n"),
                _ => return Err(EngineError::Invalid("field has no copyable value".into())),
            };
            Ok(Zeroizing::new(text))
        })
    }

    pub fn totp_code(&self, id: &str) -> Result<TotpCode> {
        let id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .ok_or_else(|| EngineError::NotFound("item".into()))?;
            let uri = item
                .text(fkeys::TOTP)
                .ok_or_else(|| EngineError::NotFound("2FA secret".into()))?;
            let spec = TotpSpec::parse(uri)?;
            let now = now_ms() / 1000;
            Ok(TotpCode {
                code: spec.code_at(now),
                period: spec.period,
                remaining: spec.seconds_remaining(now),
            })
        })
    }

    pub fn generate_password(opts: &PasswordOptions) -> Result<Zeroizing<String>> {
        Ok(passgen::generate_password(opts)?)
    }

    // ---- folders ----

    pub fn folders(&self) -> Result<Vec<FolderView>> {
        self.with_session(|s, _| {
            let mut out: Vec<FolderView> = s
                .state
                .folders
                .iter()
                .filter_map(|(id, f)| {
                    let name = f.name.clone()?;
                    let id_s = id.to_string();
                    let items = s
                        .state
                        .items
                        .values()
                        .filter(|i| !i.is_trashed() && i.text(fkeys::FOLDER) == Some(id_s.as_str()))
                        .count();
                    Some(FolderView {
                        id: id_s,
                        name,
                        items,
                    })
                })
                .collect();
            out.sort_by_key(|f| f.name.to_lowercase());
            Ok(out)
        })
    }

    /// Creates (no id) or renames a folder.
    pub fn save_folder(&self, id: Option<&str>, name: &str) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Invalid("folder name is required".into()));
        }
        let folder = match id {
            Some(id) => FolderId::parse(id)?,
            None => FolderId::random()?,
        };
        self.with_session(|s, ctx| {
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::FolderSet {
                    folder,
                    hlc,
                    name: Some(name.to_string()),
                },
            )
        })?;
        self.after_local_write();
        Ok(folder.to_string())
    }

    pub fn delete_folder(&self, id: &str) -> Result<()> {
        let folder = FolderId::parse(id)?;
        self.with_session(|s, ctx| {
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::FolderSet {
                    folder,
                    hlc,
                    name: None,
                },
            )
        })?;
        self.after_local_write();
        Ok(())
    }

    // ---- devices ----

    pub fn devices(&self) -> Result<Vec<DeviceInfo>> {
        let ctx = self.ctx()?;
        let view = ctx.replica.view();
        let rows = self.store().peers()?;
        let online: Vec<DeviceId> = self
            .sync_service()
            .map(|s| {
                s.peers()
                    .into_iter()
                    .filter(|p| p.visible || p.last_ok_secs_ago.is_some_and(|a| a < 120))
                    .filter_map(|p| p.device)
                    .collect()
            })
            .unwrap_or_default();
        let names: BTreeMap<DeviceId, (String, String)> = self
            .inner
            .session
            .lock()
            .expect("session")
            .as_ref()
            .map(|s| {
                s.state
                    .devices
                    .iter()
                    .map(|(id, d)| (*id, (d.name.clone(), d.platform.clone())))
                    .collect()
            })
            .unwrap_or_default();
        let my_name = self.device_name().unwrap_or_default();
        let label = |id: &DeviceId| -> String {
            names
                .get(id)
                .map(|n| n.0.clone())
                .or_else(|| (*id == ctx.me).then(|| my_name.clone()))
                .unwrap_or_else(|| format!("Device {}", &id.fingerprint()[..4]))
        };
        let mut out: Vec<DeviceInfo> = view
            .devices()
            .map(|(id, rec)| {
                let row = rows.iter().find(|r| r.device == *id);
                DeviceInfo {
                    id: id.to_hex(),
                    fingerprint: id.fingerprint(),
                    name: label(id),
                    platform: names.get(id).map(|n| n.1.clone()).unwrap_or_default(),
                    status: devices::status_name(rec.status).to_string(),
                    this_device: *id == ctx.me,
                    genesis: rec.is_genesis,
                    added_by: rec.added_by.as_ref().map(label),
                    added_ms: rec.added_ms,
                    last_seen_ms: row.and_then(|r| r.last_seen_ms),
                    last_sync_ms: row.and_then(|r| r.last_sync_ms),
                    last_error: row.and_then(|r| r.last_error.clone()),
                    online: online.contains(id),
                    addrs: row.map(|r| r.addrs.clone()).unwrap_or_default(),
                }
            })
            .collect();
        out.sort_by_key(|d| (!d.this_device, d.status != "active", d.name.to_lowercase()));
        Ok(out)
    }

    pub fn rename_this_device(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EngineError::Invalid("device name is required".into()));
        }
        self.store().meta_set(meta::DEVICE_NAME, name.as_bytes())?;
        self.write_device_meta(name)
    }

    /// Revokes a lost device on every member. It can no longer connect, and
    /// devices it adds afterwards are void.
    pub fn revoke(&self, device: &str) -> Result<()> {
        let target = DeviceId::from_hex(device)?;
        self.with_session(|s, ctx| {
            if target == ctx.me {
                return Err(EngineError::Invalid(
                    "to remove this device, use 'leave vault'".into(),
                ));
            }
            if !ctx.replica.view().is_known(&target) {
                return Err(EngineError::NotFound("device".into()));
            }
            let cutoff_seq = ctx.replica.store().max_seq(&target)?;
            let op = RosterOp::DeviceRevoked {
                device: target,
                cutoff_seq,
                revoked_ms: now_ms(),
            };
            ctx.replica
                .append_local(|seq| create_roster_op(ctx.vault_id, &s.sk, seq, &op))?;
            s.catch_up(&ctx.replica)?;
            Ok(())
        })?;
        self.after_local_write();
        Ok(())
    }

    // ---- pairing ----

    /// Opens an invite. Show `code_display` / `qr_svg` to the user.
    pub fn invite_start(&self) -> Result<InviteInfo> {
        if self.status() != VaultStatus::Unlocked {
            return Err(EngineError::Locked);
        }
        let sync = self.sync_service().ok_or(EngineError::SyncNotRunning)?;
        let code = pairing::generate_code()?;
        let handler = Arc::new(devices::EngineInviteHandler {
            engine: Arc::downgrade(&self.inner),
        });
        sync.start_invite(code.clone(), handler)?;
        let addrs: Vec<String> = sync
            .listen_addrs()
            .iter()
            .map(ToString::to_string)
            .collect();
        let qr_text = format!("localpass://pair?c={code}&a={}", addrs.join(","));
        Ok(InviteInfo {
            code_display: pairing::format_code(&code),
            code,
            qr_svg: devices::qr_svg(&qr_text)?,
            qr_text,
            addrs,
            expires_in_secs: pairing::INVITE_TTL.as_secs(),
        })
    }

    pub fn invite_cancel(&self) {
        if let Some(s) = self.sync_service() {
            s.cancel_invite();
        }
        for (_, tx) in std::mem::take(&mut *self.inner.approvals.lock().expect("approvals")) {
            let _ = tx.send(false);
        }
    }

    /// Answers a [`EngineEvent::PairingRequest`].
    pub fn invite_respond(&self, request_id: u64, approve: bool) -> Result<()> {
        let tx = self
            .inner
            .approvals
            .lock()
            .expect("approvals")
            .remove(&request_id)
            .ok_or_else(|| EngineError::NotFound("pairing request".into()))?;
        let _ = tx.send(approve);
        Ok(())
    }

    pub(crate) fn register_pairing_request(&self, req: &JoinRequest) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        let id = self.inner.next_request.fetch_add(1, Ordering::SeqCst);
        self.inner
            .approvals
            .lock()
            .expect("approvals")
            .insert(id, tx);
        self.emit(EngineEvent::PairingRequest(PairingRequest {
            request_id: id,
            device_name: req.name.clone(),
            platform: req.platform.clone(),
            fingerprint: req.keys.sign_pk.fingerprint(),
        }));
        rx
    }

    /// Inviter side of an approved join: records the device and hands over keys.
    pub(crate) fn author_device_added(&self, req: &JoinRequest) -> Result<JoinAccept> {
        let inviter_addrs: Vec<String> = self
            .sync_service()
            .map(|s| s.listen_addrs().iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        let accept = self.with_session(|s, ctx| {
            let op = RosterOp::DeviceAdded {
                keys: req.keys.clone(),
                added_ms: now_ms(),
            };
            let added = ctx
                .replica
                .append_local(|seq| create_roster_op(ctx.vault_id, &s.sk, seq, &op))?;
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::DeviceMetaSet {
                    device: req.keys.sign_pk,
                    hlc,
                    name: req.name.clone(),
                    platform: req.platform.clone(),
                },
            )?;
            let mut proof = ctx.replica.membership_proof()?;
            let genesis = match proof.first() {
                Some(g) if g.seq() == 1 && g.author() == ctx.replica.genesis() => proof.remove(0),
                _ => ctx
                    .replica
                    .store()
                    .get_op(&ctx.replica.genesis(), 1)?
                    .ok_or_else(|| EngineError::Invalid("missing genesis op".into()))?,
            };
            proof.push(added);
            let (nk, _) = ctx
                .net
                .lock()
                .expect("net")
                .clone()
                .ok_or(EngineError::SyncNotRunning)?;
            Ok(JoinAccept {
                vault_id: ctx.vault_id,
                genesis,
                proof,
                vk: s.vk.as_bytes().to_vec(),
                nk: nk.as_bytes().to_vec(),
                inviter: ctx.me,
                inviter_addrs,
            })
        })?;
        self.after_local_write();
        self.emit(EngineEvent::PairingFinished {
            device: Some(req.keys.sign_pk.to_hex()),
            error: None,
        });
        Ok(accept)
    }

    /// Joins an existing vault. `input` is the 8-digit code or a scanned QR
    /// payload; `extra_addrs` are typed addresses of the inviting device.
    pub async fn join(
        &self,
        input: &str,
        extra_addrs: &[String],
        password: &str,
        device_name: &str,
    ) -> Result<()> {
        if self.status() != VaultStatus::NoVault {
            return Err(EngineError::AlreadyHasVault);
        }
        check_password(password)?;
        let (code, mut addrs) = devices::parse_join_input(input, extra_addrs)?;
        if addrs.is_empty() {
            addrs = pairing::discover_inviters(Duration::from_secs(8)).await?;
        }
        if addrs.is_empty() {
            return Err(EngineError::NoInviterFound);
        }
        let sk = SigningKey::generate()?;
        let noise = NoiseKeypair::generate()?;
        let accept = pairing::join_via(
            &addrs,
            &code,
            &sk,
            &noise,
            device_name,
            &self.inner.config.platform,
        )
        .await?;
        let keys = VaultKeys {
            vk: Key32::from_slice(&accept.vk)?,
            nk: Key32::from_slice(&accept.nk)?,
            noise,
            sk,
        };
        let mut proof = vec![accept.genesis.clone()];
        proof.extend(accept.proof.iter().cloned());
        let mut dial: Vec<SocketAddr> = accept
            .inviter_addrs
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect();
        for a in &addrs {
            if !dial.contains(a) {
                dial.push(*a);
            }
        }

        let this = self.clone();
        let (password, name) = (
            Zeroizing::new(password.to_string()),
            device_name.to_string(),
        );
        let (vault_id, genesis) = (accept.vault_id, accept.genesis.clone());
        tokio::task::spawn_blocking(move || {
            this.persist_new_vault(
                vault_id,
                genesis.author(),
                keys,
                &password,
                &name,
                &[genesis],
                Some(&proof),
            )
        })
        .await
        .map_err(|e| EngineError::Invalid(e.to_string()))??;
        self.write_device_meta(device_name)?;
        info!(inviter = %accept.inviter.fingerprint(), "joined vault");

        self.start_sync().await?;
        let mut synced = false;
        for addr in &dial {
            match self.sync_with(*addr, false).await {
                Ok(()) => {
                    synced = true;
                    break;
                }
                Err(e) => warn!(%addr, error = %e, "first sync attempt failed"),
            }
        }
        if !synced {
            self.retry_first_sync(dial);
        }
        Ok(())
    }

    /// The first sync after joining can fail: the other device may be busy or restarting,
    /// or the network may block discovery, in which case nothing else would ever find it.
    /// Keep trying for a few minutes; once one succeeds the normal scheduler takes over.
    fn retry_first_sync(&self, addrs: Vec<SocketAddr>) {
        let engine = self.clone();
        tokio::spawn(async move {
            let mut delay = Duration::from_secs(3);
            for _ in 0..10 {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
                if !engine.sync_running() {
                    return;
                }
                for addr in &addrs {
                    if engine.sync_with(*addr, false).await.is_ok() {
                        return;
                    }
                }
            }
        });
    }

    // ---- sync ----

    fn sync_service(&self) -> Option<Arc<SyncService>> {
        self.inner.sync.lock().expect("sync").clone()
    }

    pub fn sync_running(&self) -> bool {
        self.inner.sync.lock().expect("sync").is_some()
    }

    /// Starts background sync (works while locked) and keeps it on until [`Engine::stop_sync`].
    pub async fn start_sync(&self) -> Result<()> {
        self.inner.sync_wanted.store(true, Ordering::SeqCst);
        self.start_service().await
    }

    async fn start_service(&self) -> Result<()> {
        if self.sync_running() {
            return Ok(());
        }
        let ctx = self.ctx()?;
        let (nk, noise) = ctx
            .net
            .lock()
            .expect("net")
            .clone()
            .ok_or_else(|| EngineError::Secret("unlock once to repair sync keys".into()))?;
        let svc = Arc::new(
            SyncService::start(
                ctx.replica.clone(),
                NetKeys { noise, nk },
                self.inner.config.sync.clone(),
            )
            .await?,
        );
        let mut rx = svc.subscribe();
        {
            let mut slot = self.inner.sync.lock().expect("sync");
            if slot.is_some() {
                svc.shutdown();
                return Ok(());
            }
            *slot = Some(svc);
        }
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            loop {
                let ev = rx.recv().await;
                let Some(inner) = weak.upgrade() else { break };
                let engine = Engine { inner };
                match ev {
                    Ok(ev) => engine.on_sync_event(ev),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if engine.catch_up().unwrap_or(false) {
                            engine.emit(EngineEvent::VaultChanged);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(())
    }

    fn on_sync_event(&self, ev: SyncEvent) {
        match &ev {
            SyncEvent::SyncCompleted {
                received,
                roster_changed,
                ..
            } if *received > 0 || *roster_changed => match self.catch_up() {
                Ok(true) => self.emit(EngineEvent::VaultChanged),
                Ok(false) => {}
                Err(e) => warn!(error = %e, "applying synced ops failed"),
            },
            SyncEvent::InviteFinished {
                device: None,
                error,
            } => {
                self.emit(EngineEvent::PairingFinished {
                    device: None,
                    error: error.clone(),
                });
            }
            _ => {}
        }
        self.emit(EngineEvent::Sync(ev));
    }

    pub fn stop_sync(&self) {
        self.inner.sync_wanted.store(false, Ordering::SeqCst);
        self.shut_down_service();
    }

    fn shut_down_service(&self) {
        if let Some(s) = self.inner.sync.lock().expect("sync").take() {
            s.shutdown();
        }
    }

    /// Syncs with every reachable peer now and waits.
    pub async fn sync_now(&self) -> Result<Vec<SyncAttempt>> {
        let sync = self.sync_service().ok_or(EngineError::SyncNotRunning)?;
        let res = sync.sync_now().await;
        if self.catch_up().unwrap_or(false) {
            self.emit(EngineEvent::VaultChanged);
        }
        Ok(res)
    }

    pub async fn sync_with(&self, addr: SocketAddr, allow_public: bool) -> Result<()> {
        let sync = self.sync_service().ok_or(EngineError::SyncNotRunning)?;
        sync.sync_with(addr, allow_public).await?;
        if self.catch_up().unwrap_or(false) {
            self.emit(EngineEvent::VaultChanged);
        }
        Ok(())
    }

    /// One background round (Android worker): discover, sync with everyone, stop.
    pub async fn sync_once(&self, discovery_wait: Duration) -> Result<Vec<SyncAttempt>> {
        /// Stops a service this call started, even if the caller drops the future (a timeout), unless
        /// the app asked for sync to stay on meanwhile (the user opened it during the round).
        struct StopOnDrop<'a>(Option<&'a Engine>);
        impl Drop for StopOnDrop<'_> {
            fn drop(&mut self) {
                if let Some(engine) = self.0
                    && !engine.inner.sync_wanted.load(Ordering::SeqCst)
                {
                    engine.shut_down_service();
                }
            }
        }

        let started_here = !self.sync_running();
        self.start_service().await?;
        let _stop = StopOnDrop(started_here.then_some(self));
        tokio::time::sleep(discovery_wait).await;
        self.sync_now().await
    }

    pub fn add_manual_peer(&self, addr: &str, allow_public: bool) -> Result<()> {
        let addr = devices::parse_addr(addr)
            .ok_or_else(|| EngineError::Invalid("expected ip or ip:port".into()))?;
        match self.sync_service() {
            Some(s) => s.add_manual_peer(addr, allow_public)?,
            None => self
                .store()
                .add_manual_peer(&addr.to_string(), allow_public)?,
        }
        Ok(())
    }

    pub fn sync_status(&self) -> SyncStatus {
        let names: BTreeMap<DeviceId, String> = self
            .inner
            .session
            .lock()
            .expect("session")
            .as_ref()
            .map(|s| {
                s.state
                    .devices
                    .iter()
                    .map(|(id, d)| (*id, d.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        match self.sync_service() {
            None => SyncStatus {
                running: false,
                port: None,
                listen_addrs: vec![],
                peers: vec![],
                invite_active: false,
            },
            Some(s) => SyncStatus {
                running: true,
                port: Some(s.port()),
                listen_addrs: s.listen_addrs().iter().map(ToString::to_string).collect(),
                peers: s
                    .peers()
                    .into_iter()
                    .map(|p| peer_status(p, &names))
                    .collect(),
                invite_active: s.invite_active(),
            },
        }
    }
}

fn peer_status(p: PeerSnapshot, names: &BTreeMap<DeviceId, String>) -> PeerStatus {
    PeerStatus {
        device: p.device.map(|d| d.to_hex()),
        name: p.device.and_then(|d| names.get(&d).cloned()),
        addrs: p.addrs.iter().map(ToString::to_string).collect(),
        source: format!("{:?}", p.source).to_lowercase(),
        visible: p.visible,
        busy: p.busy,
        last_ok_secs_ago: p.last_ok_secs_ago,
        failures: p.failures,
        last_error: p.last_error,
    }
}

fn check_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(EngineError::Invalid(format!(
            "master password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    Ok(())
}
