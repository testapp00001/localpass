//! The COM server Windows starts (`lp-winauth-provider.exe -ActivateAuthenticator -Embedding`).
//!
//! This layer only unpacks the OS structs, calls the [`Handler`](crate::handler::Handler) and copies its
//! answer into a fresh `CoTaskMemAlloc` block. Calls arrive on RPC worker threads (the process is in the
//! MTA) and block for the whole ceremony; a `CancelOperation` arrives concurrently on another thread,
//! possibly on another instance, which is why the handler keeps its in-flight state process-wide.
//!
//! The class object is registered first thing and the [`Provider`] is built on first use, because a
//! server that registers late gets DCOM event 10010. The process exits after [`IDLE_EXIT`] with nothing
//! in flight; Windows starts it again on demand.

#![allow(non_snake_case)] // COM method names

use std::ffi::c_void;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_NOINTERFACE, S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoRegisterClassObject, CoRevokeClassObject, CoTaskMemAlloc, CoTaskMemFree,
    IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::Win32::System::Console::FreeConsole;
use windows::core::{BOOL, GUID, HRESULT, IUnknown, Interface, Ref};

use crate::handler::OpCall;
use crate::log::hr;
use crate::plog;
use crate::provider::{CLSID, CLSID_TEXT, Provider};
use crate::sys::{
    CancelRequest, CoTaskMemBuf, OperationRequest, OperationResponse, PLUGIN_LOCKED, bytes,
};

/// With no call in flight for this long the server exits.
pub const IDLE_EXIT: Duration = Duration::from_secs(150);

static START: OnceLock<Instant> = OnceLock::new();
static LAST_ACTIVITY_MS: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static BUILD: OnceLock<fn() -> Result<Provider, String>> = OnceLock::new();
static PROVIDER: OnceLock<Result<Arc<Provider>, String>> = OnceLock::new();

