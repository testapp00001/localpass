//! Property tests for the CBOR, COSE key and authenticator data parsers (phase 5.1). They must
//! never panic or allocate by what a length field claims, and what the library writes it must
//! read back. (Today they are fed by the app's own data and by tests; they are also what a
//! relying-party-side check of our output would use, so strictness matters.)

use std::time::{Duration, Instant};

use ciborium::Value;
use lp_core::cbor;
use lp_core::passkey::{
    ALG_ES256, FLAG_AT, FLAG_BE, FLAG_BS, FLAG_ED, FLAG_UP, FLAG_UV, PasskeyKey, PasskeyPublic,
    authenticator_data, parse_authenticator_data, rp_id_hash,
};
use proptest::collection::vec;
use proptest::prelude::*;

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).expect("CBOR to a Vec");
    out
}

fn int(n: i64) -> Value {
    Value::Integer(n.into())
}

/// A COSE_Key map with these parameters, in the order the library writes them.
fn cose(kty: i64, alg: i64, crv: i64, x: &[u8], y: &[u8]) -> Value {
    Value::Map(vec![
        (int(1), int(kty)),
        (int(3), int(alg)),
        (int(-1), int(crv)),
        (int(-2), Value::Bytes(x.to_vec())),
        (int(-3), Value::Bytes(y.to_vec())),
    ])
}

fn some_public() -> PasskeyPublic {
    PasskeyKey::generate().unwrap().public()
}

