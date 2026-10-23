//! Pure translation between Windows' decoded plugin requests and the broker's passkey requests, and
//! back into what `WebAuthNEncode*Response` takes. No COM, no pipe, no clock: everything here is tested
//! with the captured webauthn.io request and fabricated app answers.
//!
//! # Trust
//!
//! The OS request carries the relying party id but no origin: Windows' WebAuthn client has already bound
//! the request to the RP. The provider therefore sends `https://{rpId}` as the origin (the broker
//! already trusts the origin string of a paired client; the browser extension supplies its own the same
//! way) and the engine still validates the RP id's syntax, IP literals and the public-suffix list.
//!
//! # HRESULTs
//!
//! [`hresult_for`] is the one table that decides what Windows (and through it the browser) is told; the
//! notes ("Phase 4.3/4.4 build") reproduce it.

use lp_broker::protocol::AssertedPasskey;
use lp_core::passkey::{
    ALG_ES256, PasskeyCandidate, PasskeyCreateRequest, PasskeyCreated, parse_authenticator_data,
    rp_id_hash,
};
use windows::Win32::Foundation::E_FAIL;
use windows::core::HRESULT;

use crate::relay::RelayError;
use crate::sys::{
    CredentialRow, DecodedGetAssertion, DecodedMakeCredential, NTE_DEVICE_NOT_FOUND, NTE_EXISTS,
    NTE_INVALID_PARAMETER, NTE_NOT_FOUND, NTE_NOT_SUPPORTED, NTE_USER_CANCELLED,
};

/// WebAuthn limits the user handle to 64 bytes.
pub const MAX_USER_ID: usize = 64;

/// A MakeCredential request as plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MakeRequest {
    /// The RP id exactly as the OS sent it (UTF-8).
    pub rp_id: String,
    pub rp_name: String,
    pub user_id: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
    /// COSE algorithm ids in the site's order of preference.
    pub algorithms: Vec<i64>,
    /// Credential ids on the exclude list.
    pub exclude: Vec<Vec<u8>>,
    pub client_data_hash: Vec<u8>,
}

impl MakeRequest {
    pub fn from_decoded(decoded: &DecodedMakeCredential<'_>) -> Result<Self, RelayError> {
        let user = decoded
            .user()
            .ok_or_else(|| RelayError::Invalid("the request has no user".into()))?;
        let rp = decoded.rp();
        Ok(Self {
            rp_id: decoded.rp_id(),
            rp_name: rp.map(|rp| rp.name).unwrap_or_default(),
            user_id: user.id,
            user_name: user.name,
            user_display_name: user.display_name,
            algorithms: decoded.algorithms().into_iter().map(i64::from).collect(),
            exclude: decoded.exclude_credential_ids(),
            client_data_hash: decoded.client_data_hash().to_vec(),
        })
    }
}

/// A GetAssertion request as plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetRequest {
    pub rp_id: String,
    pub client_data_hash: Vec<u8>,
    /// Credential ids on the allow list; empty = any discoverable credential.
    pub allow: Vec<Vec<u8>>,
}

impl GetRequest {
    pub fn from_decoded(decoded: &DecodedGetAssertion<'_>) -> Self {
        Self {
            rp_id: decoded.rp_id(),
            client_data_hash: decoded.client_data_hash().to_vec(),
            allow: decoded.allow_credential_ids(),
        }
    }
}

/// `https://{rpId}` for an RP id that looks like a host name; anything else is refused before it can
/// be glued into a URL.
pub fn origin_for(rp_id: &str) -> Result<String, RelayError> {
    let plausible = !rp_id.is_empty()
        && rp_id.len() <= 253
        && rp_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && !rp_id.starts_with(['.', '-'])
        && !rp_id.ends_with(['.', '-'])
        && !rp_id.contains("..");
    if plausible {
        Ok(format!("https://{rp_id}"))
    } else {
        Err(RelayError::Invalid(
            "the relying party id is not a host name".into(),
        ))
    }
}

