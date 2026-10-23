//! Property tests for the broker's JSON protocol (phase 5.1): whatever a local process or a
//! page-driven extension sends, parsing never panics, ids are strictly `u64`, and the table of
//! what a provider-kind client may do cannot silently grow when a request is added.

use lp_broker::protocol::{
    ClientMessage, Request, RequestEnvelope, ResponseEnvelope, ServerMessage,
};
use proptest::collection::vec;
use proptest::prelude::*;
use serde_json::{Map, Value, json};

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

/// Any JSON value up to depth 6, with awkward numbers and strings.
fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        any::<u64>().prop_map(Value::from),
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::from),
        ".{0,24}".prop_map(Value::from),
    ];
    leaf.prop_recursive(6, 64, 6, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..6).prop_map(Value::Array),
            vec(("[a-zA-Z]{1,8}", inner), 0..6)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// One minimal valid message per request. The list is checked against [`classify`], whose match
/// has no wildcard, so adding a request without classifying it here does not compile.
fn known_requests() -> Vec<(&'static str, Value)> {
    let origin = "https://example.com";
    vec![
        ("status", json!({"op": "status"})),
        ("unlock", json!({"op": "unlock"})),
        ("lock", json!({"op": "lock"})),
        ("findLogins", json!({"op": "findLogins", "origin": origin})),
        ("searchItems", json!({"op": "searchItems", "query": "x"})),
        (
            "fillLogin",
            json!({"op": "fillLogin", "itemId": "i", "origin": origin}),
        ),
        (
            "saveLogin",
            json!({"op": "saveLogin", "origin": origin, "username": "u", "password": "p"}),
        ),
        ("generatePassword", json!({"op": "generatePassword"})),
        (
            "passkeyCandidates",
            json!({"op": "passkeyCandidates", "origin": origin, "rpId": "example.com"}),
        ),
        ("passkeyListAll", json!({"op": "passkeyListAll"})),
        (
            "passkeyCreate",
            json!({"op": "passkeyCreate", "origin": origin, "rpId": "example.com",
                   "userHandle": "dXNlcg", "userName": "alice", "algorithms": [-7]}),
        ),
        (
            "passkeyAssert",
            json!({"op": "passkeyAssert", "origin": origin, "rpId": "example.com",
                   "itemId": "i", "clientDataHash": "AAAA"}),
        ),
        (
            "passkeyAssertAny",
            json!({"op": "passkeyAssertAny", "origin": origin, "rpId": "example.com",
                   "clientDataHash": "AAAA"}),
        ),
    ]
}

/// (ordinal, provider may use it, only a provider may use it, may ask the user first). No wildcard.
fn classify(request: &Request) -> (usize, bool, bool, bool) {
    match request {
        Request::Status => (0, true, false, false),
        Request::Unlock => (1, true, false, false),
        Request::Lock => (2, false, false, false),
        Request::FindLogins { .. } => (3, false, false, false),
        Request::SearchItems { .. } => (4, false, false, false),
        Request::FillLogin { .. } => (5, false, false, true),
        Request::SaveLogin { .. } => (6, false, false, true),
        Request::GeneratePassword { .. } => (7, false, false, false),
        Request::PasskeyCandidates { .. } => (8, true, false, false),
        Request::PasskeyListAll => (9, true, true, false),
        Request::PasskeyCreate { .. } => (10, true, false, true),
        Request::PasskeyAssert { .. } => (11, true, false, true),
        Request::PasskeyAssertAny { .. } => (12, true, true, true),
    }
}

const REQUEST_KINDS: usize = 13;

