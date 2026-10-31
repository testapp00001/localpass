//! A Rust client for the broker, used by the Windows passkey provider (and by tests).
//!
//! It speaks the same protocol as the browser extension: it proves it knows its secret and
//! checks that the other end does too (so a process squatting on the pipe name is refused), can
//! pair once with approval in the app, and then sends requests. It is generic over the byte
//! stream, so tests run it over in-memory pipes and the provider over [`connect_pipe`].
//!
//! ```text
//! first time:  Client::pair(stream, ..)      -> (client, Paired { credentials, .. })   keep `credentials`
//! afterwards:  Client::connect(stream, .., &credentials)
//! then:        client.request(&Request::..)  -> Value
//! ```
//!
//! # One connection per operation
//!
//! Open a connection for each operation you may need to cancel and drop it to cancel: the app
//! then abandons the pending unlock or Windows Hello prompt (see `Broker::serve`). A client that
//! returns an error from a call because of I/O, a timeout or a protocol error, or whose call was
//! dropped half way, is unusable afterwards (later calls fail with [`ClientError::Closed`]).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::io;
use std::time::Duration;

use lp_core::crypto::{ct_eq, random_bytes};
use lp_core::passkey::b64u;
use lp_core::status::VaultStatus;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;
use zeroize::Zeroizing;

use crate::clients::ClientKind;
use crate::framing::{read_message, write_message};
use crate::protocol::{
    CLIENT_PROOF_LABEL, ClientMessage, PROTOCOL_VERSION, ResponseEnvelope, SERVER_PROOF_LABEL,
    ServerMessage, proof,
};

/// Why a call failed. The first group says what the provider should tell the OS.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The app does not know this client: it never paired, or the user removed it.
    #[error("this program is not connected to LocalPass (pair it again)")]
    Unpaired,
    /// The user did not approve: pairing was denied, or Windows Hello was cancelled or failed.
    #[error("the request was not approved")]
    Denied,
    /// The app is already waiting for another connection request.
    #[error("LocalPass is waiting for another connection request; try again shortly")]
    Busy,
    /// The vault is locked and could not be unlocked for this request.
    #[error("the vault is locked")]
    Locked,
    /// The other end of the pipe could not prove it knows our secret: it is not LocalPass.
    #[error("the program on the other end could not prove it is LocalPass")]
    ServerProofMismatch,
    /// The pipe could not be opened or broke. See [`ClientError::app_not_running`].
    #[error("could not talk to LocalPass: {0}")]
    Io(#[from] io::Error),
    /// LocalPass did not answer a step in time; the name says which step.
    #[error("LocalPass did not answer in time ({0})")]
    Timeout(&'static str),
    /// The app refused or failed the request. `code` is stable (`invalid`, `notFound`,
    /// `forbidden`, `unsupported`, `passkeyExists`, `noVault`, `internal`, ...).
    #[error("LocalPass said {code}: {message}")]
    Remote { code: String, message: String },
    /// The other end sent something that is not part of the protocol.
    #[error("unexpected reply from LocalPass: {0}")]
    Protocol(String),
    /// The connection is closed, or an earlier call left it unusable.
    #[error("the connection to LocalPass is closed")]
    Closed,
}

impl ClientError {
    /// Whether this is "the app is not running" (no pipe to open), so the caller can start it.
    pub fn app_not_running(&self) -> bool {
        matches!(self, ClientError::Io(e) if crate::pipe::app_not_running(e))
    }

    fn remote(code: String, message: String) -> Self {
        match code.as_str() {
            "locked" => ClientError::Locked,
            "denied" => ClientError::Denied,
            "busy" => ClientError::Busy,
            _ => ClientError::Remote { code, message },
        }
    }
}

/// How long each step may take. The app has its own limits (10 s per handshake step, 180 s for
/// the user to approve pairing, 120 s each for unlocking and Windows Hello), so the defaults are
/// a little longer and let the app's own answer arrive first.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    /// Each handshake step, and opening the pipe.
    pub step: Duration,
    /// Waiting for the user to approve pairing in the app.
    pub pairing: Duration,
    /// A request. Creating or using a passkey can wait for an unlock and then for Hello.
    pub request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            step: Duration::from_secs(10),
            pairing: Duration::from_secs(200),
            request: Duration::from_secs(300),
        }
    }
}

