//! Messages between a client (the browser extension through the native helper, or the OS
//! passkey provider) and the app.
//!
//! A connection starts with a handshake and then carries requests:
//!
//! ```text
//! client                                   app
//!   Hello{name, kind, clientId?}  ------>
//!                                 <------  Challenge{nonce}   (known client)
//!   Auth{clientNonce, proof}      ------>
//!                                 <------  Ready{status, serverProof}
//!
//!   Hello{name, kind}             ------>                      (new client)
//!                                 <------  Unpaired
//!   Pair                          ------>
//!                                 <------  Pairing{code}       (shown in both UIs)
//!                                 <------  Paired{clientId, secret, status} | Denied
//!
//!   {id, op, ...}                 ------>  {id, ok, result | error}
//! ```
//!
//! `proof` and `serverProof` are HMAC-SHA256 values keyed with the client's secret, so each side
//! proves it knows the secret without sending it, and a process squatting on the pipe name
//! can't impersonate the app.

use lp_core::crypto::hmac_sha256;
use lp_core::passgen::PasswordOptions;
use lp_core::passkey::{
    PasskeyAssertRequest, PasskeyAssertion, PasskeyCreateRequest, b64u, b64u_list,
};
use lp_core::status::VaultStatus;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clients::ClientKind;

pub const PROTOCOL_VERSION: u32 = 1;

/// Domain separation for the client's half of the handshake proof.
pub const CLIENT_PROOF_LABEL: &[u8] = b"lp-broker/client/v1";
/// Domain separation for the app's half of the handshake proof.
pub const SERVER_PROOF_LABEL: &[u8] = b"lp-broker/server/v1";

