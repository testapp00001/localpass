//! Run-time binding of the `webauthn.dll` plugin exports, thin wrappers over them, and RAII owners for
//! every buffer the OS allocates.
//!
//! The plugin functions and `WebAuthNDecode*/Encode*` are exports of `webauthn.dll`; the SDK import
//! library has none of them (notes F1), so [`WebAuthn::load`] resolves them with `LoadLibraryExW`
//! (System32 only) + `GetProcAddress`. A missing export is an [`LoadError::Unsupported`], never a crash.
//! The DLL is never unloaded, so the function pointers (and the guards that copy a "free" pointer) stay
//! valid for the life of the process.
//!
//! Deliberately NOT bound: `WebAuthNPluginSetAuthenticatorState` (enabling the provider is the user's
//! Windows Hello-gated choice) and every `EXPERIMENTAL_*` / `*2` function (older API generation, or
//! declared but not exported, notes F2/F3).
//!
//! Ownership summary (notes section 2.6): decoded requests, add responses, public keys, UV responses and
//! credential arrays are freed by their guards' `Drop`; encoded responses are `CoTaskMemAlloc` buffers
//! owned by [`CoTaskMemBuf`], which can hand the allocation to the COM response with
//! [`CoTaskMemBuf::write_to`] / [`CoTaskMemBuf::into_raw`].

use std::ffi::{CStr, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::mem::forget;
use std::ops::Deref;
use std::ptr::{NonNull, null, null_mut};
use std::slice;

use windows::Win32::Foundation::{E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HMODULE};
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree};
use windows::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows::core::{GUID, HRESULT, PCSTR, w};

use super::types::*;
use super::{
    ASSERTION_VERSION, ATTESTATION_VERSION, CREDENTIAL_TYPE_PUBLIC_KEY, CREDENTIAL_VERSION,
    USER_ENTITY_VERSION, bytes, wide,
};

/// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`: what `GetAllCredentials` returns for an empty cache.
pub const HR_FILE_NOT_FOUND: HRESULT = HRESULT(0x8007_0002_u32 as i32);

/// `WEBAUTHN_PLUGIN_STATUS_CHANGE_CALLBACK` (`CALLBACK` = `__stdcall`).
pub type StatusChangeCallback = extern "system" fn(context: *mut c_void);

// ------------------------------------------------------------------ errors

/// Why [`WebAuthn::load`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// `LoadLibraryExW("webauthn.dll")` failed.
    Library(String),
    /// `webauthn.dll` does not export this function: the OS is too old for the plugin API (the first
    /// missing one in [`WebAuthn::EXPORTS`] order; [`WebAuthn::missing_exports`] lists them all).
    Unsupported(&'static str),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Library(e) => write!(f, "cannot load webauthn.dll: {e}"),
            Self::Unsupported(name) => write!(f, "webauthn.dll does not export {name}"),
        }
    }
}

impl std::error::Error for LoadError {}

// ------------------------------------------------------------------ the export table

/// `AUTHENTICATOR_STATE` as the OS reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticatorState {
    Disabled,
    Enabled,
    /// A value this crate does not know.
    Other(i32),
}

impl AuthenticatorState {
    pub fn from_raw(raw: i32) -> Self {
        match raw {
            super::STATE_DISABLED => Self::Disabled,
            super::STATE_ENABLED => Self::Enabled,
            other => Self::Other(other),
        }
    }
    pub fn raw(self) -> i32 {
        match self {
            Self::Disabled => super::STATE_DISABLED,
            Self::Enabled => super::STATE_ENABLED,
            Self::Other(v) => v,
        }
    }
}

type Hr = HRESULT;

macro_rules! webauthn_exports {
    ($($field:ident = $export:literal : $ty:ty),* $(,)?) => {
        /// The `webauthn.dll` plugin exports this crate calls, resolved once. Plain function pointers:
        /// `Send + Sync`, `load()` it into a `OnceLock`.
        pub struct WebAuthn {
            $($field: $ty,)*
        }

        impl WebAuthn {
            /// Names of every export bound by this struct, in table order.
            pub const EXPORTS: &'static [&'static str] = &[$($export),*];

            /// Names `resolve` cannot find, in table order (the whole list, for diagnostics).
            pub fn missing_exports(
                mut resolve: impl FnMut(&CStr) -> Option<*const c_void>,
            ) -> Vec<&'static str> {
                let mut missing = Vec::new();
                $(
                    if resolve(export_name(concat!($export, "\0"))).is_none() {
                        missing.push($export);
                    }
                )*
                missing
            }

            /// Bind the table through `resolve` (name -> address). Fails with the first missing name.
            ///
            /// # Safety
            /// Every address `resolve` returns must be the entry point of a function with the
            /// documented signature of that export (what `GetProcAddress` on `webauthn.dll` gives).
            pub unsafe fn from_resolver(
                mut resolve: impl FnMut(&CStr) -> Option<*const c_void>,
            ) -> Result<WebAuthn, LoadError> {
                Ok(WebAuthn {
                    $($field: {
                        let address = resolve(export_name(concat!($export, "\0")))
                            .ok_or(LoadError::Unsupported($export))?;
                        // SAFETY: the caller promises `address` is this export, whose signature is `$ty`;
                        // a data pointer and a fn pointer have the same size on every Windows target.
                        unsafe { std::mem::transmute::<*const c_void, $ty>(address) }
                    },)*
                })
            }
        }
    };
}