/// The broker request for a MakeCredential. Windows asks for `rk` and `uv`; LocalPass always stores a
/// resident key and always verifies the user in the app, so `uv` is sent as required by the relay.
pub fn create_request(request: &MakeRequest) -> Result<PasskeyCreateRequest, RelayError> {
    let origin = origin_for(&request.rp_id)?;
    if !request.algorithms.contains(&ALG_ES256) {
        return Err(RelayError::Unsupported(
            "LocalPass passkeys are ES256 (-7) only".into(),
        ));
    }
    if request.user_id.is_empty() || request.user_id.len() > MAX_USER_ID {
        return Err(RelayError::Invalid(
            "the user id must be 1 to 64 bytes".into(),
        ));
    }
    Ok(PasskeyCreateRequest {
        origin,
        rp_id: request.rp_id.clone(),
        rp_name: if request.rp_name.is_empty() {
            request.rp_id.clone()
        } else {
            request.rp_name.clone()
        },
        user_handle: request.user_id.clone(),
        user_name: request.user_name.clone(),
        user_display_name: request.user_display_name.clone(),
        algorithms: request.algorithms.clone(),
        exclude: request.exclude.clone(),
        // The app runs its own Windows Hello check; this flag is the app's to set.
        user_verified: false,
    })
}

/// What `WebAuthNEncodeMakeCredentialResponse` needs: the authenticator data and the credential id,
/// checked against the request so a confused app cannot make Windows return another site's data.
pub struct CreatedParts {
    pub auth_data: Vec<u8>,
    pub credential_id: Vec<u8>,
}

pub fn created_parts(rp_id: &str, created: &PasskeyCreated) -> Result<CreatedParts, RelayError> {
    let parsed = parse_authenticator_data(&created.auth_data)
        .map_err(|e| RelayError::Other(format!("the app returned bad authenticator data: {e}")))?;
    let Some((credential_id, _public)) = parsed.credential else {
        return Err(RelayError::Other(
            "the app returned no attested credential data".into(),
        ));
    };
    if parsed.rp_id_hash != rp_id_hash(rp_id) {
        return Err(RelayError::Other(
            "the app's authenticator data is for another relying party".into(),
        ));
    }
    if credential_id != created.credential_id {
        return Err(RelayError::Other(
            "the app's credential id does not match its authenticator data".into(),
        ));
    }
    Ok(CreatedParts {
        auth_data: created.auth_data.clone(),
        credential_id,
    })
}

/// The origin to send for a sign-in, once the request is known to be able to succeed (a 32-byte client
/// data hash), so a doomed request never starts the app or asks the user.
///
/// Which passkey signs is the app's decision, inside the one `PasskeyAssertAny` request: exactly one must
/// match the RP and the allow list. Several (no allow list, or an allow list naming more than one) cannot
/// be chosen between without a picker, which LocalPass does not have yet: that is `NotFound` and a known
/// limit (Windows' own cache list is expected to narrow the allow list to the passkey the user picked).
pub fn assert_origin(request: &GetRequest) -> Result<String, RelayError> {
    if request.client_data_hash.len() != 32 {
        return Err(RelayError::Invalid(
            "the client data hash must be 32 bytes".into(),
        ));
    }
    origin_for(&request.rp_id)
}

/// Checks an assertion from the app before it is encoded for Windows.
pub fn check_assertion(request: &GetRequest, asserted: &AssertedPasskey) -> Result<(), RelayError> {
    let assertion = &asserted.assertion;
    let parsed = parse_authenticator_data(&assertion.auth_data)
        .map_err(|e| RelayError::Other(format!("the app returned bad authenticator data: {e}")))?;
    if parsed.rp_id_hash != rp_id_hash(&request.rp_id) {
        return Err(RelayError::Other(
            "the app's authenticator data is for another relying party".into(),
        ));
    }
    if !request.allow.is_empty() && !request.allow.contains(&assertion.credential_id) {
        return Err(RelayError::Other(
            "the app signed with a credential that is not on the allow list".into(),
        ));
    }
    if assertion.signature.is_empty() {
        return Err(RelayError::Other("the app returned no signature".into()));
    }
    Ok(())
}

/// A credential-cache row for a passkey. Windows wants an RP name and a user display name, so the
/// RP id and the user name stand in when they are empty.
pub fn cache_row(candidate: &PasskeyCandidate) -> CredentialRow {
    let or = |text: &str, fallback: &str| if text.is_empty() { fallback } else { text }.to_owned();
    CredentialRow {
        credential_id: candidate.credential_id.clone(),
        rp_id: candidate.rp_id.clone(),
        rp_name: or(&candidate.rp_name, &candidate.rp_id),
        user_id: candidate.user_handle.clone(),
        user_name: candidate.user_name.clone(),
        user_display_name: or(&candidate.user_display_name, &candidate.user_name),
    }
}

