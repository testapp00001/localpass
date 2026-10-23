//! Tests of the loader, the decode/encode wrappers and the buffer owners.
//!
//! Everything here is read-only against the OS: it decodes and encodes byte buffers in `webauthn.dll`
//! and asks for the state of a CLSID nobody registered. It never calls Add/Remove/Update, the credential
//! cache, the status callbacks or the user-verification prompt, so the registered spike authenticator and
//! Windows' passkey settings are left alone.

use std::ffi::{CStr, c_void};
use std::ptr::dangling;
use std::sync::OnceLock;

use data_encoding::HEXLOWER;
use sha2::{Digest, Sha256};
use windows::Win32::System::Com::{CoGetMalloc, CoTaskMemFree};
use windows::core::GUID;

use super::*;

/// The MakeCredential request webauthn.io sent in the stage B spike run (179 bytes).
const WEBAUTHN_IO_MAKE_CREDENTIAL: &str = "a601582043cc5af7c1bf8896702177ce43d30904e545e135c5c2a858d0feea3df0b4a11e02a26269646b776562617574686e2e696f646e616d656b776562617574686e2e696f03a362696453776562617574686e696f2d7465737430313031646e616d656874657374303130316b646973706c61794e616d656874657374303130310481a263616c672664747970656a7075626c69632d6b657906a16b6372656450726f746563740207a262726bf5627576f5";

const CLIENT_DATA_HASH: &str = "43cc5af7c1bf8896702177ce43d30904e545e135c5c2a858d0feea3df0b4a11e";

/// A CLSID nobody registered (random, not the spike's).
const UNREGISTERED: GUID = GUID::from_u128(0x5f0c_2a9e_7b31_4d58_9c6e_0a1b_2c3d_4e5f);

fn api() -> &'static WebAuthn {
    static API: OnceLock<WebAuthn> = OnceLock::new();
    API.get_or_init(|| WebAuthn::load().expect("this OS has the plugin API (build >= 26100.6725)"))
}

fn unhex(s: &str) -> Vec<u8> {
    HEXLOWER.decode(s.as_bytes()).expect("valid hex")
}

fn find(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

// ---- tiny CBOR writer for hand-built requests

fn head(major: u8, n: usize) -> Vec<u8> {
    let m = major << 5;
    match n {
        0..=23 => vec![m | n as u8],
        24..=255 => vec![m | 24, n as u8],
        _ => {
            let mut v = vec![m | 25];
            v.extend((n as u16).to_be_bytes());
            v
        }
    }
}

fn text(s: &str) -> Vec<u8> {
    [head(3, s.len()), s.as_bytes().to_vec()].concat()
}

fn bstr(b: &[u8]) -> Vec<u8> {
    [head(2, b.len()), b.to_vec()].concat()
}

/// A canonical `{"id": <bytes>, "type": "public-key"}` credential descriptor.
fn descriptor(id: &[u8]) -> Vec<u8> {
    [
        vec![0xA2],
        text("id"),
        bstr(id),
        text("type"),
        text("public-key"),
    ]
    .concat()
}

// ------------------------------------------------------------------ the loader

#[test]
fn bound_exports_are_the_final_api_only() {
    let names = WebAuthn::EXPORTS;
    assert_eq!(names.len(), 23);
    let mut sorted = names.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), names.len(), "no export is bound twice");
    for name in names {
        assert!(name.starts_with("WebAuthN"), "{name}");
        assert!(!name.contains("EXPERIMENTAL"), "{name}");
        assert!(
            !name.ends_with('2'),
            "{name}: the *2 functions are not exported"
        );
        assert!(
            !name.contains("SetAuthenticatorState"),
            "{name}: the user's choice"
        );
        assert!(
            NOTES_EXPORTS.contains(name),
            "{name} is not in the notes' list"
        );
    }
    // The ones the task names explicitly.
    for must in [
        "WebAuthNPluginGetAuthenticatorState",
        "WebAuthNPluginAddAuthenticator",
        "WebAuthNPluginRemoveAuthenticator",
        "WebAuthNPluginUpdateAuthenticatorDetails",
        "WebAuthNPluginFreeAddAuthenticatorResponse",
        "WebAuthNPluginAuthenticatorAddCredentials",
        "WebAuthNPluginAuthenticatorRemoveCredentials",
        "WebAuthNPluginAuthenticatorRemoveAllCredentials",
        "WebAuthNPluginAuthenticatorGetAllCredentials",
        "WebAuthNPluginAuthenticatorFreeCredentialDetailsArray",
        "WebAuthNPluginGetOperationSigningPublicKey",
        "WebAuthNPluginGetUserVerificationPublicKey",
        "WebAuthNPluginFreePublicKeyResponse",
        "WebAuthNPluginPerformUserVerification",
        "WebAuthNPluginFreeUserVerificationResponse",
        "WebAuthNPluginRegisterStatusChangeCallback",
        "WebAuthNPluginUnregisterStatusChangeCallback",
        "WebAuthNDecodeMakeCredentialRequest",
        "WebAuthNFreeDecodedMakeCredentialRequest",
        "WebAuthNDecodeGetAssertionRequest",
        "WebAuthNFreeDecodedGetAssertionRequest",
        "WebAuthNEncodeMakeCredentialResponse",
        "WebAuthNEncodeGetAssertionResponse",
    ] {
        assert!(names.contains(&must), "{must} is not bound");
    }
}