fn export_name(nul_terminated: &'static str) -> &'static CStr {
    CStr::from_bytes_with_nul(nul_terminated.as_bytes())
        .expect("export names have one trailing NUL")
}

webauthn_exports! {
    get_state = "WebAuthNPluginGetAuthenticatorState":
        unsafe extern "system" fn(*const GUID, *mut i32) -> Hr,
    add_authenticator = "WebAuthNPluginAddAuthenticator":
        unsafe extern "system" fn(*const AddAuthenticatorOptions, *mut *mut AddAuthenticatorResponse) -> Hr,
    remove_authenticator = "WebAuthNPluginRemoveAuthenticator":
        unsafe extern "system" fn(*const GUID) -> Hr,
    update_authenticator = "WebAuthNPluginUpdateAuthenticatorDetails":
        unsafe extern "system" fn(*const UpdateAuthenticatorDetails) -> Hr,
    free_add_response = "WebAuthNPluginFreeAddAuthenticatorResponse":
        unsafe extern "system" fn(*mut AddAuthenticatorResponse),
    add_credentials = "WebAuthNPluginAuthenticatorAddCredentials":
        unsafe extern "system" fn(*const GUID, u32, *const PluginCredentialDetails) -> Hr,
    remove_credentials = "WebAuthNPluginAuthenticatorRemoveCredentials":
        unsafe extern "system" fn(*const GUID, u32, *const PluginCredentialDetails) -> Hr,
    remove_all_credentials = "WebAuthNPluginAuthenticatorRemoveAllCredentials":
        unsafe extern "system" fn(*const GUID) -> Hr,
    get_all_credentials = "WebAuthNPluginAuthenticatorGetAllCredentials":
        unsafe extern "system" fn(*const GUID, *mut u32, *mut *mut PluginCredentialDetails) -> Hr,
    free_credential_details = "WebAuthNPluginAuthenticatorFreeCredentialDetailsArray":
        unsafe extern "system" fn(u32, *mut PluginCredentialDetails),
    get_op_sign_key = "WebAuthNPluginGetOperationSigningPublicKey":
        unsafe extern "system" fn(*const GUID, *mut u32, *mut *mut u8) -> Hr,
    get_uv_key = "WebAuthNPluginGetUserVerificationPublicKey":
        unsafe extern "system" fn(*const GUID, *mut u32, *mut *mut u8) -> Hr,
    free_public_key = "WebAuthNPluginFreePublicKeyResponse":
        unsafe extern "system" fn(*mut u8),
    perform_uv = "WebAuthNPluginPerformUserVerification":
        unsafe extern "system" fn(*const UserVerificationRequest, *mut u32, *mut *mut u8) -> Hr,
    free_uv_response = "WebAuthNPluginFreeUserVerificationResponse":
        unsafe extern "system" fn(*mut u8),
    register_status_callback = "WebAuthNPluginRegisterStatusChangeCallback":
        unsafe extern "system" fn(StatusChangeCallback, *mut c_void, *const GUID, *mut u32) -> Hr,
    unregister_status_callback = "WebAuthNPluginUnregisterStatusChangeCallback":
        unsafe extern "system" fn(*mut u32) -> Hr,
    decode_make_credential = "WebAuthNDecodeMakeCredentialRequest":
        unsafe extern "system" fn(u32, *const u8, *mut *mut MakeCredentialRequest) -> Hr,
    free_decoded_make_credential = "WebAuthNFreeDecodedMakeCredentialRequest":
        unsafe extern "system" fn(*mut MakeCredentialRequest),
    decode_get_assertion = "WebAuthNDecodeGetAssertionRequest":
        unsafe extern "system" fn(u32, *const u8, *mut *mut GetAssertionRequest) -> Hr,
    free_decoded_get_assertion = "WebAuthNFreeDecodedGetAssertionRequest":
        unsafe extern "system" fn(*mut GetAssertionRequest),
    encode_make_credential = "WebAuthNEncodeMakeCredentialResponse":
        unsafe extern "system" fn(*const CredentialAttestation, *mut u32, *mut *mut u8) -> Hr,
    encode_get_assertion = "WebAuthNEncodeGetAssertionResponse":
        unsafe extern "system" fn(*const GetAssertionResponse, *mut u32, *mut *mut u8) -> Hr,
}

impl fmt::Debug for WebAuthn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebAuthn")
            .field("exports", &Self::EXPORTS.len())
            .finish_non_exhaustive()
    }
}

/// Every final-API export the notes list (section 2.4/2.5), probed for presence only by
/// [`probe_exports`]. One more than [`WebAuthn::EXPORTS`]: `GetUserVerificationCount` is not bound.
pub const NOTES_EXPORTS: &[&str] = &[
    "WebAuthNPluginGetAuthenticatorState",
    "WebAuthNPluginAddAuthenticator",
    "WebAuthNPluginFreeAddAuthenticatorResponse",
    "WebAuthNPluginRemoveAuthenticator",
    "WebAuthNPluginUpdateAuthenticatorDetails",
    "WebAuthNPluginAuthenticatorAddCredentials",
    "WebAuthNPluginAuthenticatorRemoveCredentials",
    "WebAuthNPluginAuthenticatorRemoveAllCredentials",
    "WebAuthNPluginAuthenticatorGetAllCredentials",
    "WebAuthNPluginAuthenticatorFreeCredentialDetailsArray",
    "WebAuthNPluginGetOperationSigningPublicKey",
    "WebAuthNPluginGetUserVerificationPublicKey",
    "WebAuthNPluginGetUserVerificationCount",
    "WebAuthNPluginFreePublicKeyResponse",
    "WebAuthNPluginPerformUserVerification",
    "WebAuthNPluginFreeUserVerificationResponse",
    "WebAuthNPluginRegisterStatusChangeCallback",
    "WebAuthNPluginUnregisterStatusChangeCallback",
    "WebAuthNDecodeMakeCredentialRequest",
    "WebAuthNFreeDecodedMakeCredentialRequest",
    "WebAuthNDecodeGetAssertionRequest",
    "WebAuthNFreeDecodedGetAssertionRequest",
    "WebAuthNEncodeMakeCredentialResponse",
    "WebAuthNEncodeGetAssertionResponse",
];

