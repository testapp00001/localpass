//! Calls the registered provider's COM server the way Windows does, from a plain (unpackaged) process, to
//! check the real activation and marshalling without a passkey ceremony or a user gesture:
//!
//! 1. `CoCreateInstance` starts `lp-winauth-provider.exe -ActivateAuthenticator -Embedding`.
//! 2. `GetLockStatus` (no signature needed).
//! 3. `MakeCredential` with a forged request: the server must refuse it with `NTE_BAD_SIGNATURE`
//!    (the request struct, its GUID and its buffers crossed the proxy/stub intact) and return an
//!    empty response (length 0). This call is what exposed that the response buffer pointer must
//!    never be NULL: the stub answers a NULL one with RPC_X_NULL_REF_POINTER (0x800706F4).
//! 4. `CancelOperation` for an unknown transaction: `NTE_NOT_FOUND`.
//!
//! It never reaches the app or the user's vault. Run it with the package registered:
//! `cargo run -p lp-winauth --example com_probe` (the server stays up 150 s and then exits).

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    use lp_winauth::com::IPluginAuthenticator;
    use lp_winauth::provider::CLSID;
    use lp_winauth::sys::{
        CancelRequest, NTE_BAD_SIGNATURE, NTE_NOT_FOUND, OperationRequest, OperationResponse,
        REQUEST_TYPE_CTAP2_CBOR,
    };
    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::core::{GUID, Interface};

    // SAFETY: COM initialisation for this thread, as the server's callers do.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED)
            .ok()
            .expect("CoInitializeEx")
    };
    // SAFETY: plain activation by class id.
    let plugin: IPluginAuthenticator =
        unsafe { CoCreateInstance(&CLSID, None, CLSCTX_LOCAL_SERVER) }
            .expect("the OS could not start the provider (is the package registered?)");
    println!("activated the provider's COM server");

    let mut status = -1i32;
    // SAFETY: valid out pointer.
    let hr = unsafe { (plugin.vtable().GetLockStatus)(plugin.as_raw(), &mut status) };
    println!("GetLockStatus -> hr={:#010x} status={status}", hr.0 as u32);
    assert!(hr.is_ok() && (status == 0 || status == 1));

    let mut body = *b"not a CTAP2 request, and signed by nobody";
    let mut signature = [0x5Au8; 96];
    // A NULL window handle is refused by the proxy/stub itself (RPC_X_NULL_REF_POINTER, 0x800706F4):
    // Windows always passes the caller's window, here the desktop window stands in.
    // SAFETY: plain call.
    let desktop = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetDesktopWindow() };
    let request = OperationRequest {
        hwnd: desktop.cast(),
        transaction_id: GUID::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
        cb_request_signature: signature.len() as u32,
        pb_request_signature: signature.as_mut_ptr(),
        request_type: REQUEST_TYPE_CTAP2_CBOR,
        cb_encoded_request: body.len() as u32,
        pb_encoded_request: body.as_mut_ptr(),
    };
    let mut response = OperationResponse::default();
    // SAFETY: valid request and response structs for the call.
    let hr = unsafe { (plugin.vtable().MakeCredential)(plugin.as_raw(), &request, &mut response) };
    println!(
        "MakeCredential (forged) -> hr={:#010x} responseBytes={}",
        hr.0 as u32, response.cb_encoded_response
    );
    assert_eq!(hr, NTE_BAD_SIGNATURE, "a forged request must be refused");
    assert_eq!(response.cb_encoded_response, 0);
    if !response.pb_encoded_response.is_null() {
        // SAFETY: the proxy allocated it with CoTaskMemAlloc for us.
        unsafe {
            windows::Win32::System::Com::CoTaskMemFree(Some(
                response.pb_encoded_response.cast_const().cast(),
            ))
        };
    }

    let cancel = CancelRequest {
        transaction_id: GUID::from_u128(0xfeed_face_0000_0000_0000_0000_0000_0001),
        cb_request_signature: signature.len() as u32,
        pb_request_signature: signature.as_mut_ptr(),
    };
    // SAFETY: valid cancel request.
    let hr = unsafe { (plugin.vtable().CancelOperation)(plugin.as_raw(), &cancel) };
    println!(
        "CancelOperation (unknown transaction) -> hr={:#010x}",
        hr.0 as u32
    );
    assert_eq!(hr, NTE_NOT_FOUND);
    println!("ok");
}