/// A resolver that finds every export except `absent` (never called through, only compared with null).
fn resolver_without<'a>(absent: &'a [&'a str]) -> impl FnMut(&CStr) -> Option<*const c_void> + 'a {
    move |name| {
        let name = name.to_str().expect("ascii export name");
        if absent.contains(&name) {
            None
        } else {
            Some(dangling::<c_void>())
        }
    }
}

#[test]
fn loader_reports_exactly_the_missing_exports() {
    let absent = [
        "WebAuthNEncodeGetAssertionResponse",
        "WebAuthNPluginAddAuthenticator",
    ];
    // All of them, in table order.
    assert_eq!(
        WebAuthn::missing_exports(resolver_without(&absent)),
        vec![
            "WebAuthNPluginAddAuthenticator",
            "WebAuthNEncodeGetAssertionResponse"
        ]
    );
    assert!(WebAuthn::missing_exports(resolver_without(&[])).is_empty());
    // SAFETY: the fake addresses are never called.
    unsafe {
        // The first missing one, as an Unsupported error that names it.
        let err = WebAuthn::from_resolver(resolver_without(&absent)).unwrap_err();
        assert_eq!(
            err,
            LoadError::Unsupported("WebAuthNPluginAddAuthenticator")
        );
        assert_eq!(
            err.to_string(),
            "webauthn.dll does not export WebAuthNPluginAddAuthenticator"
        );
        let err =
            WebAuthn::from_resolver(resolver_without(&["WebAuthNEncodeGetAssertionResponse"]))
                .unwrap_err();
        assert_eq!(
            err,
            LoadError::Unsupported("WebAuthNEncodeGetAssertionResponse")
        );
        // Nothing missing: the table binds.
        assert!(WebAuthn::from_resolver(resolver_without(&[])).is_ok());
    }
}

#[test]
fn every_export_is_requested_by_its_exact_name() {
    let mut asked = Vec::new();
    // SAFETY: the fake addresses are never called.
    let bound = unsafe {
        WebAuthn::from_resolver(|name| {
            asked.push(name.to_str().unwrap().to_owned());
            Some(dangling::<c_void>())
        })
    };
    assert!(bound.is_ok());
    assert_eq!(asked, WebAuthn::EXPORTS);
}

#[test]
fn this_os_exports_the_whole_plugin_api() {
    // The dev PC (build 26200, webauthn.dll 10.0.26100.7171) has all of them; an OS older than
    // 26100.6725 would report the first missing name here instead of crashing at load.
    match WebAuthn::load() {
        Ok(_) => {}
        Err(e) => panic!("{e}"),
    }
    let probed = probe_exports().expect("webauthn.dll loads");
    for name in NOTES_EXPORTS {
        assert!(
            probed.iter().any(|(n, present)| n == name && *present),
            "{name} missing"
        );
    }
}

#[test]
fn state_of_an_unregistered_clsid_is_not_found() {
    // Read-only: GetAuthenticatorState on a random CLSID.
    let err = api().authenticator_state(&UNREGISTERED).unwrap_err();
    assert_eq!(err, NTE_NOT_FOUND);
}

// ------------------------------------------------------------------ MakeCredential decode

#[test]
fn decodes_the_captured_webauthn_io_make_credential_request() {
    let encoded = unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
    assert_eq!(encoded.len(), 179, "the captured request is 179 bytes");
    let req = api().decode_make_credential(&encoded).unwrap();

    assert_eq!(req.version, CTAP_MAKE_CREDENTIAL_REQUEST_VERSION);
    assert_eq!(req.rp_id_utf8(), b"webauthn.io");
    assert_eq!(req.rp_id(), "webauthn.io");
    let hash = req.client_data_hash();
    assert_eq!(hash.len(), 32);
    assert_eq!(hash, unhex(CLIENT_DATA_HASH));
    assert_eq!(&hash[..4], &[0x43, 0xcc, 0x5a, 0xf7]);
    assert_eq!(&hash[30..], &[0xa1, 0x1e]);

    let rp = req.rp().expect("rp entity");
    assert_eq!(rp.id, "webauthn.io");
    assert_eq!(rp.name, "webauthn.io");
    // SAFETY: the decoder's entity pointers are valid while `req` lives.
    unsafe {
        assert_eq!((*req.rp_information).version, RP_ENTITY_VERSION);
        assert_eq!((*req.user_information).version, USER_ENTITY_VERSION);
    }

    let user = req.user().expect("user entity");
    assert_eq!(user.id, b"webauthnio-test0101");
    assert_eq!(user.id.len(), 19);
    assert_eq!(user.name, "test0101");
    assert_eq!(user.display_name, "test0101");

    // Fields deep in the struct: these only come out right if every offset in between is right.
    assert_eq!(req.algorithms(), vec![COSE_ALG_ES256]);
    assert_eq!(
        req.credential_parameters(),
        vec![(CREDENTIAL_TYPE_PUBLIC_KEY.to_owned(), -7)]
    );
    assert_eq!(req.credential_list.count, 0, "no exclude list");
    assert!(req.exclude_credential_ids().is_empty());
    assert_eq!(req.cred_protect, 2, "credProtect 2 from the extensions map");
    let options = req.options().expect("authenticator options");
    assert_eq!(options.version, CTAP_AUTHENTICATOR_OPTIONS_VERSION);
    assert_eq!(options.resident_key(), Some(true), "rk: true");
    assert_eq!(options.uv(), Some(true), "uv: true");
    assert_eq!(options.require_resident_key, 1);
    assert_eq!(options.uv, 1);
    assert_eq!(options.up, 0, "up not sent");
    assert_eq!(req.empty_pin_auth, 0);
    assert_eq!(req.cb_pin_auth, 0);
    assert!(req.pb_pin_auth.is_null());
    assert!(req.hmac_secret_mc_extension.is_null());
    assert_eq!(req.cb_cred_blob_ext, 0);
}

#[test]
fn decodes_an_exclude_list() {
    let mut encoded = unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
    // 06 a1 6b: the start of key 6 (extensions map). Insert key 5 (excludeList) in front of it.
    let at = encoded
        .windows(3)
        .position(|w| w == [0x06, 0xA1, 0x6B])
        .expect("key 6 in the captured request");
    let (id1, id2) = ([0x11u8; 12], [0x22u8; 40]);
    let exclude = [vec![0x05, 0x82], descriptor(&id1), descriptor(&id2)].concat();
    encoded.splice(at..at, exclude);
    encoded[0] = 0xA7; // map of 7 entries

    let req = api().decode_make_credential(&encoded).unwrap();
    assert_eq!(req.credential_list.count, 2);
    assert_eq!(
        req.exclude_credential_ids(),
        vec![id1.to_vec(), id2.to_vec()]
    );
    // SAFETY: the decoder's list holds `count` valid entries while `req` lives.
    unsafe {
        let first = &**req.credential_list.credentials;
        assert_eq!(first.version, CREDENTIAL_EX_VERSION);
        assert_eq!(first.cb_id, 12);
        assert_eq!(wstr(first.credential_type), CREDENTIAL_TYPE_PUBLIC_KEY);
    }
    // The rest of the request is unchanged.
    assert_eq!(req.rp_id(), "webauthn.io");
    assert_eq!(req.cred_protect, 2);
    assert_eq!(req.options().unwrap().resident_key(), Some(true));
}

#[test]
fn malformed_make_credential_requests_are_errors_not_crashes() {
    let good = unhex(WEBAUTHN_IO_MAKE_CREDENTIAL);
    assert!(api().decode_make_credential(&[]).is_err());
    assert!(api().decode_make_credential(&[0xFF; 8]).is_err());
    assert!(api().decode_make_credential(&good[..100]).is_err());
    assert!(
        api()
            .decode_make_credential(&good[..good.len() - 1])
            .is_err()
    );
}

// ------------------------------------------------------------------ GetAssertion decode

#[test]
fn decodes_a_get_assertion_request_with_an_allow_list() {
    let hash = unhex(CLIENT_DATA_HASH);
    let (id1, id2) = (vec![0xA1u8; 32], vec![0xB2u8; 16]);
    let encoded = [
        vec![0xA4],
        vec![0x01],
        text("webauthn.io"),
        vec![0x02],
        bstr(&hash),
        vec![0x03, 0x82],
        descriptor(&id1),
        descriptor(&id2),
        vec![0x05, 0xA1],
        text("uv"),
        vec![0xF5],
    ]
    .concat();

    let req = api().decode_get_assertion(&encoded).unwrap();
    assert_eq!(req.version, 1);
    assert_eq!(req.rp_id_utf8(), b"webauthn.io");
    assert_eq!(req.rp_id(), "webauthn.io");
    // SAFETY: the decoder's UTF-16 copy is NUL-terminated and valid while `req` lives.
    assert_eq!(unsafe { wstr(req.rp_id_utf16) }, "webauthn.io");
    assert_eq!(req.client_data_hash(), hash);
    assert_eq!(req.credential_list.count, 2);
    assert_eq!(req.allow_credential_ids(), vec![id1, id2]);
    let options = req.options().expect("options");
    assert_eq!(options.uv(), Some(true));
    assert_eq!(options.resident_key(), None);
    assert!(req.hmac_salt_extension.is_null());
    assert_eq!(req.cb_cred_large_blob_compressed, 0);
    assert_eq!(req.cb_json_ext, 0);
}

#[test]
fn decodes_a_get_assertion_request_without_an_allow_list() {
    let hash = [0x5Au8; 32];
    let encoded = [
        vec![0xA2],
        vec![0x01],
        text("example.com"),
        vec![0x02],
        bstr(&hash),
    ]
    .concat();
    let req = api().decode_get_assertion(&encoded).unwrap();
    assert_eq!(req.rp_id(), "example.com");
    assert_eq!(req.credential_list.count, 0);
    assert!(req.allow_credential_ids().is_empty());
    assert!(req.options().is_none());
    assert!(api().decode_get_assertion(&[0xFF, 0x00]).is_err());
    assert!(api().decode_get_assertion(&[]).is_err());
}

// ------------------------------------------------------------------ encode

/// Well-formed authenticator data for an ES256 credential: `rpIdHash || 0x5D || counter 0 || AAGUID 0 ||
/// len || credId || COSE key`.
fn registration_auth_data(rp_id: &str, credential_id: &[u8]) -> Vec<u8> {
    use p256::ecdsa::SigningKey;
    let sk = SigningKey::from_slice(&[7u8; 32]).unwrap();
    let point = sk.verifying_key().to_sec1_point(false);
    let (x, y) = (&point.as_bytes()[1..33], &point.as_bytes()[33..65]);
    let mut data = Sha256::digest(rp_id.as_bytes()).to_vec();
    data.push(0x5D);
    data.extend([0u8; 4]);
    data.extend([0u8; 16]);
    data.extend((credential_id.len() as u16).to_be_bytes());
    data.extend(credential_id);
    data.extend([0xA5, 0x01, 0x02, 0x03, 0x26, 0x20, 0x01, 0x21, 0x58, 0x20]);
    data.extend(x);
    data.extend([0x22, 0x58, 0x20]);
    data.extend(y);
    data
}

/// Is `ptr` a live `CoTaskMemAlloc` block? `DidAlloc`: 1 yes, 0 no, -1 cannot tell.
fn did_alloc(ptr: *const u8) -> i32 {
    // SAFETY: MEMCTX_TASK = 1 asks for the COM task allocator; DidAlloc only inspects the address.
    unsafe {
        let malloc = CoGetMalloc(1).expect("COM task allocator");
        malloc.DidAlloc(Some(ptr.cast()))
    }
}

#[test]
fn encodes_a_none_attestation_into_a_cotaskmem_buffer() {
    let credential_id = [0xC3u8; 32];
    let auth_data = registration_auth_data("webauthn.io", &credential_id);
    assert_eq!(auth_data.len(), 32 + 1 + 4 + 16 + 2 + 32 + 77);

    let encoded = api()
        .encode_make_credential_none(&auth_data, &credential_id)
        .unwrap();
    assert!(!encoded.is_empty());
    let cbor = encoded.as_slice().to_vec();
    eprintln!(
        "MakeCredential response ({} bytes): {}",
        cbor.len(),
        HEXLOWER.encode(&cbor)
    );
    // The authenticator data goes through byte for byte, tagged with the "none" format.
    assert!(find(&cbor, &auth_data), "authData is carried verbatim");
    assert!(find(&cbor, b"none"));
    assert!(cbor.len() > auth_data.len());
    // It is a CBOR map (major type 5).
    assert_eq!(cbor[0] >> 5, 5);

    // The allocation is a CoTaskMem block (1 = yes; -1 = the allocator cannot tell; 0 would be wrong).
    let did = did_alloc(encoded.as_slice().as_ptr());
    eprintln!("IMalloc::DidAlloc(encoded) = {did}");
    assert_ne!(did, 0, "the encoder's buffer is not a CoTaskMem allocation");

    // Handing it to the COM response moves ownership; free it the way the stub does.
    let mut response = OperationResponse::default();
    encoded.write_to(&mut response);
    assert_eq!(response.cb_encoded_response as usize, cbor.len());
    assert!(!response.pb_encoded_response.is_null());
    // SAFETY: the response owns the allocation now.
    unsafe {
        assert_eq!(
            bytes(response.pb_encoded_response, response.cb_encoded_response),
            cbor
        );
        CoTaskMemFree(Some(response.pb_encoded_response.cast_const().cast()));
    }
}

#[test]
fn into_raw_gives_the_allocation_away_without_freeing_it() {
    let auth_data = registration_auth_data("example.com", &[1, 2, 3]);
    let encoded = api()
        .encode_make_credential_none(&auth_data, &[1, 2, 3])
        .unwrap();
    let expected = encoded.as_slice().to_vec();
    let (len, ptr) = encoded.into_raw();
    // SAFETY: still our allocation; nothing freed it.
    unsafe {
        assert_eq!(bytes(ptr, len), expected);
        CoTaskMemFree(Some(ptr.cast_const().cast()));
    }
}

#[test]
fn encodes_a_get_assertion_response() {
    let credential_id = [0xC3u8; 32];
    let user_id = b"user-handle-0101";
    // rpIdHash || UP|UV|BE|BS || counter 0
    let mut auth_data = Sha256::digest(b"webauthn.io").to_vec();
    auth_data.push(0x1D);
    auth_data.extend([0u8; 4]);
    let signature = [0x30u8, 0x44, 0x02, 0x20, 0x11, 0x22, 0x33, 0x44];
    let output = AssertionOutput {
        auth_data: &auth_data,
        signature: &signature,
        credential_id: &credential_id,
        user: Some(AssertionUser {
            id: user_id,
            name: "test0101",
            display_name: "Test User",
        }),
        number_of_credentials: 1,
        user_selected: true,
    };
    let encoded = api().encode_get_assertion(&output).unwrap();
    let cbor = encoded.as_slice().to_vec();
    eprintln!(
        "GetAssertion response ({} bytes): {}",
        cbor.len(),
        HEXLOWER.encode(&cbor)
    );
    assert_eq!(cbor[0] >> 5, 5, "a CBOR map");
    assert!(find(&cbor, &auth_data));
    assert!(find(&cbor, &signature));
    assert!(find(&cbor, &credential_id));
    assert!(find(&cbor, user_id));
    assert!(find(&cbor, b"test0101"));
    assert!(find(&cbor, b"public-key"));
    assert_ne!(did_alloc(encoded.as_slice().as_ptr()), 0);

    // Without a user entity (credential with an allow list) it still encodes.
    let output = AssertionOutput {
        user: None,
        user_selected: false,
        ..output
    };
    let encoded = api().encode_get_assertion(&output).unwrap();
    assert!(find(encoded.as_slice(), &signature));
    assert!(!find(encoded.as_slice(), b"test0101"));
}

#[test]
fn cotaskmem_buf_of_nothing_is_harmless() {
    // SAFETY: a null buffer is explicitly allowed.
    let empty = unsafe { CoTaskMemBuf::from_raw(std::ptr::null_mut(), 0) };
    assert!(empty.is_empty());
    assert_eq!(empty.as_slice(), &[] as &[u8]);
    drop(empty);
    let mut response = OperationResponse::default();
    // SAFETY: as above.
    unsafe { CoTaskMemBuf::from_raw(std::ptr::null_mut(), 0) }.write_to(&mut response);
    assert_eq!(response.cb_encoded_response, 0);
    assert!(response.pb_encoded_response.is_null());
}

#[test]
fn tri_state_options() {
    let opts = CtapAuthenticatorOptions {
        version: 1,
        up: 1,
        uv: -1,
        require_resident_key: 0,
    };
    assert_eq!(opts.up(), Some(true));
    assert_eq!(opts.uv(), Some(false));
    assert_eq!(opts.resident_key(), None);
}

#[test]
fn authenticator_state_values() {
    assert_eq!(
        AuthenticatorState::from_raw(STATE_DISABLED),
        AuthenticatorState::Disabled
    );
    assert_eq!(
        AuthenticatorState::from_raw(STATE_ENABLED),
        AuthenticatorState::Enabled
    );
    assert_eq!(
        AuthenticatorState::from_raw(7),
        AuthenticatorState::Other(7)
    );
    assert_eq!(AuthenticatorState::Other(7).raw(), 7);
    assert_eq!(AuthenticatorState::Enabled.raw(), STATE_ENABLED);
}