fn load_module() -> Result<HMODULE, LoadError> {
    // SAFETY: plain library load of a system DLL by name, restricted to System32. Never freed.
    unsafe { LoadLibraryExW(w!("webauthn.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) }
        .map_err(|e| LoadError::Library(e.to_string()))
}

fn resolve_in(module: HMODULE, name: &CStr) -> Option<*const c_void> {
    // SAFETY: `module` is a loaded module and `name` is NUL-terminated.
    unsafe { GetProcAddress(module, PCSTR(name.as_ptr().cast())) }.map(|f| f as *const c_void)
}

/// Which of [`NOTES_EXPORTS`] (and the two exports the notes say not to use: the unexported
/// `EXPERIMENTAL_WebAuthNPluginAddAuthenticator2` and `WebAuthNPluginSetAuthenticatorState`) the OS has.
/// Presence only: nothing is called.
pub fn probe_exports() -> Result<Vec<(String, bool)>, LoadError> {
    let module = load_module()?;
    let extra = [
        "EXPERIMENTAL_WebAuthNPluginAddAuthenticator2",
        "WebAuthNPluginSetAuthenticatorState",
    ];
    Ok(NOTES_EXPORTS
        .iter()
        .chain(extra.iter())
        .map(|name| {
            let c = format!("{name}\0");
            let present = CStr::from_bytes_with_nul(c.as_bytes())
                .ok()
                .and_then(|c| resolve_in(module, c))
                .is_some();
            ((*name).to_owned(), present)
        })
        .collect())
}

fn check(hr: HRESULT) -> Result<(), HRESULT> {
    if hr.is_err() { Err(hr) } else { Ok(()) }
}

fn len32(data: &[u8]) -> Result<u32, HRESULT> {
    u32::try_from(data.len()).map_err(|_| E_INVALIDARG)
}

/// `NULL` for an empty slice, else the slice's data pointer as the headers' mutable `PBYTE`. The OS only
/// reads input buffers; `*mut` is the header's spelling.
fn pbyte(data: &[u8]) -> *mut u8 {
    if data.is_empty() {
        null_mut()
    } else {
        data.as_ptr().cast_mut()
    }
}

// ------------------------------------------------------------------ owners of OS buffers

/// Bytes allocated by `webauthn.dll` (a public key or a user-verification response), freed on drop with
/// the export that matches how they were obtained.
#[derive(Debug)]
pub struct OsBytes {
    ptr: *mut u8,
    len: u32,
    free: unsafe extern "system" fn(*mut u8),
}

impl OsBytes {
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: the API returned `len` readable bytes at `ptr`, valid until `free` runs in `drop`.
        unsafe { bytes(self.ptr, self.len) }
    }
}

impl Deref for OsBytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        self.as_slice()
    }
}

impl Drop for OsBytes {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: `ptr` came from the matching getter and has not been freed.
            unsafe { (self.free)(self.ptr) };
        }
    }
}

/// The response of `WebAuthNPluginAddAuthenticator`: the operation-signing public key blob.
#[derive(Debug)]
pub struct AddResponse {
    ptr: *mut AddAuthenticatorResponse,
    free: unsafe extern "system" fn(*mut AddAuthenticatorResponse),
}

impl AddResponse {
    /// The `BCRYPT_PUBLIC_KEY_BLOB` (empty if the OS returned none).
    pub fn op_sign_public_key(&self) -> &[u8] {
        // SAFETY: a non-null response stays valid until `free` runs in `drop`, and so do its buffers.
        unsafe {
            match self.ptr.as_ref() {
                Some(r) => bytes(r.pb_op_sign_pub_key, r.cb_op_sign_pub_key),
                None => &[],
            }
        }
    }
}

impl Drop for AddResponse {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: allocated by AddAuthenticator, freed exactly once.
            unsafe { (self.free)(self.ptr) };
        }
    }
}

/// A credential-cache row with owned data (what the safe cache wrappers take and give).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRow {
    pub credential_id: Vec<u8>,
    pub rp_id: String,
    pub rp_name: String,
    pub user_id: Vec<u8>,
    pub user_name: String,
    pub user_display_name: String,
}

/// The array `WebAuthNPluginAuthenticatorGetAllCredentials` returns, freed on drop.
#[derive(Debug)]
pub struct CredentialDetailsArray {
    count: u32,
    ptr: *mut PluginCredentialDetails,
    free: unsafe extern "system" fn(u32, *mut PluginCredentialDetails),
}

impl CredentialDetailsArray {
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }
    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
    /// The raw rows (pointers inside are valid while `self` lives).
    pub fn as_slice(&self) -> &[PluginCredentialDetails] {
        if self.ptr.is_null() || self.count == 0 {
            &[]
        } else {
            // SAFETY: the API returned `count` consecutive structs at `ptr`, valid until `free`.
            unsafe { slice::from_raw_parts(self.ptr, self.count as usize) }
        }
    }
    /// Copy the rows out as owned data.
    pub fn rows(&self) -> Vec<CredentialRow> {
        self.as_slice()
            .iter()
            .map(|d| {
                // SAFETY: the row's pointers are valid while the array lives (NUL-terminated UTF-16 and
                // byte buffers with their counts).
                unsafe {
                    CredentialRow {
                        credential_id: bytes(d.pb_credential_id, d.cb_credential_id).to_vec(),
                        rp_id: wstring(d.rp_id),
                        rp_name: wstring(d.rp_name),
                        user_id: bytes(d.pb_user_id, d.cb_user_id).to_vec(),
                        user_name: wstring(d.user_name),
                        user_display_name: wstring(d.user_display_name),
                    }
                }
            })
            .collect()
    }
}

impl Drop for CredentialDetailsArray {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: allocated by GetAllCredentials with this count, freed exactly once.
            unsafe { (self.free)(self.count, self.ptr) };
        }
    }
}

/// A registered status-change callback; unregistered on drop.
#[derive(Debug)]
pub struct StatusRegistration {
    cookie: u32,
    unregister: unsafe extern "system" fn(*mut u32) -> Hr,
}

impl Drop for StatusRegistration {
    fn drop(&mut self) {
        // SAFETY: `cookie` is the registration handle the OS gave us.
        let _ = unsafe { (self.unregister)(&mut self.cookie) };
    }
}