/// Any CBOR value up to a small depth, with every item type including tags and floats.
fn cbor_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(|n| Value::Integer(n.into())),
        any::<u64>().prop_map(|n| Value::Integer(n.into())),
        vec(any::<u8>(), 0..40).prop_map(Value::Bytes),
        ".{0,20}".prop_map(Value::Text),
        any::<bool>().prop_map(Value::Bool),
        Just(Value::Null),
        any::<f64>().prop_map(Value::Float),
    ];
    leaf.prop_recursive(4, 40, 5, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..5).prop_map(Value::Array),
            vec((inner.clone(), inner.clone()), 0..5).prop_map(Value::Map),
            (any::<u64>(), inner).prop_map(|(tag, v)| Value::Tag(tag, Box::new(v))),
        ]
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn authdata_parse_never_panics(
        bytes in vec(any::<u8>(), 0..512),
        flags in any::<u8>(),
        id_len in prop_oneof![Just(0u16), Just(0xFFFF), Just(0x8000), any::<u16>()],
    ) {
        let _ = parse_authenticator_data(&bytes);
        // The same, with the attested-data flag forced on and the id length forced.
        let mut forced = bytes.clone();
        if forced.len() < 55 {
            forced.resize(55, 0);
        }
        forced[32] = flags | FLAG_AT;
        let _ = parse_authenticator_data(&forced);
        forced[53..55].copy_from_slice(&id_len.to_be_bytes());
        let _ = parse_authenticator_data(&forced);
        // And with a real key and a corrupted tail.
        let mut real = authenticator_data("example.com", flags & !FLAG_AT, 0, Some((&[7u8; 32], &some_public())));
        let cut = bytes.len() % real.len();
        real.truncate(cut);
        let _ = parse_authenticator_data(&real);
    }

    #[test]
    fn authdata_roundtrip(
        rp_id in any::<String>(),
        flags in any::<u8>(),
        sign_count in any::<u32>(),
        credential_id in vec(any::<u8>(), 0..=1024),
        attested in any::<bool>(),
    ) {
        let flags = flags & !FLAG_AT;
        let public = some_public();
        let data = authenticator_data(&rp_id, flags, sign_count, attested.then_some((credential_id.as_slice(), &public)));
        let parsed = parse_authenticator_data(&data).expect("what we write we can read");
        prop_assert_eq!(parsed.rp_id_hash, rp_id_hash(&rp_id));
        prop_assert_eq!(parsed.sign_count, sign_count);
        if attested {
            prop_assert_eq!(parsed.flags, flags | FLAG_AT);
            prop_assert_eq!(parsed.credential, Some((credential_id.clone(), public)));
        } else {
            prop_assert_eq!(parsed.flags, flags);
            prop_assert!(parsed.credential.is_none());
        }
        // Any byte appended without the extension flag is refused; with it, it is allowed.
        let mut longer = data.clone();
        longer.push(0xa0);
        prop_assert_eq!(parse_authenticator_data(&longer).is_ok(), flags & FLAG_ED != 0);
    }

    #[test]
    fn authdata_mutations_never_panic(
        flip in vec((any::<prop::sample::Index>(), any::<u8>()), 1..6),
        insert in prop::option::of((any::<prop::sample::Index>(), vec(any::<u8>(), 0..8))),
        cut in prop::option::of(any::<prop::sample::Index>()),
    ) {
        let mut data = authenticator_data(
            "example.com",
            FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS,
            0,
            Some((&[9u8; 32], &some_public())),
        );
        for (at, byte) in flip {
            let i = at.index(data.len());
            data[i] ^= byte;
        }
        if let Some((at, bytes)) = insert {
            let i = at.index(data.len() + 1);
            data.splice(i..i, bytes);
        }
        if let Some(at) = cut {
            data.truncate(at.index(data.len() + 1));
        }
        let _ = parse_authenticator_data(&data);
    }

    #[test]
    fn cose_roundtrip(x in any::<[u8; 32]>(), y in any::<[u8; 32]>()) {
        // Any 32+32 bytes are a well-formed key; whether it is on the curve is `verify`'s business.
        let public = PasskeyPublic { x, y };
        let bytes = public.to_cose();
        prop_assert_eq!(PasskeyPublic::from_cose(&bytes).ok(), Some(public.clone()));
        prop_assert_eq!(bytes.len(), 77);
        // Re-encoding what was read gives the same bytes.
        prop_assert_eq!(PasskeyPublic::from_cose(&bytes).unwrap().to_cose(), bytes);
        // An off-curve key (nearly always) cannot verify anything, and says so without panicking.
        let verdict = public.verify(b"message", &[0x30, 0x06, 0x02, 0x01, 0x01, 0x02, 0x01, 0x01]);
        prop_assert!(verdict.is_err());
        prop_assert_eq!(public.to_spki_der().len(), 91);
    }

    #[test]
    fn cose_rejects_wrong_parameters(
        kty in prop_oneof![Just(2i64), Just(1), Just(3), any::<i64>()],
        alg in prop_oneof![Just(-7i64), Just(-8), Just(-257), Just(-35), any::<i64>()],
        crv in prop_oneof![Just(1i64), Just(2), Just(6), any::<i64>()],
        x_len in prop_oneof![Just(32usize), Just(31), Just(33), Just(0), 0usize..80],
        y_len in prop_oneof![Just(32usize), Just(31), Just(33), Just(0), 0usize..80],
    ) {
        let x = vec![1u8; x_len];
        let y = vec![2u8; y_len];
        let result = PasskeyPublic::from_cose(&encode(&cose(kty, alg, crv, &x, &y)));
        let all_right = kty == 2 && alg == ALG_ES256 && crv == 1 && x_len == 32 && y_len == 32;
        prop_assert_eq!(result.is_ok(), all_right);
    }

    #[test]
    fn cose_keys_must_be_one_clean_item(
        value in cbor_value(),
        junk in vec(any::<u8>(), 1..16),
    ) {
        // Arbitrary CBOR is read or refused, never a panic; a map that is not a P-256 key is refused.
        let bytes = encode(&value);
        let _ = PasskeyPublic::from_cose(&bytes);
        prop_assert!(PasskeyPublic::from_cose(&bytes).is_err() || matches!(value, Value::Map(_)));

        // A real key followed by anything is refused.
        let mut key = some_public().to_cose();
        key.extend_from_slice(&junk);
        prop_assert!(PasskeyPublic::from_cose(&key).is_err());
        // Truncated keys are refused.
        let whole = some_public().to_cose();
        for cut in [0, 1, 5, 40, 76] {
            prop_assert!(PasskeyPublic::from_cose(&whole[..cut]).is_err());
        }
    }

    #[test]
    fn cose_labels_may_not_repeat_but_extras_are_accepted(extra_label in 4i64..50, repeat in 0usize..5) {
        let public = some_public();
        let Value::Map(mut entries) = cbor::from_slice::<Value>(&public.to_cose()).unwrap() else {
            unreachable!()
        };
        // Pinned: labels this code does not know are ignored (other COSE keys carry some).
        entries.push((int(extra_label), Value::Bytes(vec![1, 2, 3])));
        prop_assert_eq!(PasskeyPublic::from_cose(&encode(&Value::Map(entries.clone()))).ok(), Some(public.clone()));
        // Repeating any label, even with the same value, is refused.
        let (label, value) = entries[repeat].clone();
        entries.push((label, value));
        prop_assert!(PasskeyPublic::from_cose(&encode(&Value::Map(entries))).is_err());
    }

    #[test]
    fn cbor_values_never_panic(bytes in vec(any::<u8>(), 0..300), value in cbor_value()) {
        let _ = cbor::from_slice::<Value>(&bytes);
        let _ = cbor::from_slice::<Vec<u8>>(&bytes);
        let _ = cbor::from_slice::<(String, u64)>(&bytes);
        let encoded = encode(&value);
        let decoded = cbor::from_slice::<Value>(&encoded);
        prop_assert!(decoded.is_ok(), "{:?}", value);
        if !has_nan_or_tag(&value) {
            prop_assert_eq!(decoded.ok(), Some(value.clone()));
        }
        // Cut anywhere: an error, never a panic.
        for cut in 0..encoded.len().min(40) {
            let _ = cbor::from_slice::<Value>(&encoded[..cut]);
        }
    }

    #[test]
    fn pkcs8_garbage_is_error(bytes in vec(any::<u8>(), 0..200), flip in any::<prop::sample::Index>()) {
        prop_assert!(PasskeyKey::from_pkcs8_der(&bytes).is_err());
        // A real key with a byte changed either fails or is some other key, never a panic.
        let der = PasskeyKey::generate().unwrap().to_pkcs8_der().unwrap();
        let mut broken = der.to_vec();
        let i = flip.index(broken.len());
        broken[i] ^= 0xff;
        if let Ok(key) = PasskeyKey::from_pkcs8_der(&broken) {
            let _ = key.public().to_cose();
        }
        prop_assert!(PasskeyKey::from_pkcs8_der(&der[..der.len() - 1]).is_err());
    }
}

