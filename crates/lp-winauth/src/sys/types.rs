//! `#[repr(C)]` mirrors of the FINAL Windows WebAuthn plugin API structs (x64).
//!
//! Headers (Windows SDK 10.0.26100.0): `pluginauthenticator.h` (operation request/response/cancel),
//! `webauthnplugin.h` (add/update/credential/UV structs and the decoded CTAP requests) and `webauthn.h`
//! (entities, credentials, attestation v8, assertion v6). The `EXPERIMENTAL_*` generation is not mirrored.
//! Every struct's size, alignment and field offsets are pinned in `layout_tests.rs`, whose numbers come
//! from compiling `tools/layout_check.c` against those headers.
//!
//! Conventions: `DWORD` = `u32`, `LONG` = `i32`, `BOOL` = [`Bool`], `PCWSTR`/`LPCWSTR` = `*const u16`
//! (NUL-terminated UTF-16), `PBYTE` = `*mut u8` and `const BYTE*` = `*const u8` exactly as the headers
//! spell them, `REFCLSID`/`REFGUID` inside structs = `*const GUID`, `HWND` = [`Hwnd`]. Version fields are
//! plain `u32`. The structs hold raw pointers: whoever fills one owns the pointed-to memory and keeps it
//! alive for the call; whoever reads one the OS filled must not outlive the OS buffer.

use std::ffi::c_void;

use windows::core::GUID;

/// Win32 `BOOL`: a 32-bit int, 0 = false.
pub type Bool = i32;
/// Win32 `HWND`, opaque: the OS hands it over, we never dereference it.
pub type Hwnd = *mut c_void;

/// All-zero is the documented "unset" value of every field in these plain C structs (integers and raw
/// pointers only), so `Default` is the zeroed struct, like `= {}` in C.
macro_rules! zeroed_default {
    ($($t:ty),* $(,)?) => {$(
        impl Default for $t {
            fn default() -> Self {
                // SAFETY: the struct consists only of integers and raw pointers, for which all-zero bits
                // are valid (0 and null).
                unsafe { std::mem::zeroed() }
            }
        }
    )*};
}

// ------------------------------------------------------------------ pluginauthenticator.h

/// `WEBAUTHN_PLUGIN_OPERATION_REQUEST`: what `IPluginAuthenticator::MakeCredential/GetAssertion` receive.
/// The memory belongs to the COM stub and is valid only during the call.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OperationRequest {
    /// Top-level window of the caller.
    pub hwnd: Hwnd,
    pub transaction_id: GUID,
    pub cb_request_signature: u32,
    pub pb_request_signature: *mut u8,
    /// `WEBAUTHN_PLUGIN_REQUEST_TYPE` (enum, 4 bytes); 1 = CTAP2 CBOR.
    pub request_type: u32,
    pub cb_encoded_request: u32,
    pub pb_encoded_request: *mut u8,
}

/// `WEBAUTHN_PLUGIN_OPERATION_RESPONSE`: filled by the plugin. `pb_encoded_response` must come from
/// `CoTaskMemAlloc` (it crosses the COM stub).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OperationResponse {
    pub cb_encoded_response: u32,
    pub pb_encoded_response: *mut u8,
}

/// `WEBAUTHN_PLUGIN_CANCEL_OPERATION_REQUEST`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CancelRequest {
    pub transaction_id: GUID,
    pub cb_request_signature: u32,
    pub pb_request_signature: *mut u8,
}

// ------------------------------------------------------------------ webauthnplugin.h: management

/// `WEBAUTHN_PLUGIN_ADD_AUTHENTICATOR_OPTIONS`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AddAuthenticatorOptions {
    pub authenticator_name: *const u16,
    pub clsid: *const GUID,
    /// Required in practice: NULL is rejected with `NTE_INVALID_PARAMETER` (spike, stage A).
    pub plugin_rp_id: *const u16,
    pub light_logo_svg: *const u16,
    pub dark_logo_svg: *const u16,
    pub cb_authenticator_info: u32,
    pub pb_authenticator_info: *const u8,
    pub c_supported_rp_ids: u32,
    pub supported_rp_ids: *const *const u16,
}

