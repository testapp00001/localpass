//! The JSON form of WebAuthn requests and responses, as Android's Credential Manager hands
//! them to a credential provider (`PublicKeyCredentialCreationOptions` / `...RequestOptions`
//! in, `RegistrationResponseJSON` / `AuthenticationResponseJSON` out).
//!
//! This is only the translation. The engine calls in `credentials.rs` do the checking and the
//! signing. A browser that calls the provider supplies the hash of the client data it built, so
//! the responses here leave `clientDataJSON` out: the browser adds its own.

use lp_core::passkey::b64u;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{EngineError, PasskeyAssertion, PasskeyCreateRequest, PasskeyCreated, Result};

#[derive(Deserialize, Default)]
#[serde(default)]
struct RpJson {
    id: Option<String>,
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct UserJson {
    id: String,
    name: String,
    #[serde(rename = "displayName")]
    display_name: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ParamJson {
    alg: i64,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct DescriptorJson {
    id: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct CreateJson {
    rp: RpJson,
    user: UserJson,
    #[serde(rename = "pubKeyCredParams")]
    pub_key_cred_params: Vec<ParamJson>,
    #[serde(rename = "excludeCredentials")]
    exclude_credentials: Vec<DescriptorJson>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct GetJson {
    #[serde(rename = "rpId")]
    rp_id: Option<String>,
    #[serde(rename = "allowCredentials")]
    allow_credentials: Vec<DescriptorJson>,
}

fn invalid(what: impl std::fmt::Display) -> EngineError {
    EngineError::Invalid(format!("the passkey request is not valid: {what}"))
}

fn ids(list: &[DescriptorJson]) -> Result<Vec<Vec<u8>>> {
    if list.len() > crate::credentials::MAX_ID_LIST {
        return Err(invalid("too many credential ids"));
    }
    list.iter()
        .map(|d| b64u::decode(&d.id).map_err(|e| invalid(format!("credential id ({e})"))))
        .collect()
}

/// What the caller asks to create, before the user has been asked.
#[derive(Debug, PartialEq, Eq)]
pub struct CreateOptions {
    pub rp_id: String,
    pub rp_name: String,
    pub user_handle: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
    /// COSE algorithm ids in the site's order of preference (empty: any).
    pub algorithms: Vec<i64>,
    pub exclude: Vec<Vec<u8>>,
}

impl CreateOptions {
    /// The engine request for this creation. `user_verified` says the caller has just checked
    /// the user (fingerprint, screen lock).
    pub fn into_request(self, origin: &str, user_verified: bool) -> PasskeyCreateRequest {
        PasskeyCreateRequest {
            origin: origin.to_string(),
            rp_id: self.rp_id,
            rp_name: self.rp_name,
            user_handle: self.user_handle,
            user_name: self.user_name,
            user_display_name: self.user_display_name,
            algorithms: self.algorithms,
            exclude: self.exclude,
            user_verified,
        }
    }
}

/// Parses creation options. `default_rp_id` (the origin's host) is used when the site leaves
/// `rp.id` out, as the specification says.
pub fn parse_create_options(json: &str, default_rp_id: &str) -> Result<CreateOptions> {
    let parsed: CreateJson = serde_json::from_str(json).map_err(invalid)?;
    let user_handle =
        b64u::decode(&parsed.user.id).map_err(|e| invalid(format!("user id ({e})")))?;
    Ok(CreateOptions {
        rp_id: parsed
            .rp
            .id
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| default_rp_id.to_string()),
        rp_name: parsed.rp.name,
        user_handle,
        user_name: parsed.user.name,
        user_display_name: parsed.user.display_name,
        algorithms: parsed.pub_key_cred_params.iter().map(|p| p.alg).collect(),
        exclude: ids(&parsed.exclude_credentials)?,
    })
}

/// What the caller asks to sign in with.
#[derive(Debug, PartialEq, Eq)]
pub struct GetOptions {
    pub rp_id: String,
    /// Credential ids the site accepts (empty: any passkey for the site).
    pub allow: Vec<Vec<u8>>,
}

pub fn parse_get_options(json: &str, default_rp_id: &str) -> Result<GetOptions> {
    let parsed: GetJson = serde_json::from_str(json).map_err(invalid)?;
    Ok(GetOptions {
        rp_id: parsed
            .rp_id
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| default_rp_id.to_string()),
        allow: ids(&parsed.allow_credentials)?,
    })
}

/// The `RegistrationResponseJSON` for a created passkey.
pub fn registration_response_json(created: &PasskeyCreated) -> String {
    let id = b64u::encode(&created.credential_id);
    let response: Value = json!({
        "id": id,
        "rawId": id,
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "attestationObject": b64u::encode(&created.attestation_object),
            "authenticatorData": b64u::encode(&created.auth_data),
            "transports": ["internal"],
            "publicKeyAlgorithm": created.algorithm,
            "publicKey": b64u::encode(&created.public_key_spki),
        },
        "clientExtensionResults": {},
    });
    response.to_string()
}

/// The `AuthenticationResponseJSON` for a signed challenge.
pub fn assertion_response_json(assertion: &PasskeyAssertion) -> String {
    let id = b64u::encode(&assertion.credential_id);
    let response: Value = json!({
        "id": id,
        "rawId": id,
        "type": "public-key",
        "authenticatorAttachment": "platform",
        "response": {
            "authenticatorData": b64u::encode(&assertion.auth_data),
            "signature": b64u::encode(&assertion.signature),
            "userHandle": b64u::encode(&assertion.user_handle),
        },
        "clientExtensionResults": {},
    });
    response.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREATE: &str = r#"{
        "challenge": "dGVzdC1jaGFsbGVuZ2U",
        "rp": {"name": "Example", "id": "example.com"},
        "user": {"id": "dXNlci0x", "name": "alice@example.com", "displayName": "Alice"},
        "pubKeyCredParams": [{"type": "public-key", "alg": -7}, {"type": "public-key", "alg": -257}],
        "excludeCredentials": [{"type": "public-key", "id": "AQID", "transports": ["internal"]}],
        "authenticatorSelection": {"residentKey": "required", "userVerification": "preferred"},
        "attestation": "none"
    }"#;

    #[test]
    fn huge_credential_lists_are_refused_while_parsing() {
        let list = |n: usize| vec![r#"{"type":"public-key","id":"AQID"}"#; n].join(",");
        let get = |n: usize| format!(r#"{{"allowCredentials":[{}]}}"#, list(n));
        assert!(parse_get_options(&get(256), "example.com").is_ok());
        assert!(parse_get_options(&get(257), "example.com").is_err());
        let create = format!(
            r#"{{"user":{{"id":"dXNlci0x"}},"excludeCredentials":[{}]}}"#,
            list(257)
        );
        assert!(parse_create_options(&create, "example.com").is_err());
    }

    #[test]
    fn creation_options_are_read() {
        let o = parse_create_options(CREATE, "fallback.test").unwrap();
        assert_eq!(o.rp_id, "example.com");
        assert_eq!(o.rp_name, "Example");
        assert_eq!(o.user_handle, b"user-1");
        assert_eq!(o.user_name, "alice@example.com");
        assert_eq!(o.user_display_name, "Alice");
        assert_eq!(o.algorithms, vec![-7, -257]);
        assert_eq!(o.exclude, vec![vec![1, 2, 3]]);
        let r = o.into_request("https://example.com", true);
        assert_eq!(r.origin, "https://example.com");
        assert!(r.user_verified);
    }

    #[test]
    fn a_missing_rp_id_is_the_origins_host() {
        let o = parse_create_options(
            r#"{"rp":{"name":"X"},"user":{"id":"dQ","name":"u"},"challenge":"AA"}"#,
            "shop.example.com",
        )
        .unwrap();
        assert_eq!(o.rp_id, "shop.example.com");
        assert!(o.algorithms.is_empty());
        let g = parse_get_options(r#"{"challenge":"AA"}"#, "shop.example.com").unwrap();
        assert_eq!(g.rp_id, "shop.example.com");
        assert!(g.allow.is_empty());
    }

    #[test]
    fn request_options_are_read() {
        let g = parse_get_options(
            r#"{"challenge":"AA","rpId":"example.com","allowCredentials":[{"type":"public-key","id":"BAUG"}],"userVerification":"required"}"#,
            "ignored",
        )
        .unwrap();
        assert_eq!(g.rp_id, "example.com");
        assert_eq!(g.allow, vec![vec![4, 5, 6]]);
    }

    #[test]
    fn bad_requests_are_refused() {
        for bad in [
            "not json",
            r#"{"rp":{"id":"a.com"},"user":{"id":"***"}}"#,
            r#"{"rp":{"id":"a.com"},"user":{"id":"dQ"},"excludeCredentials":[{"id":"***"}]}"#,
        ] {
            assert!(
                matches!(
                    parse_create_options(bad, "a.com"),
                    Err(EngineError::Invalid(_))
                ),
                "{bad}"
            );
        }
        assert!(parse_get_options(r#"{"allowCredentials":[{"id":"***"}]}"#, "a.com").is_err());
    }

    #[test]
    fn responses_carry_what_a_relying_party_needs() {
        let created = PasskeyCreated {
            item_id: "item".into(),
            credential_id: vec![9; 16],
            auth_data: vec![1, 2, 3],
            attestation_object: vec![4, 5],
            public_key: vec![6],
            public_key_spki: vec![7, 8],
            algorithm: -7,
        };
        let v: Value = serde_json::from_str(&registration_response_json(&created)).unwrap();
        assert_eq!(v["id"], v["rawId"]);
        assert_eq!(v["type"], "public-key");
        assert_eq!(v["response"]["publicKeyAlgorithm"], -7);
        assert_eq!(v["response"]["attestationObject"], b64u::encode(&[4, 5]));
        assert_eq!(v["response"]["publicKey"], b64u::encode(&[7, 8]));
        assert!(v["response"].get("clientDataJSON").is_none());

        let signed = PasskeyAssertion {
            item_id: "item".into(),
            credential_id: vec![9; 16],
            auth_data: vec![1],
            signature: vec![2],
            user_handle: vec![3],
        };
        let v: Value = serde_json::from_str(&assertion_response_json(&signed)).unwrap();
        assert_eq!(v["response"]["signature"], b64u::encode(&[2]));
        assert_eq!(v["response"]["userHandle"], b64u::encode(&[3]));
        assert!(v["response"].get("clientDataJSON").is_none());
    }
}