/// Who we are to the app.
#[derive(Clone, Debug)]
pub struct Options {
    /// Shown to the user when pairing, for example "Windows passkey provider".
    pub name: String,
    pub kind: ClientKind,
    pub timeouts: Timeouts,
}

impl Options {
    pub fn new(name: impl Into<String>, kind: ClientKind) -> Self {
        Self {
            name: name.into(),
            kind,
            timeouts: Timeouts::default(),
        }
    }
}

/// What pairing gives back: keep it somewhere only this user can read (DPAPI on Windows).
#[derive(Clone)]
pub struct Credentials {
    pub client_id: String,
    pub secret: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for Credentials {
    // The secret must never end up in logs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("client_id", &self.client_id)
            .field("secret", &"<hidden>")
            .finish()
    }
}

/// The result of a successful pairing.
#[derive(Clone, Debug)]
pub struct Paired {
    pub credentials: Credentials,
    pub status: VaultStatus,
}

/// An authenticated connection to the app.
pub struct Client<S> {
    stream: S,
    timeouts: Timeouts,
    next_id: u64,
    /// Requests sent and not yet answered; the answer is parked here if it arrives while
    /// another request is being waited for (the app answers in whatever order they finish).
    pending: HashMap<u64, Option<ResponseEnvelope>>,
    status: VaultStatus,
    app_version: String,
    broken: bool,
}

impl<S> fmt::Debug for Client<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("status", &self.status)
            .field("app_version", &self.app_version)
            .field("broken", &self.broken)
            .finish()
    }
}

async fn step<T>(
    what: &'static str,
    wait: Duration,
    work: impl Future<Output = io::Result<T>>,
) -> Result<T, ClientError> {
    match timeout(wait, work).await {
        Ok(result) => Ok(result?),
        Err(_) => Err(ClientError::Timeout(what)),
    }
}

async fn send<S, T>(
    stream: &mut S,
    what: &'static str,
    wait: Duration,
    message: &T,
) -> Result<(), ClientError>
where
    S: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(message)
        .map_err(|e| ClientError::Protocol(format!("cannot encode {what}: {e}")))?;
    step(what, wait, write_message(stream, &bytes)).await
}

/// Reads the next frame, `Closed` if the app hung up.
async fn recv_bytes<S: AsyncRead + Unpin>(
    stream: &mut S,
    what: &'static str,
    wait: Duration,
) -> Result<Vec<u8>, ClientError> {
    step(what, wait, read_message(stream))
        .await?
        .ok_or(ClientError::Closed)
}

async fn recv_server<S: AsyncRead + Unpin>(
    stream: &mut S,
    what: &'static str,
    wait: Duration,
) -> Result<ServerMessage, ClientError> {
    let bytes = recv_bytes(stream, what, wait).await?;
    serde_json::from_slice(&bytes)
        .map_err(|e| ClientError::Protocol(format!("{what}: not a handshake message ({e})")))
}

/// A handshake `Error` from the app as the error the caller should see.
fn handshake_error(code: String, message: String) -> ClientError {
    match code.as_str() {
        // The secret was refused: as far as the app is concerned we are not paired.
        "auth" => ClientError::Unpaired,
        _ => ClientError::remote(code, message),
    }
}

fn unexpected(what: &str, got: &ServerMessage) -> ClientError {
    let kind = match got {
        ServerMessage::Challenge { .. } => "challenge",
        ServerMessage::Unpaired => "unpaired",
        ServerMessage::Pairing { .. } => "pairing",
        ServerMessage::Paired { .. } => "paired",
        ServerMessage::Ready { .. } => "ready",
        ServerMessage::Denied => "denied",
        ServerMessage::Error { .. } => "error",
    };
    ClientError::Protocol(format!("{kind} where {what} was expected"))
}