/// A buffer allocated with `CoTaskMemAlloc` by `WebAuthNEncode*Response`. Freed on drop, unless the
/// allocation is handed to the COM response with [`write_to`](Self::write_to) or
/// [`into_raw`](Self::into_raw) (the COM stub then owns and frees it).
pub struct CoTaskMemBuf {
    ptr: *mut u8,
    len: u32,
}

impl CoTaskMemBuf {
    /// Take ownership of a `CoTaskMemAlloc` allocation of `len` bytes.
    ///
    /// # Safety
    /// `ptr` must be null or a live `CoTaskMemAlloc` allocation of at least `len` bytes, owned by the caller.
    pub unsafe fn from_raw(ptr: *mut u8, len: u32) -> Self {
        Self { ptr, len }
    }
    /// A new `CoTaskMemAlloc` block holding a copy of `data`. Every answer to the OS needs its own block:
    /// the stub frees what it is given, so one block must never be returned twice.
    pub fn copy_of(data: &[u8]) -> Result<Self, HRESULT> {
        let len = u32::try_from(data.len()).map_err(|_| E_OUTOFMEMORY)?;
        // SAFETY: plain allocation; the copy stays inside the `data.len()` bytes just allocated.
        unsafe {
            let ptr = CoTaskMemAlloc(data.len().max(1)).cast::<u8>();
            if ptr.is_null() {
                return Err(E_OUTOFMEMORY);
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            Ok(Self { ptr, len })
        }
    }
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY: `ptr` is a live allocation of `len` bytes (constructor invariant), freed only in drop.
        unsafe { bytes(self.ptr, self.len) }
    }
    pub fn len(&self) -> usize {
        self.len as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Give the allocation away as `(length, pointer)`; the receiver must `CoTaskMemFree` it.
    pub fn into_raw(self) -> (u32, *mut u8) {
        let raw = (self.len, self.ptr);
        forget(self);
        raw
    }
    /// Move the allocation into the COM response (`cbEncodedResponse`, `pbEncodedResponse`). The stub
    /// frees it after the call returns `S_OK`; on a failure return leave it empty but NON-NULL (a 1-byte placeholder with length 0): the stub rejects a NULL reference pointer, see `com.rs`.
    pub fn write_to(self, response: &mut OperationResponse) {
        let (len, ptr) = self.into_raw();
        response.cb_encoded_response = len;
        response.pb_encoded_response = ptr;
    }
}

impl Drop for CoTaskMemBuf {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // SAFETY: the buffer is a CoTaskMemAlloc allocation owned by `self`.
            unsafe { CoTaskMemFree(Some(self.ptr.cast_const().cast())) };
        }
    }
}

impl fmt::Debug for CoTaskMemBuf {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoTaskMemBuf({} bytes)", self.len)
    }
}

// ------------------------------------------------------------------ decoded requests

/// Owned summary of the RP entity of a MakeCredential request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpInfo {
    pub id: String,
    pub name: String,
}

/// Owned summary of the user entity of a MakeCredential request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserInfo {
    pub id: Vec<u8>,
    pub name: String,
    pub display_name: String,
}

/// A decoded `MakeCredential` request. Derefs to the raw struct; the accessors copy the common fields
/// out safely. Freed on drop. Borrows the encoded input in case the decoder points into it.
#[derive(Debug)]
pub struct DecodedMakeCredential<'a> {
    ptr: NonNull<MakeCredentialRequest>,
    free: unsafe extern "system" fn(*mut MakeCredentialRequest),
    _input: PhantomData<&'a [u8]>,
}

impl Deref for DecodedMakeCredential<'_> {
    type Target = MakeCredentialRequest;
    fn deref(&self) -> &MakeCredentialRequest {
        // SAFETY: the decoder returned a valid struct that lives until `free` runs in `drop`.
        unsafe { self.ptr.as_ref() }
    }
}

impl DecodedMakeCredential<'_> {
    /// The RP id exactly as the platform sent it (UTF-8): hash these bytes for `rpIdHash`.
    pub fn rp_id_utf8(&self) -> &[u8] {
        // SAFETY: decoder-owned buffer with its count, valid while `self` lives.
        unsafe { bytes(self.pb_rp_id, self.cb_rp_id) }
    }
    pub fn rp_id(&self) -> String {
        String::from_utf8_lossy(self.rp_id_utf8()).into_owned()
    }
    pub fn client_data_hash(&self) -> &[u8] {
        // SAFETY: as above.
        unsafe { bytes(self.pb_client_data_hash, self.cb_client_data_hash) }
    }
    pub fn rp(&self) -> Option<RpInfo> {
        // SAFETY: a non-null entity pointer is decoder-owned and valid while `self` lives.
        unsafe {
            self.rp_information.as_ref().map(|rp| RpInfo {
                id: wstring(rp.id),
                name: wstring(rp.name),
            })
        }
    }
    pub fn user(&self) -> Option<UserInfo> {
        // SAFETY: as above.
        unsafe { self.user_information.as_ref().map(|user| user_info(user)) }
    }
    /// `(credential type, COSE algorithm)` for each `pubKeyCredParams` entry.
    pub fn credential_parameters(&self) -> Vec<(String, i32)> {
        // SAFETY: decoder-owned array of `count` structs.
        unsafe { cose_parameters(&self.credential_parameters) }
    }
    /// The COSE algorithm ids, in request order.
    pub fn algorithms(&self) -> Vec<i32> {
        self.credential_parameters()
            .into_iter()
            .map(|(_, alg)| alg)
            .collect()
    }
    /// Credential ids of the exclude list.
    pub fn exclude_credential_ids(&self) -> Vec<Vec<u8>> {
        // SAFETY: decoder-owned array of pointers to credential structs.
        unsafe { credential_ids(&self.credential_list) }
    }
    pub fn options(&self) -> Option<CtapAuthenticatorOptions> {
        // SAFETY: a non-null options pointer is decoder-owned.
        unsafe { self.authenticator_options.as_ref().copied() }
    }
}