/// The HRESULT table: what Windows is told for each way an operation can end. Windows shows its own
/// generic text whatever is returned; the browser maps the code to a DOMException
/// (`WebAuthNGetErrorName`: `NTE_USER_CANCELLED`, `NTE_NOT_FOUND`, `NTE_DEVICE_NOT_FOUND` become
/// `NotAllowedError`, `NTE_EXISTS` `InvalidStateError`, `NTE_NOT_SUPPORTED` `ConstraintError`,
/// `NTE_INVALID_PARAMETER` `NotSupportedError`, anything else `UnknownError`).
///
/// | Situation | HRESULT |
/// |---|---|
/// | user cancelled, Hello denied or dismissed, vault locked and not unlocked, Windows cancelled the call | `NTE_USER_CANCELLED` |
/// | no passkey for the RP / allow list, or several and no way to choose | `NTE_NOT_FOUND` |
/// | the app is not running and could not be started, is not LocalPass, or this provider is not paired | `NTE_DEVICE_NOT_FOUND` |
/// | the exclude list names a passkey we hold | `NTE_EXISTS` |
/// | no ES256 (-7) in the offered algorithms | `NTE_NOT_SUPPORTED` |
/// | malformed request (undecodable, no user, bad RP id or hash length) | `NTE_INVALID_PARAMETER` |
/// | the request signature does not verify (see `handler`) | `NTE_BAD_SIGNATURE` |
/// | `CancelOperation` for a transaction we do not know | `NTE_NOT_FOUND` |
/// | anything else (app error, bad data from the app, timeout while the app was connected) | `E_FAIL` |
pub fn hresult_for(error: &RelayError) -> HRESULT {
    match error {
        RelayError::Cancelled => NTE_USER_CANCELLED,
        RelayError::NotFound => NTE_NOT_FOUND,
        RelayError::NotPaired | RelayError::Unreachable(_) => NTE_DEVICE_NOT_FOUND,
        RelayError::Exists => NTE_EXISTS,
        RelayError::Unsupported(_) => NTE_NOT_SUPPORTED,
        RelayError::Invalid(_) => NTE_INVALID_PARAMETER,
        RelayError::Other(_) => E_FAIL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::{NTE_BAD_SIGNATURE, WebAuthn};
    use crate::testing::{self, WEBAUTHN_IO_MAKE_CREDENTIAL};
    use lp_core::passkey::{
        FLAG_AT, FLAG_BE, FLAG_BS, FLAG_UP, FLAG_UV, PasskeyAssertion, PasskeyKey,
        authenticator_data, new_credential_id,
    };

    fn api() -> &'static WebAuthn {
        testing::api()
    }

    fn candidate(rp: &str, id: u8, name: &str) -> PasskeyCandidate {
        PasskeyCandidate {
            item_id: format!("item-{id}"),
            rp_id: rp.into(),
            rp_name: String::new(),
            user_name: name.into(),
            user_display_name: String::new(),
            credential_id: vec![id; 32],
            user_handle: vec![id, 1, 2],
            created_ms: 1,
        }
    }

    #[test]
    fn the_captured_webauthn_io_request_becomes_a_broker_request() {
        let encoded = testing::unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
        assert_eq!(encoded.len(), 179);
        let decoded = api().decode_make_credential(&encoded).unwrap();
        let request = MakeRequest::from_decoded(&decoded).unwrap();
        assert_eq!(request.rp_id, "webauthn.io");
        assert_eq!(request.rp_name, "webauthn.io");
        assert_eq!(request.user_id, b"webauthnio-test0101");
        assert_eq!(request.user_name, "test0101");
        assert_eq!(request.user_display_name, "test0101");
        assert_eq!(request.algorithms, vec![-7]);
        assert!(request.exclude.is_empty(), "the OS sent no exclude list");
        assert_eq!(request.client_data_hash.len(), 32);

        let create = create_request(&request).unwrap();
        assert_eq!(create.origin, "https://webauthn.io");
        assert_eq!(create.rp_id, "webauthn.io");
        assert_eq!(create.user_handle, b"webauthnio-test0101");
        assert_eq!(create.algorithms, vec![-7]);
        assert!(!create.user_verified, "the app's own Hello decides");
    }

    #[test]
    fn a_decoded_exclude_list_is_passed_on() {
        let mut encoded = testing::unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
        testing::insert_exclude_list(&mut encoded, &[vec![0x11; 12], vec![0x22; 40]]);
        let decoded = api().decode_make_credential(&encoded).unwrap();
        let request = MakeRequest::from_decoded(&decoded).unwrap();
        assert_eq!(request.exclude, vec![vec![0x11; 12], vec![0x22; 40]]);
        assert_eq!(create_request(&request).unwrap().exclude, request.exclude);
    }

    #[test]
    fn make_credential_requirements_are_enforced() {
        let base = MakeRequest {
            rp_id: "example.com".into(),
            rp_name: String::new(),
            user_id: vec![1; 16],
            user_name: "alice".into(),
            user_display_name: String::new(),
            algorithms: vec![-257, -7],
            exclude: vec![],
            client_data_hash: vec![0; 32],
        };
        let ok = create_request(&base).unwrap();
        assert_eq!(ok.rp_name, "example.com", "RP name falls back to the id");

        for algorithms in [vec![], vec![-257], vec![-8, -36]] {
            let refused = create_request(&MakeRequest {
                algorithms,
                ..base.clone()
            });
            assert!(matches!(refused, Err(RelayError::Unsupported(_))));
            assert_eq!(hresult_for(&refused.unwrap_err()), NTE_NOT_SUPPORTED);
        }
        for user_id in [vec![], vec![1; 65]] {
            let refused = create_request(&MakeRequest {
                user_id,
                ..base.clone()
            });
            assert!(matches!(refused, Err(RelayError::Invalid(_))));
        }
        assert!(
            create_request(&MakeRequest {
                user_id: vec![1; 64],
                ..base
            })
            .is_ok()
        );
    }

    #[test]
    fn rp_ids_become_origins_only_when_they_look_like_hosts() {
        assert_eq!(origin_for("example.com").unwrap(), "https://example.com");
        assert_eq!(
            origin_for("a-b.c_d.example").unwrap(),
            "https://a-b.c_d.example"
        );
        assert_eq!(origin_for("localhost").unwrap(), "https://localhost");
        for bad in [
            "",
            "evil.com/@good.com",
            "evil.com:8443",
            "user@evil.com",
            "a b.com",
            "ex\u{e4}mple.com",
            ".example.com",
            "example.com.",
            "a..b",
            "-a.com",
            "evil.com#x",
            "evil.com?x",
            "evil.com\\x",
        ] {
            assert!(
                matches!(origin_for(bad), Err(RelayError::Invalid(_))),
                "{bad:?} must be refused"
            );
        }
        assert!(origin_for(&"a".repeat(254)).is_err());
    }

    #[test]
    fn a_get_assertion_request_with_and_without_an_allow_list() {
        let hash = [0x5A; 32];
        let encoded =
            testing::get_assertion_request("example.com", &hash, &[vec![7; 32], vec![8; 16]]);
        let decoded = api().decode_get_assertion(&encoded).unwrap();
        let request = GetRequest::from_decoded(&decoded);
        assert_eq!(request.rp_id, "example.com");
        assert_eq!(request.client_data_hash, hash);
        assert_eq!(request.allow, vec![vec![7; 32], vec![8; 16]]);

        let bare = testing::get_assertion_request("example.com", &hash, &[]);
        let decoded = api().decode_get_assertion(&bare).unwrap();
        assert!(GetRequest::from_decoded(&decoded).allow.is_empty());
    }

    #[test]
    fn assertion_requests_need_a_32_byte_hash() {
        let mut request = GetRequest {
            rp_id: "example.com".into(),
            client_data_hash: vec![3; 32],
            allow: vec![],
        };
        assert_eq!(assert_origin(&request).unwrap(), "https://example.com");
        request.client_data_hash = vec![3; 31];
        assert!(matches!(
            assert_origin(&request),
            Err(RelayError::Invalid(_))
        ));
        request.client_data_hash = vec![3; 32];
        request.rp_id = "not a host".into();
        assert!(matches!(
            assert_origin(&request),
            Err(RelayError::Invalid(_))
        ));
    }

    #[test]
    fn answers_from_the_app_are_checked_before_they_go_to_windows() {
        let key = PasskeyKey::generate().unwrap();
        let credential_id = new_credential_id().unwrap();
        let auth_data = authenticator_data(
            "example.com",
            FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS,
            0,
            Some((&credential_id, &key.public())),
        );
        let created = |auth_data: Vec<u8>, id: Vec<u8>| PasskeyCreated {
            item_id: "i".into(),
            credential_id: id,
            auth_data,
            attestation_object: vec![],
            public_key: vec![],
            public_key_spki: vec![],
            algorithm: -7,
        };
        let good = created_parts(
            "example.com",
            &created(auth_data.clone(), credential_id.clone()),
        )
        .unwrap();
        assert_eq!(good.auth_data, auth_data);
        assert_eq!(good.credential_id, credential_id);
        assert_eq!(auth_data[32], 0x5D, "UP|UV|BE|BS|AT");
        assert_eq!(auth_data[32] & FLAG_AT, FLAG_AT);

        // Another site's data, another id, no attested data, garbage.
        assert!(
            created_parts(
                "evil.example",
                &created(auth_data.clone(), credential_id.clone())
            )
            .is_err()
        );
        assert!(created_parts("example.com", &created(auth_data.clone(), vec![9; 32])).is_err());
        let bare = authenticator_data("example.com", FLAG_UP, 0, None);
        assert!(created_parts("example.com", &created(bare, credential_id.clone())).is_err());
        assert!(created_parts("example.com", &created(vec![1, 2, 3], credential_id)).is_err());

        let request = GetRequest {
            rp_id: "example.com".into(),
            client_data_hash: vec![0; 32],
            allow: vec![],
        };
        let assertion = |auth: Vec<u8>, id: Vec<u8>, sig: Vec<u8>| AssertedPasskey {
            assertion: PasskeyAssertion {
                item_id: "item-1".into(),
                credential_id: id,
                auth_data: auth,
                signature: sig,
                user_handle: vec![],
            },
            user_name: "alice".into(),
            user_display_name: String::new(),
        };
        let flags = FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS;
        let auth = authenticator_data("example.com", flags, 0, None);
        assert!(check_assertion(&request, &assertion(auth.clone(), vec![1; 32], vec![1])).is_ok());
        assert!(check_assertion(&request, &assertion(auth.clone(), vec![1; 32], vec![])).is_err());
        let foreign = authenticator_data("evil.example", flags, 0, None);
        assert!(check_assertion(&request, &assertion(foreign, vec![1; 32], vec![1])).is_err());
        // With an allow list the credential that signed must be on it.
        let narrowed = GetRequest {
            allow: vec![vec![1; 32], vec![2; 32]],
            ..request
        };
        assert!(check_assertion(&narrowed, &assertion(auth.clone(), vec![2; 32], vec![1])).is_ok());
        assert!(check_assertion(&narrowed, &assertion(auth, vec![3; 32], vec![1])).is_err());
    }

    #[test]
    fn cache_rows_get_fallbacks_for_empty_names() {
        let mut c = candidate("example.com", 1, "alice");
        let row = cache_row(&c);
        assert_eq!(row.rp_name, "example.com");
        assert_eq!(row.user_display_name, "alice");
        assert_eq!(row.user_id, vec![1, 1, 2]);
        assert_eq!(row.credential_id, vec![1; 32]);
        c.rp_name = "Example".into();
        c.user_display_name = "Alice A.".into();
        let row = cache_row(&c);
        assert_eq!(
            (row.rp_name.as_str(), row.user_display_name.as_str()),
            ("Example", "Alice A.")
        );
    }

    #[test]
    fn the_hresult_table() {
        let cases: [(RelayError, HRESULT); 9] = [
            (RelayError::Cancelled, NTE_USER_CANCELLED),
            (RelayError::NotFound, NTE_NOT_FOUND),
            (RelayError::NotPaired, NTE_DEVICE_NOT_FOUND),
            (RelayError::Unreachable("x".into()), NTE_DEVICE_NOT_FOUND),
            (RelayError::Exists, NTE_EXISTS),
            (RelayError::Unsupported("x".into()), NTE_NOT_SUPPORTED),
            (RelayError::Invalid("x".into()), NTE_INVALID_PARAMETER),
            (RelayError::Other("x".into()), E_FAIL),
            (RelayError::Other(String::new()), E_FAIL),
        ];
        for (error, expected) in cases {
            assert_eq!(hresult_for(&error), expected, "{error:?}");
        }
        // The bad-signature code is the handler's, not a relay error; make sure it is distinct.
        assert_ne!(NTE_BAD_SIGNATURE, E_FAIL);
    }
}
