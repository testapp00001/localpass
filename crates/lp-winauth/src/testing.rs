//! Helpers for this crate's tests and its integration tests: request builders, response parsers, and
//! fakes of the app ([`FakeRelay`], an in-memory vault that really creates and signs ES256 passkeys), of
//! the OS request signature ([`FakeVerifier`]) and of Windows' credential cache ([`FakeSink`]). Not part
//! of the provider's behaviour.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use ciborium::Value;
use lp_broker::protocol::AssertedPasskey;
use lp_core::passkey::{
    FLAG_BE, FLAG_BS, FLAG_UP, FLAG_UV, PasskeyAssertion, PasskeyCandidate, PasskeyCreateRequest,
    PasskeyCreated, PasskeyKey, attestation_object_none, authenticator_data, new_credential_id,
    parse_authenticator_data,
};
use lp_core::status::VaultStatus;

use windows::Win32::Foundation::E_FAIL;
use windows::core::HRESULT;

use crate::cache::CacheApi;
use crate::handler::{CacheSink, OpVerifier};
use crate::relay::{CancelToken, Relay, RelayError};
use crate::sys::{CredentialRow, NTE_EXISTS, NTE_NOT_FOUND, WebAuthn};

/// The MakeCredential request webauthn.io sent in the stage B spike run (179 bytes).
pub const WEBAUTHN_IO_MAKE_CREDENTIAL: &str = "a601582043cc5af7c1bf8896702177ce43d30904e545e135c5c2a858d0feea3df0b4a11e02a26269646b776562617574686e2e696f646e616d656b776562617574686e2e696f03a362696453776562617574686e696f2d7465737430313031646e616d656874657374303130316b646973706c61794e616d656874657374303130310481a263616c672664747970656a7075626c69632d6b657906a16b6372656450726f746563740207a262726bf5627576f5";

/// The only signature [`FakeVerifier`] accepts.
pub const GOOD_SIGNATURE: &[u8] = b"good-signature";

/// `webauthn.dll`, loaded once (decode and encode work without package identity).
pub fn api() -> &'static WebAuthn {
    static API: OnceLock<WebAuthn> = OnceLock::new();
    API.get_or_init(|| WebAuthn::load().expect("webauthn.dll with the plugin exports"))
}

pub fn unhex(text: &str) -> Vec<u8> {
    data_encoding::HEXLOWER
        .decode(text.as_bytes())
        .expect("valid hex")
}

fn cbor_bytes(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).expect("CBOR to a Vec");
    out
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

fn int(n: i64) -> Value {
    Value::Integer(n.into())
}

fn descriptor(id: &[u8]) -> Value {
    Value::Map(vec![
        (text("id"), Value::Bytes(id.to_vec())),
        (text("type"), text("public-key")),
    ])
}

/// A CTAP2 `authenticatorMakeCredential` request like the ones Windows sends (resident key and user
/// verification requested, `exclude` only when not empty).
pub fn make_credential_request(
    rp_id: &str,
    rp_name: &str,
    user_id: &[u8],
    user_name: &str,
    algorithms: &[i64],
    exclude: &[Vec<u8>],
) -> Vec<u8> {
    let mut map = vec![
        (int(1), Value::Bytes(vec![0x11; 32])),
        (
            int(2),
            Value::Map(vec![
                (text("id"), text(rp_id)),
                (text("name"), text(rp_name)),
            ]),
        ),
        (
            int(3),
            Value::Map(vec![
                (text("id"), Value::Bytes(user_id.to_vec())),
                (text("name"), text(user_name)),
                (text("displayName"), text(user_name)),
            ]),
        ),
        (
            int(4),
            Value::Array(
                algorithms
                    .iter()
                    .map(|alg| {
                        Value::Map(vec![
                            (text("alg"), int(*alg)),
                            (text("type"), text("public-key")),
                        ])
                    })
                    .collect(),
            ),
        ),
    ];
    if !exclude.is_empty() {
        map.push((
            int(5),
            Value::Array(exclude.iter().map(|id| descriptor(id)).collect()),
        ));
    }
    map.push((
        int(7),
        Value::Map(vec![
            (text("rk"), Value::Bool(true)),
            (text("uv"), Value::Bool(true)),
        ]),
    ));
    cbor_bytes(&Value::Map(map))
}