impl Drop for DecodedMakeCredential<'_> {
    fn drop(&mut self) {
        // SAFETY: freeing exactly what the decoder returned, once.
        unsafe { (self.free)(self.ptr.as_ptr()) };
    }
}

/// A decoded `GetAssertion` request; see [`DecodedMakeCredential`].
#[derive(Debug)]
pub struct DecodedGetAssertion<'a> {
    ptr: NonNull<GetAssertionRequest>,
    free: unsafe extern "system" fn(*mut GetAssertionRequest),
    _input: PhantomData<&'a [u8]>,
}

impl Deref for DecodedGetAssertion<'_> {
    type Target = GetAssertionRequest;
    fn deref(&self) -> &GetAssertionRequest {
        // SAFETY: the decoder returned a valid struct that lives until `free` runs in `drop`.
        unsafe { self.ptr.as_ref() }
    }
}

impl DecodedGetAssertion<'_> {
    /// The RP id as UTF-8 bytes: hash these for `rpIdHash`.
    pub fn rp_id_utf8(&self) -> &[u8] {
        // SAFETY: decoder-owned buffer with its count, valid while `self` lives.
        unsafe { bytes(self.pb_rp_id, self.cb_rp_id) }
    }
    pub fn rp_id(&self) -> String {
        String::from_utf8_lossy(self.rp_id_utf8()).into_owned()
    }
    pub fn client_data_hash(&self) -> &[u8] {
        // SAFETY: as above.
        unsafe { bytes(self.pb_client_data_hash, self.cb_client_data_hash) }
    }
    /// Credential ids of the allow list (empty = any discoverable credential).
    pub fn allow_credential_ids(&self) -> Vec<Vec<u8>> {
        // SAFETY: decoder-owned array of pointers to credential structs.
        unsafe { credential_ids(&self.credential_list) }
    }
    pub fn options(&self) -> Option<CtapAuthenticatorOptions> {
        // SAFETY: a non-null options pointer is decoder-owned.
        unsafe { self.authenticator_options.as_ref().copied() }
    }
}

impl Drop for DecodedGetAssertion<'_> {
    fn drop(&mut self) {
        // SAFETY: freeing exactly what the decoder returned, once.
        unsafe { (self.free)(self.ptr.as_ptr()) };
    }
}

/// UTF-16 to `String`, empty for NULL.
///
/// # Safety
/// `ptr` must be null or NUL-terminated UTF-16.
unsafe fn wstring(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let mut len = 0;
    // SAFETY: the caller guarantees a NUL terminator inside the allocation.
    while unsafe { *ptr.add(len) } != 0 {
        len += 1;
    }
    // SAFETY: `len` code units were just read.
    String::from_utf16_lossy(unsafe { slice::from_raw_parts(ptr, len) })
}

/// # Safety
/// `user` must point to a valid entity with valid buffers.
unsafe fn user_info(user: &UserEntity) -> UserInfo {
    // SAFETY: per the function contract.
    unsafe {
        UserInfo {
            id: bytes(user.pb_id, user.cb_id).to_vec(),
            name: wstring(user.name),
            display_name: wstring(user.display_name),
        }
    }
}

/// # Safety
/// `list.params` must be null or point to `list.count` structs with NUL-terminated type strings.
unsafe fn cose_parameters(list: &CoseCredentialParameters) -> Vec<(String, i32)> {
    if list.params.is_null() || list.count == 0 {
        return Vec::new();
    }
    // SAFETY: per the function contract.
    unsafe { slice::from_raw_parts(list.params, list.count as usize) }
        .iter()
        // SAFETY: the type string belongs to the same decoder allocation.
        .map(|p| (unsafe { wstring(p.credential_type) }, p.alg))
        .collect()
}

/// # Safety
/// `list.credentials` must be null or point to `list.count` pointers to valid credential structs.
unsafe fn credential_ids(list: &CredentialList) -> Vec<Vec<u8>> {
    if list.credentials.is_null() || list.count == 0 {
        return Vec::new();
    }
    // SAFETY: per the function contract.
    unsafe { slice::from_raw_parts(list.credentials, list.count as usize) }
        .iter()
        // SAFETY: each non-null entry is a decoder-owned struct with its id buffer.
        .filter_map(|&c| unsafe { c.as_ref() })
        .map(|c| unsafe { bytes(c.pb_id, c.cb_id) }.to_vec())
        .collect()
}

// ------------------------------------------------------------------ safe inputs

/// Everything `AddAuthenticator` / `UpdateAuthenticatorDetails` take, as plain Rust data.
#[derive(Debug, Clone)]
pub struct AuthenticatorDetails<'a> {
    pub name: &'a str,
    pub clsid: GUID,
    /// Add only: `None` is sent as NULL, which the OS rejects (`NTE_INVALID_PARAMETER`).
    pub plugin_rp_id: Option<&'a str>,
    /// Base64 of a UTF-8 SVG 1.1 (passed as a wide string).
    pub light_logo_svg: Option<&'a str>,
    pub dark_logo_svg: Option<&'a str>,
    /// CTAP2 `authenticatorGetInfo` CBOR (see `register::authenticator_info`).
    pub authenticator_info: &'a [u8],
    /// Empty = all relying parties.
    pub supported_rp_ids: &'a [&'a str],
}

/// The UTF-16 copies the option structs point into; keep alive across the call.
struct WideDetails {
    name: Vec<u16>,
    rp_id: Option<Vec<u16>>,
    light: Option<Vec<u16>>,
    dark: Option<Vec<u16>>,
    // Keeps the strings alive; `rp_ptrs` points into them.
    _rps: Vec<Vec<u16>>,
    rp_ptrs: Vec<*const u16>,
}

