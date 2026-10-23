//! One JSON-in, JSON-out entry point for OS credential providers (the Android Credential
//! Manager and autofill services). Kotlin code can't call the engine directly, so each request
//! is a small JSON object and each reply is `{"ok": ...}` or `{"error": {"code", "message"}}`.
//!
//! The origin is always the one the OS reports for the calling app, never something the page
//! said, and the engine checks it again. Secrets are returned only by `fillLogin` and
//! `passkeyGet`, and only for the item that was named.

use lp_core::crypto::Key32;
use lp_core::passkey::b64u;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::credentials::PasskeyAssertRequest;
use crate::webauthn_json::{self, parse_create_options, parse_get_options};
use crate::{Engine, EngineError, Result, VaultStatus};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
enum Call {
    Status,
    /// Unlocks with the master password.
    UnlockPassword {
        password: String,
    },
    /// Unlocks with the quick-unlock key the OS released after a fingerprint (base64).
    UnlockQuick {
        key: String,
    },
    FindLogins {
        origin: String,
    },
    FillLogin {
        origin: String,
        item_id: String,
    },
    SaveLogin {
        origin: String,
        username: String,
        password: String,
    },
    /// Passkeys that fit a sign-in request.
    PasskeyList {
        origin: String,
        options_json: String,
    },
    /// Checks a creation request and says what would be created, changing nothing.
    PasskeyCheckCreate {
        origin: String,
        options_json: String,
    },
    /// The passkey a creation request would replace (same site and account), so the user can be
    /// asked first. Changes nothing; `{"replaces": null}` when there is none.
    PasskeyWouldReplace {
        origin: String,
        options_json: String,
    },
    PasskeyCreate {
        origin: String,
        options_json: String,
        user_verified: bool,
    },
    PasskeyGet {
        origin: String,
        options_json: String,
        item_id: String,
        /// base64url of the 32-byte hash of the client data the browser built.
        client_data_hash: String,
        user_verified: bool,
    },
}

fn code(e: &EngineError) -> &'static str {
    match e {
        EngineError::Locked => "locked",
        EngineError::NoVault => "noVault",
        EngineError::WrongPassword => "wrongPassword",
        EngineError::NotFound(_) => "notFound",
        EngineError::Invalid(_) | EngineError::Core(_) => "invalid",
        EngineError::Unsupported(_) => "unsupported",
        EngineError::PasskeyExists => "passkeyExists",
        _ => "error",
    }
}

fn status_name(status: VaultStatus) -> &'static str {
    match status {
        VaultStatus::NoVault => "noVault",
        VaultStatus::Locked => "locked",
        VaultStatus::Unlocked => "unlocked",
    }
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value> {
    serde_json::to_value(value).map_err(|e| EngineError::Invalid(e.to_string()))
}

fn run(engine: &Engine, call: Call) -> Result<Value> {
    match call {
        Call::Status => Ok(json!({
            "status": status_name(engine.status()),
            "quickUnlock": engine.quick_unlock_enabled(),
        })),
        Call::UnlockPassword { password } => {
            engine.unlock(&password)?;
            Ok(Value::Null)
        }
        Call::UnlockQuick { key } => {
            let bytes = data_encoding::BASE64
                .decode(key.as_bytes())
                .map_err(|_| EngineError::Invalid("bad quick-unlock key".into()))?;
            engine.unlock_quick(&Key32::from_slice(&bytes)?)?;
            Ok(Value::Null)
        }
        Call::FindLogins { origin } => to_value(&engine.find_logins(&origin)?),
        Call::FillLogin { origin, item_id } => to_value(&engine.fill_login(&item_id, &origin)?),
        Call::SaveLogin {
            origin,
            username,
            password,
        } => to_value(&engine.save_login(&origin, &username, &password, None)?),
        Call::PasskeyList {
            origin,
            options_json,
        } => {
            let options = parse_get_options(&options_json, &default_rp_id(&origin)?)?;
            to_value(&engine.passkey_candidates(&origin, &options.rp_id, &options.allow)?)
        }
        Call::PasskeyCheckCreate {
            origin,
            options_json,
        } => {
            let options = parse_create_options(&options_json, &default_rp_id(&origin)?)?;
            let summary = json!({
                "rpId": options.rp_id,
                "rpName": options.rp_name,
                "userName": options.user_name,
                "userDisplayName": options.user_display_name,
            });
            engine.passkey_can_create(&options.into_request(&origin, false))?;
            Ok(summary)
        }
        Call::PasskeyWouldReplace {
            origin,
            options_json,
        } => {
            let options = parse_create_options(&options_json, &default_rp_id(&origin)?)?;
            let existing = engine.passkey_would_replace(&options.into_request(&origin, false))?;
            Ok(json!({ "replaces": existing }))
        }
        Call::PasskeyCreate {
            origin,
            options_json,
            user_verified,
        } => {
            let options = parse_create_options(&options_json, &default_rp_id(&origin)?)?;
            let created = engine.passkey_create(&options.into_request(&origin, user_verified))?;
            Ok(json!({
                "responseJson": webauthn_json::registration_response_json(&created),
                "itemId": created.item_id,
            }))
        }
        Call::PasskeyGet {
            origin,
            options_json,
            item_id,
            client_data_hash,
            user_verified,
        } => {
            let options = parse_get_options(&options_json, &default_rp_id(&origin)?)?;
            let client_data_hash = b64u::decode(&client_data_hash)
                .map_err(|_| EngineError::Invalid("bad client data hash".into()))?;
            let assertion = engine.passkey_assert(&PasskeyAssertRequest {
                origin,
                rp_id: options.rp_id,
                item_id,
                client_data_hash,
                user_verified,
            })?;
            Ok(json!({ "responseJson": webauthn_json::assertion_response_json(&assertion) }))
        }
    }
}

fn default_rp_id(origin: &str) -> Result<String> {
    Ok(lp_core::origin::Origin::parse(origin)?.host)
}

/// Runs one request. Never panics on bad input; a malformed request is an `invalid` error.
pub fn call(engine: &Engine, request: &str) -> String {
    let reply = match serde_json::from_str::<Call>(request) {
        Err(e) => Err(EngineError::Invalid(format!("bad request: {e}"))),
        Ok(call) => run(engine, call),
    };
    match reply {
        Ok(value) => json!({ "ok": value }),
        Err(e) => json!({ "error": { "code": code(&e), "message": e.to_string() } }),
    }
    .to_string()
}
