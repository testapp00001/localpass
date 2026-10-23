//! The heart of the provider: what happens for MakeCredential, GetAssertion, CancelOperation and
//! GetLockStatus, as plain functions over the request bytes. No COM is involved, so the whole flow is
//! tested with a fake app ([`Relay`]) and a fake request-signature check ([`OpVerifier`]); the COM layer
//! in [`crate::com`] only unpacks the OS structs and copies the answer into a CoTaskMem buffer.
//!
//! Order for every operation:
//!
//! 1. The request signature is verified against the OS operation-signing key BEFORE anything is relayed,
//!    so a process that is not Windows' WebAuthn client cannot reach the app's signer (`NTE_BAD_SIGNATURE`,
//!    and nothing is recorded).
//! 2. The transaction id is looked up in the process-wide in-flight map. Windows retries a failed call
//!    with the SAME transaction id (measured in the spike), and a retry or a concurrent duplicate must not
//!    create a second credential or show a second Hello prompt: later callers get the first call's
//!    result. Finished results are kept for [`RESULT_TTL`].
//! 3. Decode, translate, relay (one pipe connection per operation, dropped on cancel), validate the
//!    app's answer, encode with `WebAuthNEncode*Response`.
//!
//! The app's own Windows Hello check is the only user verification: this module never calls
//! `WebAuthNPluginPerformUserVerification` (design decision recorded in the notes).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use lp_core::passkey::PasskeyCandidate;
use lp_core::status::VaultStatus;
use windows::Win32::Foundation::{E_FAIL, S_OK};
use windows::core::{GUID, HRESULT};

use crate::log::{hr, short_id};
use crate::plog;
use crate::relay::{CancelToken, Relay, RelayError};
use crate::sys::{
    AssertionOutput, AssertionUser, CredentialRow, NTE_BAD_SIGNATURE, NTE_INVALID_PARAMETER,
    NTE_NOT_FOUND, PLUGIN_LOCKED, PLUGIN_UNLOCKED, REQUEST_TYPE_CTAP2_CBOR, WebAuthn,
};
use crate::translate::{
    GetRequest, MakeRequest, assert_origin, cache_row, check_assertion, create_request,
    created_parts, hresult_for,
};

/// How long a finished transaction's result is kept for repeated calls.
pub const RESULT_TTL: Duration = Duration::from_secs(60);

/// Checks that a request was signed by Windows' WebAuthn client.
pub trait OpVerifier: Send + Sync {
    /// Verifies `signature` over `message` (the encoded request). `Ok` says how (for the log).
    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<String, String>;
}

/// Where a freshly created credential is added to Windows' credential cache (needs package identity,
/// so the handler tests use a fake).
pub trait CacheSink: Send + Sync {
    /// Best effort: a failure must not fail the MakeCredential that already succeeded.
    fn credential_created(&self, row: &CredentialRow);
}

/// An OS operation request, unpacked from `WEBAUTHN_PLUGIN_OPERATION_REQUEST`.
#[derive(Debug, Clone, Copy)]
pub struct OpCall<'a> {
    pub transaction_id: GUID,
    /// The caller's top-level window (only logged).
    pub hwnd: usize,
    pub request_type: u32,
    pub encoded: &'a [u8],
    pub signature: &'a [u8],
}

/// The result of an operation: the HRESULT for the COM call and, on success, the encoded response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub hr: HRESULT,
    /// `WebAuthNEncode*Response` output (copied; the COM layer allocates a fresh CoTaskMem block for
    /// every call it answers, so a repeated transaction never hands the OS the same pointer twice).
    pub response: Vec<u8>,
}

impl Outcome {
    pub fn failure(hr: HRESULT) -> Self {
        Self {
            hr,
            response: Vec::new(),
        }
    }

    pub fn success(response: Vec<u8>) -> Self {
        Self { hr: S_OK, response }
    }

    fn from_error(error: &RelayError) -> Self {
        Self::failure(hresult_for(error))
    }
}

struct Flight {
    /// The encoded request, kept to check a later cancel's signature against.
    request: Vec<u8>,
    token: CancelToken,
    finished: Mutex<Option<(Outcome, Instant)>>,
    done: Condvar,
}

impl Flight {
    fn new(request: Vec<u8>) -> Self {
        Self {
            request,
            token: CancelToken::new(),
            finished: Mutex::new(None),
            done: Condvar::new(),
        }
    }

    fn finish(&self, outcome: Outcome) {
        let mut slot = self.finished.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some((outcome, Instant::now()));
        self.done.notify_all();
    }

    fn wait(&self) -> Outcome {
        let slot = self.finished.lock().unwrap_or_else(|e| e.into_inner());
        let slot = self
            .done
            .wait_while(slot, |finished| finished.is_none())
            .unwrap_or_else(|e| e.into_inner());
        slot.as_ref()
            .map(|(o, _)| o.clone())
            .expect("woken when set")
    }

    fn is_finished(&self) -> bool {
        self.finished
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }

