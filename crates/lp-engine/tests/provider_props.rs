//! Property tests for the Android JSON entry point, `lp_engine::provider::call` (phase 5.1).
//! Kotlin builds the request, but some of its content comes from other apps and from web pages
//! (origins, WebAuthn options), so the entry point must answer anything with one JSON object, keep
//! a locked vault shut, and never repeat a password back.

use std::path::Path;
use std::sync::{Arc, OnceLock};

use lp_core::crypto::{KdfParams, sha256};
use lp_core::origin::Origin;
use lp_core::passkey::b64u;
use lp_engine::{Engine, EngineConfig, FileSecretStore, ItemInput, provider};
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

const ORIGIN: &str = "https://login.example.com";
const PASSWORD: &str = "master pw";

fn engine(dir: &Path) -> Engine {
    let mut c = EngineConfig::new(dir, "test");
    c.kdf = KdfParams::insecure_for_tests();
    Engine::open(c, Arc::new(FileSecretStore::new(dir.join("device.key")))).unwrap()
}

fn creation_options(rp: &str, user: &str) -> String {
    json!({
        "challenge": b64u::encode(b"challenge"),
        "rp": { "id": rp, "name": "Example" },
        "user": { "id": b64u::encode(user.as_bytes()), "name": user, "displayName": user },
        "pubKeyCredParams": [{ "type": "public-key", "alg": -7 }],
    })
    .to_string()
}

fn request_options(rp: &str) -> String {
    json!({ "challenge": b64u::encode(b"challenge"), "rpId": rp }).to_string()
}

/// Two engines made once (a vault costs a key derivation): one locked, one unlocked and holding
/// a login, a passkey, a trashed passkey and a passkey for another site.
struct Fixture {
    _dirs: Vec<tempfile::TempDir>,
    locked: Engine,
    unlocked: Engine,
    good_passkey: String,
    login: String,
    trashed_passkey: String,
    other_site_passkey: String,
}

fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let (d1, d2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let locked = engine(d1.path());
        locked.create_vault(PASSWORD, "Phone").unwrap();
        locked.lock();
        let unlocked = engine(d2.path());
        unlocked.create_vault(PASSWORD, "Phone").unwrap();
        let create = |origin: &str, rp: &str, user: &str| -> String {
            let reply = call_str(
                &unlocked,
                &json!({"op": "passkeyCreate", "origin": origin,
                        "optionsJson": creation_options(rp, user), "userVerified": true})
                .to_string(),
            );
            reply["ok"]["itemId"]
                .as_str()
                .expect("passkey created")
                .to_string()
        };
        let good_passkey = create(ORIGIN, "example.com", "alice");
        let trashed_passkey = create(ORIGIN, "example.com", "bob");
        unlocked.trash_item(&trashed_passkey).unwrap();
        let other_site_passkey = create("https://other.test", "other.test", "carol");
        let login = unlocked
            .save_item(&ItemInput {
                title: "Example".into(),
                username: "alice".into(),
                password: "s3cret".into(),
                urls: vec!["https://example.com".into()],
                ..Default::default()
            })
            .unwrap();
        Fixture {
            _dirs: vec![d1, d2],
            locked,
            unlocked,
            good_passkey,
            login,
            trashed_passkey,
            other_site_passkey,
        }
    })
}

fn call_str(e: &Engine, request: &str) -> Value {
    let text = provider::call(e, request);
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("reply is not JSON ({err}): {text}"))
}

const CODES: &[&str] = &[
    "locked",
    "noVault",
    "wrongPassword",
    "notFound",
    "invalid",
    "unsupported",
    "passkeyExists",
    "error",
];

/// A reply is an object with exactly one of `ok` and `error`, and an error has a known code and a
/// message.
fn check_shape(reply: &Value) -> Result<(), TestCaseError> {
    let object = reply
        .as_object()
        .ok_or_else(|| TestCaseError::fail("not an object"))?;
    prop_assert_eq!(object.len(), 1, "{}", reply);
    if let Some(error) = object.get("error") {
        let code = error["code"].as_str().unwrap_or("");
        prop_assert!(CODES.contains(&code), "unknown code in {}", reply);
        prop_assert!(error["message"].is_string());
    } else {
        prop_assert!(object.contains_key("ok"));
    }
    Ok(())
}

