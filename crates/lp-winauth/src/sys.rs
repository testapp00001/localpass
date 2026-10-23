//! Hand-written bindings for the FINAL Windows WebAuthn plugin authenticator API.
//!
//! - [`types`]: the `#[repr(C)]` structs, pinned field by field to the SDK headers in `layout_tests`.
//! - [`api`]: [`WebAuthn`], the run-time-bound `webauthn.dll` exports with thin wrappers, and the RAII
//!   owners of every OS-allocated buffer.
//!
//! The 21 `WebAuthNPlugin*` functions and `WebAuthNDecode*/Encode*` are exports of `webauthn.dll`; the SDK
//! import library has none of them (notes F1). `EXPERIMENTAL_*` and `*2` functions and
//! `WebAuthNPluginSetAuthenticatorState` are deliberately not bound. All layouts are x64.

mod api;
#[cfg(test)]
mod api_tests;
#[cfg(test)]
mod layout_tests;
mod types;

use windows::core::HRESULT;

pub use api::*;
pub use types::*;

/// `WEBAUTHN_PLUGIN_REQUEST_TYPE_CTAP2_CBOR`.
pub const REQUEST_TYPE_CTAP2_CBOR: u32 = 1;
/// `PLUGIN_LOCK_STATUS`.
pub const PLUGIN_LOCKED: i32 = 0;
pub const PLUGIN_UNLOCKED: i32 = 1;
/// `AUTHENTICATOR_STATE`.
pub const STATE_DISABLED: i32 = 0;
pub const STATE_ENABLED: i32 = 1;

/// Struct versions this crate writes (`*_CURRENT_VERSION` in the headers).
pub const RP_ENTITY_VERSION: u32 = 1;
pub const USER_ENTITY_VERSION: u32 = 1;
pub const COSE_PARAMETER_VERSION: u32 = 1;
pub const CREDENTIAL_VERSION: u32 = 1;
pub const CREDENTIAL_EX_VERSION: u32 = 1;
/// `WEBAUTHN_CREDENTIAL_ATTESTATION_CURRENT_VERSION`.
pub const ATTESTATION_VERSION: u32 = 8;
/// `WEBAUTHN_ASSERTION_CURRENT_VERSION`.
pub const ASSERTION_VERSION: u32 = 6;
pub const CTAP_AUTHENTICATOR_OPTIONS_VERSION: u32 = 1;
pub const CTAP_MAKE_CREDENTIAL_REQUEST_VERSION: u32 = 1;
/// `WEBAUTHN_ATTESTATION_DECODE_NONE`.
pub const ATTESTATION_DECODE_NONE: u32 = 0;
/// `WEBAUTHN_COSE_ALGORITHM_ECDSA_P256_WITH_SHA256`.
pub const COSE_ALG_ES256: i32 = -7;
/// `WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY`.
pub const CREDENTIAL_TYPE_PUBLIC_KEY: &str = "public-key";

const fn hr(code: u32) -> HRESULT {
    HRESULT(code as i32)
}

/// The user said no, or the operation was cancelled.
pub const NTE_USER_CANCELLED: HRESULT = hr(0x8009_0036);
/// No such credential, or the authenticator is not registered.
pub const NTE_NOT_FOUND: HRESULT = hr(0x8009_0011);
/// Exclude list hit (the browser reports `InvalidStateError`).
pub const NTE_EXISTS: HRESULT = hr(0x8009_000F);
pub const NTE_NOT_SUPPORTED: HRESULT = hr(0x8009_0029);
pub const NTE_INVALID_PARAMETER: HRESULT = hr(0x8009_0027);
pub const NTE_BAD_SIGNATURE: HRESULT = hr(0x8009_0006);
pub const NTE_DEVICE_NOT_FOUND: HRESULT = hr(0x8009_0035);
/// `HRESULT_FROM_WIN32(ERROR_BUSY)`.
pub const HR_BUSY: HRESULT = hr(0x8007_00AA);
/// `HRESULT_FROM_WIN32(ERROR_CANCELLED)`.
pub const HR_CANCELLED: HRESULT = hr(0x8007_04C7);
/// `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`.
pub const HR_TIMEOUT: HRESULT = hr(0x8007_05B4);

/// A byte slice from a pointer and length that the OS handed over (empty for a null pointer).
///
/// # Safety
/// `ptr` must be null or point to `len` readable bytes that outlive `'a`.
pub unsafe fn bytes<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees `ptr` covers `len` bytes.
        unsafe { std::slice::from_raw_parts(ptr, len as usize) }
    }
}

/// A NUL-terminated UTF-16 string as `String` (`<null>` for a null pointer), for log lines.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn wstr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return "<null>".to_owned();
    }
    let mut len = 0;
    // SAFETY: the caller guarantees a NUL terminator, so the scan stops inside the allocation.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` code units were just read.
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(ptr, len) })
}

/// NUL-terminated UTF-16, the form every `LPCWSTR` parameter wants.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wstr_and_bytes_handle_null() {
        // SAFETY: null pointers are explicitly allowed.
        unsafe {
            assert_eq!(wstr(std::ptr::null()), "<null>");
            assert!(bytes(std::ptr::null(), 5).is_empty());
        }
        let w = wide("rp.example");
        // SAFETY: `wide` NUL-terminates.
        assert_eq!(unsafe { wstr(w.as_ptr()) }, "rp.example");
        assert_eq!(w.last(), Some(&0));
    }

    #[test]
    fn hresult_constants_are_failures() {
        for code in [
            NTE_USER_CANCELLED,
            NTE_NOT_FOUND,
            NTE_EXISTS,
            NTE_NOT_SUPPORTED,
            NTE_INVALID_PARAMETER,
            NTE_BAD_SIGNATURE,
            NTE_DEVICE_NOT_FOUND,
            HR_BUSY,
            HR_CANCELLED,
            HR_TIMEOUT,
            HR_FILE_NOT_FOUND,
        ] {
            assert!(code.is_err());
        }
    }
}