    /// Still running, or finished less than [`RESULT_TTL`] ago.
    fn is_live(&self, now: Instant) -> bool {
        match &*self.finished.lock().unwrap_or_else(|e| e.into_inner()) {
            None => true,
            Some((_, at)) => now.duration_since(*at) < RESULT_TTL,
        }
    }
}

pub struct Handler {
    api: &'static WebAuthn,
    relay: Arc<dyn Relay>,
    verifier: Arc<dyn OpVerifier>,
    sink: Arc<dyn CacheSink>,
    flights: Mutex<HashMap<u128, Arc<Flight>>>,
}

impl Handler {
    pub fn new(
        api: &'static WebAuthn,
        relay: Arc<dyn Relay>,
        verifier: Arc<dyn OpVerifier>,
        sink: Arc<dyn CacheSink>,
    ) -> Self {
        Self {
            api,
            relay,
            verifier,
            sink,
            flights: Mutex::new(HashMap::new()),
        }
    }

    /// Operations that have started and not finished (the idle exit waits for zero).
    pub fn in_flight(&self) -> usize {
        self.flights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|f| !f.is_finished())
            .count()
    }

    pub fn make_credential(&self, call: &OpCall<'_>) -> Outcome {
        plog!(
            "MakeCredential tx={} hWnd=0x{:x} cbReq={} cbSig={}",
            tx_text(&call.transaction_id),
            call.hwnd,
            call.encoded.len(),
            call.signature.len()
        );
        if let Some(refused) = self.precheck(call) {
            return refused;
        }
        self.once(call, |token| self.do_make_credential(call.encoded, token))
    }

    pub fn get_assertion(&self, call: &OpCall<'_>) -> Outcome {
        plog!(
            "GetAssertion tx={} hWnd=0x{:x} cbReq={} cbSig={}",
            tx_text(&call.transaction_id),
            call.hwnd,
            call.encoded.len(),
            call.signature.len()
        );
        if let Some(refused) = self.precheck(call) {
            return refused;
        }
        self.once(call, |token| self.do_get_assertion(call.encoded, token))
    }

    /// `CancelOperation`: withdraws the pending prompt of a known transaction (the pending call then
    /// returns `NTE_USER_CANCELLED`). The signature is checked against the stored original request and
    /// the result is logged, but a cancel for a known transaction is honoured either way: cancelling is
    /// not destructive, and the exact signed bytes of a cancel are not documented.
    pub fn cancel(&self, transaction_id: &GUID, signature: &[u8]) -> HRESULT {
        let flight = self
            .flights
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&transaction_id.to_u128())
            .cloned();
        let Some(flight) = flight else {
            plog!(
                "CancelOperation tx={}: unknown transaction -> NTE_NOT_FOUND",
                tx_text(transaction_id)
            );
            return NTE_NOT_FOUND;
        };
        let over_request = self.verifier.verify(&flight.request, signature).is_ok();
        let over_guid = !over_request
            && self
                .verifier
                .verify(&guid_bytes(transaction_id), signature)
                .is_ok();
        let covers = match (over_request, over_guid) {
            (true, _) => "the original request bytes",
            (false, true) => "the transaction id",
            (false, false) => "neither the request bytes nor the transaction id (honoured anyway)",
        };
        if flight.is_finished() {
            plog!(
                "CancelOperation tx={}: already finished; signature covers {covers}",
                tx_text(transaction_id)
            );
            return S_OK;
        }
        plog!(
            "CancelOperation tx={}: cancelling; signature covers {covers}",
            tx_text(transaction_id)
        );
        flight.token.cancel();
        S_OK
    }

    /// `GetLockStatus`: unlocked only when the app answers `unlocked`. Never prompts, never starts the app.
    pub fn lock_status(&self) -> i32 {
        match self.relay.status() {
            Ok(VaultStatus::Unlocked) => PLUGIN_UNLOCKED,
            Ok(_) | Err(_) => PLUGIN_LOCKED,
        }
    }

    // ---------------------------------------------------------------- shared steps

    /// Request type and signature. `Some(outcome)` = refuse; nothing was recorded or relayed.
    fn precheck(&self, call: &OpCall<'_>) -> Option<Outcome> {
        if call.request_type != REQUEST_TYPE_CTAP2_CBOR || call.encoded.is_empty() {
            plog!(
                "refusing request type {} with {} bytes",
                call.request_type,
                call.encoded.len()
            );
            return Some(Outcome::failure(NTE_INVALID_PARAMETER));
        }
        match self.verifier.verify(call.encoded, call.signature) {
            Ok(how) => {
                plog!("request signature ok ({how})");
                None
            }
            Err(why) => {
                plog!("request signature FAILED ({why}): refusing, nothing relayed");
                Some(Outcome::failure(NTE_BAD_SIGNATURE))
            }
        }
    }

    /// Runs `work` once per transaction id: the first caller does the work, a repeated or concurrent
    /// caller with the same id waits for it and gets a copy of its outcome.
    fn once(&self, call: &OpCall<'_>, work: impl FnOnce(&CancelToken) -> Outcome) -> Outcome {
        let key = call.transaction_id.to_u128();
        let (flight, owner) = {
            let mut flights = self.flights.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            flights.retain(|_, f| f.is_live(now));
            match flights.get(&key) {
                Some(existing) => (existing.clone(), false),
                None => {
                    let flight = Arc::new(Flight::new(call.encoded.to_vec()));
                    flights.insert(key, flight.clone());
                    (flight, true)
                }
            }
        };
        // The map lock is released: the work below can take minutes (Windows Hello).
        if !owner {
            plog!(
                "tx={} was seen before: returning the first call's result",
                tx_text(&call.transaction_id)
            );
            return flight.wait();
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| work(&flight.token))).unwrap_or_else(|_| {
            plog!("the operation panicked");
            Outcome::failure(E_FAIL)
        });
        flight.finish(outcome.clone());
        plog!(
            "tx={} finished hr={} responseBytes={}",
            tx_text(&call.transaction_id),
            hr(outcome.hr),
            outcome.response.len()
        );
        outcome
    }

    // ---------------------------------------------------------------- MakeCredential

    fn do_make_credential(&self, encoded: &[u8], token: &CancelToken) -> Outcome {
        let request = match self.api.decode_make_credential(encoded) {
            Ok(decoded) => match MakeRequest::from_decoded(&decoded) {
                Ok(request) => request,
                Err(e) => return fail(&e),
            },
            Err(code) => {
                plog!("WebAuthNDecodeMakeCredentialRequest failed hr={}", hr(code));
                return Outcome::failure(NTE_INVALID_PARAMETER);
            }
        };
        plog!(
            "MakeCredential rp={} algs={:?} exclude={} userIdBytes={}",
            request.rp_id,
            request.algorithms,
            request.exclude.len(),
            request.user_id.len()
        );
        let create = match create_request(&request) {
            Ok(create) => create,
            Err(e) => return fail(&e),
        };
        if token.is_cancelled() {
            return fail(&RelayError::Cancelled);
        }
        let created = match self.relay.create(token, create) {
            Ok(created) => created,
            Err(e) => return fail(&e),
        };
        let parts = match created_parts(&request.rp_id, &created) {
            Ok(parts) => parts,
            Err(e) => return fail(&e),
        };
        plog!(
            "the app created credential {}",
            short_id(&parts.credential_id)
        );
        // Windows only offers this credential for sign-in once it is in the cache.
        self.sink.credential_created(&cache_row(&PasskeyCandidate {
            item_id: created.item_id.clone(),
            rp_id: request.rp_id.clone(),
            rp_name: request.rp_name.clone(),
            user_name: request.user_name.clone(),
            user_display_name: request.user_display_name.clone(),
            credential_id: parts.credential_id.clone(),
            user_handle: request.user_id.clone(),
            created_ms: 0,
        }));
        match self
            .api
            .encode_make_credential_none(&parts.auth_data, &parts.credential_id)
        {
            Ok(buffer) => Outcome::success(buffer.as_slice().to_vec()),
            Err(code) => {
                plog!(
                    "WebAuthNEncodeMakeCredentialResponse failed hr={}",
                    hr(code)
                );
                Outcome::failure(E_FAIL)
            }
        }
    }

    // ---------------------------------------------------------------- GetAssertion

    fn do_get_assertion(&self, encoded: &[u8], token: &CancelToken) -> Outcome {
        let request = match self.api.decode_get_assertion(encoded) {
            Ok(decoded) => GetRequest::from_decoded(&decoded),
            Err(code) => {
                plog!("WebAuthNDecodeGetAssertionRequest failed hr={}", hr(code));
                return Outcome::failure(NTE_INVALID_PARAMETER);
            }
        };
        plog!(
            "GetAssertion rp={} allow={}",
            request.rp_id,
            request.allow.len()
        );
        let origin = match assert_origin(&request) {
            Ok(origin) => origin,
            Err(e) => return fail(&e),
        };
        if token.is_cancelled() {
            return fail(&RelayError::Cancelled);
        }
        // One request to the app: it unlocks the vault if needed (that is the user check) or asks for
        // Windows Hello, picks the one passkey that matches, and signs.
        let asserted = match self.relay.assert_any(
            token,
            &origin,
            &request.rp_id,
            &request.allow,
            &request.client_data_hash,
        ) {
            Ok(asserted) => asserted,
            Err(e) => return fail(&e),
        };
        if let Err(e) = check_assertion(&request, &asserted) {
            return fail(&e);
        }
        plog!(
            "signed with credential {}",
            short_id(&asserted.assertion.credential_id)
        );
        let assertion = &asserted.assertion;
        let display_name = if asserted.user_display_name.is_empty() {
            &asserted.user_name
        } else {
            &asserted.user_display_name
        };
        let output = AssertionOutput {
            auth_data: &assertion.auth_data,
            signature: &assertion.signature,
            credential_id: &assertion.credential_id,
            user: Some(AssertionUser {
                id: &assertion.user_handle,
                name: &asserted.user_name,
                display_name,
            }),
            number_of_credentials: 1,
            user_selected: true,
        };
        match self.api.encode_get_assertion(&output) {
            Ok(buffer) => Outcome::success(buffer.as_slice().to_vec()),
            Err(code) => {
                plog!("WebAuthNEncodeGetAssertionResponse failed hr={}", hr(code));
                Outcome::failure(E_FAIL)
            }
        }
    }
}