fn json_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::from),
        any::<i64>().prop_map(Value::from),
        ".{0,24}".prop_map(Value::from),
    ];
    leaf.prop_recursive(4, 32, 4, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..4).prop_map(Value::Array),
            vec(("[a-zA-Z]{1,8}", inner), 0..4)
                .prop_map(|pairs| Value::Object(pairs.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

/// One well-formed request per operation.
fn templates() -> Vec<Value> {
    let options_create = creation_options("example.com", "dave");
    let options_get = request_options("example.com");
    vec![
        json!({"op": "status"}),
        json!({"op": "unlockPassword", "password": "wrong"}),
        json!({"op": "unlockQuick", "key": data_encoding::BASE64.encode(&[7u8; 32])}),
        json!({"op": "findLogins", "origin": ORIGIN}),
        json!({"op": "fillLogin", "origin": ORIGIN, "itemId": "0"}),
        json!({"op": "saveLogin", "origin": ORIGIN, "username": "u", "password": "p"}),
        json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": options_get}),
        json!({"op": "passkeyCheckCreate", "origin": ORIGIN, "optionsJson": options_create}),
        json!({"op": "passkeyWouldReplace", "origin": ORIGIN, "optionsJson": options_create}),
        json!({"op": "passkeyCreate", "origin": ORIGIN, "optionsJson": options_create, "userVerified": false}),
        json!({"op": "passkeyGet", "origin": ORIGIN, "optionsJson": options_get, "itemId": "0",
               "clientDataHash": b64u::encode(&sha256(b"x")), "userVerified": false}),
    ]
}

#[derive(Debug, Clone)]
enum Edit {
    Remove,
    Replace(Value),
    Huge(usize),
    Origin(&'static str),
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        Just(Edit::Remove),
        json_value().prop_map(Edit::Replace),
        (1usize..5).prop_map(|n| Edit::Huge(n * 70_000)),
        proptest::sample::select(BAD_ORIGINS).prop_map(Edit::Origin),
    ]
}

const BAD_ORIGINS: &[&str] = &[
    "android:apk-key-hash:Zm9vYmFy",
    "http://example.com",
    "http://login.example.com",
    "file:///etc/passwd",
    "javascript:alert(1)",
    "https://",
    "https://example.com.evil.test",
    "https://192.168.1.10",
    "",
    "login.example.com",
    "https://login.example.com:0",
    "https://a..b.com",
];

fn apply(
    mut request: Value,
    edits: Vec<(prop::sample::Index, Edit)>,
    extra: Vec<(String, Value)>,
) -> Value {
    let keys: Vec<String> = request.as_object().unwrap().keys().cloned().collect();
    for (at, edit) in edits {
        let key = &keys[at.index(keys.len())];
        match edit {
            Edit::Remove => {
                request.as_object_mut().unwrap().remove(key);
            }
            Edit::Replace(value) => request[key.as_str()] = value,
            Edit::Huge(n) => request[key.as_str()] = Value::String("x".repeat(n)),
            Edit::Origin(origin) => request[key.as_str()] = json!(origin),
        }
    }
    for (key, value) in extra {
        request[key] = value;
    }
    request
}

fn mutated_request() -> impl Strategy<Value = Value> {
    (
        proptest::sample::select(templates()),
        vec((any::<prop::sample::Index>(), edit()), 0..4),
        vec(("[a-zA-Z]{1,8}", json_value()), 0..3),
    )
        .prop_map(|(template, edits, extra)| apply(template, edits, extra))
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn call_always_returns_one_json_object(raw in any::<String>(), request in mutated_request()) {
        let f = fixture();
        for engine in [&f.locked, &f.unlocked] {
            check_shape(&call_str(engine, &raw))?;
            check_shape(&call_str(engine, &request.to_string()))?;
        }
    }

    /// A locked vault answers `status` and the unlock calls, and nothing else: a refusal, never data.
    #[test]
    fn locked_vault_serves_nothing(request in mutated_request()) {
        let f = fixture();
        let reply = call_str(&f.locked, &request.to_string());
        check_shape(&reply)?;
        match request["op"].as_str() {
            Some("status") => prop_assert!(reply.get("ok").is_some() || reply["error"]["code"] == "invalid"),
            _ => {
                let code = reply["error"]["code"].as_str().unwrap_or("");
                prop_assert!(
                    ["locked", "invalid", "wrongPassword", "error"].contains(&code),
                    "a locked vault answered {} to {}", reply, request
                );
            }
        }
        // And it is still locked.
        let status = call_str(&f.locked, r#"{"op":"status"}"#);
        prop_assert_eq!(status["ok"]["status"].as_str(), Some("locked"));
    }

    /// Origins the OS cannot vouch for as a website are `invalid` for every operation that has one.
    #[test]
    fn non_https_origins_are_invalid(
        origin in prop_oneof![
            proptest::sample::select(BAD_ORIGINS).prop_map(str::to_string),
            "android:apk-key-hash:[A-Za-z0-9_-]{0,44}",
            "http://[a-z]{1,8}\\.(com|org|net)",
            "(file|ftp|data|about|chrome)[:/a-z.]{0,16}",
            any::<String>(),
        ],
        which in 0usize..7,
    ) {
        prop_assume!(Origin::parse(&origin).is_err());
        let f = fixture();
        let item = f.good_passkey.as_str();
        let hash = b64u::encode(&sha256(b"x"));
        let request = [
            json!({"op": "findLogins", "origin": origin}),
            json!({"op": "fillLogin", "origin": origin, "itemId": f.login}),
            json!({"op": "saveLogin", "origin": origin, "username": "u", "password": "p"}),
            json!({"op": "passkeyList", "origin": origin, "optionsJson": request_options("example.com")}),
            json!({"op": "passkeyCheckCreate", "origin": origin, "optionsJson": creation_options("example.com", "z")}),
            json!({"op": "passkeyCreate", "origin": origin, "optionsJson": creation_options("example.com", "z"), "userVerified": true}),
            json!({"op": "passkeyGet", "origin": origin, "optionsJson": request_options("example.com"),
                   "itemId": item, "clientDataHash": hash, "userVerified": true}),
        ][which].clone();
        let reply = call_str(&f.unlocked, &request.to_string());
        prop_assert_eq!(reply["error"]["code"].as_str(), Some("invalid"), "{} for {}", reply, request);
    }

    /// Whatever else is wrong with a request, a password in it never comes back in the answer.
    #[test]
    fn errors_never_echo_the_password(
        secret in "[A-Za-z0-9]{16}",
        which in 0usize..3,
        edits in vec((any::<prop::sample::Index>(), edit()), 0..3),
        extra in vec(("[a-zA-Z]{1,8}", json_value()), 0..3),
    ) {
        let template = [
            json!({"op": "saveLogin", "origin": ORIGIN, "username": "u", "password": secret}),
            json!({"op": "unlockPassword", "password": secret}),
            json!({"op": "saveLogin", "origin": "http://login.example.com", "username": "u", "password": secret}),
        ][which].clone();
        // Edits touch every field except the one that holds the password.
        let keys: Vec<String> = template.as_object().unwrap().keys().filter(|k| *k != "password").cloned().collect();
        let mut request = template;
        for (at, edit) in edits {
            let key = &keys[at.index(keys.len())];
            match edit {
                Edit::Remove => { request.as_object_mut().unwrap().remove(key); }
                Edit::Replace(value) => request[key.as_str()] = value,
                Edit::Huge(n) => request[key.as_str()] = Value::String("x".repeat(n)),
                Edit::Origin(origin) => request[key.as_str()] = json!(origin),
            }
        }
        for (key, value) in extra {
            if key != "password" {
                request[key] = value;
            }
        }
        let f = fixture();
        for engine in [&f.locked, &f.unlocked] {
            let reply = provider::call(engine, &request.to_string());
            prop_assert!(!reply.contains(&secret), "the password came back in {}", reply);
        }
        // A request that is not even JSON, with the password inside it.
        let broken = format!(r#"{{"op":"saveLogin","password":"{secret}"#);
        prop_assert!(!provider::call(&f.unlocked, &broken).contains(&secret));
    }

    #[test]
    fn passkey_get_refuses_wrong_items(
        item in 0usize..6,
        random_item in "[A-Za-z0-9-]{0,40}",
        rp in prop_oneof![Just("example.com"), Just("other.test"), Just("evil.test"), Just("com")],
        hash_len in prop_oneof![Just(32usize), Just(31), Just(33), Just(0), 0usize..100],
        hash_text in prop::option::of("[A-Za-z0-9_=*+/ -]{0,60}"),
        origin in prop_oneof![Just(ORIGIN), Just("https://other.test"), Just("https://evil.test")],
    ) {
        let f = fixture();
        let item_id = match item {
            0 => f.good_passkey.clone(),
            1 => f.login.clone(),
            2 => f.trashed_passkey.clone(),
            3 => f.other_site_passkey.clone(),
            4 => random_item,
            _ => String::new(),
        };
        let hash = hash_text.clone().unwrap_or_else(|| b64u::encode(&vec![9u8; hash_len]));
        let request = json!({
            "op": "passkeyGet", "origin": origin, "optionsJson": request_options(rp),
            "itemId": item_id, "clientDataHash": hash, "userVerified": true,
        });
        let reply = call_str(&f.unlocked, &request.to_string());
        check_shape(&reply)?;
        let hash_ok = b64u::decode(&hash).is_ok_and(|bytes| bytes.len() == 32);
        let origin_ok = Origin::parse(origin).is_ok_and(|o| {
            o.host == rp || o.host.ends_with(&format!(".{rp}"))
        });
        let should_work = item == 0 && rp == "example.com" && origin == ORIGIN && hash_ok && origin_ok;
        let other_should_work = item == 3 && rp == "other.test" && origin == "https://other.test" && hash_ok && origin_ok;
        prop_assert_eq!(
            reply.get("ok").is_some(),
            should_work || other_should_work,
            "{} for {}", reply, request
        );
        if let Some(ok) = reply.get("ok") {
            // What a successful sign-in returns is a response JSON and nothing more.
            let response: Value = serde_json::from_str(ok["responseJson"].as_str().unwrap()).unwrap();
            prop_assert!(response["response"]["signature"].is_string());
            prop_assert!(response.get("privateKey").is_none());
        }
    }

    #[test]
    fn unlock_quick_input_validation(key in prop_oneof![
        any::<String>(),
        vec(any::<u8>(), 0..80).prop_map(|b| data_encoding::BASE64.encode(&b)),
        vec(any::<u8>(), 32..=32).prop_map(|b| data_encoding::BASE64URL.encode(&b)),
        vec(any::<u8>(), 31..=33).prop_map(|b| data_encoding::BASE64.encode(&b)),
    ]) {
        let f = fixture();
        let reply = call_str(&f.locked, &json!({"op": "unlockQuick", "key": key}).to_string());
        check_shape(&reply)?;
        // Never unlocks: no key of ours was ever enrolled for quick unlock.
        prop_assert!(reply.get("error").is_some());
        let decoded = data_encoding::BASE64.decode(key.as_bytes());
        if decoded.as_ref().map_or(true, |b| b.len() != 32) {
            prop_assert_eq!(reply["error"]["code"].as_str(), Some("invalid"), "{}", reply);
        }
        let status = call_str(&f.locked, r#"{"op":"status"}"#);
        prop_assert_eq!(status["ok"]["status"].as_str(), Some("locked"));
    }

    /// V12: texts and lists in a passkey request have caps; one over the cap is `invalid`.
    #[test]
    fn oversize_passkey_requests_are_invalid(
        name_len in 257usize..5_000,
        list_len in 257usize..400,
        id_bytes in 1024usize..3_000,
        which in 0usize..4,
    ) {
        let f = fixture();
        let big = "n".repeat(name_len);
        let options = match which {
            0 => json!({"rp": {"id": "example.com", "name": big}, "user": {"id": "AQ", "name": "u"},
                        "pubKeyCredParams": [{"alg": -7}]}),
            1 => json!({"rp": {"id": "example.com"}, "user": {"id": "AQ", "name": big},
                        "pubKeyCredParams": [{"alg": -7}]}),
            2 => json!({"rp": {"id": "example.com"}, "user": {"id": "AQ", "name": "u"},
                        "pubKeyCredParams": [{"alg": -7}],
                        "excludeCredentials": vec![json!({"id": "AQID"}); list_len]}),
            _ => json!({"rp": {"id": "example.com"}, "user": {"id": "AQ", "name": "u"},
                        "pubKeyCredParams": [{"alg": -7}],
                        "excludeCredentials": [{"id": b64u::encode(&vec![1u8; id_bytes])}]}),
        };
        let reply = call_str(
            &f.unlocked,
            &json!({"op": "passkeyCheckCreate", "origin": ORIGIN, "optionsJson": options.to_string()}).to_string(),
        );
        prop_assert_eq!(reply["error"]["code"].as_str(), Some("invalid"), "{}", reply);
        // Sign-in lists are capped in the same way.
        let allow = json!({"rpId": "example.com", "allowCredentials": vec![json!({"id": "AQID"}); list_len]});
        let reply = call_str(
            &f.unlocked,
            &json!({"op": "passkeyList", "origin": ORIGIN, "optionsJson": allow.to_string()}).to_string(),
        );
        prop_assert_eq!(reply["error"]["code"].as_str(), Some("invalid"), "{}", reply);
    }
}

#[test]
fn deep_and_huge_requests_get_an_answer() {
    let f = fixture();
    let depth = 20_000;
    let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    for request in [
        nested.clone(),
        format!(r#"{{"op":"status","x":{nested}}}"#),
        format!(r#"{{"op":"findLogins","origin":{nested}}}"#),
        format!(
            r#"{{"op":"saveLogin","origin":"{ORIGIN}","username":"u","password":"{}"}}"#,
            "p".repeat(2_000_000)
        ),
        format!(
            r#"{{"op":"unlockPassword","password":"{}"}}"#,
            "p".repeat(2_000_000)
        ),
        format!(r#"{{"op":"{}"}}"#, "x".repeat(2_000_000)),
        String::from("\u{feff}{\"op\":\"status\"}"),
        String::new(),
        "null".into(),
        "\0".into(),
    ] {
        for engine in [&f.locked, &f.unlocked] {
            let reply = call_str(engine, &request);
            assert_eq!(
                reply.as_object().map(Map::len),
                Some(1),
                "{}",
                &request[..request.len().min(60)]
            );
        }
    }
}
