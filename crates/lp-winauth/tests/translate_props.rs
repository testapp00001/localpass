//! Property tests for the pure request/response translation of the Windows passkey provider
//! (phase 5.1). Windows hands the provider an RP id, user fields and credential ids it did not
//! check, and the app hands back authenticator data the provider must not pass on unchecked, so
//! both directions get arbitrary and hostile input here. Nothing in this file touches COM, the
//! pipe or `webauthn.dll`.

#![cfg(windows)]

use std::sync::LazyLock;

use lp_broker::protocol::AssertedPasskey;
use lp_core::origin::{Origin, validate_rp_id};
use lp_core::passkey::{
    ALG_ES256, FLAG_BE, FLAG_BS, FLAG_UP, FLAG_UV, PasskeyAssertion, PasskeyCandidate,
    PasskeyCreated, PasskeyKey, authenticator_data, rp_id_hash,
};
use lp_winauth::relay::RelayError;
use lp_winauth::sys::{
    NTE_DEVICE_NOT_FOUND, NTE_EXISTS, NTE_INVALID_PARAMETER, NTE_NOT_FOUND, NTE_NOT_SUPPORTED,
    NTE_USER_CANCELLED,
};
use lp_winauth::translate::{
    GetRequest, MAX_USER_ID, MakeRequest, assert_origin, cache_row, check_assertion,
    create_request, created_parts, hresult_for, origin_for,
};
use proptest::collection::vec;
use proptest::prelude::*;
use windows::Win32::Foundation::E_FAIL;

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

static KEY: LazyLock<PasskeyKey> = LazyLock::new(|| PasskeyKey::generate().unwrap());

/// What the OS might call an RP id: plausible host names, numeric forms the URL parser reads as IP
/// addresses, and anything at all.
fn rp_id() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "([a-zA-Z0-9-]{1,12}\\.){0,4}[a-z]{2,6}",
        2 => "[a-zA-Z0-9._-]{0,70}",
        2 => "[0-9xA-Fa-f.]{1,20}",
        1 => any::<String>(),
        1 => "[a-z0-9.-]{1,12}[/@:?#\\\\ %\\t\\n\\u{0}\\u{202e}\\u{ff0e}][a-z0-9.-]{0,12}",
    ]
}

fn plausible_rp_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 253
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        && !id.starts_with(['.', '-'])
        && !id.ends_with(['.', '-'])
        && !id.contains("..")
}

fn algorithms() -> impl Strategy<Value = Vec<i64>> {
    vec(
        prop_oneof![Just(-7i64), Just(-8), Just(-257), Just(0), any::<i64>()],
        0..5,
    )
}

fn make_request() -> impl Strategy<Value = MakeRequest> {
    (
        rp_id(),
        prop_oneof![Just(String::new()), ".{0,30}"],
        vec(any::<u8>(), 0..70),
        ".{0,30}",
        prop_oneof![Just(String::new()), ".{0,30}"],
        algorithms(),
        vec(vec(any::<u8>(), 0..10), 0..4),
        vec(any::<u8>(), 0..40),
    )
        .prop_map(
            |(
                rp_id,
                rp_name,
                user_id,
                user_name,
                user_display_name,
                algorithms,
                exclude,
                client_data_hash,
            )| MakeRequest {
                rp_id,
                rp_name,
                user_id,
                user_name,
                user_display_name,
                algorithms,
                exclude,
                client_data_hash,
            },
        )
}