fn fail(error: &RelayError) -> Outcome {
    let outcome = Outcome::from_error(error);
    plog!("operation failed: {error} -> hr={}", hr(outcome.hr));
    outcome
}

/// The 16 bytes of a GUID as they lie in memory (the order a signature over the struct would cover).
fn guid_bytes(guid: &GUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    out[..4].copy_from_slice(&guid.data1.to_le_bytes());
    out[4..6].copy_from_slice(&guid.data2.to_le_bytes());
    out[6..8].copy_from_slice(&guid.data3.to_le_bytes());
    out[8..].copy_from_slice(&guid.data4);
    out
}

fn tx_text(tx: &GUID) -> String {
    format!("{tx:?}")
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use super::*;
    use crate::sys::{NTE_DEVICE_NOT_FOUND, NTE_EXISTS, NTE_NOT_SUPPORTED, NTE_USER_CANCELLED};
    use crate::testing::{
        self, FakeRelay, FakeSink, FakeVerifier, GOOD_SIGNATURE, WEBAUTHN_IO_MAKE_CREDENTIAL,
    };
    use lp_core::passkey::{PasskeyPublic, parse_authenticator_data, rp_id_hash};

    struct Rig {
        handler: Arc<Handler>,
        relay: Arc<FakeRelay>,
        sink: Arc<FakeSink>,
        verifier: Arc<FakeVerifier>,
    }

    fn rig() -> Rig {
        let relay = Arc::new(FakeRelay::new());
        let sink = Arc::new(FakeSink::default());
        let verifier = Arc::new(FakeVerifier::default());
        let handler = Arc::new(Handler::new(
            testing::api(),
            relay.clone(),
            verifier.clone(),
            sink.clone(),
        ));
        Rig {
            handler,
            relay,
            sink,
            verifier,
        }
    }

    fn call<'a>(tx: u128, encoded: &'a [u8], signature: &'a [u8]) -> OpCall<'a> {
        OpCall {
            transaction_id: GUID::from_u128(tx),
            hwnd: 0x508ca,
            request_type: REQUEST_TYPE_CTAP2_CBOR,
            encoded,
            signature,
        }
    }

    fn captured() -> Vec<u8> {
        testing::unhex(WEBAUTHN_IO_MAKE_CREDENTIAL)
    }

    fn create_one(rig: &Rig, rp: &str, user: &str, tx: u128) -> (Vec<u8>, PasskeyPublic) {
        let request = testing::make_credential_request(rp, rp, user.as_bytes(), user, &[-7], &[]);
        let outcome = rig
            .handler
            .make_credential(&call(tx, &request, GOOD_SIGNATURE));
        assert_eq!(outcome.hr, S_OK, "{outcome:?}");
        let credential_id = testing::attestation_credential_id(&outcome.response);
        let auth = testing::attestation_auth_data(&outcome.response);
        let parsed = parse_authenticator_data(&auth).unwrap();
        let (id, public) = parsed.credential.expect("attested credential data");
        assert_eq!(id, credential_id);
        (credential_id, public)
    }

    #[test]
    fn make_credential_relays_and_encodes_the_captured_request() {
        let rig = rig();
        let encoded = captured();
        let outcome = rig
            .handler
            .make_credential(&call(1, &encoded, GOOD_SIGNATURE));
        assert_eq!(outcome.hr, S_OK);

        // What was sent to the app.
        let sent = rig.relay.last_create().expect("relayed");
        assert_eq!(sent.origin, "https://webauthn.io");
        assert_eq!(sent.rp_id, "webauthn.io");
        assert_eq!(sent.user_handle, b"webauthnio-test0101");
        assert_eq!(sent.user_name, "test0101");
        assert_eq!(sent.algorithms, vec![-7]);
        assert!(!sent.user_verified);

        // What came back: `none` attestation with the app's authenticator data.
        let auth = testing::attestation_auth_data(&outcome.response);
        let parsed = parse_authenticator_data(&auth).unwrap();
        assert_eq!(parsed.rp_id_hash, rp_id_hash("webauthn.io"));
        assert_eq!(parsed.flags, 0x5D);
        assert_eq!(parsed.sign_count, 0);
        assert_eq!(&auth[37..53], &[0u8; 16], "zero AAGUID");
        assert_eq!(testing::attestation_format(&outcome.response), "none");

        // The credential was pushed to the cache, one row.
        let rows = rig.sink.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].rp_id, "webauthn.io");
        assert_eq!(rows[0].user_name, "test0101");
        assert_eq!(rows[0].user_id, b"webauthnio-test0101");
        assert_eq!(rig.handler.in_flight(), 0);
    }

    #[test]
    fn a_repeated_transaction_returns_the_first_result_without_a_second_credential() {
        let rig = rig();
        let encoded = captured();
        let first = rig
            .handler
            .make_credential(&call(7, &encoded, GOOD_SIGNATURE));
        let second = rig
            .handler
            .make_credential(&call(7, &encoded, GOOD_SIGNATURE));
        assert_eq!(first, second);
        assert_eq!(rig.relay.creates(), 1, "one credential, not two");
        assert_eq!(rig.sink.rows().len(), 1);
        // A new transaction is a new credential.
        let third = rig
            .handler
            .make_credential(&call(8, &encoded, GOOD_SIGNATURE));
        assert_eq!(third.hr, S_OK);
        assert_eq!(rig.relay.creates(), 2);
    }

    #[test]
    fn a_failed_first_call_is_repeated_as_the_same_failure_without_a_new_prompt() {
        // The spike saw Windows call again with the same tx 3.4 s and 9 s after a cancel: those
        // retries must not show Hello again.
        let rig = rig();
        rig.relay.set_create_error(Some(RelayError::Cancelled));
        let encoded = captured();
        for _ in 0..3 {
            let outcome = rig
                .handler
                .make_credential(&call(9, &encoded, GOOD_SIGNATURE));
            assert_eq!(outcome, Outcome::failure(NTE_USER_CANCELLED));
        }
        assert_eq!(rig.relay.create_attempts(), 1);
        // The same user, a new transaction: asked again.
        rig.relay.set_create_error(None);
        assert_eq!(
            rig.handler
                .make_credential(&call(10, &encoded, GOOD_SIGNATURE))
                .hr,
            S_OK
        );
    }

    #[test]
    fn concurrent_duplicates_share_one_credential() {
        let rig = rig();
        rig.relay.set_create_delay(Duration::from_millis(300));
        let encoded = Arc::new(captured());
        let finished = Arc::new(AtomicUsize::new(0));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (handler, encoded, finished) =
                    (rig.handler.clone(), encoded.clone(), finished.clone());
                thread::spawn(move || {
                    let outcome = handler.make_credential(&call(11, &encoded, GOOD_SIGNATURE));
                    finished.fetch_add(1, Ordering::SeqCst);
                    outcome
                })
            })
            .collect();
        let outcomes: Vec<Outcome> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(finished.load(Ordering::SeqCst), 4);
        assert!(outcomes.iter().all(|o| o.hr == S_OK && *o == outcomes[0]));
        assert_eq!(rig.relay.creates(), 1);
        assert_eq!(rig.sink.rows().len(), 1);
    }

    #[test]
    fn finished_results_expire() {
        let rig = rig();
        let encoded = captured();
        let first = rig
            .handler
            .make_credential(&call(21, &encoded, GOOD_SIGNATURE));
        assert_eq!(first.hr, S_OK);
        // Pretend the result is older than the TTL.
        {
            let flights = rig.handler.flights.lock().unwrap();
            let flight = flights.get(&21).unwrap();
            let mut slot = flight.finished.lock().unwrap();
            let (outcome, _) = slot.take().unwrap();
            *slot = Some((
                outcome,
                Instant::now() - RESULT_TTL - Duration::from_secs(1),
            ));
        }
        let again = rig
            .handler
            .make_credential(&call(21, &encoded, GOOD_SIGNATURE));
        assert_eq!(again.hr, S_OK);
        assert_eq!(
            rig.relay.creates(),
            2,
            "an old transaction id is a new operation"
        );
    }

    #[test]
    fn a_bad_signature_is_refused_before_anything_is_relayed_or_recorded() {
        let rig = rig();
        let encoded = captured();
        let outcome = rig.handler.make_credential(&call(30, &encoded, b"forged"));
        assert_eq!(outcome, Outcome::failure(NTE_BAD_SIGNATURE));
        assert_eq!(rig.relay.create_attempts(), 0);
        assert_eq!(rig.relay.assert_attempts(), 0);
        assert!(
            rig.handler.flights.lock().unwrap().is_empty(),
            "nothing recorded"
        );

        let get = testing::get_assertion_request("example.com", &[1; 32], &[]);
        assert_eq!(
            rig.handler.get_assertion(&call(31, &get, b"")),
            Outcome::failure(NTE_BAD_SIGNATURE)
        );
        assert_eq!(rig.relay.assert_attempts(), 0);

        // The forged call did not poison the transaction: the real one still works.
        assert_eq!(
            rig.handler
                .make_credential(&call(30, &encoded, GOOD_SIGNATURE))
                .hr,
            S_OK
        );
        // A signature over other bytes fails too.
        let other = testing::make_credential_request("example.com", "Ex", b"u", "u", &[-7], &[]);
        assert_eq!(
            rig.handler
                .make_credential(&call(32, &other, b"signature-of-another-request"))
                .hr,
            NTE_BAD_SIGNATURE
        );
    }

    #[test]
    fn requests_of_another_type_or_without_a_body_are_invalid() {
        let rig = rig();
        let encoded = captured();
        let wrong_type = OpCall {
            request_type: 2,
            ..call(40, &encoded, GOOD_SIGNATURE)
        };
        assert_eq!(
            rig.handler.make_credential(&wrong_type).hr,
            NTE_INVALID_PARAMETER
        );
        assert_eq!(
            rig.handler
                .make_credential(&call(41, &[], GOOD_SIGNATURE))
                .hr,
            NTE_INVALID_PARAMETER
        );
        // Undecodable CBOR with a valid signature.
        assert_eq!(
            rig.handler
                .make_credential(&call(42, &[0xFF; 9], GOOD_SIGNATURE))
                .hr,
            NTE_INVALID_PARAMETER
        );
        assert_eq!(
            rig.handler
                .get_assertion(&call(43, &[0xFF; 9], GOOD_SIGNATURE))
                .hr,
            NTE_INVALID_PARAMETER
        );
        assert_eq!(rig.relay.create_attempts(), 0);
    }

    #[test]
    fn make_credential_errors_map_to_the_table() {
        let rig = rig();
        // No ES256 offered: refused before the app is asked.
        let rsa_only =
            testing::make_credential_request("example.com", "Ex", b"u1", "u", &[-257], &[]);
        assert_eq!(
            rig.handler
                .make_credential(&call(50, &rsa_only, GOOD_SIGNATURE))
                .hr,
            NTE_NOT_SUPPORTED
        );
        assert_eq!(rig.relay.create_attempts(), 0);

        let request = testing::make_credential_request("example.com", "Ex", b"u1", "u", &[-7], &[]);
        for (error, expected) in [
            (RelayError::Cancelled, NTE_USER_CANCELLED),
            (RelayError::NotPaired, NTE_DEVICE_NOT_FOUND),
            (RelayError::Unreachable("down".into()), NTE_DEVICE_NOT_FOUND),
            (RelayError::Other("boom".into()), E_FAIL),
            (RelayError::Exists, NTE_EXISTS),
        ] {
            rig.relay.set_create_error(Some(error.clone()));
            let tx = 60 + rig.relay.create_attempts() as u128;
            assert_eq!(
                rig.handler
                    .make_credential(&call(tx, &request, GOOD_SIGNATURE))
                    .hr,
                expected,
                "{error:?}"
            );
        }
        assert!(
            rig.sink.rows().is_empty(),
            "failed creations are not cached"
        );
    }

    #[test]
    fn an_exclude_list_hit_is_nte_exists() {
        let rig = rig();
        let (credential_id, _) = create_one(&rig, "example.com", "alice", 70);
        // Same RP, the exclude list names the credential we hold.
        let request = testing::make_credential_request(
            "example.com",
            "Ex",
            b"alice",
            "alice",
            &[-7],
            std::slice::from_ref(&credential_id),
        );
        let outcome = rig
            .handler
            .make_credential(&call(71, &request, GOOD_SIGNATURE));
        assert_eq!(outcome, Outcome::failure(NTE_EXISTS));
        assert_eq!(rig.relay.creates(), 1);
        // The exclude list was passed to the app.
        assert_eq!(
            rig.relay.last_create().unwrap().exclude,
            vec![credential_id]
        );
        // An exclude list naming other credentials does not block.
        let other = testing::make_credential_request(
            "example.com",
            "Ex",
            b"bob",
            "bob",
            &[-7],
            &[vec![9; 16]],
        );
        assert_eq!(
            rig.handler
                .make_credential(&call(72, &other, GOOD_SIGNATURE))
                .hr,
            S_OK
        );
    }

    #[test]
    fn a_cancel_withdraws_a_pending_prompt() {
        let rig = rig();
        rig.relay.block_create();
        let encoded = Arc::new(captured());
        let pending = {
            let (handler, encoded) = (rig.handler.clone(), encoded.clone());
            thread::spawn(move || handler.make_credential(&call(80, &encoded, GOOD_SIGNATURE)))
        };
        rig.relay.wait_until_blocked();
        assert_eq!(rig.handler.in_flight(), 1);

        // A repeated call for the same tx waits for the same result.
        let duplicate = {
            let (handler, encoded) = (rig.handler.clone(), encoded.clone());
            thread::spawn(move || handler.make_credential(&call(80, &encoded, GOOD_SIGNATURE)))
        };

        // Windows cancels: S_OK, and the pending call ends as "cancelled".
        let tx = GUID::from_u128(80);
        assert_eq!(rig.handler.cancel(&tx, GOOD_SIGNATURE), S_OK);
        assert_eq!(
            pending.join().unwrap(),
            Outcome::failure(NTE_USER_CANCELLED)
        );
        assert_eq!(
            duplicate.join().unwrap(),
            Outcome::failure(NTE_USER_CANCELLED)
        );
        assert_eq!(
            rig.relay.withdrawn(),
            1,
            "the prompt was withdrawn exactly once"
        );
        assert_eq!(rig.relay.creates(), 0);
        assert!(rig.sink.rows().is_empty());
        assert_eq!(rig.handler.in_flight(), 0);

        // A cancel of a finished transaction is harmless.
        assert_eq!(rig.handler.cancel(&tx, GOOD_SIGNATURE), S_OK);
    }

    #[test]
    fn cancel_is_honoured_even_if_the_signature_is_unverifiable_and_unknown_ids_are_not_found() {
        let rig = rig();
        rig.relay.block_create();
        let encoded = Arc::new(captured());
        let pending = {
            let (handler, encoded) = (rig.handler.clone(), encoded.clone());
            thread::spawn(move || handler.make_credential(&call(90, &encoded, GOOD_SIGNATURE)))
        };
        rig.relay.wait_until_blocked();
        // Unknown transaction.
        assert_eq!(
            rig.handler.cancel(&GUID::from_u128(12345), GOOD_SIGNATURE),
            NTE_NOT_FOUND
        );
        // A signature that does not verify still cancels a known one.
        let verified_before = rig.verifier.calls();
        assert_eq!(rig.handler.cancel(&GUID::from_u128(90), b"nonsense"), S_OK);
        assert!(
            rig.verifier.calls() > verified_before,
            "the signature was checked and logged"
        );
        assert_eq!(pending.join().unwrap().hr, NTE_USER_CANCELLED);
    }

    #[test]
    fn get_assertion_signs_for_the_one_matching_credential() {
        let rig = rig();
        let (credential_id, public) = create_one(&rig, "example.com", "alice", 100);
        create_one(&rig, "other.example", "zed", 101);

        let hash = [0x42u8; 32];
        let request = testing::get_assertion_request(
            "example.com",
            &hash,
            std::slice::from_ref(&credential_id),
        );
        let outcome = rig
            .handler
            .get_assertion(&call(102, &request, GOOD_SIGNATURE));
        assert_eq!(outcome.hr, S_OK, "{outcome:?}");

        let assertion = testing::parse_assertion(&outcome.response);
        assert_eq!(assertion.credential_id, credential_id);
        assert_eq!(assertion.user_id, b"alice");
        assert_eq!(assertion.user_name, "alice");
        let parsed = parse_authenticator_data(&assertion.auth_data).unwrap();
        assert_eq!(parsed.rp_id_hash, rp_id_hash("example.com"));
        assert_eq!(parsed.flags, 0x1D);
        let mut signed = assertion.auth_data.clone();
        signed.extend_from_slice(&hash);
        public
            .verify(&signed, &assertion.signature)
            .expect("the assertion verifies");
        assert!(assertion.user_selected);

        // ONE request went to the app, with the site, the allow list and the 32-byte hash; the user
        // check is the app's (it sends no "verified" flag).
        assert_eq!(rig.relay.assert_attempts(), 1);
        let sent = rig.relay.last_assert().unwrap();
        assert_eq!(sent.origin, "https://example.com");
        assert_eq!(sent.rp_id, "example.com");
        assert_eq!(sent.allow, vec![credential_id]);
        assert_eq!(sent.client_data_hash, hash);
    }

    #[test]
    fn get_assertion_without_an_allow_list_uses_the_only_credential() {
        let rig = rig();
        let (credential_id, _) = create_one(&rig, "example.com", "alice", 110);
        let request = testing::get_assertion_request("example.com", &[1; 32], &[]);
        let outcome = rig
            .handler
            .get_assertion(&call(111, &request, GOOD_SIGNATURE));
        assert_eq!(outcome.hr, S_OK);
        assert_eq!(
            testing::parse_assertion(&outcome.response).credential_id,
            credential_id
        );
    }

    #[test]
    fn zero_or_several_matching_passkeys_are_not_found() {
        let rig = rig();
        let request = testing::get_assertion_request("example.com", &[1; 32], &[]);
        // None.
        assert_eq!(
            rig.handler
                .get_assertion(&call(120, &request, GOOD_SIGNATURE)),
            Outcome::failure(NTE_NOT_FOUND)
        );
        // Two accounts at the same site and no allow list: no picker, so not found (known limit).
        let (first, _) = create_one(&rig, "example.com", "alice", 121);
        let (second, _) = create_one(&rig, "example.com", "bob", 122);
        assert_eq!(
            rig.handler
                .get_assertion(&call(123, &request, GOOD_SIGNATURE))
                .hr,
            NTE_NOT_FOUND
        );
        assert_eq!(rig.relay.asserts(), 0, "nothing was signed");
        // An allow list naming both: still ambiguous.
        let both = testing::get_assertion_request(
            "example.com",
            &[1; 32],
            &[first.clone(), second.clone()],
        );
        assert_eq!(
            rig.handler
                .get_assertion(&call(124, &both, GOOD_SIGNATURE))
                .hr,
            NTE_NOT_FOUND
        );
        // An allow list naming one is fine, and one naming nobody we hold is not found.
        let narrowed = testing::get_assertion_request("example.com", &[1; 32], &[second]);
        assert_eq!(
            rig.handler
                .get_assertion(&call(125, &narrowed, GOOD_SIGNATURE))
                .hr,
            S_OK
        );
        let stranger = testing::get_assertion_request("example.com", &[1; 32], &[vec![0xEE; 32]]);
        assert_eq!(
            rig.handler
                .get_assertion(&call(126, &stranger, GOOD_SIGNATURE))
                .hr,
            NTE_NOT_FOUND
        );
        // A hash that is not 32 bytes is invalid, and the app is not asked.
        let (_solo, _) = create_one(&rig, "solo.example", "solo", 127);
        let attempts = rig.relay.assert_attempts();
        let short = testing::get_assertion_request("solo.example", &[1; 20], &[]);
        assert_eq!(
            rig.handler
                .get_assertion(&call(128, &short, GOOD_SIGNATURE))
                .hr,
            NTE_INVALID_PARAMETER
        );
        assert_eq!(rig.relay.assert_attempts(), attempts);
    }

    #[test]
    fn get_assertion_errors_map_to_the_table() {
        let rig = rig();
        create_one(&rig, "example.com", "alice", 130);
        let request = testing::get_assertion_request("example.com", &[1; 32], &[]);
        let mut tx = 131;
        for (error, expected) in [
            (RelayError::Cancelled, NTE_USER_CANCELLED),
            (RelayError::NotPaired, NTE_DEVICE_NOT_FOUND),
            (RelayError::Unreachable("down".into()), NTE_DEVICE_NOT_FOUND),
            (RelayError::NotFound, NTE_NOT_FOUND),
            (RelayError::Other("boom".into()), E_FAIL),
        ] {
            // The sign-in fails (vault locked and not unlocked, Hello denied, no match, app gone).
            rig.relay.set_assert_error(Some(error.clone()));
            assert_eq!(
                rig.handler
                    .get_assertion(&call(tx, &request, GOOD_SIGNATURE))
                    .hr,
                expected
            );
            tx += 1;
            rig.relay.set_assert_error(None);
        }
        assert_eq!(
            rig.handler
                .get_assertion(&call(tx, &request, GOOD_SIGNATURE))
                .hr,
            S_OK
        );
    }

    #[test]
    fn an_app_that_is_not_running_fails_fast_with_device_not_found() {
        let rig = rig();
        rig.relay
            .set_all_errors(RelayError::Unreachable("LocalPass is not running".into()));
        let started = Instant::now();
        let request = testing::get_assertion_request("example.com", &[1; 32], &[]);
        assert_eq!(
            rig.handler
                .get_assertion(&call(140, &request, GOOD_SIGNATURE))
                .hr,
            NTE_DEVICE_NOT_FOUND
        );
        let make = captured();
        assert_eq!(
            rig.handler
                .make_credential(&call(141, &make, GOOD_SIGNATURE))
                .hr,
            NTE_DEVICE_NOT_FOUND
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_cancel_while_the_app_asks_the_user_withdraws_the_prompt_and_signs_nothing() {
        let rig = rig();
        create_one(&rig, "example.com", "alice", 150);
        rig.relay.block_assert();
        let request = Arc::new(testing::get_assertion_request("example.com", &[1; 32], &[]));
        let pending = {
            let (handler, request) = (rig.handler.clone(), request.clone());
            thread::spawn(move || handler.get_assertion(&call(151, &request, GOOD_SIGNATURE)))
        };
        rig.relay.wait_until_blocked();
        assert_eq!(
            rig.handler.cancel(&GUID::from_u128(151), GOOD_SIGNATURE),
            S_OK
        );
        assert_eq!(pending.join().unwrap().hr, NTE_USER_CANCELLED);
        assert_eq!(rig.relay.asserts(), 0);
        assert_eq!(rig.relay.withdrawn(), 1, "the app's prompt was withdrawn");
    }

    #[test]
    fn guids_are_laid_out_like_the_struct_in_memory() {
        let guid = GUID::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        assert_eq!(
            guid_bytes(&guid),
            [
                0x67, 0x45, 0x23, 0x01, 0xab, 0x89, 0xef, 0xcd, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55,
                0x66, 0x77
            ]
        );
    }

    #[test]
    fn lock_status_is_unlocked_only_when_the_app_says_so() {
        let rig = rig();
        rig.relay.set_status(Ok(VaultStatus::Unlocked));
        assert_eq!(rig.handler.lock_status(), PLUGIN_UNLOCKED);
        for status in [
            Ok(VaultStatus::Locked),
            Ok(VaultStatus::NoVault),
            Err(RelayError::Unreachable("down".into())),
            Err(RelayError::NotPaired),
        ] {
            rig.relay.set_status(status);
            assert_eq!(rig.handler.lock_status(), PLUGIN_LOCKED);
        }
        assert_eq!(
            rig.relay.assert_attempts() + rig.relay.create_attempts(),
            0,
            "status only"
        );
    }

    #[test]
    fn a_panic_in_the_work_becomes_e_fail_and_is_remembered() {
        let rig = rig();
        rig.relay.set_panic_on_create(true);
        let encoded = captured();
        let first = rig
            .handler
            .make_credential(&call(160, &encoded, GOOD_SIGNATURE));
        assert_eq!(first, Outcome::failure(E_FAIL));
        assert_eq!(
            rig.handler
                .make_credential(&call(160, &encoded, GOOD_SIGNATURE)),
            first
        );
        assert_eq!(rig.handler.in_flight(), 0);
    }
}