/// Inserts an exclude list into the captured webauthn.io request (key 5, before the extensions map).
pub fn insert_exclude_list(encoded: &mut Vec<u8>, ids: &[Vec<u8>]) {
    let at = encoded
        .windows(3)
        .position(|w| w == [0x06, 0xA1, 0x6B])
        .expect("key 6 in the captured request");
    let list = cbor_bytes(&Value::Array(ids.iter().map(|id| descriptor(id)).collect()));
    let mut insert = vec![0x05];
    insert.extend(list);
    encoded.splice(at..at, insert);
    encoded[0] = 0xA7; // a map of 7 entries
}

/// A CTAP2 `authenticatorGetAssertion` request.
pub fn get_assertion_request(rp_id: &str, client_data_hash: &[u8], allow: &[Vec<u8>]) -> Vec<u8> {
    let mut map = vec![
        (int(1), text(rp_id)),
        (int(2), Value::Bytes(client_data_hash.to_vec())),
    ];
    if !allow.is_empty() {
        map.push((
            int(3),
            Value::Array(allow.iter().map(|id| descriptor(id)).collect()),
        ));
    }
    map.push((int(5), Value::Map(vec![(text("uv"), Value::Bool(true))])));
    cbor_bytes(&Value::Map(map))
}

fn parse(bytes: &[u8]) -> Vec<(Value, Value)> {
    let value: Value = ciborium::de::from_reader(bytes).expect("CBOR");
    value.into_map().expect("a CBOR map")
}

fn field(map: &[(Value, Value)], key: i64) -> Option<&Value> {
    map.iter()
        .find(|(k, _)| {
            k.as_integer()
                .is_some_and(|i| i128::from(i) == i128::from(key))
        })
        .map(|(_, v)| v)
}

fn map_text<'a>(map: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    map.iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

/// The `fmt` of a MakeCredential response (`{1: fmt, 2: authData, 3: attStmt}`).
pub fn attestation_format(response: &[u8]) -> String {
    let map = parse(response);
    field(&map, 1)
        .and_then(Value::as_text)
        .expect("fmt")
        .to_owned()
}

/// The authenticator data of a MakeCredential response.
pub fn attestation_auth_data(response: &[u8]) -> Vec<u8> {
    let map = parse(response);
    field(&map, 2)
        .and_then(Value::as_bytes)
        .expect("authData")
        .clone()
}

/// The credential id inside a MakeCredential response's authenticator data.
pub fn attestation_credential_id(response: &[u8]) -> Vec<u8> {
    parse_authenticator_data(&attestation_auth_data(response))
        .expect("authData")
        .credential
        .expect("attested credential data")
        .0
}

/// A GetAssertion response taken apart: `{1: {id,type}, 2: authData, 3: signature, 4: user, 6: true}`.
#[derive(Debug)]
pub struct ParsedAssertion {
    pub credential_id: Vec<u8>,
    pub auth_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub user_id: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
    pub user_selected: bool,
    pub number_of_credentials: Option<u64>,
}

pub fn parse_assertion(response: &[u8]) -> ParsedAssertion {
    let map = parse(response);
    let credential = field(&map, 1)
        .and_then(Value::as_map)
        .expect("credential")
        .clone();
    let user = field(&map, 4)
        .and_then(Value::as_map)
        .expect("user")
        .clone();
    let bytes_of = |v: Option<&Value>| v.and_then(Value::as_bytes).expect("bytes").clone();
    let string_of = |v: Option<&Value>| v.and_then(Value::as_text).expect("text").to_owned();
    ParsedAssertion {
        credential_id: bytes_of(map_text(&credential, "id")),
        auth_data: bytes_of(field(&map, 2)),
        signature: bytes_of(field(&map, 3)),
        user_id: bytes_of(map_text(&user, "id")),
        user_name: string_of(map_text(&user, "name")),
        user_display_name: string_of(map_text(&user, "displayName")),
        user_selected: field(&map, 6).and_then(Value::as_bool).unwrap_or(false),
        number_of_credentials: field(&map, 5)
            .and_then(Value::as_integer)
            .and_then(|i| u64::try_from(i128::from(i)).ok()),
    }
}

// ------------------------------------------------------------------ fakes

/// Accepts exactly [`GOOD_SIGNATURE`].
#[derive(Default)]
pub struct FakeVerifier {
    calls: AtomicUsize,
}

impl FakeVerifier {
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl OpVerifier for FakeVerifier {
    fn verify(&self, _message: &[u8], signature: &[u8]) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if signature == GOOD_SIGNATURE {
            Ok("fake".into())
        } else {
            Err("not the good signature".into())
        }
    }
}