proptest! {
    #![proptest_config(config())]

    /// The origin is `https://` plus a host-like id and nothing else, so no id can smuggle a
    /// userinfo, port, path or another host into the URL. Whatever origin comes out, the engine's
    /// own check (`Origin::parse` + `validate_rp_id`) only accepts the id when it is the host
    /// itself, never for an address the URL parser made out of a numeric id.
    #[test]
    fn origin_for_builds_only_host_like_origins(id in rp_id()) {
        match origin_for(&id) {
            Ok(origin) => {
                prop_assert!(plausible_rp_id(&id), "accepted {id:?}");
                prop_assert_eq!(origin.as_str(), format!("https://{id}"));
                if let Ok(parsed) = Origin::parse(&origin) {
                    prop_assert_eq!(&parsed.scheme, "https");
                    prop_assert_eq!(parsed.port, None);
                    if validate_rp_id(&id, &parsed).is_ok() {
                        prop_assert!(!parsed.is_ip, "{id:?} is valid for the address {parsed:?}");
                        prop_assert_eq!(&parsed.host, &id.to_ascii_lowercase());
                    }
                }
            }
            Err(e) => {
                prop_assert!(!plausible_rp_id(&id), "refused {id:?}");
                prop_assert!(matches!(e, RelayError::Invalid(_)));
            }
        }
    }

    /// `create_request` decides in a fixed order (RP id, algorithm, user id) and passes the request
    /// through unchanged otherwise; it never claims the user was verified.
    #[test]
    fn create_request_follows_the_rules(request in make_request()) {
        let origin_ok = origin_for(&request.rp_id).is_ok();
        let alg_ok = request.algorithms.contains(&ALG_ES256);
        let user_ok = (1..=MAX_USER_ID).contains(&request.user_id.len());
        match create_request(&request) {
            Ok(out) => {
                prop_assert!(origin_ok && alg_ok && user_ok);
                prop_assert_eq!(out.origin, format!("https://{}", request.rp_id));
                prop_assert_eq!(&out.rp_id, &request.rp_id);
                let name = if request.rp_name.is_empty() { &request.rp_id } else { &request.rp_name };
                prop_assert_eq!(&out.rp_name, name);
                prop_assert_eq!(&out.user_handle, &request.user_id);
                prop_assert_eq!(&out.user_name, &request.user_name);
                prop_assert_eq!(&out.user_display_name, &request.user_display_name);
                prop_assert_eq!(&out.algorithms, &request.algorithms);
                prop_assert_eq!(&out.exclude, &request.exclude);
                prop_assert!(!out.user_verified, "only the app may set user verification");
            }
            Err(RelayError::Invalid(_)) => prop_assert!(!origin_ok || (alg_ok && !user_ok)),
            Err(RelayError::Unsupported(_)) => prop_assert!(origin_ok && !alg_ok),
            Err(other) => prop_assert!(false, "unexpected error {other:?}"),
        }
    }

    /// A sign-in is only started for a 32-byte client data hash and an RP id `origin_for` accepts.
    #[test]
    fn assert_origin_needs_a_32_byte_hash_and_a_host_like_id(
        id in rp_id(),
        hash in vec(any::<u8>(), 30..35),
        allow in vec(vec(any::<u8>(), 0..8), 0..3),
    ) {
        let request = GetRequest { rp_id: id.clone(), client_data_hash: hash.clone(), allow };
        let result = assert_origin(&request);
        prop_assert_eq!(result.is_ok(), hash.len() == 32 && origin_for(&id).is_ok());
        if let Ok(origin) = result {
            prop_assert_eq!(origin, format!("https://{id}"));
        }
    }

    /// What the app returns for a new passkey reaches Windows only when it is for the RP that was
    /// asked, names the same credential twice consistently and is complete. Cut anywhere, mutated
    /// or random, it is an error and never a panic.
    #[test]
    fn created_parts_accepts_only_what_belongs_to_the_request(
        id in rp_id(),
        other in rp_id(),
        credential_id in vec(any::<u8>(), 1..300),
        flags in any::<u8>().prop_map(|f| f & (FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS)),
        count in any::<u32>(),
        cut in any::<prop::sample::Index>(),
        junk in vec(any::<u8>(), 0..400),
        flip in any::<prop::sample::Index>(),
    ) {
        let public = KEY.public();
        let auth_data = authenticator_data(&id, flags, count, Some((&credential_id, &public)));
        let created = PasskeyCreated {
            item_id: "item".into(),
            credential_id: credential_id.clone(),
            auth_data: auth_data.clone(),
            attestation_object: Vec::new(),
            public_key: Vec::new(),
            public_key_spki: Vec::new(),
            algorithm: ALG_ES256,
        };
        let ok = created_parts(&id, &created).expect("the honest answer is accepted");
        prop_assert_eq!(&ok.auth_data, &auth_data);
        prop_assert_eq!(&ok.credential_id, &credential_id);

        if other != id {
            prop_assert!(created_parts(&other, &created).is_err());
        }

        let mut wrong_id = created.clone();
        wrong_id.credential_id[flip.index(credential_id.len())] ^= 1;
        prop_assert!(created_parts(&id, &wrong_id).is_err());

        let mut cut_off = created.clone();
        cut_off.auth_data.truncate(cut.index(auth_data.len()));
        prop_assert!(created_parts(&id, &cut_off).is_err(), "a cut answer was accepted");

        let mut no_credential = created.clone();
        no_credential.auth_data = authenticator_data(&id, flags, count, None);
        prop_assert!(created_parts(&id, &no_credential).is_err());

        let mut garbage = created;
        garbage.auth_data = junk;
        prop_assert!(created_parts(&id, &garbage).is_err());
    }

    /// An assertion from the app is passed to Windows only when it is for this RP, signed with a
    /// credential the site allowed (any, if it named none) and carries a signature.
    #[test]
    fn check_assertion_matches_the_request(
        id in rp_id(),
        other in rp_id(),
        credential_id in vec(any::<u8>(), 1..40),
        allow in vec(vec(any::<u8>(), 1..40), 0..3),
        include_signed in any::<bool>(),
        signature in prop_oneof![Just(Vec::new()), vec(any::<u8>(), 1..80)],
        asserted_for_other in any::<bool>(),
        junk in vec(any::<u8>(), 0..200),
        garbage in any::<bool>(),
    ) {
        let mut allow = allow;
        if include_signed {
            allow.push(credential_id.clone());
        }
        let request = GetRequest { rp_id: id.clone(), client_data_hash: vec![0; 32], allow: allow.clone() };
        let signed_rp = if asserted_for_other { &other } else { &id };
        let auth_data = if garbage {
            junk
        } else {
            authenticator_data(signed_rp, FLAG_UP | FLAG_UV, 0, None)
        };
        let asserted = AssertedPasskey {
            assertion: PasskeyAssertion {
                item_id: "item".into(),
                credential_id: credential_id.clone(),
                auth_data,
                signature: signature.clone(),
                user_handle: vec![1],
            },
            user_name: String::new(),
            user_display_name: String::new(),
        };
        let hash_matches = !garbage && rp_id_hash(signed_rp) == rp_id_hash(&id);
        let expected = hash_matches
            && (allow.is_empty() || allow.contains(&credential_id))
            && !signature.is_empty();
        prop_assert_eq!(check_assertion(&request, &asserted).is_ok(), expected);
    }

    /// The row Windows caches for a passkey keeps the ids and names and fills the two names
    /// Windows insists on from the RP id and the user name.
    #[test]
    fn cache_rows_keep_the_data_and_fill_empty_names(
        rp in rp_id(),
        rp_name in prop_oneof![Just(String::new()), ".{0,30}"],
        user_name in prop_oneof![Just(String::new()), ".{0,30}"],
        display in prop_oneof![Just(String::new()), ".{0,30}"],
        credential_id in vec(any::<u8>(), 0..70),
        handle in vec(any::<u8>(), 0..70),
    ) {
        let candidate = PasskeyCandidate {
            item_id: "item".into(),
            rp_id: rp.clone(),
            rp_name: rp_name.clone(),
            user_name: user_name.clone(),
            user_display_name: display.clone(),
            credential_id: credential_id.clone(),
            user_handle: handle.clone(),
            created_ms: 0,
        };
        let row = cache_row(&candidate);
        prop_assert_eq!(&row.credential_id, &credential_id);
        prop_assert_eq!(&row.user_id, &handle);
        prop_assert_eq!(&row.rp_id, &rp);
        prop_assert_eq!(&row.user_name, &user_name);
        prop_assert_eq!(&row.rp_name, if rp_name.is_empty() { &rp } else { &rp_name });
        prop_assert_eq!(&row.user_display_name, if display.is_empty() { &user_name } else { &display });
    }

    /// Windows hears the documented code for each way an operation ends, whatever the message.
    #[test]
    fn hresults_follow_the_documented_table(message in ".{0,40}", which in 0usize..8) {
        let error = match which {
            0 => RelayError::Cancelled,
            1 => RelayError::NotFound,
            2 => RelayError::NotPaired,
            3 => RelayError::Unreachable(message),
            4 => RelayError::Exists,
            5 => RelayError::Unsupported(message),
            6 => RelayError::Invalid(message),
            _ => RelayError::Other(message),
        };
        // No wildcard: a new `RelayError` variant must be classified here before this compiles.
        let expected = match &error {
            RelayError::Cancelled => NTE_USER_CANCELLED,
            RelayError::NotFound => NTE_NOT_FOUND,
            RelayError::NotPaired | RelayError::Unreachable(_) => NTE_DEVICE_NOT_FOUND,
            RelayError::Exists => NTE_EXISTS,
            RelayError::Unsupported(_) => NTE_NOT_SUPPORTED,
            RelayError::Invalid(_) => NTE_INVALID_PARAMETER,
            RelayError::Other(_) => E_FAIL,
        };
        let code = hresult_for(&error);
        prop_assert_eq!(code, expected);
        prop_assert!(code.is_err(), "an error must never map to a success code");
    }
}

