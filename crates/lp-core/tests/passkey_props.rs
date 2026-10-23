//! Property tests for the base64url helpers every WebAuthn id, handle and hash goes through
//! (phase 5.1). The strings come from pages and from the Android JSON bridge.

use lp_core::passkey::{b64u, b64u_list};
use proptest::collection::vec;
use proptest::prelude::*;
use serde::Deserialize;

/// 256 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);
    ProptestConfig::with_cases(cases)
}

#[derive(Deserialize)]
struct One {
    #[serde(with = "b64u")]
    v: Vec<u8>,
}

#[derive(Deserialize)]
struct Many {
    #[serde(with = "b64u_list")]
    v: Vec<Vec<u8>>,
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn b64u_roundtrip(bytes in vec(any::<u8>(), 0..300)) {
        let text = b64u::encode(&bytes);
        prop_assert!(text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        prop_assert_eq!(b64u::decode(&text), Ok(bytes));
    }

    /// Whatever decodes is the canonical spelling of its bytes (padding aside), so one id has
    /// one text form and equality checks on the text mean something.
    #[test]
    fn b64u_decode_is_canonical(text in "[A-Za-z0-9_=+/ -]{0,40}") {
        if let Ok(bytes) = b64u::decode(&text) {
            prop_assert_eq!(b64u::encode(&bytes), text.trim_end_matches('='));
        }
    }

    #[test]
    fn b64u_never_panics(text in any::<String>(), raw in vec(any::<u8>(), 0..64)) {
        let _ = b64u::decode(&text);
        let _ = b64u::decode(&String::from_utf8_lossy(&raw));
        let _ = b64u::decode(&format!("{text}\0"));
        // Through serde, as the broker and the JSON bridge use it: same answer as `decode`.
        let one = serde_json::from_value::<One>(serde_json::json!({"v": text}));
        prop_assert_eq!(one.ok().map(|o| o.v), b64u::decode(&text).ok());
        let many = serde_json::from_value::<Many>(serde_json::json!({"v": [text.clone(), text.clone()]}));
        let expected = b64u::decode(&text).ok().map(|b| vec![b.clone(), b]);
        prop_assert_eq!(many.ok().map(|m| m.v), expected);
    }

    /// Characters outside the alphabet are errors, not skipped.
    #[test]
    fn b64u_rejects_foreign_characters(
        bytes in vec(any::<u8>(), 1..40),
        at in any::<prop::sample::Index>(),
        bad in prop_oneof![
            Just('+'), Just('/'), Just(' '), Just('\n'), Just('\0'), Just('é'), Just('='),
            Just('\u{ff21}'), Just('*'), Just('.'),
        ],
    ) {
        let mut text = b64u::encode(&bytes);
        let position = at.index(text.len());
        text.insert(position, bad);
        let tail_padding = text.trim_end_matches('=') == b64u::encode(&bytes);
        if !tail_padding {
            prop_assert!(b64u::decode(&text).is_err(), "{:?} accepted", text);
        }
    }

    #[test]
    fn b64u_list_rejects_any_bad_element(
        items in vec(vec(any::<u8>(), 0..20), 1..8),
        at in any::<prop::sample::Index>(),
    ) {
        let mut texts: Vec<String> = items.iter().map(|b| b64u::encode(b)).collect();
        let parsed: Many = serde_json::from_value(serde_json::json!({"v": texts})).unwrap();
        prop_assert_eq!(parsed.v, items);
        let position = at.index(texts.len());
        texts[position] = "not*base64".to_string();
        let broken = serde_json::from_value::<Many>(serde_json::json!({"v": texts}));
        prop_assert!(broken.is_err());
        // A non-string element, and a non-list.
        let number = serde_json::from_value::<Many>(serde_json::json!({"v": [1]}));
        prop_assert!(number.is_err());
        let text = serde_json::from_value::<Many>(serde_json::json!({"v": "AAAA"}));
        prop_assert!(text.is_err());
    }
}

/// Padding is accepted and dropped, however much of it there is (pinned: it is harmless because
/// the canonical re-encoding has none, but it is a looser rule than the specification's).
#[test]
fn b64u_padding_rules_pinned() {
    assert_eq!(b64u::decode("====").unwrap(), Vec::<u8>::new());
    assert_eq!(b64u::decode("AAAA====").unwrap(), vec![0, 0, 0]);
    assert_eq!(b64u::decode("AQ==").unwrap(), vec![1]);
    assert_eq!(b64u::decode("AQ").unwrap(), vec![1]);
    // Padding in the middle is not padding.
    assert!(b64u::decode("A=QA").is_err());
    // Non-canonical trailing bits.
    assert!(b64u::decode("AR").is_err());
    // A string of one character cannot be base64.
    assert!(b64u::decode("A").is_err());
    assert_eq!(b64u::decode("").unwrap(), Vec::<u8>::new());
}
