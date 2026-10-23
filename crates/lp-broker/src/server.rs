//! The broker server: authenticates each connection (pairing a new client, or a
//! challenge-response with a known one) and then answers its requests from the engine.
//!
//! It is written against [`Host`] so the app supplies the pieces that need a screen
//! (approving a new client, unlocking, Windows Hello) and tests supply scripted ones.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use lp_core::crypto::{ct_eq, random_below, random_bytes};
use lp_core::hlc::now_ms;
use lp_core::passkey::b64u;
use lp_engine::{
    Engine, EngineError, PasskeyAssertRequest, SaveLoginOutcome, SavePlan, SaveStatus, VaultStatus,
};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, broadcast, mpsc};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tracing::{debug, info, warn};

use crate::clients::{ClientKind, ClientRecord, ClientStore};
use crate::framing::{MAX_MESSAGE, read_message, read_message_timed, write_message};
use crate::protocol::{
    AssertedPasskey, CLIENT_PROOF_LABEL, ClientMessage, PROTOCOL_VERSION, Request, RequestEnvelope,
    ResponseEnvelope, SERVER_PROOF_LABEL, ServerMessage, UserVerification, proof,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Most connections that are still in the handshake. Sockets that never authenticate use only
/// these slots and never take one of the [`MAX_SESSIONS`], so they cannot lock out a client that
/// is already connected.
const MAX_HANDSHAKES: usize = 32;
/// Most authenticated connections at once. The browser opens one and the Windows provider one
/// per operation, so this only stops abuse by a program that holds a client secret.
const MAX_SESSIONS: usize = 16;
/// Most requests one connection may have in flight (waiting for the user, or running).
const MAX_IN_FLIGHT: usize = 16;
/// An authenticated connection that sends nothing for this long is closed (the client
/// reconnects when it has something to say). It is much longer than any prompt.
const IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long a client may take to answer a handshake step.
const HANDSHAKE_STEP: Duration = Duration::from_secs(10);
/// How long the user has to approve a new client.
const PAIRING_APPROVAL: Duration = Duration::from_secs(180);
/// How long an unlock or user check may take.
const USER_ACTION: Duration = Duration::from_secs(120);

/// What the user is asked to approve when a new client connects.
#[derive(Clone, Debug)]
pub struct PairingPrompt {
    pub name: String,
    pub kind: ClientKind,
    /// Six digits, also shown by the client, so the user can tell the right request apart.
    pub code: String,
}

/// What a request would overwrite.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OverwriteKind {
    /// The password of a saved login.
    Login,
    /// A passkey for the same site and account.
    Passkey,
}

/// What the user is asked before a request changes something they already have. Every text
/// field is safe to show (no control characters, cut to 60 characters).
#[derive(Clone, Debug)]
pub struct OverwritePrompt {
    pub kind: OverwriteKind,
    /// The title of the saved login or passkey that would change.
    pub title: String,
    pub username: String,
    /// The site the request comes from (the browser's host, or the passkey's site id).
    pub site: String,
    /// For a login: the address it is saved for.
    pub saved_for: Option<String>,
    /// For a login: the request comes from that very host. False means another address of the
    /// same site, which the dialog should point out.
    pub same_host: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Ask for Windows Hello (or similar) for every passkey use, even when the site
    /// says user verification is discouraged.
    pub verify_every_passkey: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            verify_every_passkey: true,
        }
    }
}

/// The app side of the broker.
///
/// The futures returned by `approve_pairing`, `unlock` and `verify_user` are dropped (not
/// completed) when the client that caused them disconnects, so an implementation should clean up
/// in `Drop` or by being cancel-safe: close the dialog, cancel the OS operation.
pub trait Host: Send + Sync + 'static {
    fn engine(&self) -> &Engine;
    fn clients(&self) -> &dyn ClientStore;
    fn policy(&self) -> Policy;
    /// Shows the pairing request and returns whether the user approved it.
    fn approve_pairing<'a>(&'a self, prompt: &'a PairingPrompt) -> BoxFuture<'a, bool>;
    /// Unlocks the vault (quick unlock, or by showing the unlock screen). True once unlocked.
    fn unlock<'a>(&'a self) -> BoxFuture<'a, bool>;
    /// Checks that the user is present and verified (Windows Hello). True if they were.
    /// `reason` says what for, for example "Sign in to example.com" (control characters removed,
    /// site-supplied text cut to 60 characters); show it where the platform allows.
    fn verify_user<'a>(&'a self, reason: &'a str) -> BoxFuture<'a, bool>;
    /// Asks whether a save or a new passkey may replace what the user already has (a saved
    /// password, a passkey for the same account) and returns whether they agreed. Nothing is
    /// changed without a yes; new logins and new passkeys do not ask. Like `verify_user`, the
    /// future is dropped when the client leaves.
    fn confirm_overwrite<'a>(&'a self, prompt: &'a OverwritePrompt) -> BoxFuture<'a, bool>;
}