/// The handshake proof: HMAC-SHA256 keyed with the client's secret over the label, both nonces
/// and the client id. The client sends it with [`CLIENT_PROOF_LABEL`], the app answers with
/// [`SERVER_PROOF_LABEL`].
pub fn proof(
    label: &[u8],
    secret: &[u8],
    server_nonce: &[u8],
    client_nonce: &[u8],
    id: &str,
) -> [u8; 32] {
    hmac_sha256(secret, &[label, server_nonce, client_nonce, id.as_bytes()])
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMessage {
    Hello {
        v: u32,
        /// Shown to the user when pairing, for example "Chrome extension".
        name: String,
        #[serde(default)]
        kind: ClientKind,
        #[serde(default, rename = "clientId")]
        client_id: Option<String>,
    },
    Auth {
        #[serde(rename = "clientNonce")]
        client_nonce: String,
        proof: String,
    },
    Pair,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ServerMessage {
    Challenge {
        nonce: String,
    },
    /// The client id is unknown (never paired, or removed in the app).
    Unpaired,
    /// A pairing request is waiting for the user; `code` is also shown in the app.
    Pairing {
        code: String,
    },
    Paired {
        #[serde(rename = "clientId")]
        client_id: String,
        secret: String,
        status: VaultStatus,
    },
    Ready {
        status: VaultStatus,
        #[serde(rename = "serverProof")]
        server_proof: String,
        #[serde(rename = "appVersion")]
        app_version: String,
    },
    Denied,
    Error {
        code: String,
        message: String,
    },
}

/// How strictly the site wants the user checked, as in WebAuthn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UserVerification {
    Required,
    #[default]
    Preferred,
    Discouraged,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// Vault status and device name. Never prompts.
    Status,
    /// Asks the app to unlock (Windows Hello, or opens the window).
    Unlock,
    Lock,
    /// Logins for a page. Never prompts: a locked vault answers `locked`.
    FindLogins {
        origin: String,
    },
    /// Titles and usernames matching `query`, no secrets.
    SearchItems {
        query: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    /// Username, password and 2FA code for a login. `force` fills a login that was saved for a
    /// different site (the user chose it from the full list).
    FillLogin {
        // Not `id`: that name is the request id on the envelope.
        #[serde(rename = "itemId")]
        item_id: String,
        origin: String,
        #[serde(default)]
        force: bool,
    },
    SaveLogin {
        origin: String,
        username: String,
        password: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    GeneratePassword {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<PasswordOptions>,
    },
    PasskeyCandidates {
        origin: String,
        #[serde(rename = "rpId")]
        rp_id: String,
        #[serde(default, with = "b64u_list")]
        allow: Vec<Vec<u8>>,
    },
    /// Metadata of every passkey in the vault (no private keys), for the Windows passkey
    /// provider's credential cache. Never prompts: a locked vault answers `locked`.
    PasskeyListAll,
    PasskeyCreate {
        #[serde(flatten)]
        request: PasskeyCreateRequest,
        #[serde(default)]
        uv: UserVerification,
    },
    PasskeyAssert {
        #[serde(flatten)]
        request: PasskeyAssertRequest,
        #[serde(default)]
        uv: UserVerification,
    },
    /// Sign-in in ONE request, for the Windows passkey provider: the app unlocks if needed (that
    /// unlock is the user check, so Windows Hello is shown once), finds the passkeys for `rp_id`
    /// limited to `allow` as `PasskeyCandidates` does, and signs when exactly one matches. None is
    /// `notFound`, several is `ambiguous` (nothing is signed and the user is not asked). The reply
    /// is the same as `PasskeyAssert`'s.
    PasskeyAssertAny {
        origin: String,
        #[serde(rename = "rpId")]
        rp_id: String,
        #[serde(default, with = "b64u_list")]
        allow: Vec<Vec<u8>>,
        /// SHA-256 of the client data JSON.
        #[serde(rename = "clientDataHash", with = "b64u")]
        client_data_hash: Vec<u8>,
        #[serde(default)]
        uv: UserVerification,
    },
}

impl Request {
    /// Operations a provider-kind client may use.
    pub fn allowed_for_provider(&self) -> bool {
        matches!(
            self,
            Request::Status
                | Request::Unlock
                | Request::PasskeyCandidates { .. }
                | Request::PasskeyListAll
                | Request::PasskeyCreate { .. }
                | Request::PasskeyAssert { .. }
                | Request::PasskeyAssertAny { .. }
        )
    }

    /// Operations only a provider-kind client may use (the extension has its own, richer flow).
    /// `PasskeyListAll` is for the Windows credential cache: it lists every passkey with no prompt,
    /// which the extension never needs and which would sidestep the check that `SearchItems` has.
    pub fn provider_only(&self) -> bool {
        matches!(
            self,
            Request::PasskeyAssertAny { .. } | Request::PasskeyListAll
        )
    }

    /// Operations that reveal a secret, sign, or write: these may ask the user to unlock first.
    pub fn is_interactive(&self) -> bool {
        matches!(
            self,
            Request::FillLogin { .. }
                | Request::SaveLogin { .. }
                | Request::PasskeyCreate { .. }
                | Request::PasskeyAssert { .. }
                | Request::PasskeyAssertAny { .. }
        )
    }
}

/// The reply to [`Request::PasskeyAssertAny`]: the same assertion as `PasskeyAssert` gives, plus the
/// names of the passkey that was chosen (the provider has no vault to look them up in).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AssertedPasskey {
    #[serde(flatten)]
    pub assertion: PasskeyAssertion,
    #[serde(default, rename = "userName")]
    pub user_name: String,
    #[serde(default, rename = "userDisplayName")]
    pub user_display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub id: u64,
    #[serde(flatten)]
    pub request: Request,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResponseEnvelope {
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
}

impl ResponseEnvelope {
    pub fn success(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result: Some(result),
            error: None,
        }
    }

    pub fn failure(id: u64, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: None,
            error: Some(ErrorBody {
                code: code.to_string(),
                message: message.into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_from_the_json_the_extension_sends() {
        let find: RequestEnvelope =
            serde_json::from_str(r#"{"id":1,"op":"findLogins","origin":"https://example.com"}"#)
                .unwrap();
        assert_eq!(find.id, 1);
        assert!(
            matches!(find.request, Request::FindLogins { ref origin } if origin == "https://example.com")
        );

        let create: RequestEnvelope = serde_json::from_str(
            r#"{"id":2,"op":"passkeyCreate","origin":"https://example.com","rpId":"example.com",
                "userHandle":"dXNlcg","userName":"alice","algorithms":[-7],"uv":"required"}"#,
        )
        .unwrap();
        let Request::PasskeyCreate { request, uv } = create.request else {
            panic!("passkeyCreate")
        };
        assert_eq!(request.user_handle, b"user");
        assert_eq!(request.rp_id, "example.com");
        assert_eq!(uv, UserVerification::Required);
        assert!(!request.user_verified, "defaults to not verified");

        let assert_req: RequestEnvelope = serde_json::from_str(
            r#"{"id":3,"op":"passkeyAssert","origin":"https://example.com","rpId":"example.com",
                "itemId":"x","clientDataHash":"AAAA"}"#,
        )
        .unwrap();
        assert!(matches!(
            assert_req.request,
            Request::PasskeyAssert {
                uv: UserVerification::Preferred,
                ..
            }
        ));

        assert!(serde_json::from_str::<RequestEnvelope>(r#"{"id":4,"op":"nope"}"#).is_err());
        assert!(serde_json::from_str::<RequestEnvelope>(r#"{"op":"status"}"#).is_err());
    }

    #[test]
    fn handshake_messages_use_camel_case() {
        let hello: ClientMessage = serde_json::from_str(
            r#"{"type":"hello","v":1,"name":"Chrome","kind":"provider","clientId":"abc"}"#,
        )
        .unwrap();
        assert!(matches!(
            hello,
            ClientMessage::Hello {
                kind: ClientKind::Provider,
                ..
            }
        ));
        assert!(matches!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"pair"}"#).unwrap(),
            ClientMessage::Pair
        ));
        let ready = serde_json::to_string(&ServerMessage::Ready {
            status: VaultStatus::Locked,
            server_proof: "p".into(),
            app_version: "1".into(),
        })
        .unwrap();
        assert_eq!(
            ready,
            r#"{"type":"ready","status":"locked","serverProof":"p","appVersion":"1"}"#
        );
    }

    #[test]
    fn providers_only_get_passkey_operations() {
        assert!(Request::Status.allowed_for_provider());
        assert!(!Request::Lock.allowed_for_provider());
        assert!(
            !Request::FindLogins {
                origin: String::new()
            }
            .allowed_for_provider()
        );
        assert!(
            !Request::FillLogin {
                item_id: String::new(),
                origin: String::new(),
                force: false
            }
            .allowed_for_provider()
        );
        assert!(Request::Unlock.allowed_for_provider());
        assert!(Request::PasskeyListAll.allowed_for_provider());
        let any = Request::PasskeyAssertAny {
            origin: String::new(),
            rp_id: String::new(),
            allow: vec![],
            client_data_hash: vec![],
            uv: UserVerification::Required,
        };
        assert!(any.allowed_for_provider() && any.is_interactive() && any.provider_only());
        assert!(!Request::Unlock.provider_only() && Request::PasskeyListAll.provider_only());
        assert!(
            !Request::PasskeyListAll.is_interactive(),
            "listing never prompts"
        );
        assert!(!Request::Status.is_interactive());
        assert!(
            !Request::FindLogins {
                origin: String::new()
            }
            .is_interactive()
        );
    }

    #[test]
    fn an_any_assertion_reply_is_an_assertion_with_names() {
        let reply = serde_json::json!({
            "itemId": "i", "credentialId": "AQI", "authData": "AQI", "signature": "AQI",
            "userHandle": "AQI", "userName": "alice", "userDisplayName": "Alice",
        });
        let any: AssertedPasskey = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(any.user_name, "alice");
        assert_eq!(any.assertion.credential_id, vec![1, 2]);
        assert_eq!(serde_json::to_value(&any).unwrap(), reply);
        // A plain `PasskeyAssert` reply (no names) still parses, and so does this one as one.
        let plain: PasskeyAssertion = serde_json::from_value(reply).unwrap();
        assert_eq!(plain.item_id, "i");
        let nameless: AssertedPasskey = serde_json::from_value(serde_json::json!({
            "itemId": "i", "credentialId": "AQI", "authData": "AQI", "signature": "AQI",
            "userHandle": "AQI",
        }))
        .unwrap();
        assert!(nameless.user_name.is_empty());
    }

    #[test]
    fn the_list_all_request_has_a_stable_wire_name() {
        let env: RequestEnvelope =
            serde_json::from_str(r#"{"id":7,"op":"passkeyListAll"}"#).unwrap();
        assert_eq!(env.id, 7);
        assert!(matches!(env.request, Request::PasskeyListAll));
        assert_eq!(
            serde_json::to_string(&env).unwrap(),
            r#"{"id":7,"op":"passkeyListAll"}"#
        );
    }

    #[test]
    fn every_request_survives_serializing_and_parsing_again() {
        let requests = vec![
            (Request::Status, "status"),
            (Request::Unlock, "unlock"),
            (Request::Lock, "lock"),
            (
                Request::FindLogins {
                    origin: "https://example.com".into(),
                },
                "findLogins",
            ),
            (
                Request::SearchItems {
                    query: "ex".into(),
                    limit: Some(5),
                },
                "searchItems",
            ),
            (
                Request::FillLogin {
                    item_id: "i".into(),
                    origin: "https://example.com".into(),
                    force: true,
                },
                "fillLogin",
            ),
            (
                Request::SaveLogin {
                    origin: "https://example.com".into(),
                    username: "a".into(),
                    password: "b".into(),
                    title: None,
                },
                "saveLogin",
            ),
            (
                Request::GeneratePassword { options: None },
                "generatePassword",
            ),
            (
                Request::PasskeyCandidates {
                    origin: "https://example.com".into(),
                    rp_id: "example.com".into(),
                    allow: vec![vec![1, 2, 3], vec![4]],
                },
                "passkeyCandidates",
            ),
            (Request::PasskeyListAll, "passkeyListAll"),
            (
                Request::PasskeyCreate {
                    request: PasskeyCreateRequest {
                        origin: "https://example.com".into(),
                        rp_id: "example.com".into(),
                        rp_name: "Example".into(),
                        user_handle: vec![9; 16],
                        user_name: "alice".into(),
                        user_display_name: "Alice".into(),
                        algorithms: vec![-7],
                        exclude: vec![vec![5, 6]],
                        user_verified: false,
                    },
                    uv: UserVerification::Required,
                },
                "passkeyCreate",
            ),
            (
                Request::PasskeyAssert {
                    request: PasskeyAssertRequest {
                        origin: "https://example.com".into(),
                        rp_id: "example.com".into(),
                        item_id: "item".into(),
                        client_data_hash: vec![7; 32],
                        user_verified: false,
                    },
                    uv: UserVerification::Discouraged,
                },
                "passkeyAssert",
            ),
            (
                Request::PasskeyAssertAny {
                    origin: "https://example.com".into(),
                    rp_id: "example.com".into(),
                    allow: vec![vec![1, 2, 3]],
                    client_data_hash: vec![7; 32],
                    uv: UserVerification::Required,
                },
                "passkeyAssertAny",
            ),
        ];
        for (n, (request, op)) in requests.into_iter().enumerate() {
            let sent = RequestEnvelope {
                id: n as u64 + 1,
                request,
            };
            let wire = serde_json::to_value(&sent).unwrap();
            assert_eq!(wire["op"], op);
            assert_eq!(wire["id"], n as u64 + 1);
            let parsed: RequestEnvelope = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(parsed.id, sent.id);
            assert_eq!(serde_json::to_value(&parsed).unwrap(), wire, "{op}");
        }
    }
}
