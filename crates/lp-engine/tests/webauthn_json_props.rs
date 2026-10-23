//! Property tests for the WebAuthn JSON that Android's browsers hand to the credential provider
//! (phase 5.1). The JSON is written by a web page, so every field can be any type or size.

use std::time::{Duration, Instant};

use lp_core::origin::{Origin, validate_rp_id};
use lp_core::passkey::b64u;
use lp_engine::EngineError;
use lp_engine::webauthn_json::{parse_create_options, parse_get_options};
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
        "[A-Za-z0-9_=+/-]{0,24}".prop_map(Value::from),
    ];
    leaf.prop_recursive(5, 48, 5, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..5).prop_map(Value::Array),
            vec(("[a-zA-Z]{1,10}", inner), 0..5)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

fn create_json() -> Value {
    json!({
        "challenge": "dGVzdC1jaGFsbGVuZ2U",
        "rp": {"name": "Example", "id": "example.com"},
        "user": {"id": "dXNlci0x", "name": "alice@example.com", "displayName": "Alice"},
        "pubKeyCredParams": [{"type": "public-key", "alg": -7}, {"type": "public-key", "alg": -257}],
        "excludeCredentials": [{"type": "public-key", "id": "AQID", "transports": ["internal"]}],
        "authenticatorSelection": {"residentKey": "required", "userVerification": "preferred"},
        "attestation": "none"
    })
}

fn get_json() -> Value {
    json!({
        "challenge": "dGVzdC1jaGFsbGVuZ2U",
        "rpId": "example.com",
        "allowCredentials": [{"type": "public-key", "id": "AQID"}, {"type": "public-key", "id": "BAUG"}],
        "userVerification": "preferred"
    })
}

/// Every path in the creation options that the parser reads.
const CREATE_PATHS: &[&[&str]] = &[
    &["rp"],
    &["rp", "id"],
    &["rp", "name"],
    &["user"],
    &["user", "id"],
    &["user", "name"],
    &["user", "displayName"],
    &["pubKeyCredParams"],
    &["pubKeyCredParams", "0"],
    &["pubKeyCredParams", "0", "alg"],
    &["excludeCredentials"],
    &["excludeCredentials", "0"],
    &["excludeCredentials", "0", "id"],
    &["challenge"],
];

const GET_PATHS: &[&[&str]] = &[
    &["rpId"],
    &["allowCredentials"],
    &["allowCredentials", "0"],
    &["allowCredentials", "0", "id"],
    &["allowCredentials", "1", "id"],
    &["challenge"],
];

/// Replaces (or removes) the value at `path`; a path that does not exist is left alone.
fn set(root: &mut Value, path: &[&str], replacement: Option<Value>) {
    let (last, parents) = path.split_last().expect("a path");
    let mut node = root;
    for part in parents {
        let next = match part.parse::<usize>() {
            Ok(index) => node.get_mut(index),
            Err(_) => node.get_mut(*part),
        };
        match next {
            Some(next) => node = next,
            None => return,
        }
    }
    match (last.parse::<usize>(), replacement) {
        (Ok(index), Some(value)) => {
            if let Some(slot) = node.get_mut(index) {
                *slot = value;
            }
        }
        (Ok(index), None) => {
            if let Some(items) = node.as_array_mut()
                && index < items.len()
            {
                items.remove(index);
            }
        }
        (Err(_), Some(value)) => {
            if let Some(slot) = node.get_mut(*last) {
                *slot = value;
            }
        }
        (Err(_), None) => {
            if let Some(object) = node.as_object_mut() {
                object.remove(*last);
            }
        }
    }
}