/// An error sent back to the client: a stable `code` to branch on and a message to show.
#[derive(Debug)]
pub struct BrokerError {
    pub code: &'static str,
    pub message: String,
}

impl BrokerError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl From<EngineError> for BrokerError {
    fn from(e: EngineError) -> Self {
        let code = match &e {
            EngineError::Locked => "locked",
            EngineError::NoVault => "noVault",
            EngineError::NotFound(_) => "notFound",
            EngineError::Invalid(_) | EngineError::Core(lp_core::CoreError::Invalid(_)) => {
                "invalid"
            }
            EngineError::Unsupported(_) => "unsupported",
            EngineError::PasskeyExists => "passkeyExists",
            _ => "internal",
        };
        Self::new(code, e.to_string())
    }
}

type Outcome = Result<Value, BrokerError>;

pub struct Broker<H: Host> {
    host: Arc<H>,
    /// Connections that have not authenticated yet.
    handshakes: Arc<Semaphore>,
    /// Authenticated connections.
    sessions: Arc<Semaphore>,
    /// Only one pairing request at a time.
    pairing: Mutex<()>,
    /// One unlock prompt at a time: a request that waited its turn finds the vault unlocked by
    /// the user's check for another request, and must not count that as its own.
    unlocking: Mutex<()>,
    /// How many unlock prompts have ended without the vault being unlocked. A request that
    /// waited for its turn while one ended that way does not prompt again.
    unlock_failures: AtomicU64,
    /// One Windows Hello (or similar) prompt at a time: the OS shows one anyway, and a flood of
    /// requests must not queue a pile of dialogs.
    asking: Mutex<()>,
    /// Ids of clients the app has just forgotten (see [`Broker::revoke`]).
    revoked: broadcast::Sender<String>,
}

fn to_value<T: Serialize>(value: &T) -> Outcome {
    serde_json::to_value(value).map_err(|e| BrokerError::new("internal", e.to_string()))
}

async fn send<W: AsyncWrite + Unpin>(w: &mut W, message: &ServerMessage) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(message).expect("server messages serialize");
    write_message(w, &bytes).await
}

async fn recv<R: AsyncRead + Unpin>(r: &mut R, wait: Duration) -> Option<ClientMessage> {
    let bytes = timeout(wait, read_message(r)).await.ok()?.ok()??;
    serde_json::from_slice(&bytes).ok()
}

/// Resolves when the peer hangs up (or the stream fails). Whatever it sends meanwhile is
/// dropped: while the user decides, a client has nothing to say.
async fn hung_up<R: AsyncRead + Unpin>(reader: &mut R) {
    while let Ok(Some(_)) = read_message(reader).await {}
}

/// Text from a website (its name) made safe to show in a prompt or log: no control characters, none
/// of the invisible ones that reorder or hide the text around them (the same set the extension's
/// `displayText` drops), at most 60 characters.
fn display_text(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() && !is_invisible(*c))
        .take(60)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Characters with no width, or that reorder or hide the text around them: soft hyphen, Arabic letter
/// mark, Mongolian vowel separator, zero-width and direction marks, bidi embeddings and isolates,
/// invisible operators, the byte order mark and the interlinear annotation marks.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// What the user is told when a passkey is about to be created: the site's name when it has
/// one, always with the site id, which is the part a clever name cannot fake.
fn create_reason(rp_name: &str, rp_id: &str) -> String {
    let name = display_text(rp_name);
    let id = display_text(rp_id);
    if name.is_empty() || name == id {
        format!("Create a passkey for {id}")
    } else {
        format!("Create a passkey for {name} ({id})")
    }
}