/// Records the rows the handler adds to the credential cache.
#[derive(Default)]
pub struct FakeSink {
    rows: Mutex<Vec<CredentialRow>>,
}

impl FakeSink {
    pub fn rows(&self) -> Vec<CredentialRow> {
        self.rows.lock().unwrap().clone()
    }
}

impl CacheSink for FakeSink {
    fn credential_created(&self, row: &CredentialRow) {
        self.rows.lock().unwrap().push(row.clone());
    }
}

struct Stored {
    candidate: PasskeyCandidate,
    key: PasskeyKey,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    Create,
    Assert,
}

struct Inner {
    passkeys: Vec<Stored>,
    status: Result<VaultStatus, RelayError>,
    list_error: Option<RelayError>,
    create_error: Option<RelayError>,
    assert_error: Option<RelayError>,
    create_delay: Duration,
    block: Option<Block>,
    blocked: bool,
    panic_on_create: bool,
    creates: usize,
    create_attempts: usize,
    asserts: usize,
    assert_attempts: usize,
    withdrawn: usize,
    last_create: Option<PasskeyCreateRequest>,
    last_assert: Option<AssertCall>,
}

/// What the provider asked the app for in a sign-in (`Relay::assert_any`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssertCall {
    pub origin: String,
    pub rp_id: String,
    pub allow: Vec<Vec<u8>>,
    pub client_data_hash: Vec<u8>,
}

/// An in-memory stand-in for the app: it holds real ES256 passkeys, creates and signs like the engine
/// does (zero AAGUID, flags 0x5D / 0x1D, counter 0), and can be told to fail, wait or panic. It honours
/// the [`CancelToken`] the way the real relay does: a cancel ends a pending operation as
/// `Cancelled` and counts as a withdrawn prompt.
pub struct FakeRelay {
    inner: Mutex<Inner>,
    changed: Condvar,
}

impl Default for FakeRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeRelay {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                passkeys: Vec::new(),
                status: Ok(VaultStatus::Unlocked),
                list_error: None,
                create_error: None,
                assert_error: None,
                create_delay: Duration::ZERO,
                block: None,
                blocked: false,
                panic_on_create: false,
                creates: 0,
                create_attempts: 0,
                asserts: 0,
                assert_attempts: 0,
                withdrawn: 0,
                last_create: None,
                last_assert: None,
            }),
            changed: Condvar::new(),
        }
    }

    /// A vault that already holds these passkeys (each with a fresh key).
    pub fn with_passkeys(candidates: Vec<PasskeyCandidate>) -> Self {
        let relay = Self::new();
        relay.inner.lock().unwrap().passkeys = candidates
            .into_iter()
            .map(|candidate| Stored {
                candidate,
                key: PasskeyKey::generate().unwrap(),
            })
            .collect();
        relay
    }

    /// A relay whose `list_all` always fails with `error`.
    pub fn failing_list(error: RelayError) -> Self {
        let relay = Self::new();
        relay.inner.lock().unwrap().list_error = Some(error);
        relay
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap())
    }

    pub fn set_status(&self, status: Result<VaultStatus, RelayError>) {
        self.with(|i| i.status = status);
    }
    pub fn set_create_error(&self, error: Option<RelayError>) {
        self.with(|i| i.create_error = error);
    }
    pub fn set_assert_error(&self, error: Option<RelayError>) {
        self.with(|i| i.assert_error = error);
    }
    /// Every operation fails with `error` (the app is down).
    pub fn set_all_errors(&self, error: RelayError) {
        self.with(|i| {
            i.status = Err(error.clone());
            i.list_error = Some(error.clone());
            i.create_error = Some(error.clone());
            i.assert_error = Some(error);
        });
    }
    pub fn set_create_delay(&self, delay: Duration) {
        self.with(|i| i.create_delay = delay);
    }
    pub fn set_panic_on_create(&self, panic: bool) {
        self.with(|i| i.panic_on_create = panic);
    }
    /// The next `create` waits (like a Hello prompt) until its token is cancelled.
    pub fn block_create(&self) {
        self.with(|i| i.block = Some(Block::Create));
    }
    /// The next `assert_any` waits (like a Windows Hello prompt) until its token is cancelled.
    pub fn block_assert(&self) {
        self.with(|i| i.block = Some(Block::Assert));
    }
    /// Waits (5 s at most) until a blocked operation is waiting.
    pub fn wait_until_blocked(&self) {
        let guard = self.inner.lock().unwrap();
        let (guard, timeout) = self
            .changed
            .wait_timeout_while(guard, Duration::from_secs(5), |i| !i.blocked)
            .unwrap();
        assert!(!timeout.timed_out() && guard.blocked, "nothing blocked");
    }

    pub fn creates(&self) -> usize {
        self.with(|i| i.creates)
    }
    pub fn create_attempts(&self) -> usize {
        self.with(|i| i.create_attempts)
    }
    pub fn asserts(&self) -> usize {
        self.with(|i| i.asserts)
    }
    /// Sign-ins asked of the app, whether or not they succeeded.
    pub fn assert_attempts(&self) -> usize {
        self.with(|i| i.assert_attempts)
    }
    /// Prompts that were withdrawn because the operation was cancelled.
    pub fn withdrawn(&self) -> usize {
        self.with(|i| i.withdrawn)
    }
    pub fn last_create(&self) -> Option<PasskeyCreateRequest> {
        self.with(|i| i.last_create.clone())
    }
    pub fn last_assert(&self) -> Option<AssertCall> {
        self.with(|i| i.last_assert.clone())
    }

    /// Waits like a prompt: until `cancel` fires (then the prompt is withdrawn) or 30 s pass.
    fn hold(&self, which: Block, cancel: &CancelToken) -> Result<(), RelayError> {
        let blocking = self.with(|i| {
            let blocking = i.block == Some(which);
            if blocking {
                i.blocked = true;
                self.changed.notify_all();
            }
            blocking
        });
        if !blocking {
            return Ok(());
        }
        let started = Instant::now();
        loop {
            if cancel.wait_cancelled(Duration::from_millis(20)) {
                self.with(|i| {
                    i.withdrawn += 1;
                    i.blocked = false;
                    i.block = None;
                });
                return Err(RelayError::Cancelled);
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err(RelayError::Other("the test never cancelled".into()));
            }
        }
    }
}