impl WideDetails {
    fn new(d: &AuthenticatorDetails<'_>) -> Self {
        let rps: Vec<Vec<u16>> = d.supported_rp_ids.iter().map(|s| wide(s)).collect();
        let rp_ptrs = rps.iter().map(|v| v.as_ptr()).collect();
        Self {
            name: wide(d.name),
            rp_id: d.plugin_rp_id.map(wide),
            light: d.light_logo_svg.map(wide),
            dark: d.dark_logo_svg.map(wide),
            _rps: rps,
            rp_ptrs,
        }
    }
    fn rp_ptrs(&self) -> *const *const u16 {
        if self.rp_ptrs.is_empty() {
            null()
        } else {
            self.rp_ptrs.as_ptr()
        }
    }
}

fn opt_ptr(v: &Option<Vec<u16>>) -> *const u16 {
    v.as_ref().map_or(null(), |v| v.as_ptr())
}

/// Wide-string and byte buffers behind a slice of [`PluginCredentialDetails`].
struct WideRows {
    strings: Vec<[Vec<u16>; 4]>,
}

impl WideRows {
    fn new(rows: &[CredentialRow]) -> Self {
        Self {
            strings: rows
                .iter()
                .map(|r| {
                    [
                        wide(&r.rp_id),
                        wide(&r.rp_name),
                        wide(&r.user_name),
                        wide(&r.user_display_name),
                    ]
                })
                .collect(),
        }
    }
    fn details(&self, rows: &[CredentialRow]) -> Result<Vec<PluginCredentialDetails>, HRESULT> {
        rows.iter()
            .zip(&self.strings)
            .map(|(r, s)| {
                Ok(PluginCredentialDetails {
                    cb_credential_id: len32(&r.credential_id)?,
                    pb_credential_id: pbyte(&r.credential_id),
                    rp_id: s[0].as_ptr(),
                    rp_name: s[1].as_ptr(),
                    cb_user_id: len32(&r.user_id)?,
                    pb_user_id: pbyte(&r.user_id),
                    user_name: s[2].as_ptr(),
                    user_display_name: s[3].as_ptr(),
                })
            })
            .collect()
    }
}

/// A user for [`AssertionOutput`].
#[derive(Debug, Clone, Copy)]
pub struct AssertionUser<'a> {
    pub id: &'a [u8],
    pub name: &'a str,
    pub display_name: &'a str,
}

/// What a GetAssertion response carries, as plain Rust data.
#[derive(Debug, Clone, Copy)]
pub struct AssertionOutput<'a> {
    pub auth_data: &'a [u8],
    /// DER ECDSA signature over `authData || clientDataHash`.
    pub signature: &'a [u8],
    pub credential_id: &'a [u8],
    pub user: Option<AssertionUser<'a>>,
    pub number_of_credentials: u32,
    pub user_selected: bool,
}

// ------------------------------------------------------------------ the wrappers

impl WebAuthn {
    /// Load `webauthn.dll` from System32 and resolve every export in [`WebAuthn::EXPORTS`].
    pub fn load() -> Result<WebAuthn, LoadError> {
        let module = load_module()?;
        // SAFETY: GetProcAddress on webauthn.dll returns the export with its documented signature.
        unsafe { WebAuthn::from_resolver(|name| resolve_in(module, name)) }
    }

    // ---- registration

    /// `WebAuthNPluginGetAuthenticatorState`. `Err(NTE_NOT_FOUND)` = not registered.
    pub fn authenticator_state(&self, clsid: &GUID) -> Result<AuthenticatorState, HRESULT> {
        let mut state = -1;
        // SAFETY: valid GUID and out pointer.
        check(unsafe { (self.get_state)(clsid, &mut state) })?;
        Ok(AuthenticatorState::from_raw(state))
    }

    /// `WebAuthNPluginAddAuthenticator`. Needs package identity. The authenticator starts disabled.
    pub fn add_authenticator(&self, d: &AuthenticatorDetails<'_>) -> Result<AddResponse, HRESULT> {
        let w = WideDetails::new(d);
        let options = AddAuthenticatorOptions {
            authenticator_name: w.name.as_ptr(),
            clsid: &d.clsid,
            plugin_rp_id: opt_ptr(&w.rp_id),
            light_logo_svg: opt_ptr(&w.light),
            dark_logo_svg: opt_ptr(&w.dark),
            cb_authenticator_info: len32(d.authenticator_info)?,
            pb_authenticator_info: d.authenticator_info.as_ptr(),
            c_supported_rp_ids: u32::try_from(w.rp_ptrs.len()).map_err(|_| E_INVALIDARG)?,
            supported_rp_ids: w.rp_ptrs(),
        };
        let mut response: *mut AddAuthenticatorResponse = null_mut();
        // SAFETY: `options` and every buffer it points to (`d`, `w`) outlive the call.
        let hr = unsafe { (self.add_authenticator)(&options, &mut response) };
        // Wrap first so a non-null response is freed even when `hr` is a failure.
        let guard = AddResponse {
            ptr: response,
            free: self.free_add_response,
        };
        check(hr)?;
        Ok(guard)
    }

    /// `WebAuthNPluginUpdateAuthenticatorDetails`. `clsid_new` equal to `d.clsid` keeps the op-sign key.
    /// `d.plugin_rp_id` is ignored (the update struct has no such field).
    pub fn update_authenticator(
        &self,
        d: &AuthenticatorDetails<'_>,
        clsid_new: &GUID,
    ) -> Result<(), HRESULT> {
        let w = WideDetails::new(d);
        let details = UpdateAuthenticatorDetails {
            authenticator_name: w.name.as_ptr(),
            clsid: &d.clsid,
            clsid_new,
            light_logo_svg: opt_ptr(&w.light),
            dark_logo_svg: opt_ptr(&w.dark),
            cb_authenticator_info: len32(d.authenticator_info)?,
            pb_authenticator_info: d.authenticator_info.as_ptr(),
            c_supported_rp_ids: u32::try_from(w.rp_ptrs.len()).map_err(|_| E_INVALIDARG)?,
            supported_rp_ids: w.rp_ptrs(),
        };
        // SAFETY: `details` and the buffers it points to outlive the call.
        check(unsafe { (self.update_authenticator)(&details) })
    }