/// NaN is not equal to itself, and the library folds some tags (big numbers) into integers.
fn has_nan_or_tag(value: &Value) -> bool {
    match value {
        Value::Float(f) => f.is_nan(),
        Value::Array(items) => items.iter().any(has_nan_or_tag),
        Value::Map(entries) => entries
            .iter()
            .any(|(k, v)| has_nan_or_tag(k) || has_nan_or_tag(v)),
        Value::Tag(..) => true,
        _ => false,
    }
}

/// Data shapes that claim far more than they carry, or nest without end.
fn bombs() -> Vec<(&'static str, Vec<u8>)> {
    let max = [0xff; 8];
    let with = |head: u8, rest: &[u8]| [&[head][..], rest].concat();
    vec![
        ("array of u64::MAX items", with(0x9b, &max)),
        ("map of u64::MAX pairs", with(0xbb, &max)),
        ("byte string of u64::MAX", with(0x5b, &max)),
        ("text string of u64::MAX", with(0x7b, &max)),
        ("byte string of 1 GiB", vec![0x5a, 0x40, 0, 0, 0]),
        ("text string of 2 GiB", vec![0x7a, 0x80, 0, 0, 0]),
        ("array of 4 billion", vec![0x9a, 0xff, 0xff, 0xff, 0xff]),
        (
            "100k nested arrays",
            [vec![0x81; 100_000], vec![0x00]].concat(),
        ),
        (
            "100k nested maps",
            [vec![0xa1; 100_000], vec![0x00]].concat(),
        ),
        (
            "100k nested tags",
            [vec![0xc1; 100_000], vec![0x00]].concat(),
        ),
        ("100k nested indefinite arrays", vec![0x9f; 100_000]),
        (
            "1M items in an indefinite array",
            [vec![0x9f], vec![0x00; 1_000_000]].concat(),
        ),
        (
            "1M chunks of an indefinite byte string",
            [vec![0x5f], [0x41, 0x00].repeat(500_000)].concat(),
        ),
        (
            "indefinite text with a bad chunk",
            vec![0x7f, 0x41, 0x00, 0xff],
        ),
        ("reserved additional info", vec![0x1c]),
        ("break outside a container", vec![0xff]),
        ("a lone tag", vec![0xc1]),
        ("empty input", vec![]),
    ]
}

#[test]
fn cbor_length_bombs_fail_fast() {
    let started = Instant::now();
    for (name, bytes) in bombs() {
        let one = Instant::now();
        // Not every one is an error (a million zeros in an unterminated array is just unfinished),
        // but each one must come back, quickly, without the memory a length field asks for.
        assert!(
            cbor::from_slice::<Value>(&bytes).is_err(),
            "{name}: accepted"
        );
        assert!(
            cbor::from_slice::<Vec<u8>>(&bytes).is_err(),
            "{name}: accepted as bytes"
        );
        assert!(
            PasskeyPublic::from_cose(&bytes).is_err(),
            "{name}: accepted as a key"
        );
        // Behind a valid attested-data header, so the parser reaches the CBOR.
        let mut data = authenticator_data("example.com", FLAG_AT, 0, None);
        data.resize(37, 0);
        data.extend_from_slice(&[0u8; 16]);
        data.extend_from_slice(&0u16.to_be_bytes());
        data.extend_from_slice(&bytes);
        assert!(
            parse_authenticator_data(&data).is_err(),
            "{name}: accepted in authenticator data"
        );
        assert!(
            one.elapsed() < Duration::from_secs(5),
            "{name}: {:?}",
            one.elapsed()
        );
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "all bombs: {:?}",
        started.elapsed()
    );
}

#[test]
fn cbor_depth_bomb_fails() {
    // Deeper than the recursion limit, but a valid finish: the decoder must refuse, not overflow
    // the stack.
    let depth = 200_000;
    for opener in [0x81u8, 0xa1, 0xc1, 0x9f] {
        let mut bytes = vec![opener; depth];
        bytes.push(0x00);
        bytes.extend(std::iter::repeat_n(
            0xff,
            if opener == 0x9f { depth } else { 0 },
        ));
        assert!(
            cbor::from_slice::<Value>(&bytes).is_err(),
            "opener {opener:#x}"
        );
    }
}

#[test]
fn what_a_real_authenticator_wrote_still_reads() {
    // A fixed COSE key and attested data, so a change to the strictness above cannot silently
    // reject the library's own output.
    let public = PasskeyPublic {
        x: [3; 32],
        y: [4; 32],
    };
    let data = authenticator_data(
        "example.com",
        FLAG_UP | FLAG_UV | FLAG_BE | FLAG_BS,
        0,
        Some((b"id", &public)),
    );
    let parsed = parse_authenticator_data(&data).unwrap();
    assert_eq!(parsed.credential, Some((b"id".to_vec(), public)));
}