fn is_invalid<T>(result: &Result<T, EngineError>) -> bool {
    matches!(result, Err(EngineError::Invalid(_)))
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn create_options_never_panic(
        text in any::<String>(),
        value in json_value(),
        edits in vec((0..CREATE_PATHS.len(), prop::option::of(json_value())), 1..4),
        default_rp in "[a-z.]{0,20}",
    ) {
        let outcomes = [
            parse_create_options(&text, &default_rp),
            parse_create_options(&value.to_string(), &default_rp),
        ];
        for outcome in &outcomes {
            prop_assert!(outcome.is_ok() || is_invalid(outcome), "{:?}", outcome);
        }
        // Wrong types and missing fields at every place the parser looks.
        let mut options = create_json();
        for (path, replacement) in edits {
            set(&mut options, CREATE_PATHS[path], replacement);
        }
        let parsed = parse_create_options(&options.to_string(), &default_rp);
        prop_assert!(parsed.is_ok() || is_invalid(&parsed), "{:?}", parsed);
        if let Ok(parsed) = parsed {
            prop_assert!(parsed.exclude.len() <= 256);
            // serde also reads a JSON array as a struct, field by field; the checks below are
            // about the documented object form.
            if options["rp"].is_object() || options.get("rp").is_none() {
                // The relying party id is the given one, or the origin's host.
                let given = options["rp"]["id"].as_str().unwrap_or("");
                let expected = if given.is_empty() { default_rp.clone() } else { given.to_string() };
                prop_assert_eq!(parsed.rp_id, expected);
            }
            if options["user"].is_object() {
                let handle = options["user"]["id"].as_str().unwrap_or("");
                prop_assert_eq!(b64u::encode(&parsed.user_handle), handle.trim_end_matches('='));
            }
            if let Some(params) = options["pubKeyCredParams"].as_array() {
                prop_assert_eq!(parsed.algorithms.len(), params.len());
            }
        }
    }

    #[test]
    fn get_options_never_panic(
        text in any::<String>(),
        value in json_value(),
        edits in vec((0..GET_PATHS.len(), prop::option::of(json_value())), 1..4),
        default_rp in "[a-z.]{0,20}",
    ) {
        for outcome in [parse_get_options(&text, &default_rp), parse_get_options(&value.to_string(), &default_rp)] {
            prop_assert!(outcome.is_ok() || is_invalid(&outcome), "{:?}", outcome);
        }
        let mut options = get_json();
        for (path, replacement) in edits {
            set(&mut options, GET_PATHS[path], replacement);
        }
        let parsed = parse_get_options(&options.to_string(), &default_rp);
        prop_assert!(parsed.is_ok() || is_invalid(&parsed), "{:?}", parsed);
        if let Ok(parsed) = parsed {
            let given = options["rpId"].as_str().unwrap_or("");
            let expected = if given.is_empty() { default_rp.clone() } else { given.to_string() };
            prop_assert_eq!(parsed.rp_id, expected);
            prop_assert!(parsed.allow.len() <= 256);
        }
    }

    /// Credential ids are base64url (padding tolerated), or the request is `Invalid`.
    #[test]
    fn base64url_edge_cases_in_ids(id in "[A-Za-z0-9_=+/ \u{e9}-]{0,40}") {
        let options = json!({"allowCredentials": [{"id": id}]}).to_string();
        let parsed = parse_get_options(&options, "example.com");
        match b64u::decode(&id) {
            Ok(bytes) => prop_assert_eq!(parsed.unwrap().allow, vec![bytes]),
            Err(_) => prop_assert!(is_invalid(&parsed)),
        }
    }

    /// A hostile relying party id passes through the parser untouched (it is only a translation)
    /// and is then refused by the rule every caller applies next.
    #[test]
    fn hostile_rp_id_is_refused_downstream(
        rp_id in prop_oneof![
            Just("EXAMPLE.com".to_string()), Just("example.com.".to_string()), Just("exаmple.com".to_string()),
            Just("192.168.1.10".to_string()), Just("1.10".to_string()), Just("com".to_string()),
            Just("co.uk".to_string()), Just(" example.com".to_string()), Just("example.com/".to_string()),
            Just("example.com:443".to_string()), Just("https://example.com".to_string()),
            Just("evil.test".to_string()), Just("*.example.com".to_string()), Just("a\0b.example.com".to_string()),
            "[^a-z]{1,12}",
        ],
    ) {
        for origin in ["https://login.example.com", "https://192.168.1.10", "https://x.co.uk"] {
            let origin = Origin::parse(origin).unwrap();
            let create = json!({"rp": {"id": rp_id}, "user": {"id": "AQ"}}).to_string();
            let get = json!({"rpId": rp_id}).to_string();
            let from_create = parse_create_options(&create, &origin.host).unwrap().rp_id;
            let from_get = parse_get_options(&get, &origin.host).unwrap().rp_id;
            prop_assert_eq!(&from_create, &rp_id);
            prop_assert_eq!(&from_get, &rp_id);
            let verdict = validate_rp_id(&rp_id, &origin);
            let plausible = origin.host == rp_id || origin.host.ends_with(&format!(".{rp_id}"));
            if verdict.is_ok() {
                // Only a real suffix of the host on a label boundary, as WebAuthn says.
                prop_assert!(plausible && !origin.is_ip, "{} accepted for {}", rp_id, origin.host);
            }
        }
    }
}