/// `WEBAUTHN_PLUGIN_ADD_AUTHENTICATOR_RESPONSE`: the operation-signing public key blob.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct AddAuthenticatorResponse {
    pub cb_op_sign_pub_key: u32,
    pub pb_op_sign_pub_key: *mut u8,
}

/// `WEBAUTHN_PLUGIN_UPDATE_AUTHENTICATOR_DETAILS`: as the add options but `clsid_new` replaces the RP id.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UpdateAuthenticatorDetails {
    pub authenticator_name: *const u16,
    pub clsid: *const GUID,
    pub clsid_new: *const GUID,
    pub light_logo_svg: *const u16,
    pub dark_logo_svg: *const u16,
    pub cb_authenticator_info: u32,
    pub pb_authenticator_info: *const u8,
    pub c_supported_rp_ids: u32,
    pub supported_rp_ids: *const *const u16,
}

/// `WEBAUTHN_PLUGIN_CREDENTIAL_DETAILS`: one row of the OS credential cache. Arrays of these are arrays
/// of structs, not of pointers.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PluginCredentialDetails {
    pub cb_credential_id: u32,
    pub pb_credential_id: *const u8,
    pub rp_id: *const u16,
    pub rp_name: *const u16,
    pub cb_user_id: u32,
    pub pb_user_id: *const u8,
    pub user_name: *const u16,
    pub user_display_name: *const u16,
}

/// `WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserVerificationRequest {
    pub hwnd: Hwnd,
    /// `REFGUID rguidTransactionId`: the transaction id of the operation request.
    pub transaction_id: *const GUID,
    pub username: *const u16,
    pub display_hint: *const u16,
}

// ------------------------------------------------------------------ webauthn.h: shared types

/// `WEBAUTHN_RP_ENTITY_INFORMATION`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct RpEntity {
    pub version: u32,
    pub id: *const u16,
    pub name: *const u16,
    pub icon: *const u16,
}

/// `WEBAUTHN_USER_ENTITY_INFORMATION`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UserEntity {
    pub version: u32,
    pub cb_id: u32,
    pub pb_id: *mut u8,
    pub name: *const u16,
    pub icon: *const u16,
    pub display_name: *const u16,
}

/// `WEBAUTHN_COSE_CREDENTIAL_PARAMETER`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CoseCredentialParameter {
    pub version: u32,
    pub credential_type: *const u16,
    pub alg: i32,
}

/// `WEBAUTHN_COSE_CREDENTIAL_PARAMETERS`: a counted array of parameters (by value, not pointers).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CoseCredentialParameters {
    pub count: u32,
    pub params: *mut CoseCredentialParameter,
}

/// `WEBAUTHN_CREDENTIAL`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Credential {
    pub version: u32,
    pub cb_id: u32,
    pub pb_id: *mut u8,
    pub credential_type: *const u16,
}

/// `WEBAUTHN_CREDENTIAL_EX`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CredentialEx {
    pub version: u32,
    pub cb_id: u32,
    pub pb_id: *mut u8,
    pub credential_type: *const u16,
    pub transports: u32,
}

/// `WEBAUTHN_CREDENTIAL_LIST`: `count` pointers to [`CredentialEx`].
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CredentialList {
    pub count: u32,
    pub credentials: *mut *mut CredentialEx,
}

/// `WEBAUTHN_EXTENSION`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Extension {
    pub identifier: *const u16,
    pub cb_extension: u32,
    pub extension: *mut c_void,
}

/// `WEBAUTHN_EXTENSIONS`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Extensions {
    pub count: u32,
    pub extensions: *mut Extension,
}

/// `WEBAUTHN_HMAC_SECRET_SALT`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct HmacSecretSalt {
    pub cb_first: u32,
    pub pb_first: *mut u8,
    pub cb_second: u32,
    pub pb_second: *mut u8,
}

// ------------------------------------------------------------------ webauthnplugin.h: decoded CTAP

/// `WEBAUTHN_CTAPCBOR_AUTHENTICATOR_OPTIONS`. Each option: +1 true, 0 undefined, -1 false.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CtapAuthenticatorOptions {
    pub version: u32,
    pub up: i32,
    pub uv: i32,
    pub require_resident_key: i32,
}