impl Relay for FakeRelay {
    fn status(&self) -> Result<VaultStatus, RelayError> {
        self.with(|i| i.status.clone())
    }

    fn create(
        &self,
        cancel: &CancelToken,
        request: PasskeyCreateRequest,
    ) -> Result<PasskeyCreated, RelayError> {
        let (delay, panic_now) = self.with(|i| {
            i.create_attempts += 1;
            i.last_create = Some(request.clone());
            (i.create_delay, i.panic_on_create)
        });
        assert!(!panic_now, "FakeRelay was told to panic");
        self.hold(Block::Create, cancel)?;
        if !delay.is_zero() && cancel.wait_cancelled(delay) {
            return Err(RelayError::Cancelled);
        }
        self.with(|i| {
            if let Some(error) = &i.create_error {
                return Err(error.clone());
            }
            if i.passkeys.iter().any(|s| {
                s.candidate.rp_id == request.rp_id
                    && request.exclude.contains(&s.candidate.credential_id)
            }) {
                return Err(RelayError::Exists);
            }
            let key = PasskeyKey::generate().unwrap();
            let public = key.public();
            let credential_id = new_credential_id().unwrap();
            let auth_data = authenticator_data(
                &request.rp_id,
                FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS,
                0,
                Some((&credential_id, &public)),
            );
            let item_id = format!("item-{}", i.passkeys.len() + 1);
            i.passkeys.push(Stored {
                candidate: PasskeyCandidate {
                    item_id: item_id.clone(),
                    rp_id: request.rp_id.clone(),
                    rp_name: request.rp_name.clone(),
                    user_name: request.user_name.clone(),
                    user_display_name: request.user_display_name.clone(),
                    credential_id: credential_id.clone(),
                    user_handle: request.user_handle.clone(),
                    created_ms: 1,
                },
                key,
            });
            i.creates += 1;
            Ok(PasskeyCreated {
                item_id,
                credential_id,
                attestation_object: attestation_object_none(&auth_data),
                auth_data,
                public_key: public.to_cose(),
                public_key_spki: public.to_spki_der(),
                algorithm: -7,
            })
        })
    }

