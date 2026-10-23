//! Prints a registration and a sign-in produced by the engine, shaped like what a browser
//! hands to a website, as JSON on stdout. `tools/passkey-verify` checks it with SimpleWebAuthn,
//! an independent relying-party implementation:
//!
//! ```sh
//! cargo run -q -p lp-engine --example passkey_fixture > fixture.json
//! cd tools/passkey-verify && bun install && bun run verify.ts ../../fixture.json
//! ```

use std::sync::Arc;

use lp_core::crypto::{KdfParams, random_bytes, sha256};
use lp_core::passkey::b64u;
use lp_engine::{
    Engine, EngineConfig, FileSecretStore, PasskeyAssertRequest, PasskeyCreateRequest,
};
use serde_json::json;

fn main() {
    let dir = tempfile::tempdir().expect("temp dir");
    let mut config = EngineConfig::new(dir.path(), "fixture");
    config.kdf = KdfParams::insecure_for_tests();
    config.sync.mdns = false;
    let engine = Engine::open(
        config,
        Arc::new(FileSecretStore::new(dir.path().join("device.key"))),
    )
    .expect("engine");
    engine
        .create_vault("fixture master pw", "Fixture")
        .expect("vault");

    let origin = "https://login.example.com";
    let rp_id = "example.com";
    let user_handle = b"user-1234";

    // What a browser would put in clientDataJSON, byte for byte.
    let client_data = |kind: &str, challenge: &str| {
        format!(
            r#"{{"type":"{kind}","challenge":"{challenge}","origin":"{origin}","crossOrigin":false}}"#
        )
    };

    let create_challenge = b64u::encode(&random_bytes::<32>().expect("random"));
    let created = engine
        .passkey_create(&PasskeyCreateRequest {
            origin: origin.into(),
            rp_id: rp_id.into(),
            rp_name: "Example".into(),
            user_handle: user_handle.to_vec(),
            user_name: "alice".into(),
            user_display_name: "Alice".into(),
            algorithms: vec![-7, -257],
            exclude: vec![],
            user_verified: true,
        })
        .expect("create");
    let create_json = client_data("webauthn.create", &create_challenge);

    let get_challenge = b64u::encode(&random_bytes::<32>().expect("random"));
    let get_json = client_data("webauthn.get", &get_challenge);
    let assertion = engine
        .passkey_assert(&PasskeyAssertRequest {
            origin: origin.into(),
            rp_id: rp_id.into(),
            item_id: created.item_id.clone(),
            client_data_hash: sha256(get_json.as_bytes()).to_vec(),
            user_verified: true,
        })
        .expect("assert");

    let id = b64u::encode(&created.credential_id);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "origin": origin,
            "rpId": rp_id,
            "registration": {
                "challenge": create_challenge,
                "response": {
                    "id": id,
                    "rawId": id,
                    "type": "public-key",
                    "clientExtensionResults": {},
                    "response": {
                        "clientDataJSON": b64u::encode(create_json.as_bytes()),
                        "attestationObject": b64u::encode(&created.attestation_object),
                        "transports": ["internal", "hybrid"],
                    },
                },
            },
            "authentication": {
                "challenge": get_challenge,
                "response": {
                    "id": id,
                    "rawId": id,
                    "type": "public-key",
                    "clientExtensionResults": {},
                    "response": {
                        "clientDataJSON": b64u::encode(get_json.as_bytes()),
                        "authenticatorData": b64u::encode(&assertion.auth_data),
                        "signature": b64u::encode(&assertion.signature),
                        "userHandle": b64u::encode(&assertion.user_handle),
                    },
                },
            },
        }))
        .expect("json")
    );
}