fn now_ms() -> u64 {
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Marks a call in flight so the idle exit never fires mid-operation.
struct Activity;

impl Activity {
    fn begin() -> Self {
        IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
        Activity
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
        IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The process-wide provider, built on first use.
pub fn provider() -> Result<Arc<Provider>, String> {
    PROVIDER
        .get_or_init(|| {
            let build = BUILD.get().ok_or("the provider was never configured")?;
            build().map(Arc::new)
        })
        .clone()
}

/// `IPluginAuthenticator` (pa.h, IID d26bcf6f-...). Public so the `com_probe` example can call a running
/// server through COM the way Windows does.
#[windows_core::interface("d26bcf6f-b54c-43ff-9f06-d5bf148625f7")]
pub unsafe trait IPluginAuthenticator: windows_core::IUnknown {
    unsafe fn MakeCredential(
        &self,
        request: *const OperationRequest,
        response: *mut OperationResponse,
    ) -> HRESULT;
    unsafe fn GetAssertion(
        &self,
        request: *const OperationRequest,
        response: *mut OperationResponse,
    ) -> HRESULT;
    unsafe fn CancelOperation(&self, request: *const CancelRequest) -> HRESULT;
    unsafe fn GetLockStatus(&self, status: *mut i32) -> HRESULT;
}

#[windows_core::implement(IPluginAuthenticator)]
struct Plugin;

impl IPluginAuthenticator_Impl for Plugin_Impl {
    unsafe fn MakeCredential(
        &self,
        request: *const OperationRequest,
        response: *mut OperationResponse,
    ) -> HRESULT {
        guarded("MakeCredential", || {
            run_operation(request, response, |provider, call| {
                provider.handler.make_credential(call)
            })
        })
    }

    unsafe fn GetAssertion(
        &self,
        request: *const OperationRequest,
        response: *mut OperationResponse,
    ) -> HRESULT {
        guarded("GetAssertion", || {
            run_operation(request, response, |provider, call| {
                provider.handler.get_assertion(call)
            })
        })
    }

    unsafe fn CancelOperation(&self, request: *const CancelRequest) -> HRESULT {
        guarded("CancelOperation", || {
            // SAFETY: the stub passes a valid request for the duration of the call, or null.
            let Some(request) = (unsafe { request.as_ref() }) else {
                plog!("CancelOperation with a null request");
                return E_INVALIDARG;
            };
            // SAFETY: pointer and length come from the same request struct.
            let signature =
                unsafe { bytes(request.pb_request_signature, request.cb_request_signature) };
            match provider() {
                Ok(provider) => provider.handler.cancel(&request.transaction_id, signature),
                Err(why) => {
                    plog!("CancelOperation: provider unavailable: {why}");
                    E_FAIL
                }
            }
        })
    }

    unsafe fn GetLockStatus(&self, status: *mut i32) -> HRESULT {
        guarded("GetLockStatus", || {
            if status.is_null() {
                return E_INVALIDARG;
            }
            let value = provider().map_or(PLUGIN_LOCKED, |p| p.handler.lock_status());
            // SAFETY: non-null out pointer from the stub.
            unsafe { *status = value };
            plog!("GetLockStatus -> {value}");
            S_OK
        })
    }
}

fn guarded(what: &str, f: impl FnOnce() -> HRESULT) -> HRESULT {
    let _activity = Activity::begin();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(_) => {
            plog!("{what} panicked");
            E_FAIL
        }
    }
}

/// Shared body of MakeCredential and GetAssertion.
fn run_operation(
    request: *const OperationRequest,
    response: *mut OperationResponse,
    operation: impl FnOnce(&Provider, &OpCall<'_>) -> crate::handler::Outcome,
) -> HRESULT {
    // The response struct is caller-allocated. `pa.idl` declares `pointer_default(ref)`, so its
    // `pbEncodedResponse` is a reference pointer and the stub raises RPC_X_NULL_REF_POINTER
    // (0x800706F4) when it marshals the reply with a NULL one, whatever the length: the caller would then
    // see that RPC error instead of our HRESULT (and Windows retries the call). Every return therefore
    // carries a non-null buffer, a 1-byte CoTaskMem placeholder with length 0 on failure.
    set_placeholder(response);
    // SAFETY: the stub passes a valid request for the duration of the call, or null.
    let (Some(request), false) = (unsafe { request.as_ref() }, response.is_null()) else {
        plog!("operation with a null request or response");
        return E_INVALIDARG;
    };
    let provider = match provider() {
        Ok(provider) => provider,
        Err(why) => {
            plog!("provider unavailable: {why}");
            return E_FAIL;
        }
    };
    // SAFETY: pointers and lengths come from the same request struct, valid for this call.
    let (encoded, signature) = unsafe {
        (
            bytes(request.pb_encoded_request, request.cb_encoded_request),
            bytes(request.pb_request_signature, request.cb_request_signature),
        )
    };
    let call = OpCall {
        transaction_id: request.transaction_id,
        hwnd: request.hwnd as usize,
        request_type: request.request_type,
        encoded,
        signature,
    };
    let outcome = operation(&provider, &call);
    if outcome.hr.is_err() {
        return outcome.hr;
    }
    if outcome.response.is_empty() {
        plog!("success without a response body");
        return E_FAIL;
    }
    // A fresh block for every call: a repeated transaction must not return the same pointer twice.
    match CoTaskMemBuf::copy_of(&outcome.response) {
        Ok(buffer) => {
            // SAFETY: `response` was checked non-null above; the stub frees the block after the call.
            let response = unsafe { &mut *response };
            if !response.pb_encoded_response.is_null() {
                // SAFETY: the placeholder from `set_placeholder`, still owned by us.
                unsafe { CoTaskMemFree(Some(response.pb_encoded_response.cast_const().cast())) };
            }
            buffer.write_to(response);
            S_OK
        }
        Err(code) => {
            plog!("could not allocate the response hr={}", hr(code));
            code
        }
    }
}

/// Starts the caller's response empty but non-null (see [`run_operation`]).
fn set_placeholder(response: *mut OperationResponse) {
    if response.is_null() {
        return;
    }
    // SAFETY: caller-allocated response struct from the stub; plain allocation.
    unsafe {
        (*response).cb_encoded_response = 0;
        (*response).pb_encoded_response = CoTaskMemAlloc(1).cast::<u8>();
    }
}

#[windows_core::implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> windows_core::Result<()> {
        let _activity = Activity::begin();
        // SAFETY: `iid` is a valid pointer supplied by COM, or null.
        let iid_text = unsafe { iid.as_ref() }.map_or("<null>".to_owned(), |g| format!("{g:?}"));
        plog!(
            "IClassFactory::CreateInstance iid={iid_text} aggregated={}",
            outer.is_some()
        );
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if iid.is_null() || object.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let plugin: IPluginAuthenticator = Plugin.into();
        // SAFETY: `iid` and `object` were checked; `query` writes an AddRef'd interface or an error.
        let code = unsafe { plugin.query(iid, object) };
        if code.is_err() {
            plog!("QueryInterface failed hr={}", hr(code));
            return Err(if code == E_NOINTERFACE {
                E_NOINTERFACE.into()
            } else {
                code.into()
            });
        }
        Ok(())
    }

    fn LockServer(&self, lock: BOOL) -> windows_core::Result<()> {
        plog!("IClassFactory::LockServer({})", lock.as_bool());
        Ok(())
    }
}

/// Runs the COM server until it has been idle for [`IDLE_EXIT`]. `build` makes the [`Provider`] on first
/// use; `on_started` runs on a background thread once the class is registered (the binary starts the
/// credential-cache sync there).
pub fn serve(build: fn() -> Result<Provider, String>, on_started: fn()) -> ExitCode {
    // SAFETY: a console window is useless to a COM-launched server.
    unsafe {
        let _ = FreeConsole();
    }
    START.get_or_init(Instant::now);
    let _ = BUILD.set(build);
    plog!("COM server: registering class object clsid={CLSID_TEXT}");
    let factory: IClassFactory = Factory.into();
    // SAFETY: `factory` is a valid class factory kept alive until revoked.
    let cookie = match unsafe {
        CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)
    } {
        Ok(cookie) => cookie,
        Err(e) => {
            plog!("CoRegisterClassObject FAILED: {e}");
            return ExitCode::FAILURE;
        }
    };
    plog!("CoRegisterClassObject ok cookie={cookie}");
    LAST_ACTIVITY_MS.store(now_ms(), Ordering::SeqCst);
    std::thread::spawn(on_started);
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let idle = now_ms().saturating_sub(LAST_ACTIVITY_MS.load(Ordering::SeqCst));
        let busy = IN_FLIGHT.load(Ordering::SeqCst) > 0
            || PROVIDER
                .get()
                .and_then(|p| p.as_ref().ok())
                .is_some_and(|p| p.handler.in_flight() > 0);
        if !busy && idle >= IDLE_EXIT.as_millis() as u64 {
            break;
        }
    }
    // SAFETY: the cookie came from CoRegisterClassObject above.
    let revoked = unsafe { CoRevokeClassObject(cookie) };
    plog!(
        "idle for {}s: CoRevokeClassObject ok={}, exiting",
        IDLE_EXIT.as_secs(),
        revoked.is_ok()
    );
    ExitCode::SUCCESS
}