impl<H: Host> Broker<H> {
    pub fn new(host: Arc<H>) -> Arc<Self> {
        Arc::new(Self {
            host,
            handshakes: Arc::new(Semaphore::new(MAX_HANDSHAKES)),
            sessions: Arc::new(Semaphore::new(MAX_SESSIONS)),
            pairing: Mutex::new(()),
            unlocking: Mutex::new(()),
            unlock_failures: AtomicU64::new(0),
            asking: Mutex::new(()),
            revoked: broadcast::channel(16).0,
        })
    }

    /// Ends every open connection of a client the app has just forgotten, and withdraws the
    /// prompts its requests were waiting on. Call it after removing the client from the store;
    /// even without this call a removed client's next request is refused (and a request that
    /// was waiting for the user reveals nothing), because each one re-checks the store.
    pub fn revoke(&self, client_id: &str) {
        let _ = self.revoked.send(client_id.to_string());
    }

    /// Whether the client is still the one the store holds (not forgotten, not re-paired).
    fn still_paired(&self, client: &ClientRecord) -> bool {
        self.host.clients().get(&client.id).is_some_and(|now| {
            now.created_ms == client.created_ms && ct_eq(&now.secret, &client.secret)
        })
    }

    fn check_paired(&self, client: &ClientRecord) -> Result<(), BrokerError> {
        if self.still_paired(client) {
            Ok(())
        } else {
            Err(BrokerError::new(
                "unpaired",
                "this program was removed in LocalPass",
            ))
        }
    }

    /// Resolves when `client` has been forgotten.
    async fn wait_revoked(&self, ids: &mut broadcast::Receiver<String>, client: &ClientRecord) {
        loop {
            match ids.recv().await {
                Ok(id) if id == client.id => return,
                Ok(_) | Err(RecvError::Lagged(_)) => {
                    if !self.still_paired(client) {
                        return;
                    }
                }
                Err(RecvError::Closed) => std::future::pending().await,
            }
        }
    }

    /// Serves one connection until it closes. Never panics on bad input.
    pub async fn serve<S>(self: &Arc<Self>, stream: S)
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let Ok(handshake_slot) = self.handshakes.clone().try_acquire_owned() else {
            debug!("too many broker connections still in the handshake; dropping one");
            return;
        };
        // Subscribed before the handshake ends, so a client forgotten meanwhile is not missed.
        let mut revoked = self.revoked.subscribe();
        let (mut reader, mut writer) = tokio::io::split(stream);
        let Some((client, session)) = self.handshake(&mut reader, &mut writer).await else {
            return;
        };
        drop(handshake_slot);
        if !self.still_paired(&client) {
            return;
        }

        // Responses go through one writer task so slow requests (waiting for the user) don't
        // hold up quick ones on the same connection.
        let (out, mut queue) = mpsc::channel::<Vec<u8>>(32);
        let writer_task = tokio::spawn(async move {
            while let Some(bytes) = queue.recv().await {
                match write_message(&mut writer, &bytes).await {
                    Ok(()) => {}
                    // Nothing this big is queued (replies are checked first); skip it rather
                    // than silently ending the writer and with it every later answer.
                    Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
                    Err(_) => break,
                }
            }
        });