impl CtapAuthenticatorOptions {
    /// Map the tri-state `LONG` (+1 / 0 / -1) to `Some(true)` / `None` / `Some(false)`.
    pub fn tri(value: i32) -> Option<bool> {
        match value {
            0 => None,
            v => Some(v > 0),
        }
    }
    pub fn up(&self) -> Option<bool> {
        Self::tri(self.up)
    }
    pub fn uv(&self) -> Option<bool> {
        Self::tri(self.uv)
    }
    pub fn resident_key(&self) -> Option<bool> {
        Self::tri(self.require_resident_key)
    }
}

/// `WEBAUTHN_CTAPCBOR_ECC_PUBLIC_KEY`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CtapEccPublicKey {
    pub version: u32,
    pub kty: i32,
    pub alg: i32,
    pub crv: i32,
    pub cb_x: u32,
    pub pb_x: *mut u8,
    pub cb_y: u32,
    pub pb_y: *mut u8,
}

/// `WEBAUTHN_CTAPCBOR_HMAC_SALT_EXTENSION`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CtapHmacSaltExtension {
    pub version: u32,
    pub key_agreement: *mut CtapEccPublicKey,
    pub cb_encrypted_salt: u32,
    pub pb_encrypted_salt: *mut u8,
    pub cb_salt_auth: u32,
    pub pb_salt_auth: *mut u8,
}

/// `WEBAUTHN_CTAPCBOR_MAKE_CREDENTIAL_REQUEST` (version 1), every field in header order. Filled by
/// `WebAuthNDecodeMakeCredentialRequest`; free it with `WebAuthNFreeDecodedMakeCredentialRequest`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct MakeCredentialRequest {
    pub version: u32,
    /// Raw UTF-8 bytes of the RP id: hash these for `rpIdHash`.
    pub cb_rp_id: u32,
    pub pb_rp_id: *mut u8,
    pub cb_client_data_hash: u32,
    pub pb_client_data_hash: *mut u8,
    pub rp_information: *const RpEntity,
    pub user_information: *const UserEntity,
    /// `pubKeyCredParams`, by value.
    pub credential_parameters: CoseCredentialParameters,
    /// The exclude list, by value.
    pub credential_list: CredentialList,
    pub cb_cbor_extensions_map: u32,
    pub pb_cbor_extensions_map: *mut u8,
    pub authenticator_options: *mut CtapAuthenticatorOptions,
    /// A zero-length PinAuth was included in the request.
    pub empty_pin_auth: Bool,
    pub cb_pin_auth: u32,
    pub pb_pin_auth: *mut u8,
    pub hmac_secret_ext: i32,
    pub hmac_secret_mc_extension: *mut CtapHmacSaltExtension,
    pub prf_ext: i32,
    pub cb_hmac_secret_salt_values: u32,
    pub pb_hmac_secret_salt_values: *mut u8,
    pub cred_protect: u32,
    pub pin_protocol: u32,
    pub enterprise_attestation: u32,
    pub cb_cred_blob_ext: u32,
    pub pb_cred_blob_ext: *mut u8,
    pub large_blob_key_ext: i32,
    pub large_blob_support: u32,
    pub min_pin_length_ext: i32,
    pub cb_json_ext: u32,
    pub pb_json_ext: *mut u8,
}

/// `WEBAUTHN_CTAPCBOR_GET_ASSERTION_REQUEST` (version 1), every field in header order. Filled by
/// `WebAuthNDecodeGetAssertionRequest`; free it with `WebAuthNFreeDecodedGetAssertionRequest`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GetAssertionRequest {
    pub version: u32,
    /// The RP id as UTF-16 (`pwszRpId`).
    pub rp_id_utf16: *const u16,
    /// The RP id as raw UTF-8 bytes: hash these for `rpIdHash`.
    pub cb_rp_id: u32,
    pub pb_rp_id: *mut u8,
    pub cb_client_data_hash: u32,
    pub pb_client_data_hash: *mut u8,
    /// The allow list, by value.
    pub credential_list: CredentialList,
    pub cb_cbor_extensions_map: u32,
    pub pb_cbor_extensions_map: *mut u8,
    pub authenticator_options: *mut CtapAuthenticatorOptions,
    pub empty_pin_auth: Bool,
    pub cb_pin_auth: u32,
    pub pb_pin_auth: *mut u8,
    pub hmac_salt_extension: *mut CtapHmacSaltExtension,
    pub cb_hmac_secret_salt_values: u32,
    pub pb_hmac_secret_salt_values: *mut u8,
    pub pin_protocol: u32,
    pub cred_blob_ext: i32,
    pub large_blob_key_ext: i32,
    pub cred_large_blob_operation: u32,
    pub cb_cred_large_blob_compressed: u32,
    pub pb_cred_large_blob_compressed: *mut u8,
    pub cred_large_blob_original_size: u32,
    pub cb_json_ext: u32,
    pub pb_json_ext: *mut u8,
}