    fn assert_any(
        &self,
        cancel: &CancelToken,
        origin: &str,
        rp_id: &str,
        allow: &[Vec<u8>],
        client_data_hash: &[u8],
    ) -> Result<AssertedPasskey, RelayError> {
        self.with(|i| {
            i.assert_attempts += 1;
            i.last_assert = Some(AssertCall {
                origin: origin.to_owned(),
                rp_id: rp_id.to_owned(),
                allow: allow.to_vec(),
                client_data_hash: client_data_hash.to_vec(),
            });
        });
        self.hold(Block::Assert, cancel)?;
        if cancel.is_cancelled() {
            return Err(RelayError::Cancelled);
        }
        self.with(|i| {
            if let Some(error) = &i.assert_error {
                return Err(error.clone());
            }
            // Like the app: exactly one passkey for the site (and the allow list), else not found.
            let found: Vec<&Stored> = i
                .passkeys
                .iter()
                .filter(|s| {
                    s.candidate.rp_id == rp_id
                        && (allow.is_empty() || allow.contains(&s.candidate.credential_id))
                })
                .collect();
            let [stored] = found.as_slice() else {
                return Err(RelayError::NotFound);
            };
            let auth_data =
                authenticator_data(rp_id, FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS, 0, None);
            let signature = stored.key.assertion_signature(&auth_data, client_data_hash);
            let asserted = AssertedPasskey {
                assertion: PasskeyAssertion {
                    item_id: stored.candidate.item_id.clone(),
                    credential_id: stored.candidate.credential_id.clone(),
                    auth_data,
                    signature,
                    user_handle: stored.candidate.user_handle.clone(),
                },
                user_name: stored.candidate.user_name.clone(),
                user_display_name: stored.candidate.user_display_name.clone(),
            };
            i.asserts += 1;
            Ok(asserted)
        })
    }

    fn list_all(&self) -> Result<Vec<PasskeyCandidate>, RelayError> {
        self.with(|i| {
            if let Some(error) = &i.list_error {
                return Err(error.clone());
            }
            Ok(i.passkeys.iter().map(|s| s.candidate.clone()).collect())
        })
    }
}

/// An in-memory credential cache with the real one's behaviour for duplicates and missing rows
/// (`NTE_EXISTS`, `NTE_NOT_FOUND`), that can be told to fail.
#[derive(Default)]
pub struct MemoryCache {
    rows: Mutex<Vec<CredentialRow>>,
    calls: Mutex<Vec<&'static str>>,
    fail_read: bool,
    fail_add_for: Option<u8>,
}

impl MemoryCache {
    /// A cache that cannot be read (it is unavailable).
    pub fn failing_reads() -> Self {
        Self {
            fail_read: true,
            ..Self::default()
        }
    }
    /// A cache that refuses to add a row whose credential id starts with `first_byte`.
    pub fn failing_add_for(first_byte: u8) -> Self {
        Self {
            fail_add_for: Some(first_byte),
            ..Self::default()
        }
    }
    pub fn rows(&self) -> Vec<CredentialRow> {
        self.rows.lock().unwrap().clone()
    }
    pub fn push(&self, row: CredentialRow) {
        self.rows.lock().unwrap().push(row);
    }
    /// The operations called so far, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

fn same_row(a: &CredentialRow, b: &CredentialRow) -> bool {
    a.credential_id == b.credential_id && a.rp_id == b.rp_id
}

impl CacheApi for MemoryCache {
    fn get_all(&self) -> Result<Vec<CredentialRow>, HRESULT> {
        self.calls.lock().unwrap().push("get_all");
        if self.fail_read {
            return Err(E_FAIL);
        }
        Ok(self.rows())
    }
    fn add(&self, row: &CredentialRow) -> Result<(), HRESULT> {
        self.calls.lock().unwrap().push("add");
        if self.fail_add_for == row.credential_id.first().copied() {
            return Err(E_FAIL);
        }
        let mut rows = self.rows.lock().unwrap();
        if rows.iter().any(|r| same_row(r, row)) {
            return Err(NTE_EXISTS);
        }
        rows.push(row.clone());
        Ok(())
    }
    fn remove(&self, row: &CredentialRow) -> Result<(), HRESULT> {
        self.calls.lock().unwrap().push("remove");
        let mut rows = self.rows.lock().unwrap();
        let before = rows.len();
        rows.retain(|r| !same_row(r, row));
        if rows.len() == before {
            Err(NTE_NOT_FOUND)
        } else {
            Ok(())
        }
    }
}

/// A cache row for a passkey the vault does not hold.
pub fn stale_row() -> CredentialRow {
    CredentialRow {
        credential_id: vec![0xEE; 32],
        rp_id: "stale.example".into(),
        rp_name: "Stale".into(),
        user_id: vec![0xEE],
        user_name: "gone".into(),
        user_display_name: "gone".into(),
    }
}