    /// `WebAuthNPluginRemoveAuthenticator`. Needs package identity; call before removing the package.
    pub fn remove_authenticator(&self, clsid: &GUID) -> Result<(), HRESULT> {
        // SAFETY: valid GUID.
        check(unsafe { (self.remove_authenticator)(clsid) })
    }

    // ---- keys and user verification

    fn public_key(
        &self,
        clsid: &GUID,
        getter: unsafe extern "system" fn(*const GUID, *mut u32, *mut *mut u8) -> Hr,
    ) -> Result<OsBytes, HRESULT> {
        let (mut len, mut ptr) = (0u32, null_mut::<u8>());
        // SAFETY: valid GUID and out pointers.
        check(unsafe { getter(clsid, &mut len, &mut ptr) })?;
        Ok(OsBytes {
            ptr,
            len,
            free: self.free_public_key,
        })
    }

    /// `WebAuthNPluginGetOperationSigningPublicKey`: the key the OS signs operation requests with
    /// (a CNG ECC public blob; read the curve from its magic).
    pub fn operation_signing_public_key(&self, clsid: &GUID) -> Result<OsBytes, HRESULT> {
        self.public_key(clsid, self.get_op_sign_key)
    }

    /// `WebAuthNPluginGetUserVerificationPublicKey`: verifies the signatures `perform_user_verification`
    /// returns.
    pub fn user_verification_public_key(&self, clsid: &GUID) -> Result<OsBytes, HRESULT> {
        self.public_key(clsid, self.get_uv_key)
    }

    /// `WebAuthNPluginPerformUserVerification`: shows the Windows Hello prompt and returns the UV
    /// signature (verify it against [`user_verification_public_key`](Self::user_verification_public_key)).
    /// `hwnd` is the foreground window to parent the prompt; the strings are shown in the prompt.
    pub fn perform_user_verification(
        &self,
        hwnd: Hwnd,
        transaction_id: &GUID,
        username: Option<&str>,
        display_hint: Option<&str>,
    ) -> Result<OsBytes, HRESULT> {
        let username = username.map(wide);
        let hint = display_hint.map(wide);
        let request = UserVerificationRequest {
            hwnd,
            transaction_id,
            username: opt_ptr(&username),
            display_hint: opt_ptr(&hint),
        };
        let (mut len, mut ptr) = (0u32, null_mut::<u8>());
        // SAFETY: `request` and its strings outlive the call; valid out pointers.
        check(unsafe { (self.perform_uv)(&request, &mut len, &mut ptr) })?;
        Ok(OsBytes {
            ptr,
            len,
            free: self.free_uv_response,
        })
    }

    // ---- credential cache (rows are identified by credential id + RP id; see notes section 6)

    /// `WebAuthNPluginAuthenticatorAddCredentials`.
    pub fn add_credentials(&self, clsid: &GUID, rows: &[CredentialRow]) -> Result<(), HRESULT> {
        self.credentials_call(clsid, rows, self.add_credentials)
    }

    /// `WebAuthNPluginAuthenticatorRemoveCredentials`.
    pub fn remove_credentials(&self, clsid: &GUID, rows: &[CredentialRow]) -> Result<(), HRESULT> {
        self.credentials_call(clsid, rows, self.remove_credentials)
    }

    fn credentials_call(
        &self,
        clsid: &GUID,
        rows: &[CredentialRow],
        call: unsafe extern "system" fn(*const GUID, u32, *const PluginCredentialDetails) -> Hr,
    ) -> Result<(), HRESULT> {
        let wide_rows = WideRows::new(rows);
        let details = wide_rows.details(rows)?;
        // SAFETY: `details` points into `rows` and `wide_rows`, which outlive the call; the count matches.
        check(unsafe { call(clsid, len32_of(&details)?, details.as_ptr()) })
    }

    /// `WebAuthNPluginAuthenticatorRemoveAllCredentials` (reported unreliable; prefer get-all + diff).
    pub fn remove_all_credentials(&self, clsid: &GUID) -> Result<(), HRESULT> {
        // SAFETY: valid GUID.
        check(unsafe { (self.remove_all_credentials)(clsid) })
    }

    /// `WebAuthNPluginAuthenticatorGetAllCredentials`. `ERROR_FILE_NOT_FOUND` (an empty cache, per the
    /// Microsoft sample; unverified on this PC) is returned as an empty array.
    pub fn all_credentials(&self, clsid: &GUID) -> Result<CredentialDetailsArray, HRESULT> {
        let (mut count, mut ptr) = (0u32, null_mut::<PluginCredentialDetails>());
        // SAFETY: valid GUID and out pointers.
        let hr = unsafe { (self.get_all_credentials)(clsid, &mut count, &mut ptr) };
        if hr == HR_FILE_NOT_FOUND {
            return Ok(CredentialDetailsArray {
                count: 0,
                ptr: null_mut(),
                free: self.free_credential_details,
            });
        }
        check(hr)?;
        Ok(CredentialDetailsArray {
            count,
            ptr,
            free: self.free_credential_details,
        })
    }

    /// `WebAuthNPluginRegisterStatusChangeCallback`: `callback(context)` runs when the authenticator's
    /// state changes. The returned guard unregisters on drop.
    ///
    /// # Safety
    /// `context` must stay valid (and `callback` callable from any thread) until the guard is dropped.
    pub unsafe fn register_status_change_callback(
        &self,
        callback: StatusChangeCallback,
        context: *mut c_void,
        clsid: &GUID,
    ) -> Result<StatusRegistration, HRESULT> {
        let mut cookie = 0u32;
        // SAFETY: per the function contract; valid GUID and out pointer.
        check(unsafe { (self.register_status_callback)(callback, context, clsid, &mut cookie) })?;
        Ok(StatusRegistration {
            cookie,
            unregister: self.unregister_status_callback,
        })
    }

    // ---- request decoding