// ------------------------------------------------------------------ webauthn.h: response side

/// `WEBAUTHN_CREDENTIAL_ATTESTATION` version 8 (input of `WebAuthNEncodeMakeCredentialResponse`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CredentialAttestation {
    pub version: u32,
    pub format_type: *const u16,
    pub cb_authenticator_data: u32,
    pub pb_authenticator_data: *mut u8,
    pub cb_attestation: u32,
    pub pb_attestation: *mut u8,
    pub attestation_decode_type: u32,
    pub attestation_decode: *mut c_void,
    pub cb_attestation_object: u32,
    pub pb_attestation_object: *mut u8,
    pub cb_credential_id: u32,
    pub pb_credential_id: *mut u8,
    pub extensions: Extensions,
    pub used_transport: u32,
    pub ep_att: Bool,
    pub large_blob_supported: Bool,
    pub resident_key: Bool,
    pub prf_enabled: Bool,
    pub cb_unsigned_extension_outputs: u32,
    pub pb_unsigned_extension_outputs: *mut u8,
    pub hmac_secret: *mut HmacSecretSalt,
    pub third_party_payment: Bool,
    pub transports: u32,
    pub cb_client_data_json: u32,
    pub pb_client_data_json: *mut u8,
    pub cb_registration_response_json: u32,
    pub pb_registration_response_json: *mut u8,
}

/// `WEBAUTHN_ASSERTION` version 6.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Assertion {
    pub version: u32,
    pub cb_authenticator_data: u32,
    pub pb_authenticator_data: *mut u8,
    pub cb_signature: u32,
    pub pb_signature: *mut u8,
    pub credential: Credential,
    pub cb_user_id: u32,
    pub pb_user_id: *mut u8,
    pub extensions: Extensions,
    pub cb_cred_large_blob: u32,
    pub pb_cred_large_blob: *mut u8,
    pub cred_large_blob_status: u32,
    pub hmac_secret: *mut HmacSecretSalt,
    pub used_transport: u32,
    pub cb_unsigned_extension_outputs: u32,
    pub pb_unsigned_extension_outputs: *mut u8,
    pub cb_client_data_json: u32,
    pub pb_client_data_json: *mut u8,
    pub cb_authentication_response_json: u32,
    pub pb_authentication_response_json: *mut u8,
}

/// `WEBAUTHN_CTAPCBOR_GET_ASSERTION_RESPONSE` (input of `WebAuthNEncodeGetAssertionResponse`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GetAssertionResponse {
    pub assertion: Assertion,
    pub user_information: *const UserEntity,
    pub number_of_credentials: u32,
    /// +1 = the user picked this credential, 0 = undefined, -1 = not picked.
    pub user_selected: i32,
    pub cb_large_blob_key: u32,
    pub pb_large_blob_key: *mut u8,
    pub cb_unsigned_extension_outputs: u32,
    pub pb_unsigned_extension_outputs: *mut u8,
}

zeroed_default!(
    OperationRequest,
    OperationResponse,
    CancelRequest,
    AddAuthenticatorOptions,
    AddAuthenticatorResponse,
    UpdateAuthenticatorDetails,
    PluginCredentialDetails,
    UserVerificationRequest,
    RpEntity,
    UserEntity,
    CoseCredentialParameter,
    CoseCredentialParameters,
    Credential,
    CredentialEx,
    CredentialList,
    Extension,
    Extensions,
    HmacSecretSalt,
    CtapAuthenticatorOptions,
    CtapEccPublicKey,
    CtapHmacSaltExtension,
    MakeCredentialRequest,
    GetAssertionRequest,
    CredentialAttestation,
    Assertion,
    GetAssertionResponse,
);