#[test]
fn huge_lists_finish_fast() {
    let started = Instant::now();
    let entry = r#"{"type":"public-key","id":"AQID"}"#;
    let list = |n: usize| vec![entry; n].join(",");
    for n in [257, 10_000, 100_000] {
        let get = format!(r#"{{"allowCredentials":[{}]}}"#, list(n));
        assert!(
            is_invalid(&parse_get_options(&get, "example.com")),
            "{n} entries"
        );
        let create = format!(
            r#"{{"user":{{"id":"AQ"}},"excludeCredentials":[{}]}}"#,
            list(n)
        );
        assert!(
            is_invalid(&parse_create_options(&create, "example.com")),
            "{n} entries"
        );
        let params = vec![r#"{"type":"public-key","alg":-7}"#; n].join(",");
        let create = format!(r#"{{"user":{{"id":"AQ"}},"pubKeyCredParams":[{params}]}}"#);
        // The algorithm list has no cap of its own, but it costs only what the input costs.
        let _ = parse_create_options(&create, "example.com");
    }
    // The biggest id and text the input can carry: no panic and no blow-up.
    let id = "A".repeat(1 << 20);
    let get = format!(r#"{{"allowCredentials":[{{"id":"{id}"}}]}}"#);
    let parsed = parse_get_options(&get, "example.com").unwrap();
    assert_eq!(parsed.allow[0].len(), (1 << 20) / 4 * 3);
    let name = "n".repeat(1 << 20);
    let create = format!(r#"{{"rp":{{"name":"{name}"}},"user":{{"id":"AQ","name":"{name}"}}}}"#);
    let parsed = parse_create_options(&create, "example.com").unwrap();
    assert_eq!(parsed.rp_name.len(), 1 << 20);
    // The all-pairs check of a long exclude list against a long credential list is the engine's:
    // the lists were capped above.
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn deeply_nested_options_are_invalid() {
    let depth = 5_000;
    let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let object = format!("{}1{}", r#"{"a":"#.repeat(depth), "}".repeat(depth));
    for deep in [&nested, &object] {
        // Read as one of the fields the parser knows: past the recursion limit, so `Invalid`.
        // Except `{"rp": <deep object>}`: its only key, "a", is not an `rp` field, so the value
        // under it is skipped without recursing and the options parse (see the next loop).
        for (n, text) in [
            format!(r#"{{"rp":{deep}}}"#),
            format!(r#"{{"rp":{{"id":{deep}}}}}"#),
            format!(r#"{{"user":{{"id":"AQ","name":{deep}}}}}"#),
            format!(r#"{{"excludeCredentials":[{{"id":{deep}}}]}}"#),
        ]
        .into_iter()
        .enumerate()
        {
            let head = &text[..text.len().min(40)];
            let create = parse_create_options(&text, "example.com");
            if n == 0 && deep == &object {
                assert!(
                    create.is_ok() || is_invalid(&create),
                    "create {head}: {create:?}"
                );
            } else {
                assert!(is_invalid(&create), "create {head}: {create:?}");
            }
            let get = parse_get_options(&text, "example.com");
            assert!(is_invalid(&get) || get.is_ok(), "get {head}: {get:?}");
        }
        // Under a field nobody reads, serde_json skips the value without recursing (no stack use
        // and no allocation): either answer is fine, a crash is not.
        for text in [
            deep.clone(),
            format!(r#"{{"unknown":{deep}}}"#),
            format!(r#"{{"allowCredentials":[{{"id":"AQ","transports":{deep}}}]}}"#),
        ] {
            let create = parse_create_options(&text, "example.com");
            assert!(create.is_ok() || is_invalid(&create));
            let get = parse_get_options(&text, "example.com");
            assert!(get.is_ok() || is_invalid(&get));
        }
    }
}

#[test]
fn type_confusion_table() {
    let get = |options: Value| parse_get_options(&options.to_string(), "example.com");
    let create = |options: Value| parse_create_options(&options.to_string(), "example.com");
    // Wrong types are errors, not silently ignored.
    assert!(is_invalid(&get(json!({"rpId": 5}))));
    assert!(is_invalid(&get(json!({"allowCredentials": {}}))));
    assert!(is_invalid(&get(json!({"allowCredentials": [5]}))));
    assert!(is_invalid(&get(json!({"allowCredentials": [{"id": 5}]}))));
    assert!(is_invalid(&get(json!("text"))));
    assert!(is_invalid(&get(json!(5))));
    assert!(is_invalid(&get(json!(null))));
    assert!(is_invalid(&create(
        json!({"user": {"id": "AQ"}, "pubKeyCredParams": [{"alg": 1.5}]})
    )));
    assert!(is_invalid(&create(
        json!({"user": {"id": "AQ"}, "pubKeyCredParams": [{"alg": "-7"}]})
    )));
    assert!(is_invalid(&create(
        json!({"user": {"id": "AQ"}, "pubKeyCredParams": [{"alg": u64::MAX}]})
    )));
    assert!(is_invalid(&create(json!({"user": {"id": 5}}))));
    assert!(is_invalid(&create(json!({"user": {"id": "***"}}))));
    assert!(is_invalid(&create(json!({"rp": 5}))));
    assert!(is_invalid(&create(json!({"rp": "example.com"}))));
    assert!(is_invalid(&create(json!({"rp": null}))));
    // `null` is the same as missing for the optional pieces, an empty list is empty.
    assert!(
        get(json!({"rpId": null, "allowCredentials": []}))
            .unwrap()
            .allow
            .is_empty()
    );
    assert_eq!(get(json!({})).unwrap().rp_id, "example.com");
    assert_eq!(get(json!({"rpId": ""})).unwrap().rp_id, "example.com");
    // Duplicate keys: the last one wins and nothing breaks.
    let parsed = parse_get_options(r#"{"rpId":"a.test","rpId":"example.com"}"#, "x.test");
    assert!(parsed.is_ok() || is_invalid(&parsed));
}
