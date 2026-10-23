//! Property tests for the signed op format and the CBOR payloads that peers send (phase 5.1).
//! A paired device is trusted to be a member, not to be well behaved: its bytes reach
//! `SignedOp`'s deserializer, the roster decoder and the data-op decoder, and none of them may
//! panic or accept something the signature does not cover.

use ciborium::Value;
use lp_core::cbor;
use lp_core::crypto::SigningKey;
use lp_core::ids::VaultId;
use lp_core::model::{DataOp, RosterOp};
use lp_core::op::{HEADER_LEN, MAX_BODY_LEN, OpClass, OpHeader, SignedOp};
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

fn cbor_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(|n| Value::Integer(n.into())),
        vec(any::<u8>(), 0..40).prop_map(Value::Bytes),
        ".{0,20}".prop_map(Value::Text),
        any::<bool>().prop_map(Value::Bool),
        Just(Value::Null),
    ];
    leaf.prop_recursive(3, 30, 5, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..5).prop_map(Value::Array),
            vec((inner.clone(), inner), 0..5).prop_map(Value::Map),
        ]
    })
}

fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).unwrap();
    out
}

/// A map shaped like a payload: a tag, then fields with the right names but arbitrary values.
fn payload_like(
    tags: &'static [&'static str],
    fields: &'static [&'static str],
) -> impl Strategy<Value = Value> {
    (
        prop_oneof![
            proptest::sample::select(tags).prop_map(|t| Value::Text(t.to_string())),
            ".{0,8}".prop_map(Value::Text),
            cbor_value(),
        ],
        vec((proptest::sample::select(fields), cbor_value()), 0..6),
    )
        .prop_map(|(tag, fields)| {
            let mut entries = vec![(Value::Text("t".into()), tag)];
            entries.extend(
                fields
                    .into_iter()
                    .map(|(k, v)| (Value::Text(k.to_string()), v)),
            );
            Value::Map(entries)
        })
}

fn signed(seq: u64, class: OpClass, body: Vec<u8>) -> (SigningKey, SignedOp) {
    let sk = SigningKey::generate().unwrap();
    let op = SignedOp::create(VaultId::random().unwrap(), &sk, seq, class, body).unwrap();
    (sk, op)
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn op_header_decode_never_panics_and_round_trips(
        bytes in vec(any::<u8>(), 0..200),
        exact in vec(any::<u8>(), HEADER_LEN..=HEADER_LEN),
        version_ok in any::<bool>(),
        class in 0u8..4,
    ) {
        let _ = OpHeader::decode(&bytes);
        let mut exact = exact;
        if version_ok {
            exact[0] = 1;
        }
        exact[57] = class;
        if let Ok(header) = OpHeader::decode(&exact) {
            // Every byte of the header means something: what was read writes back identically.
            prop_assert_eq!(header.encode().to_vec(), exact.clone());
            prop_assert!(header.seq >= 1);
        }
    }

    #[test]
    fn signed_op_from_parts_checks_structure(
        header in vec(any::<u8>(), 0..120),
        sig in vec(any::<u8>(), 0..80),
        body in vec(any::<u8>(), 0..200),
    ) {
        if let Ok(op) = SignedOp::from_parts(&header, &sig, body.clone()) {
            prop_assert_eq!(op.body(), &body[..]);
            prop_assert_eq!(op.header().body_len as usize, body.len());
            prop_assert_eq!(op.header_bytes().as_slice(), &header[..]);
        }
    }

    /// Whatever bytes a peer sends, a parsed op that differs from the original does not verify.
    #[test]
    fn tampered_ops_never_verify(
        body in vec(any::<u8>(), 0..300),
        seq in 1u64..1000,
        class in prop_oneof![Just(OpClass::Data), Just(OpClass::Roster)],
        flips in vec((any::<prop::sample::Index>(), 1u8..=255), 1..4),
        extra in prop::option::of(vec(any::<u8>(), 0..6)),
        cut in prop::option::of(any::<prop::sample::Index>()),
    ) {
        let (_, op) = signed(seq, class, body);
        op.verify_signature().unwrap();
        let wire = cbor::to_vec(&op).unwrap();
        prop_assert_eq!(cbor::from_slice::<SignedOp>(&wire).unwrap(), op.clone());

        let mut broken = wire.clone();
        for (at, xor) in flips {
            let i = at.index(broken.len());
            broken[i] ^= xor;
        }
        if let Some(extra) = extra {
            broken.extend(extra);
        }
        if let Some(cut) = cut {
            broken.truncate(cut.index(broken.len() + 1));
        }
        if let Ok(parsed) = cbor::from_slice::<SignedOp>(&broken) {
            prop_assert!(parsed == op || parsed.verify_signature().is_err());
        }
    }

    #[test]
    fn op_sizes_are_capped(extra in 1usize..64) {
        let sk = SigningKey::generate().unwrap();
        let vault = VaultId::random().unwrap();
        prop_assert!(SignedOp::create(vault, &sk, 1, OpClass::Data, vec![0; MAX_BODY_LEN]).is_ok());
        prop_assert!(SignedOp::create(vault, &sk, 1, OpClass::Data, vec![0; MAX_BODY_LEN + extra]).is_err());
        prop_assert!(SignedOp::create(vault, &sk, 0, OpClass::Data, vec![]).is_err());
    }

    #[test]
    fn payload_decoders_never_panic(
        bytes in vec(any::<u8>(), 0..300),
        value in cbor_value(),
        data_like in payload_like(
            &["item", "purge", "folder", "devmeta", "future"],
            &["item", "hlc", "fields", "folder", "name", "device", "platform"],
        ),
        roster_like in payload_like(
            &["genesis", "add", "revoke", "future"],
            &["vault_id", "keys", "created_ms", "added_ms", "device", "cutoff_seq", "revoked_ms"],
        ),
    ) {
        let _ = cbor::from_slice::<DataOp>(&bytes);
        let _ = cbor::from_slice::<RosterOp>(&bytes);
        for v in [&value, &data_like, &roster_like] {
            let encoded = encode(v);
            let _ = cbor::from_slice::<DataOp>(&encoded);
            let _ = cbor::from_slice::<RosterOp>(&encoded);
            // Through the op wrappers as well: a roster op with a nonsense body is `Err` here and
            // `Unknown` in the replica; a data op needs the key and is never decoded by sync.
            let (_, roster) = signed(1, OpClass::Roster, encoded.clone());
            let _ = roster.decode_roster();
            prop_assert!(roster.decode_data(&lp_core::crypto::Key32::random().unwrap()).is_err());
        }
    }
}

/// The rule the replica relies on: a payload from a newer version is `Unknown`, not an error.
#[test]
fn unknown_variants_decode_to_unknown() {
    let future = |tag: &str| {
        encode(&Value::Map(vec![
            (Value::Text("t".into()), Value::Text(tag.into())),
            (Value::Text("x".into()), Value::Integer(1.into())),
        ]))
    };
    assert_eq!(
        cbor::from_slice::<DataOp>(&future("from-the-future")).unwrap(),
        DataOp::Unknown
    );
    assert!(matches!(
        cbor::from_slice::<RosterOp>(&future("from-the-future")).unwrap(),
        RosterOp::Unknown
    ));
}