    /// `WebAuthNDecodeMakeCredentialRequest` over `OperationRequest::pbEncodedRequest`.
    pub fn decode_make_credential<'a>(
        &self,
        encoded: &'a [u8],
    ) -> Result<DecodedMakeCredential<'a>, HRESULT> {
        let mut decoded = null_mut();
        // SAFETY: `encoded` is a valid buffer of the given length; the result is freed by the guard.
        let hr = unsafe {
            (self.decode_make_credential)(len32(encoded)?, encoded.as_ptr(), &mut decoded)
        };
        check(hr)?;
        let ptr = NonNull::new(decoded).ok_or(E_POINTER)?;
        Ok(DecodedMakeCredential {
            ptr,
            free: self.free_decoded_make_credential,
            _input: PhantomData,
        })
    }

    /// `WebAuthNDecodeGetAssertionRequest`.
    pub fn decode_get_assertion<'a>(
        &self,
        encoded: &'a [u8],
    ) -> Result<DecodedGetAssertion<'a>, HRESULT> {
        let mut decoded = null_mut();
        // SAFETY: as in `decode_make_credential`.
        let hr =
            unsafe { (self.decode_get_assertion)(len32(encoded)?, encoded.as_ptr(), &mut decoded) };
        check(hr)?;
        let ptr = NonNull::new(decoded).ok_or(E_POINTER)?;
        Ok(DecodedGetAssertion {
            ptr,
            free: self.free_decoded_get_assertion,
            _input: PhantomData,
        })
    }

    // ---- response encoding

    /// `WebAuthNEncodeMakeCredentialResponse` for a prepared attestation struct.
    ///
    /// # Safety
    /// Every pointer in `attestation` must be null or valid (with its length field) for the call.
    pub unsafe fn encode_make_credential_raw(
        &self,
        attestation: &CredentialAttestation,
    ) -> Result<CoTaskMemBuf, HRESULT> {
        let (mut len, mut ptr) = (0u32, null_mut::<u8>());
        // SAFETY: per the function contract; valid out pointers.
        check(unsafe { (self.encode_make_credential)(attestation, &mut len, &mut ptr) })?;
        // SAFETY: the encoder returns a CoTaskMemAlloc buffer of `len` bytes (notes 2.6, checked in tests).
        Ok(unsafe { CoTaskMemBuf::from_raw(ptr, len) })
    }

    /// Encode a MakeCredential result with attestation format `none`: `auth_data` is the complete
    /// authenticator data (with the attested credential data), `credential_id` the new credential's id.
    pub fn encode_make_credential_none(
        &self,
        auth_data: &[u8],
        credential_id: &[u8],
    ) -> Result<CoTaskMemBuf, HRESULT> {
        let format = wide("none");
        let attestation = CredentialAttestation {
            version: ATTESTATION_VERSION,
            format_type: format.as_ptr(),
            cb_authenticator_data: len32(auth_data)?,
            pb_authenticator_data: pbyte(auth_data),
            cb_credential_id: len32(credential_id)?,
            pb_credential_id: pbyte(credential_id),
            ..Default::default()
        };
        // SAFETY: the struct points into `format`, `auth_data` and `credential_id`, alive for the call.
        unsafe { self.encode_make_credential_raw(&attestation) }
    }

    /// `WebAuthNEncodeGetAssertionResponse` for a prepared response struct.
    ///
    /// # Safety
    /// Every pointer in `response` (including the nested assertion and user entity) must be null or
    /// valid, with its length field, for the call.
    pub unsafe fn encode_get_assertion_raw(
        &self,
        response: &GetAssertionResponse,
    ) -> Result<CoTaskMemBuf, HRESULT> {
        let (mut len, mut ptr) = (0u32, null_mut::<u8>());
        // SAFETY: per the function contract; valid out pointers.
        check(unsafe { (self.encode_get_assertion)(response, &mut len, &mut ptr) })?;
        // SAFETY: as in `encode_make_credential_raw`.
        Ok(unsafe { CoTaskMemBuf::from_raw(ptr, len) })
    }

    /// Encode a GetAssertion result.
    pub fn encode_get_assertion(&self, out: &AssertionOutput<'_>) -> Result<CoTaskMemBuf, HRESULT> {
        let credential_type = wide(CREDENTIAL_TYPE_PUBLIC_KEY);
        let user_name = out.user.map(|u| wide(u.name));
        let user_display = out.user.map(|u| wide(u.display_name));
        let user_entity = match (out.user, &user_name, &user_display) {
            (Some(u), Some(name), Some(display)) => Some(UserEntity {
                version: USER_ENTITY_VERSION,
                cb_id: len32(u.id)?,
                pb_id: pbyte(u.id),
                name: name.as_ptr(),
                icon: null(),
                display_name: display.as_ptr(),
            }),
            _ => None,
        };
        let assertion = Assertion {
            version: ASSERTION_VERSION,
            cb_authenticator_data: len32(out.auth_data)?,
            pb_authenticator_data: pbyte(out.auth_data),
            cb_signature: len32(out.signature)?,
            pb_signature: pbyte(out.signature),
            credential: Credential {
                version: CREDENTIAL_VERSION,
                cb_id: len32(out.credential_id)?,
                pb_id: pbyte(out.credential_id),
                credential_type: credential_type.as_ptr(),
            },
            cb_user_id: out.user.map_or(Ok(0), |u| len32(u.id))?,
            pb_user_id: out.user.map_or(null_mut(), |u| pbyte(u.id)),
            ..Default::default()
        };
        let response = GetAssertionResponse {
            assertion,
            user_information: user_entity
                .as_ref()
                .map_or(null(), |u| u as *const UserEntity),
            number_of_credentials: out.number_of_credentials,
            user_selected: i32::from(out.user_selected),
            ..Default::default()
        };
        // SAFETY: the struct graph points into locals and caller slices that outlive the call.
        unsafe { self.encode_get_assertion_raw(&response) }
    }
}

fn len32_of<T>(items: &[T]) -> Result<u32, HRESULT> {
    u32::try_from(items.len()).map_err(|_| E_INVALIDARG)
}