impl<S> Client<S>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    fn new(stream: S, timeouts: Timeouts, status: VaultStatus, app_version: String) -> Self {
        Self {
            stream,
            timeouts,
            next_id: 0,
            pending: HashMap::new(),
            status,
            app_version,
            broken: false,
        }
    }

    /// Opens an authenticated connection for a client that paired before. Fails with
    /// [`ClientError::Unpaired`] if the app no longer knows it (pair again), and with
    /// [`ClientError::ServerProofMismatch`] if the other end is not the app.
    pub async fn connect(
        mut stream: S,
        options: &Options,
        credentials: &Credentials,
    ) -> Result<Self, ClientError> {
        let t = options.timeouts;
        let hello = ClientMessage::Hello {
            v: PROTOCOL_VERSION,
            name: options.name.clone(),
            kind: options.kind,
            client_id: Some(credentials.client_id.clone()),
        };
        send(&mut stream, "hello", t.step, &hello).await?;

        let server_nonce = match recv_server(&mut stream, "challenge", t.step).await? {
            ServerMessage::Challenge { nonce } => b64u::decode(&nonce)
                .map_err(|e| ClientError::Protocol(format!("bad challenge ({e})")))?,
            ServerMessage::Unpaired => return Err(ClientError::Unpaired),
            ServerMessage::Error { code, message } => return Err(handshake_error(code, message)),
            other => return Err(unexpected("a challenge", &other)),
        };

        let client_nonce =
            random_bytes::<32>().map_err(|e| ClientError::Io(io::Error::other(e.to_string())))?;
        let client_proof = proof(
            CLIENT_PROOF_LABEL,
            &credentials.secret,
            &server_nonce,
            &client_nonce,
            &credentials.client_id,
        );
        send(
            &mut stream,
            "auth",
            t.step,
            &ClientMessage::Auth {
                client_nonce: b64u::encode(&client_nonce),
                proof: b64u::encode(&client_proof),
            },
        )
        .await?;

        match recv_server(&mut stream, "ready", t.step).await? {
            ServerMessage::Ready {
                status,
                server_proof,
                app_version,
            } => {
                let expected = proof(
                    SERVER_PROOF_LABEL,
                    &credentials.secret,
                    &server_nonce,
                    &client_nonce,
                    &credentials.client_id,
                );
                let given = b64u::decode(&server_proof).unwrap_or_default();
                if !ct_eq(&expected, &given) {
                    return Err(ClientError::ServerProofMismatch);
                }
                Ok(Self::new(stream, t, status, app_version))
            }
            ServerMessage::Error { code, message } => Err(handshake_error(code, message)),
            other => Err(unexpected("ready", &other)),
        }
    }

    /// Pairs a new client. The user must approve it in the app, where the same six-digit code
    /// is shown, so `on_code` should show the code to the user at once. Returns the connection
    /// (already usable) and the credentials to store for next time.
    ///
    /// The app answers [`ClientError::Denied`] if the user says no or the request times out
    /// (after about 180 s), and [`ClientError::Busy`] if another request is waiting.
    pub async fn pair(
        mut stream: S,
        options: &Options,
        on_code: impl FnOnce(&str),
    ) -> Result<(Self, Paired), ClientError> {
        let t = options.timeouts;
        send(
            &mut stream,
            "hello",
            t.step,
            &ClientMessage::Hello {
                v: PROTOCOL_VERSION,
                name: options.name.clone(),
                kind: options.kind,
                client_id: None,
            },
        )
        .await?;
        match recv_server(&mut stream, "unpaired", t.step).await? {
            ServerMessage::Unpaired => {}
            ServerMessage::Error { code, message } => return Err(handshake_error(code, message)),
            other => return Err(unexpected("unpaired", &other)),
        }
        send(&mut stream, "pair", t.step, &ClientMessage::Pair).await?;
        match recv_server(&mut stream, "pairing code", t.step).await? {
            ServerMessage::Pairing { code } => on_code(&code),
            ServerMessage::Error { code, message } => return Err(handshake_error(code, message)),
            other => return Err(unexpected("a pairing code", &other)),
        }
        match recv_server(&mut stream, "pairing approval", t.pairing).await? {
            ServerMessage::Paired {
                client_id,
                secret,
                status,
            } => {
                let secret = Zeroizing::new(
                    b64u::decode(&secret)
                        .map_err(|e| ClientError::Protocol(format!("bad secret ({e})")))?,
                );
                let paired = Paired {
                    credentials: Credentials { client_id, secret },
                    status,
                };
                let client = Self::new(stream, t, status, String::new());
                Ok((client, paired))
            }
            ServerMessage::Denied => Err(ClientError::Denied),
            ServerMessage::Error { code, message } => Err(handshake_error(code, message)),
            other => Err(unexpected("the pairing answer", &other)),
        }
    }

    /// Gives the byte stream back, for example to send hand-made frames in a protocol test.
    pub fn into_stream(self) -> S {
        self.stream
    }

    /// The vault's state when the connection was made. Ask with a `status` request for a fresh one.
    pub fn status(&self) -> VaultStatus {
        self.status
    }

    /// The app's version (empty on a connection that was just paired).
    pub fn app_version(&self) -> &str {
        &self.app_version
    }

    /// Sends a request (a [`crate::protocol::Request`], or any JSON object with an `op`) and
    /// waits for its result, using the request timeout.
    pub async fn request<T: Serialize>(&mut self, body: &T) -> Result<Value, ClientError> {
        let id = self.send_request(body).await?;
        self.response(id).await
    }

    /// Like [`Client::request`], with the result parsed as `R`.
    pub async fn request_as<R: DeserializeOwned, T: Serialize>(
        &mut self,
        body: &T,
    ) -> Result<R, ClientError> {
        let value = self.request(body).await?;
        serde_json::from_value(value)
            .map_err(|e| ClientError::Protocol(format!("unexpected result shape ({e})")))
    }

    /// Sends a request without waiting; collect the answer with [`Client::response`]. Several
    /// requests may be outstanding, and the app answers them in the order they finish.
    pub async fn send_request<T: Serialize>(&mut self, body: &T) -> Result<u64, ClientError> {
        self.usable()?;
        let mut value = serde_json::to_value(body)
            .map_err(|e| ClientError::Protocol(format!("cannot encode the request: {e}")))?;
        let Some(object) = value.as_object_mut() else {
            return Err(ClientError::Protocol("a request must be an object".into()));
        };
        self.next_id += 1;
        let id = self.next_id;
        object.insert("id".into(), id.into());
        self.pending.insert(id, None);
        let bytes = serde_json::to_vec(&value).expect("a JSON value serializes");
        let sent = step(
            "request",
            self.timeouts.step,
            write_message(&mut self.stream, &bytes),
        )
        .await;
        if sent.is_err() {
            self.broken = true;
        }
        sent?;
        Ok(id)
    }

    /// Waits for the answer to request `id`.
    pub async fn response(&mut self, id: u64) -> Result<Value, ClientError> {
        self.usable()?;
        loop {
            if let Some(Some(_)) = self.pending.get(&id) {
                let envelope = self.pending.remove(&id).flatten().expect("checked above");
                return finish(envelope);
            }
            if !self.pending.contains_key(&id) {
                return Err(ClientError::Protocol(format!(
                    "request {id} is not pending"
                )));
            }
            // A read cut short (the caller dropped this future) leaves a half-read frame, so
            // the connection stays marked unusable until the read completes.
            self.broken = true;
            let bytes = recv_bytes(&mut self.stream, "request", self.timeouts.request).await?;
            let envelope: ResponseEnvelope = serde_json::from_slice(&bytes)
                .map_err(|e| ClientError::Protocol(format!("not a response ({e})")))?;
            self.broken = false;
            // Answers nobody asked for are ignored.
            if let Some(slot) = self.pending.get_mut(&envelope.id) {
                *slot = Some(envelope);
            }
        }
    }

    fn usable(&self) -> Result<(), ClientError> {
        if self.broken {
            Err(ClientError::Closed)
        } else {
            Ok(())
        }
    }
}