#[test]
fn provider_allow_list_is_exhaustive() {
    let table = known_requests();
    assert_eq!(table.len(), REQUEST_KINDS);
    let mut seen = [false; REQUEST_KINDS];
    for (name, message) in table {
        let envelope: RequestEnvelope =
            serde_json::from_value(with_id(message, 1)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (ordinal, provider, provider_only, interactive) = classify(&envelope.request);
        assert!(!seen[ordinal], "{name}: two table entries for one request");
        seen[ordinal] = true;
        let r = &envelope.request;
        assert_eq!(r.allowed_for_provider(), provider, "{name}: provider");
        assert_eq!(r.provider_only(), provider_only, "{name}: provider only");
        assert_eq!(r.is_interactive(), interactive, "{name}: interactive");
        assert!(
            !provider_only || provider,
            "{name}: provider-only implies allowed"
        );
    }
    assert!(
        seen.iter().all(|s| *s),
        "a request kind is missing from the table"
    );
}

fn with_id(mut message: Value, id: u64) -> Value {
    message["id"] = json!(id);
    message
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn client_message_parse_never_panics(bytes in vec(any::<u8>(), 0..256)) {
        let _ = serde_json::from_slice::<ClientMessage>(&bytes);
        let _ = serde_json::from_slice::<ServerMessage>(&bytes);
        let _ = serde_json::from_slice::<RequestEnvelope>(&bytes);
        let _ = serde_json::from_slice::<ResponseEnvelope>(&bytes);
    }

    #[test]
    fn json_values_never_panic_any_message_type(value in json_value()) {
        let text = value.to_string();
        let _ = serde_json::from_str::<ClientMessage>(&text);
        let _ = serde_json::from_str::<ServerMessage>(&text);
        let _ = serde_json::from_str::<RequestEnvelope>(&text);
        let _ = serde_json::from_str::<ResponseEnvelope>(&text);
        let _ = serde_json::from_value::<RequestEnvelope>(value);
    }

    /// Known operations with every field replaced by arbitrary JSON: either a clean error, or
    /// a request whose permissions are exactly the table's.
    #[test]
    fn request_envelope_parse_never_panics(
        index in 0..REQUEST_KINDS,
        id in json_value(),
        fields in vec(("[a-zA-Z]{1,12}", json_value()), 0..6),
        keep_known in any::<bool>(),
    ) {
        let (_, base) = known_requests().swap_remove(index);
        let mut message = if keep_known { base } else { json!({"op": base["op"]}) };
        for (key, value) in fields {
            message[key] = value;
        }
        message["id"] = id;
        if let Ok(envelope) = serde_json::from_value::<RequestEnvelope>(message.clone()) {
            let (_, provider, provider_only, interactive) = classify(&envelope.request);
            prop_assert_eq!(envelope.request.allowed_for_provider(), provider);
            prop_assert_eq!(envelope.request.provider_only(), provider_only);
            prop_assert_eq!(envelope.request.is_interactive(), interactive);
            prop_assert!(message["id"].is_u64(), "an id that is not a u64 was accepted");
        }
    }

    #[test]
    fn request_ids_must_be_u64(
        id in prop_oneof![
            any::<i64>().prop_filter("negative", |n| *n < 0).prop_map(Value::from),
            any::<f64>().prop_filter("finite", |f| f.is_finite()).prop_map(Value::from),
            ".{0,12}".prop_map(Value::from),
            any::<bool>().prop_map(Value::from),
            Just(Value::Null),
            Just(json!([1])),
            Just(json!({"n": 1})),
        ],
    ) {
        let message = json!({"id": id, "op": "status"});
        let parsed = serde_json::from_value::<RequestEnvelope>(message);
        prop_assert!(parsed.is_err());
        // Too big for u64, spelled as a number.
        let big = r#"{"id":18446744073709551616,"op":"status"}"#;
        let too_big = serde_json::from_str::<RequestEnvelope>(big);
        prop_assert!(too_big.is_err());
        // Missing.
        let missing = serde_json::from_str::<RequestEnvelope>(r#"{"op":"status"}"#);
        prop_assert!(missing.is_err());
    }

    #[test]
    fn every_u64_id_is_kept(id in any::<u64>()) {
        let text = format!(r#"{{"id":{id},"op":"status"}}"#);
        let envelope: RequestEnvelope = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(envelope.id, id);
    }

    #[test]
    fn unknown_operations_are_rejected(op in "[a-zA-Z]{1,16}") {
        prop_assume!(!known_requests().iter().any(|(name, _)| *name == op));
        let request = serde_json::from_value::<RequestEnvelope>(json!({"id": 1, "op": op}));
        prop_assert!(request.is_err());
        let hello = serde_json::from_value::<ClientMessage>(json!({"type": op, "v": 1, "name": "x"}));
        prop_assert!(hello.is_err());
    }

    #[test]
    fn hello_fields_have_strict_types(
        v in json_value(),
        name in json_value(),
        kind in json_value(),
        client_id in json_value(),
    ) {
        let message = json!({"type": "hello", "v": v, "name": name, "kind": kind, "clientId": client_id});
        if let Ok(ClientMessage::Hello { v: parsed, name: shown, .. }) =
            serde_json::from_value::<ClientMessage>(message.clone())
        {
            prop_assert!(message["v"].is_u64() && u64::from(parsed) == message["v"].as_u64().unwrap());
            prop_assert_eq!(message["name"].as_str(), Some(shown.as_str()));
        }
    }
}

#[test]
fn deeply_nested_json_is_rejected() {
    // 10 000 levels, as the whole message and inside a field the parser ignores or reads.
    let depth = 10_000;
    let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let deep_object = format!("{}1{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
    for text in [
        nested.clone(),
        deep_object.clone(),
        format!(r#"{{"id":1,"op":"status","x":{nested}}}"#),
        format!(r#"{{"id":1,"op":"searchItems","query":{nested}}}"#),
        format!(r#"{{"id":1,"op":"generatePassword","options":{deep_object}}}"#),
        format!(r#"{{"type":"hello","v":1,"name":{nested}}}"#),
    ] {
        assert!(serde_json::from_str::<RequestEnvelope>(&text).is_err());
        assert!(serde_json::from_str::<ClientMessage>(&text).is_err());
        assert!(serde_json::from_str::<ServerMessage>(&text).is_err());
        assert!(serde_json::from_str::<ResponseEnvelope>(&text).is_err());
    }
}

#[test]
fn huge_strings_and_lists_parse_without_trouble() {
    // 900 KB of text in a field, and a 100k-entry list: no panic, no pathological time.
    let started = std::time::Instant::now();
    let text = "a".repeat(900_000);
    let message = format!(r#"{{"id":1,"op":"searchItems","query":"{text}"}}"#);
    assert!(serde_json::from_str::<RequestEnvelope>(&message).is_ok());
    let allow = vec!["\"AAAA\""; 100_000].join(",");
    let message = format!(
        r#"{{"id":2,"op":"passkeyCandidates","origin":"https://example.com","rpId":"example.com","allow":[{allow}]}}"#
    );
    assert!(serde_json::from_str::<RequestEnvelope>(&message).is_ok());
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
}