        // Requests run as tasks owned by this connection. When the client goes away they are
        // aborted, so a pending unlock or Windows Hello prompt is dropped with the connection
        // instead of waiting out its timeout (and a passkey is never made for a caller that
        // has already left).
        let mut requests = JoinSet::new();
        loop {
            let read = tokio::select! {
                read = read_message_timed(&mut reader, IDLE_TIMEOUT, HANDSHAKE_STEP) => read,
                () = self.wait_revoked(&mut revoked, &client) => {
                    info!(client = %client.id, "closing the connection of a client that was removed");
                    break;
                }
            };
            let bytes = match read {
                Ok(Some(bytes)) => bytes,
                Ok(None) => break,
                Err(e) => {
                    debug!(error = %e, "broker read failed");
                    break;
                }
            };
            if !self.still_paired(&client) {
                break;
            }
            let envelope: RequestEnvelope = match serde_json::from_slice(&bytes) {
                Ok(env) => env,
                Err(e) => {
                    // A request we don't understand: answer it if it has an id, else hang up.
                    let id = serde_json::from_slice::<Value>(&bytes)
                        .ok()
                        .and_then(|v| v.get("id").and_then(Value::as_u64));
                    let Some(id) = id else { break };
                    let reply = ResponseEnvelope::failure(
                        id,
                        "unsupported",
                        format!("unknown request: {e}"),
                    );
                    // Never wait for room: a peer that does not read its answers gets hung up on.
                    if out
                        .try_send(serde_json::to_vec(&reply).expect("serializes"))
                        .is_err()
                    {
                        break;
                    }
                    continue;
                }
            };
            while requests.try_join_next().is_some() {}
            if requests.len() >= MAX_IN_FLIGHT {
                let reply = ResponseEnvelope::failure(
                    envelope.id,
                    "tooMany",
                    "too many requests are waiting; try again when some have finished",
                );
                if out
                    .try_send(serde_json::to_vec(&reply).expect("serializes"))
                    .is_err()
                {
                    break;
                }
                continue;
            }
            let broker = self.clone();
            let out = out.clone();
            let client = client.clone();
            requests.spawn(async move {
                let reply = match broker.dispatch(&client, envelope.request).await {
                    Ok(result) => ResponseEnvelope::success(envelope.id, result),
                    Err(e) => ResponseEnvelope::failure(envelope.id, e.code, e.message),
                };
                let mut bytes = serde_json::to_vec(&reply).expect("serializes");
                if bytes.len() > MAX_MESSAGE {
                    // Say so instead of dropping the answer: the client would wait for it.
                    let size = bytes.len();
                    bytes = serde_json::to_vec(&ResponseEnvelope::failure(
                        envelope.id,
                        "tooLarge",
                        format!("the answer ({size} bytes) is too large to send"),
                    ))
                    .expect("serializes");
                }
                let _ = out.send(bytes).await;
            });
        }
        requests.abort_all();
        drop(requests);
        drop(out);
        let _ = writer_task.await;
        drop(session);
    }

    /// Authenticates the connection. Returns the client on success, with the slot it occupies
    /// among the open sessions.
    async fn handshake<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
    ) -> Option<(ClientRecord, OwnedSemaphorePermit)>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let Some(ClientMessage::Hello {
            v,
            name,
            kind,
            client_id,
        }) = recv(reader, HANDSHAKE_STEP).await
        else {
            return None;
        };
        if v != PROTOCOL_VERSION {
            let _ = send(
                writer,
                &ServerMessage::Error {
                    code: "version".into(),
                    message: format!("protocol version {v} is not supported"),
                },
            )
            .await;
            return None;
        }

        let known = client_id
            .as_deref()
            .and_then(|id| self.host.clients().get(id));
        if let Some(record) = known {
            return self.authenticate(reader, writer, record).await;
        }
        send(writer, &ServerMessage::Unpaired).await.ok()?;
        match recv(reader, HANDSHAKE_STEP).await? {
            ClientMessage::Pair => self.pair(reader, writer, name, kind).await,
            _ => None,
        }
    }

    async fn authenticate<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
        record: ClientRecord,
    ) -> Option<(ClientRecord, OwnedSemaphorePermit)>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let nonce = random_bytes::<32>().ok()?;
        send(
            writer,
            &ServerMessage::Challenge {
                nonce: b64u::encode(&nonce),
            },
        )
        .await
        .ok()?;
        let ClientMessage::Auth {
            client_nonce,
            proof: client_proof,
        } = recv(reader, HANDSHAKE_STEP).await?
        else {
            return None;
        };
        let client_nonce = b64u::decode(&client_nonce).ok()?;
        let client_proof = b64u::decode(&client_proof).ok()?;
        let expected = proof(
            CLIENT_PROOF_LABEL,
            &record.secret,
            &nonce,
            &client_nonce,
            &record.id,
        );
        if client_nonce.len() != 32 || !ct_eq(&expected, &client_proof) {
            warn!(client = %record.id, "broker client failed authentication");
            let _ = send(
                writer,
                &ServerMessage::Error {
                    code: "auth".into(),
                    message: "authentication failed".into(),
                },
            )
            .await;
            return None;
        }
        let server_proof = proof(
            SERVER_PROOF_LABEL,
            &record.secret,
            &nonce,
            &client_nonce,
            &record.id,
        );
        let session = self.session_slot(writer).await?;
        send(
            writer,
            &ServerMessage::Ready {
                status: self.host.engine().status(),
                server_proof: b64u::encode(&server_proof),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
            },
        )
        .await
        .ok()?;
        self.host.clients().touch(&record.id, now_ms());
        Some((record, session))
    }

    async fn pair<R, W>(
        &self,
        reader: &mut R,
        writer: &mut W,
        name: String,
        kind: ClientKind,
    ) -> Option<(ClientRecord, OwnedSemaphorePermit)>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let Ok(_slot) = self.pairing.try_lock() else {
            let _ = send(
                writer,
                &ServerMessage::Error {
                    code: "busy".into(),
                    message: "another connection request is waiting for approval".into(),
                },
            )
            .await;
            return None;
        };
        let name: String = name.chars().filter(|c| !c.is_control()).take(60).collect();
        let prompt = PairingPrompt {
            name: if name.trim().is_empty() {
                "Unnamed client".into()
            } else {
                name
            },
            kind,
            code: format!("{:06}", random_below(1_000_000).ok()?),
        };
        send(
            writer,
            &ServerMessage::Pairing {
                code: prompt.code.clone(),
            },
        )
        .await
        .ok()?;
        // If the client gives up while the user is deciding, withdraw the request at once so the
        // approval prompt goes away and the next client is not told "busy".
        let approved = tokio::select! {
            answer = timeout(PAIRING_APPROVAL, self.host.approve_pairing(&prompt)) => {
                answer.unwrap_or(false)
            }
            () = hung_up(reader) => {
                debug!("the client left before its pairing request was answered");
                return None;
            }
        };
        if !approved {
            let _ = send(writer, &ServerMessage::Denied).await;
            return None;
        }
        let session = self.session_slot(writer).await?;
        let now = now_ms();
        let record = ClientRecord {
            id: b64u::encode(&random_bytes::<16>().ok()?),
            name: prompt.name,
            kind,
            secret: random_bytes::<32>().ok()?.to_vec(),
            created_ms: now,
            last_used_ms: now,
        };
        self.host.clients().insert(record.clone()).ok()?;
        info!(client = %record.id, name = %record.name, "paired a new broker client");
        send(
            writer,
            &ServerMessage::Paired {
                client_id: record.id.clone(),
                secret: b64u::encode(&record.secret),
                status: self.host.engine().status(),
            },
        )
        .await
        .ok()?;
        Some((record, session))
    }

    /// Takes a slot among the open sessions, or tells the client the app is full.
    async fn session_slot<W: AsyncWrite + Unpin>(
        &self,
        writer: &mut W,
    ) -> Option<OwnedSemaphorePermit> {
        if let Ok(slot) = self.sessions.clone().try_acquire_owned() {
            return Some(slot);
        }
        warn!("too many open broker connections; refusing one");
        let _ = send(
            writer,
            &ServerMessage::Error {
                code: "tooMany".into(),
                message: "LocalPass has too many open connections".into(),
            },
        )
        .await;
        None
    }

    /// Runs a blocking engine call off the async threads.
    async fn engine<T, F>(&self, f: F) -> Result<T, BrokerError>
    where
        T: Send + 'static,
        F: FnOnce(&Engine) -> Result<T, EngineError> + Send + 'static,
    {
        let engine = self.host.engine().clone();
        tokio::task::spawn_blocking(move || f(&engine))
            .await
            .map_err(|e| BrokerError::new("internal", e.to_string()))?
            .map_err(BrokerError::from)
    }

    /// Makes sure the vault is unlocked, asking the user to unlock it when `interactive`.
    ///
    /// Returns whether *this call* unlocked it: the user has just proved who they are (quick
    /// unlock with Windows Hello, or the master password), which the passkey requests count as
    /// their user verification instead of asking a second time. It is never carried over to a
    /// later request.
    async fn ensure_unlocked(&self, interactive: bool) -> Result<bool, BrokerError> {
        match self.host.engine().status() {
            VaultStatus::Unlocked => Ok(false),
            VaultStatus::NoVault => Err(BrokerError::new(
                "noVault",
                "LocalPass has no vault on this device yet",
            )),
            VaultStatus::Locked => {
                if !interactive {
                    return Err(BrokerError::new("locked", "The vault is locked"));
                }
                let failures_seen = self.unlock_failures.load(Ordering::SeqCst);
                let _turn = self.unlocking.lock().await;
                if self.host.engine().status() == VaultStatus::Unlocked {
                    // Another request's prompt unlocked it while this one waited: not this
                    // request's user check.
                    return Ok(false);
                }
                if self.unlock_failures.load(Ordering::SeqCst) != failures_seen {
                    // The prompt this request waited behind ended without unlocking: the user
                    // said no (or ignored it), so a queue of requests does not ask again.
                    return Err(BrokerError::new("locked", "The vault is locked"));
                }
                let unlocked = timeout(USER_ACTION, self.host.unlock())
                    .await
                    .unwrap_or(false)
                    && self.host.engine().status() == VaultStatus::Unlocked;
                if unlocked {
                    Ok(true)
                } else {
                    self.unlock_failures.fetch_add(1, Ordering::SeqCst);
                    Err(BrokerError::new("locked", "The vault is locked"))
                }
            }
        }
    }

    /// Whether the user has been verified for a passkey operation (Windows Hello).
    /// `just_unlocked`: this very request made the user unlock the vault, which already was the
    /// check, so they are not asked again.
    async fn verified(
        &self,
        client: &ClientRecord,
        uv: UserVerification,
        reason: &str,
        just_unlocked: bool,
    ) -> Result<bool, BrokerError> {
        if just_unlocked {
            return Ok(true);
        }
        if uv == UserVerification::Discouraged && !self.host.policy().verify_every_passkey {
            return Ok(false);
        }
        self.confirm_reveal(client, reason).await?;
        Ok(true)
    }

    /// Asks the user for Windows Hello (or the app's own check) and fails with `denied` unless they
    /// pass. Used before anything that reveals a secret or signs, whatever the user's relaxed
    /// passkey policy says.
    async fn confirm_reveal(&self, client: &ClientRecord, reason: &str) -> Result<(), BrokerError> {
        let approved = timeout(USER_ACTION, async {
            let _turn = self.asking.lock().await;
            self.host.verify_user(reason).await
        })
        .await
        .unwrap_or(false);
        if approved {
            self.check_paired(client)
        } else {
            Err(BrokerError::new("denied", "The request was not approved"))
        }
    }

    /// Asks the user before something they already have is replaced. `denied` unless they agree.
    async fn confirm_overwrite(
        &self,
        client: &ClientRecord,
        prompt: OverwritePrompt,
    ) -> Result<(), BrokerError> {
        let agreed = timeout(USER_ACTION, async {
            let _turn = self.asking.lock().await;
            self.host.confirm_overwrite(&prompt).await
        })
        .await
        .unwrap_or(false);
        if agreed {
            self.check_paired(client)
        } else {
            Err(BrokerError::new(
                "denied",
                "You chose not to replace what is saved in LocalPass",
            ))
        }
    }

    async fn dispatch(&self, client: &ClientRecord, request: Request) -> Outcome {
        self.check_paired(client)?;
        if client.kind == ClientKind::Provider && !request.allowed_for_provider() {
            return Err(BrokerError::new(
                "forbidden",
                "this client may only use passkey requests",
            ));
        }
        if client.kind != ClientKind::Provider && request.provider_only() {
            return Err(BrokerError::new(
                "forbidden",
                "only the Windows passkey provider may use this request",
            ));
        }
        // Status, lock and the generator work on a locked vault. Everything else needs it
        // unlocked; requests that reveal or sign something may ask the user to unlock first.
        let mut just_unlocked = false;
        if !matches!(
            request,
            Request::Status | Request::Lock | Request::GeneratePassword { .. }
        ) {
            let may_prompt = request.is_interactive() || matches!(request, Request::Unlock);
            just_unlocked = self.ensure_unlocked(may_prompt).await?;
            // The user may have removed this client while the prompt was open.
            self.check_paired(client)?;
        }
        match request {
            Request::Status => {
                let engine = self.host.engine();
                to_value(&json!({
                    "status": engine.status(),
                    "deviceName": engine.device_name(),
                    "appVersion": env!("CARGO_PKG_VERSION"),
                }))
            }
            Request::Unlock => to_value(&json!({ "status": self.host.engine().status() })),
            Request::Lock => {
                self.host.engine().lock();
                to_value(&json!({ "status": self.host.engine().status() }))
            }
            Request::FindLogins { origin } => {
                to_value(&self.engine(move |e| e.find_logins(&origin)).await?)
            }
            Request::SearchItems { query, limit } => {
                // Every login in the vault, whatever site the browser is on: the user must say
                // yes each time, so a program that merely holds the client secret gets nothing
                // silently.
                let limit = limit.unwrap_or(50).min(200);
                self.confirm_reveal(client, "Search the logins in your vault")
                    .await?;
                to_value(&self.engine(move |e| e.search_logins(&query, limit)).await?)
            }
            Request::FillLogin {
                item_id: id,
                origin,
                force,
            } => {
                // A login saved for this site fills silently. `force` (a login the user picked
                // from the full list) tries that first and otherwise needs the user's check, with
                // the login and the page named. Whatever cannot succeed (a trashed item, a note,
                // a bad page) fails before anybody is asked.
                let (same_id, same_origin) = (id.clone(), origin.clone());
                let same_site = self
                    .engine(move |e| e.fill_login(&same_id, &same_origin))
                    .await;
                let fill = match same_site {
                    Ok(fill) => fill,
                    Err(_) if force => {
                        let (check_id, check_origin) = (id.clone(), origin.clone());
                        let (label, host) = self
                            .engine(move |e| {
                                let page = lp_core::origin::Origin::parse(&check_origin)?;
                                Ok((e.login_label(&check_id)?, page.host))
                            })
                            .await?;
                        let reason = format!(
                            "Fill \"{}\" on {}",
                            display_text(&label.title),
                            display_text(&host)
                        );
                        self.confirm_reveal(client, &reason).await?;
                        self.engine(move |e| e.fill_login_anywhere(&id, &origin))
                            .await?
                    }
                    Err(e) => return Err(e),
                };
                to_value(&fill)
            }
            Request::SaveLogin {
                origin,
                username,
                password,
                title,
            } => {
                // A new login is saved silently. One that would change a saved login's password
                // (also from another address of the same site) is only applied after the user
                // agrees in the app.
                let (plan_origin, plan_user, plan_password) =
                    (origin.clone(), username.clone(), password.clone());
                let plan = self
                    .engine(move |e| e.plan_save_login(&plan_origin, &plan_user, &plan_password))
                    .await?;
                let outcome = match plan {
                    SavePlan::Unchanged { id } => SaveLoginOutcome {
                        id,
                        status: SaveStatus::Unchanged,
                    },
                    SavePlan::Create => {
                        self.engine(move |e| {
                            e.create_login(&origin, &username, &password, title.as_deref())
                        })
                        .await?
                    }
                    SavePlan::Update(change) => {
                        let site = lp_core::origin::Origin::parse(&origin)
                            .map(|o| o.host)
                            .unwrap_or_default();
                        let prompt = OverwritePrompt {
                            kind: OverwriteKind::Login,
                            title: display_text(&change.title),
                            username: display_text(&change.username),
                            site: display_text(&site),
                            saved_for: change.saved_url.as_deref().map(display_text),
                            same_host: change.same_host,
                        };
                        self.confirm_overwrite(client, prompt).await?;
                        let id = change.id;
                        self.engine(move |e| e.update_login_password(&id, &password))
                            .await?
                    }
                };
                to_value(&outcome)
            }
            Request::GeneratePassword { options } => {
                let options = options.unwrap_or_default();
                let password = self
                    .engine(move |_| Engine::generate_password(&options))
                    .await?;
                to_value(&json!({ "password": password.as_str() }))
            }
            Request::PasskeyCandidates {
                origin,
                rp_id,
                allow,
            } => to_value(
                &self
                    .engine(move |e| e.passkey_candidates(&origin, &rp_id, &allow))
                    .await?,
            ),
            Request::PasskeyListAll => to_value(&self.engine(|e| e.passkey_list_all()).await?),
            Request::PasskeyCreate { mut request, uv } => {
                let check = request.clone();
                let existing = self
                    .engine(move |e| e.passkey_would_replace(&check))
                    .await?;
                if let Some(existing) = existing {
                    // The site asked for a passkey for an account that has one already: the new
                    // passkey replaces it, so the user decides first.
                    let prompt = OverwritePrompt {
                        kind: OverwriteKind::Passkey,
                        title: display_text(&existing.rp_name),
                        username: display_text(&existing.user_name),
                        site: display_text(&existing.rp_id),
                        saved_for: None,
                        same_host: true,
                    };
                    self.confirm_overwrite(client, prompt).await?;
                }
                let reason = create_reason(&request.rp_name, &request.rp_id);
                request.user_verified = self.verified(client, uv, &reason, just_unlocked).await?;
                to_value(&self.engine(move |e| e.passkey_create(&request)).await?)
            }
            Request::PasskeyAssertAny {
                origin,
                rp_id,
                allow,
                client_data_hash,
                uv,
            } => {
                // One request from unlock to signature: the unlock this request asked for is
                // the user check (`just_unlocked`), so Windows Hello is shown once.
                let (lookup_origin, lookup_rp) = (origin.clone(), rp_id.clone());
                let found = self
                    .engine(move |e| e.passkey_candidates(&lookup_origin, &lookup_rp, &allow))
                    .await?;
                let [chosen] = found.as_slice() else {
                    return Err(if found.is_empty() {
                        BrokerError::new("notFound", "no matching passkey")
                    } else {
                        BrokerError::new("ambiguous", "several passkeys match: choose one")
                    });
                };
                if client_data_hash.len() != 32 {
                    return Err(BrokerError::new(
                        "invalid",
                        "the client data hash must be 32 bytes",
                    ));
                }
                let (item_id, user_name, user_display_name) = (
                    chosen.item_id.clone(),
                    chosen.user_name.clone(),
                    chosen.user_display_name.clone(),
                );
                let reason = format!("Sign in to {}", display_text(&rp_id));
                let mut request = PasskeyAssertRequest {
                    origin,
                    rp_id,
                    item_id,
                    client_data_hash,
                    user_verified: false,
                };
                request.user_verified = self.verified(client, uv, &reason, just_unlocked).await?;
                let assertion = self.engine(move |e| e.passkey_assert(&request)).await?;
                to_value(&AssertedPasskey {
                    assertion,
                    user_name,
                    user_display_name,
                })
            }
            Request::PasskeyAssert { mut request, uv } => {
                let reason = format!("Sign in to {}", display_text(&request.rp_id));
                request.user_verified = self.verified(client, uv, &reason, just_unlocked).await?;
                to_value(&self.engine(move |e| e.passkey_assert(&request)).await?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_text_from_websites_is_made_safe() {
        assert_eq!(
            create_reason("Example", "example.com"),
            "Create a passkey for Example (example.com)"
        );
        assert_eq!(
            create_reason("", "example.com"),
            "Create a passkey for example.com"
        );
        assert_eq!(
            create_reason("example.com", "example.com"),
            "Create a passkey for example.com"
        );
        // Control characters (a forged second line) are dropped and long names are cut.
        let reason = create_reason("Bank\r\nApprove ALL\u{7}", "evil.example");
        assert_eq!(
            reason,
            "Create a passkey for BankApprove ALL (evil.example)"
        );
        let long = create_reason(&"x".repeat(500), "a.example");
        assert!(long.chars().count() < 100, "{long}");
    }

    /// RA-7: text that reorders or hides what is around it is dropped, ordinary Unicode is kept.
    #[test]
    fn display_text_drops_invisible_and_direction_characters() {
        assert_eq!(
            display_text("a\u{202E}b\u{200B}c\u{FEFF}d\u{2066}e"),
            "abcde"
        );
        assert_eq!(display_text("\u{061C}\u{200F}x\u{00AD}y"), "xy");
        assert_eq!(display_text("Café 日本語"), "Café 日本語");
    }
}