fn finish(envelope: ResponseEnvelope) -> Result<Value, ClientError> {
    if envelope.ok {
        return Ok(envelope.result.unwrap_or(Value::Null));
    }
    let error = envelope.error.unwrap_or(crate::protocol::ErrorBody {
        code: "internal".into(),
        message: "the request failed".into(),
    });
    Err(ClientError::remote(error.code, error.message))
}

/// Opens the app's named pipe or socket (see [`crate::pipe::pipe_name_for`] for `profile`) and logs in.
#[cfg(any(windows, target_os = "linux"))]
pub async fn connect_pipe(
    profile: Option<&str>,
    options: &Options,
    credentials: &Credentials,
) -> Result<Client<crate::pipe::Stream>, ClientError> {
    let pipe = step(
        "open the pipe",
        options.timeouts.step,
        crate::pipe::connect_profile(profile),
    )
    .await?;
    Client::connect(pipe, options, credentials).await
}

/// Opens the app's named pipe or socket and pairs; see [`Client::pair`].
#[cfg(any(windows, target_os = "linux"))]
pub async fn pair_pipe(
    profile: Option<&str>,
    options: &Options,
    on_code: impl FnOnce(&str),
) -> Result<(Client<crate::pipe::Stream>, Paired), ClientError> {
    let pipe = step(
        "open the pipe",
        options.timeouts.step,
        crate::pipe::connect_profile(profile),
    )
    .await?;
    Client::pair(pipe, options, on_code).await
}