/// Hostile RP ids by hand: each is refused before it can be glued into a URL.
#[test]
fn hostile_rp_ids_are_refused() {
    for id in [
        "",
        ".",
        "-example.com",
        "example.com.",
        "example.com-",
        "exa..mple.com",
        "example.com/",
        "example.com/path",
        "user@evil.com",
        "example.com:8443",
        "example.com#evil.com",
        "example.com?x=1",
        "example.com\\evil.com",
        "example.com evil.com",
        "example.com\u{0}",
        "example.com\n",
        "exam\u{202e}ple.com",
        "example\u{ff0e}com",
        "%65xample.com",
        "[::1]",
        "bücher.example",
    ] {
        assert!(origin_for(id).is_err(), "{id:?} was accepted");
    }
    assert!(origin_for(&"a".repeat(253)).is_ok());
    assert!(origin_for(&"a".repeat(254)).is_err());
    // Address look-alikes get through here and are the engine's to refuse: pinned so a change to
    // either side shows up.
    for id in ["127.0.0.1", "0x7f.1", "2130706433", "1.10", "localhost"] {
        let origin = origin_for(id).expect("host-like");
        if let Ok(parsed) = Origin::parse(&origin)
            && parsed.is_ip
        {
            assert!(
                validate_rp_id(id, &parsed).is_err(),
                "{id} is valid for {parsed:?}"
            );
        }
    }
}
