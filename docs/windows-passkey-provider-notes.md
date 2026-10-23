# Windows passkey plugin authenticator: reference for LocalPass Phase 4

Researched 2026-10-19. Nothing here was run against the OS except read-only inspection. Evidence tags:
**[H]** SDK header with Read-tool line numbers (blank lines counted); **[L]** read-only inspection of this PC;
**[MS]** Microsoft Learn or the MIT-licensed Contoso sample (`microsoft/Windows-classic-samples/Samples/PasskeyManager`);
**[KP]** observed behaviour of KeePassPasskey (GPL-3.0, described here, never copy its code);
**[U]** UNVERIFIED, resolve in the spike. Header paths: `pa.h` = `pluginauthenticator.h`, `pa.idl`, `wp.h` = `webauthnplugin.h`,
`wa.h` = `webauthn.h`, all in `C:\Program Files (x86)\Windows Kits\10\Include\10.0.26100.0\um`; `winerror.h` is in `..\shared`.

Machine [L]: Windows 11 25H2, build 26200.7462, `webauthn.dll` 10.0.26100.7171 (sample needs >= 26100.6725), Developer Mode
`AllowDevelopmentWithoutDevLicense=1`, headers dated 2026-01-27 (= SDK 10.0.26100.7175 content).

## 1. Architecture

```
 browser / app --WebAuthN--> webauthn.dll (inside the caller) --COM, LOCAL_SERVER, std marshaling--> provider exe (ours)
                                                              IID d26bcf6f proxy/stub = OneCoreCommonProxyStub.dll [L]
 provider exe (package identity, started by COM with "-ActivateAuthenticator", MTA RPC threads):
    IPluginAuthenticator::MakeCredential / GetAssertion / CancelOperation / GetLockStatus
      1 verify request signature (op-sign public key fetched from webauthn.dll)
      2 WebAuthNPluginPerformUserVerification  (OS Windows Hello UI, returns a signature we verify)
      3 relay over \\.\pipe\localpass-broker-v1-<SID> as client kind "provider" ---> localpass.exe (engine signs)
      4 WebAuthNEncode{MakeCredential,GetAssertion}Response -> CoTaskMem buffer returned to the OS
 app/provider (any identity-bearing process): WebAuthNPluginAddAuthenticator / credential cache APIs
```

Who activates the server (the calling process or a system broker) is **[U]**; HKCU-only CLSID registration working is **[U]**.

### 1.1 Findings that change the design (read first)

| # | Finding | Evidence |
|---|---|---|
| F1 | Learn says `WebAuthnPlugin.dll`/`WebAuthnPlugin.Lib`. Neither exists here. The 21 `WebAuthNPlugin*` exports plus `WebAuthNEncode*/Decode*` are real (non-forwarded) exports of `webauthn.dll`; the SDK `webauthn.lib` has no plugin symbols and there is no `webauthnplugin.lib`. Bind with `LoadLibraryExW(L"webauthn.dll", LOAD_LIBRARY_SEARCH_SYSTEM32)` + `GetProcAddress` (what the Contoso `DelayLoad.h` does) or `raw-dylib`. | [L] PE export dump; [MS] DelayLoad.h |
| F2 | Declared but NOT exported here: `EXPERIMENTAL_WebAuthNPluginAddAuthenticator2` (wp.h:145-149), `EXPERIMENTAL_WebAuthNPluginPerformUserVerification2` (wp.h:377-382), `..UpdateAuthenticatorDetails2` (wp.h:233-236). Do not bind them. Exported but undeclared: `WebAuthNPluginGetAuthenticatorList`, `WebAuthNPluginFreeAuthenticatorList`, `WebAuthNPluginSetAuthenticatorState` (probably what Settings uses to toggle): do not use. | [L] |
| F3 | Two API generations coexist. `wa.h:1402-1936` holds an older `EXPERIMENTAL_*` plugin API (string CLSIDs, `EXPERIMENTAL_WebAuthNPluginPerformUv`, list-based credential APIs). The final API is in `wp.h`. Never mix. The old COM interface `EXPERIMENTAL_IPluginAuthenticator` (e6466e9a...) has 3 methods, `**response`, and a request struct without `requestType`; its `cbEncodedRequest` is at +40, the final one at +44, so mixing silently misreads. Implement only the final `IPluginAuthenticator` (d26bcf6f...). | [H] pa.h:81-89 vs 95-104, 164-271 vs 274-392 |
| F4 | Microsoft's own plugin (Edge "Microsoft Password Manager", registered on this PC) uses a **sparse identity package** (`uap10:AllowExternalContent`, no COM or plugin extension in the manifest), identity attached to `passkey_authenticator_plugin.exe` by a side-by-side `passkey_authenticator_plugin.exe.manifest` containing `<msix packageName= publisher= applicationId=/>`, and a **classic registry COM class** `HKLM\SOFTWARE\Classes\CLSID\{7D6FA3E8-...}\LocalServer32` = `"...\passkey_authenticator_plugin.exe" --run-mpm-plugin-operation` plus an `AppID` key with `LoadUserSettings=1`. This is the model that keeps localpass.exe in an NSIS install. | [L] |
| F5 | The OS stores each registration at `HKLM\SOFTWARE\Microsoft\FIDO\<user SID>\Plugins\{CLSID}` (readable without admin): `PackageFamilyName`, `PublisherDisplayName`, `PackageSignatureKind` (Edge: 1 = Developer), `SigningKeyAlgorithm` (Edge: `ECDSA_P256`), `Name`, `RpId`, `Base64EncodedUtf16{Light,Dark}Logo` + decoded UTF-8 logos, `AuthenticatorInfo`, `AaGuid`, `State` (0 disabled), `StateToggled`, `UvCount`, DPAPI blobs `RequestSignPrivateKey`/`RequestSignPubKey`/`AutofillCredentials`. So the registering process's package family name is recorded: **identity is required to register**. | [L] |
| F6 | No plugin-specific manifest extension category exists; both working manifests (Contoso, KeePassPasskey) only declare `windows.comServer` (ExeServer). The OS needs: a CLSID it can `CoCreateInstance`, and a registrant with package identity. | [MS][KP][L] |
| F7 | The OS request carries only RP ID (UTF-8 bytes) + client-data hash, never the origin. `lp_engine::passkey_*` all demand `origin` and run `validate_rp_id` (credentials.rs:364, 390, 529). See section 8. | [H] wp.h:477-553 |
| F8 | Windows routes GetAssertion to a plugin only for credentials present in its **credential cache** (section 6); an empty/stale cache means no call at all. MakeCredential needs no cache. | [KP] FAQ |
| F9 | **The gate:** whether `WebAuthNPluginAddAuthenticator` accepts a Developer-Mode loose registration (package signature kind None). The Contoso sample is run with VS F5, which deploys loose-file, which suggests yes, but no evidence here proves it. **Stage A answer: YES** (hr 0x00000000, `PackageSignatureKind 0`, run with identity); enabling and a real call are still stage B. See "Spike result (stage A)" in section 9. | [L] 2026-10-19 |

## 2. API surface (final API only)

### 2.1 Loading, constants, GUIDs

All `WINAPI` = `__stdcall` (x64 has one convention). `REFCLSID`/`REFGUID` in structs and parameters are **pointers** (`*const GUID`).
`wp.h` does not include `wa.h`: include `webauthn.h` first (it supplies the entity/credential/attestation types).

| Item | Value | Source |
|---|---|---|
| `IID_IPluginAuthenticator` | `d26bcf6f-b54c-43ff-9f06-d5bf148625f7`, version 1.0, `pointer_default(ref)` | pa.h:285, pa.idl:124-130 |
| `IID_EXPERIMENTAL_IPluginAuthenticator` | `e6466e9a-b2f3-47c5-b88d-89bc14a8d998` (do not implement) | pa.h:175 |
| Proxy/stub for both IIDs | CLSID `{A6FF50C0-56C0-71CA-5732-BED303A59628}` = `C:\Windows\System32\OneCoreCommonProxyStub.dll`, ThreadingModel Both; nothing for us to register | [L] |
| `WEBAUTHN_PLUGIN_REQUEST_TYPE_CTAP2_CBOR` | 1 | pa.h:75-79 |
| `PLUGIN_LOCK_STATUS` | `PluginLocked`=0, `PluginUnlocked`=1 | pa.h:152-157 |
| `AUTHENTICATOR_STATE` | `Disabled`=0, `Enabled`=1 | wp.h:54-58 |
| Struct versions | attestation 8 (wa.h:1084), assertion 6 (1212), credential 1 (300), user entity 1 (206), rp entity 1 (181), `WEBAUTHN_CTAPCBOR_*` request structs 1 | [H] |
| Strings | `L"public-key"` (wa.h:262), format `L"none"` (1074), ES256 = -7 (264) | [H] |
| ms-settings page | `ms-settings:passkeys-advancedoptions` (string present in `SystemSettings.dll`, not launched yet; Learn has no row for it) | [L] [U] |

HRESULTs (winerror.h): `NTE_USER_CANCELLED` 0x80090036 (:36800), `NTE_NOT_SUPPORTED` 0x80090029 (:36683), `NTE_INVALID_PARAMETER` 0x80090027 (:36665),
`NTE_NOT_FOUND` 0x80090011 (:36467), `NTE_EXISTS` 0x8009000F (:36449), `NTE_DEVICE_NOT_FOUND` 0x80090035 (:36791), `NTE_BAD_SIGNATURE` 0x80090006 (:36368),
`ERROR_BUSY` -> 0x800700AA, `ERROR_CANCELLED` -> 0x800704C7, `ERROR_TIMEOUT` -> 0x800705B4, `ERROR_FILE_NOT_FOUND` -> 0x80070002, `E_NOTIMPL` 0x80004001,
`CLASS_E_NOAGGREGATION` 0x80040110 (:31536). `WebAuthNGetErrorName` (wa.h:1377-1395) maps: NTE_EXISTS -> InvalidStateError;
ERROR_NOT_SUPPORTED/NTE_NOT_SUPPORTED/NTE_TOKEN_KEYSET_STORAGE_FULL -> ConstraintError; NTE_INVALID_PARAMETER -> NotSupportedError;
NTE_DEVICE_NOT_FOUND/NTE_NOT_FOUND/ERROR_CANCELLED/NTE_USER_CANCELLED/ERROR_TIMEOUT -> NotAllowedError; everything else UnknownError.

### 2.2 COM interface (pa.h:308-350 vtable, pa.idl:130-145)

Vtable order after IUnknown (QueryInterface, AddRef, Release): **MakeCredential, GetAssertion, CancelOperation, GetLockStatus**. All `HRESULT STDMETHODCALLTYPE`.

```c
HRESULT MakeCredential(PCWEBAUTHN_PLUGIN_OPERATION_REQUEST request /*[in] ref*/, PWEBAUTHN_PLUGIN_OPERATION_RESPONSE response /*[out,retval] CALLER-allocated struct: fill its fields*/);
HRESULT GetAssertion  (same two parameters);
HRESULT CancelOperation(PCWEBAUTHN_PLUGIN_CANCEL_OPERATION_REQUEST request);
HRESULT GetLockStatus(PLUGIN_LOCK_STATUS *lockStatus);
```
Also implement `IClassFactory` (`CreateInstance` must return `E_NOINTERFACE` for other IIDs and `CLASS_E_NOAGGREGATION` when `pUnkOuter != NULL`).
Both references set `*response = {}` first, return the HRESULT as the whole result, and treat each COM activation as a new object: a cancel can land on a different instance than the operation, so keep in-flight state in a process-wide map keyed by `transactionId` **[KP]**.

### 2.3 Structs from pa.h (x64 offsets)

```c
// WEBAUTHN_PLUGIN_OPERATION_REQUEST  pa.h:95-104, size 56
HWND hWnd;                 //+0  top-level window of the caller: owner for our UI, hwnd for the UV call
GUID transactionId;        //+8
DWORD cbRequestSignature;  //+24
BYTE *pbRequestSignature;  //+32 signature over SHA-256(pbEncodedRequest), verify with the op-sign key
DWORD requestType;         //+40 == 1 (CTAP2_CBOR)
DWORD cbEncodedRequest;    //+44
BYTE *pbEncodedRequest;    //+48 CBOR; give it to WebAuthNDecode*Request
// WEBAUTHN_PLUGIN_OPERATION_RESPONSE  pa.h:120-124, size 16:  DWORD cbEncodedResponse //+0;  BYTE *pbEncodedResponse //+8 (must be CoTaskMemAlloc'd)
// WEBAUTHN_PLUGIN_CANCEL_OPERATION_REQUEST  pa.h:141-146, size 32:  GUID transactionId //+0;  DWORD cbRequestSignature //+16;  BYTE *pbRequestSignature //+24
```
Strings are UTF-16 `LPCWSTR` unless a field says UTF-8. Request memory belongs to the COM stub and is valid only during the call.

### 2.4 Management and cache functions (wp.h; export names without the `EXPERIMENTAL_` prefix)

```c
HRESULT WebAuthNPluginGetAuthenticatorState(REFCLSID, AUTHENTICATOR_STATE*);                       // wp.h:60-65  NTE_NOT_FOUND = not registered [MS]
HRESULT WebAuthNPluginAddAuthenticator(PCWEBAUTHN_PLUGIN_ADD_AUTHENTICATOR_OPTIONS, PWEBAUTHN_PLUGIN_ADD_AUTHENTICATOR_RESPONSE*); // :139-143
void    WebAuthNPluginFreeAddAuthenticatorResponse(PWEBAUTHN_PLUGIN_ADD_AUTHENTICATOR_RESPONSE);  // :151-154
HRESULT WebAuthNPluginRemoveAuthenticator(REFCLSID);                                               // :160-163
HRESULT WebAuthNPluginUpdateAuthenticatorDetails(PCWEBAUTHN_PLUGIN_UPDATE_AUTHENTICATOR_DETAILS);  // :228-231 (keeps the op-sign key [KP])
HRESULT WebAuthNPluginAuthenticatorAddCredentials(REFCLSID, DWORD c, PCWEBAUTHN_PLUGIN_CREDENTIAL_DETAILS arr);    // :273-278
HRESULT WebAuthNPluginAuthenticatorRemoveCredentials(REFCLSID, DWORD c, PCWEBAUTHN_PLUGIN_CREDENTIAL_DETAILS arr); // :284-289
HRESULT WebAuthNPluginAuthenticatorRemoveAllCredentials(REFCLSID);                                 // :295-298 (reported unreliable [KP])
HRESULT WebAuthNPluginAuthenticatorGetAllCredentials(REFCLSID, DWORD*, PWEBAUTHN_PLUGIN_CREDENTIAL_DETAILS*); // :304-309  ERROR_FILE_NOT_FOUND when empty [MS]
void    WebAuthNPluginAuthenticatorFreeCredentialDetailsArray(DWORD c, PWEBAUTHN_PLUGIN_CREDENTIAL_DETAILS);  // :315-319
HRESULT WebAuthNPluginGetOperationSigningPublicKey(REFCLSID, DWORD*, PBYTE*);                      // :402-407 free with FreePublicKeyResponse
HRESULT WebAuthNPluginGetUserVerificationPublicKey(REFCLSID, DWORD*, PBYTE*);                      // :395-400
HRESULT WebAuthNPluginGetUserVerificationCount(REFCLSID, DWORD*);                                  // :389-393
void    WebAuthNPluginFreePublicKeyResponse(PBYTE);                                                // :409-410
HRESULT WebAuthNPluginPerformUserVerification(PCWEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST, DWORD *pcb, PBYTE *ppb); // :370-375
void    WebAuthNPluginFreeUserVerificationResponse(PBYTE);                                         // :384-387
typedef void (CALLBACK *WEBAUTHN_PLUGIN_STATUS_CHANGE_CALLBACK)(void *context);                    // :688
HRESULT WebAuthNPluginRegisterStatusChangeCallback(callback, void *ctx, REFCLSID, DWORD *pdwRegister); // :690-697
HRESULT WebAuthNPluginUnregisterStatusChangeCallback(DWORD *pdwRegister);                          // :699-703
```
Structs (all x64):
```c
// ADD_AUTHENTICATOR_OPTIONS wp.h:71-97, size 72   // UPDATE_AUTHENTICATOR_DETAILS wp.h:169-195, size 72: same layout except +16 is `const GUID *rclsidNew` instead of pwszPluginRpId
LPCWSTR pwszAuthenticatorName;   //+0  display name (KeePassPasskey adds a trailing space: Windows quirk [KP])
const GUID *rclsid;              //+8  plugin CLSID
LPCWSTR pwszPluginRpId;          //+16 "required for a nested WebAuthN call from the plugin" (wp.h:78); Learn says optional; NULL [U]
LPCWSTR pwszLightThemeLogoSvg;   //+24 base64 of UTF-8 SVG 1.1, passed as a wide string (optional; Edge's registry shows this) [L]
LPCWSTR pwszDarkThemeLogoSvg;    //+32
DWORD cbAuthenticatorInfo; const BYTE *pbAuthenticatorInfo; //+40,+48  CTAP2 authenticatorGetInfo CBOR (template in section 3)
DWORD cSupportedRpIds; const LPCWSTR *ppwszSupportedRpIds;  //+56,+64  0/NULL = all RPs
// ADD_AUTHENTICATOR_RESPONSE wp.h:130-137, size 16: DWORD cbOpSignPubKey; PBYTE pbOpSignPubKey (BCRYPT_PUBLIC_KEY_BLOB)
// PLUGIN_CREDENTIAL_DETAILS wp.h:242-271, size 64 (arrays are arrays of STRUCTS, not pointers)
DWORD cbCredentialId; const BYTE *pbCredentialId; LPCWSTR pwszRpId; LPCWSTR pwszRpName; DWORD cbUserId; const BYTE *pbUserId; LPCWSTR pwszUserName; LPCWSTR pwszUserDisplayName;
//  offsets +0,+8,+16,+24,+32,+40,+48,+56. Header marks credentialId, rpId, rpName, userId required (userName/displayName not; KeePassPasskey passes empty strings).
// USER_VERIFICATION_REQUEST wp.h:332-346, size 32
HWND hwnd;                       //+0  top-level window shown by the plugin and in foreground, else the caller's hWnd
const GUID *rguidTransactionId;  //+8  from the operation request
LPCWSTR pwszUsername;            //+16 shown in the Hello prompt (may be NULL)
LPCWSTR pwszDisplayHint;         //+24 text hint in the Hello prompt (may be NULL)
```

### 2.5 Decode/encode (wp.h:477-686), all exported by webauthn.dll

```c
HRESULT WebAuthNDecodeMakeCredentialRequest(DWORD cb, const BYTE *pb, PWEBAUTHN_CTAPCBOR_MAKE_CREDENTIAL_REQUEST *pp);  // :564-571
void    WebAuthNFreeDecodedMakeCredentialRequest(PWEBAUTHN_CTAPCBOR_MAKE_CREDENTIAL_REQUEST);                          // :573-577
HRESULT WebAuthNDecodeGetAssertionRequest(DWORD cb, const BYTE *pb, PWEBAUTHN_CTAPCBOR_GET_ASSERTION_REQUEST *pp);      // :639-646
void    WebAuthNFreeDecodedGetAssertionRequest(PWEBAUTHN_CTAPCBOR_GET_ASSERTION_REQUEST);                              // :648-652
HRESULT WebAuthNEncodeMakeCredentialResponse(PCWEBAUTHN_CREDENTIAL_ATTESTATION, DWORD *pcb, BYTE **ppb);               // :555-562
HRESULT WebAuthNEncodeGetAssertionResponse(PCWEBAUTHN_CTAPCBOR_GET_ASSERTION_RESPONSE, DWORD *pcb, BYTE **ppb);        // :679-686
```
Decoded MakeCredential request (wp.h:479-552), fields in order: `dwVersion`; `cbRpId,pbRpId` (**raw UTF-8 bytes: hash these for rpIdHash**);
`cbClientDataHash,pbClientDataHash`; `pRpInformation` (`WEBAUTHN_RP_ENTITY_INFORMATION*`: version, `pwszId`, `pwszName`, `pwszIcon`; wa.h:184-199);
`pUserInformation` (`WEBAUTHN_USER_ENTITY_INFORMATION*`: version, `cbId,pbId`, `pwszName`, `pwszIcon`, `pwszDisplayName`; wa.h:209-230; id <= 64 bytes);
`WebAuthNCredentialParameters` **by value** (`DWORD c; WEBAUTHN_COSE_CREDENTIAL_PARAMETER *p` array of `{dwVersion, pwszCredentialType, lAlg}`; wa.h:276-295);
`CredentialList` by value = **exclude list** (`DWORD c; WEBAUTHN_CREDENTIAL_EX **pp` array of pointers to `{dwVersion, cbId, pbId, pwszCredentialType, dwTransports}`; wa.h:344-372);
`cbCborExtensionsMap,pbCborExtensionsMap`; `pAuthenticatorOptions` (`{dwVersion, LONG lUp, lUv, lRequireResidentKey}` where +1 true, 0 undefined, -1 false; wp.h:414-428);
`fEmptyPinAuth, cbPinAuth,pbPinAuth`; `lHmacSecretExt`; `pHmacSecretMcExtension`; `lPrfExt, cbHmacSecretSaltValues,pbHmacSecretSaltValues`; `dwCredProtect`; `dwPinProtocol`;
`dwEnterpriseAttestation`; `cbCredBlobExt,pbCredBlobExt`; `lLargeBlobKeyExt`; `dwLargeBlobSupport`; `lMinPinLengthExt`; `cbJsonExt,pbJsonExt`.
Decoded GetAssertion request (wp.h:581-636): `dwVersion`; `pwszRpId` (UTF-16); `cbRpId,pbRpId` (UTF-8); `cbClientDataHash,pbClientDataHash` (32 bytes);
`CredentialList` by value = **allow list**; `cbCborExtensionsMap,pbCborExtensionsMap`; `pAuthenticatorOptions`; pin-auth trio; `pHmacSaltExtension`; `cbHmacSecretSaltValues,pb..`;
`dwPinProtocol`; `lCredBlobExt`; `lLargeBlobKeyExt`; large-blob group (`dwCredLargeBlobOperation`, `cbCredLargeBlobCompressed,pb..`, `dwCredLargeBlobOriginalSize`); `cbJsonExt,pbJsonExt`.
Inputs to encode: MakeCredential = `WEBAUTHN_CREDENTIAL_ATTESTATION` (wa.h:1086-1188): set `dwVersion`, `pwszFormatType=L"none"`, `cbAuthenticatorData,pbAuthenticatorData`,
`cbAttestation=0`; zero the rest. GetAssertion = `WEBAUTHN_CTAPCBOR_GET_ASSERTION_RESPONSE` (wp.h:654-677): `WebAuthNAssertion` by value (`WEBAUTHN_ASSERTION` v6, wa.h:1214-1285:
`dwVersion, cbAuthenticatorData,pb, cbSignature,pb, Credential{dwVersion=1,cbId,pbId,pwszCredentialType}, cbUserId,pbUserId` then zeros),
`pUserInformation` ([4]), `dwNumberOfCredentials` ([5]), `lUserSelected` ([6]), `cbLargeBlobKey,pb`, `cbUnsignedExtensionOutputs,pb`.

### 2.6 Ownership and free rules

| Buffer | Allocator | Free |
|---|---|---|
| Request / cancel structs and buffers | COM stub, copied into our process | stub, after the call returns; copy anything needed later |
| `response->pbEncodedResponse` | **CoTaskMemAlloc** (crosses the standard stub) | stub frees on S_OK (inferred: both Contoso and KeePassPasskey hand over the `WebAuthNEncode*` output unchanged) **[U]**; on failure leave the struct zeroed |
| `WebAuthNEncode*Response` out buffer | webauthn.dll; Contoso wraps it in `unique_cotaskmem_ptr` => CoTaskMem | transfer to `response`; otherwise `CoTaskMemFree` |
| Decoded request struct graph | webauthn.dll | `WebAuthNFreeDecoded{MakeCredential,GetAssertion}Request` (pointers inside die with it) |
| Add response | webauthn.dll | `WebAuthNPluginFreeAddAuthenticatorResponse` |
| UV response | webauthn.dll | `WebAuthNPluginFreeUserVerificationResponse(pb)` |
| Op-sign / UV public key | webauthn.dll | `WebAuthNPluginFreePublicKeyResponse(pb)` |
| `GetAllCredentials` array | webauthn.dll | `WebAuthNPluginAuthenticatorFreeCredentialDetailsArray(count, arr)` |
| Inputs to Add/Update/UV/Remove | caller; read only during the call; NUL-terminated UTF-16 | caller |

## 3. Registration sequence

1. Preconditions: package identity in the calling process (F5), a CLSID COM can activate (F6), TPM present and Hello PIN set (Windows creates a TPM-backed signing key at registration; no TPM means the provider never appears **[KP]**), OS >= 26100.6725 (older insider builds returned `E_NOTIMPL`, WindowsAppSDK#5482).
2. `WebAuthNPluginGetAuthenticatorState(&clsid, &state)`: `S_OK` = registered (state valid); `NTE_NOT_FOUND` = not registered **[MS]**.
3. If absent: `WebAuthNPluginAddAuthenticator` with name, CLSID, light/dark logo (base64 UTF-8 SVG), `pbAuthenticatorInfo`, `cSupportedRpIds=0`. Free the response. The op-sign key can be re-fetched any time with `GetOperationSigningPublicKey`; the sample stores it in the registry, KeePassPasskey fetches live **[MS][KP]**.
4. The authenticator starts **disabled**. The user enables it in Settings > Accounts > Passkeys > Advanced options and passes a Windows Hello check **[MS]**. Watch with `GetAuthenticatorState` or `RegisterStatusChangeCallback`. Do not call `WebAuthNPluginSetAuthenticatorState` (F2).
5. Change name, logo or AAGUID with `WebAuthNPluginUpdateAuthenticatorDetails` (`rclsidNew` = same CLSID keeps the op-sign key; remove + add rotates it).
6. Unregister: `WebAuthNPluginRemoveAuthenticator(&clsid)` from an identity-bearing process **before** removing the package (the HKLM key is per user SID and an orphan is hard to delete without admin).
7. Credential cache sync (section 6).

AuthenticatorInfo CBOR (`{1:[versions], 3:aaguid, 4:{rk,up,uv}, 9:["internal"], 10:[{alg:-7,type:"public-key"}]}`, 88 bytes, computed here, suffix identical to the sample's [MS]):
`A50182684649444F5F325F30684649444F5F325F310350` + `<16 AAGUID bytes as 32 hex>` + `04A362726BF5627570F5627576F5098168696E7465726E616C0A81A263616C672664747970656A7075626C69632D6B6579`.
The sample also lists `2:["prf","hmac-secret"]`; LocalPass supports no extensions, so omit key 2. The engine writes an all-zero AAGUID into authData (`lp_core::passkey::authenticator_data`, passkey.rs:248),
and both references use the same AAGUID in getInfo and authData. Either register a zero AAGUID (OS acceptance **[U]**) or add an AAGUID parameter to the engine.

## 4. MSIX / AppxManifest

Two viable identity models. **A** is the documented path (use it for the spike). **B** is Edge's and matches "main app stays NSIS".

### 4.1 Model A: full package, COM server in the manifest (Contoso sample + KeePassPasskey)

| Element | Needed | Tag |
|---|---|---|
| `Identity Name/Publisher/Version/ProcessorArchitecture` | yes; Publisher must equal the signing cert subject when signed | [MS][KP] |
| `Properties` DisplayName, PublisherDisplayName, Logo (file must exist) | yes | [MS] |
| `Dependencies/TargetDeviceFamily Windows.Desktop MinVersion=10.0.26100.0` | yes (sample uses 10.0.22635.4515) | [MS][KP] |
| `Application Executable=… EntryPoint="Windows.FullTrustApplication"` + `uap:VisualElements` | yes | [KP] |
| `com:Extension Category="windows.comServer"` > `ExeServer Executable Arguments DisplayName` > `Class Id DisplayName` | yes; the OS passes `Arguments` on activation | [MS][KP] |
| `uap3:Extension windows.appExecutionAlias` | optional, lets `register/unregister/status` run **with identity** from any terminal | [KP] |
| `rescap:Capability runFullTrust` | yes | [MS][KP] |
| Any "plugin authenticator" extension/category | none exists | F6 |

Minimal manifest (put it next to `lp-winauth-provider.exe` and `Assets\`; reuse `app/src-tauri/icons/{StoreLogo,Square44x44Logo,Square150x150Logo}.png`; replace `GUID`):
```xml
<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  xmlns:com="http://schemas.microsoft.com/appx/manifest/com/windows10"
  xmlns:desktop="http://schemas.microsoft.com/appx/manifest/desktop/windows10"
  IgnorableNamespaces="uap uap3 rescap com desktop">
  <Identity Name="LocalPass.PasskeyProvider" Publisher="CN=LocalPass Dev" Version="0.1.0.0" ProcessorArchitecture="x64"/>  <!-- VERIFIED shape -->
  <Properties><DisplayName>LocalPass Passkeys</DisplayName><PublisherDisplayName>LocalPass</PublisherDisplayName><Logo>Assets\StoreLogo.png</Logo></Properties> <!-- VERIFIED -->
  <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.26100.0" MaxVersionTested="10.0.26200.0"/></Dependencies>      <!-- VERIFIED (KeePassPasskey) -->
  <Resources><Resource Language="en-us"/></Resources>                                                                                       <!-- VERIFIED -->
  <Applications>
    <Application Id="Provider" Executable="lp-winauth-provider.exe" EntryPoint="Windows.FullTrustApplication">                              <!-- VERIFIED -->
      <uap:VisualElements DisplayName="LocalPass Passkeys" Description="LocalPass passkey provider" BackgroundColor="transparent"
        Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png"/>                                   <!-- VERIFIED -->
      <Extensions>
        <com:Extension Category="windows.comServer"><com:ComServer>
          <com:ExeServer Executable="lp-winauth-provider.exe" Arguments="-ActivateAuthenticator" DisplayName="LocalPass Passkey Provider">
            <com:Class Id="GUID" DisplayName="LocalPass Passkey Provider"/>                                                                  <!-- VERIFIED (both samples) -->
          </com:ExeServer></com:ComServer></com:Extension>
        <uap3:Extension Category="windows.appExecutionAlias" Executable="lp-winauth-provider.exe" EntryPoint="Windows.FullTrustApplication">
          <uap3:AppExecutionAlias><desktop:ExecutionAlias Alias="lp-winauth-provider.exe"/></uap3:AppExecutionAlias></uap3:Extension> <!-- VERIFIED (KeePassPasskey) -->
      </Extensions>
    </Application>
  </Applications>
  <Capabilities><rescap:Capability Name="runFullTrust"/></Capabilities>                                                                     <!-- VERIFIED -->
</Package>
```
Everything above is copied in shape from two working packages; **UNVERIFIED** is only whether a Dev-Mode loose registration (signature kind None) passes `AddAuthenticator` (F9).

Loose-file registration (Dev Mode, no certificate): `Add-AppxPackage -Register .\AppxManifest.xml` from the layout folder. Remove: `Get-AppxPackage LocalPass.PasskeyProvider | Remove-AppxPackage`.
Same version twice fails (0x80073CF9). Errors: Event Viewer > AppxDeployment-Server. The exe stays where it is; stop it before rebuilding.
Signed alternative (KeePassPasskey): `makeappx pack /d <dir> /p x.msix`, self-signed cert in `LocalMachine\TrustedPeople` (admin), `signtool sign /fd SHA256 /a /f x.pfx x.msix`, `Add-AppxPackage x.msix`.
Getting identity: COM activation of the ExeServer gives it automatically. For register/status runs use the execution alias, `explorer.exe shell:AppsFolder\<PFN>!Provider`, or `Invoke-CommandInDesktopPackage -PackageFamilyName <PFN> -AppId Provider -Command <exe> -Args …` (cmdlet exists here [L]). Running the exe by path gives none.
`GetCurrentPackageFamilyName` (kernel32) tells you from inside; log it.
Lifetime: COM starts the exe per demand; it must call `CoRegisterClassObject(clsid, factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE)` quickly (late = DCOM event 10010 in the System log) and may serve many calls. KeePassPasskey exits after 150 s idle and never while an operation is in flight; both samples keep a message loop on the main thread.
Packaged full-trust apps without `unvirtualizedResources` get AppData/HKCU writes redirected into the package (`%LOCALAPPDATA%\Packages\<PFN>\LocalCache`) **[U]**: keep provider logs and the pairing secret in its own storage and do not expect the main app to see them.

### 4.2 Model B: sparse identity package + exe manifest + registry CLSID (Edge, F4)

Package manifest = the Learn "grant identity" template: `Properties/uap10:AllowExternalContent=true`, `Application Executable="<exe>" uap10:TrustLevel="mediumIL" uap10:RuntimeBehavior="win32App"`,
`rescap runFullTrust` (+ `unvirtualizedResources`), no extensions [MS][L]. Exe side: external `<exe>.manifest` next to the exe with
`<msix xmlns="urn:schemas-microsoft-com:msix.v1" packageName="…" publisher="…" applicationId="…"/>` (Edge ships exactly this) [L][MS]. Register with
`Add-AppxPackage -Path id.msix -ExternalLocation <install dir>` (signed + trusted cert required by Learn), or in Dev Mode `Add-AppxPackage -Register AppxManifest.xml -ExternalLocation <dir>` **[U]**.
COM class: `HKLM\SOFTWARE\Classes\CLSID\{clsid}\LocalServer32` (Edge, per-machine) or HKCU for a per-user NSIS install **[U]**, with `AppID\{clsid}` `LoadUserSettings=1` as Edge does.
Open questions B adds: does `AddAuthenticator` accept signature kind None; does the OS activate an HKCU class; does the exe manifest identity reach a COM-launched process. Decide only after model A passes the spike.

## 5. Request handling

**Order inside MakeCredential / GetAssertion** (both references): zero `*response`; null-check; `WebAuthNDecode*Request`; verify the request signature; check cancellation; (assertion) pick the credential;
user verification; relay; encode; `WebAuthNFreeDecoded*Request` in all paths.

- **Request signature**: key = `WebAuthNPluginGetOperationSigningPublicKey(&clsid)` (a `BCRYPT_PUBLIC_KEY_BLOB`; magic 0x31534345 "ECS1" = ECDSA P-256 (bcrypt.h:541), 0x31415352 "RSA1" = RSA (:483)); Edge's registration says `ECDSA_P256` [L], the sample also handles RSA with PSS/SHA-256/salt 32 [MS]. **Stage A found an `ECS3` (P-384, 104-byte) blob for our registration despite that registry text** (section 9): read the curve from the magic.
  Verify over `SHA-256(pbEncodedRequest)`. CNG ECDSA signatures are raw `r||s` (64 bytes), not DER. Reject on failure with `NTE_BAD_SIGNATURE`: an unsigned caller would otherwise reach your signer.
- **Cancel**: `CancelOperation` carries only `transactionId` + signature. KeePassPasskey found empirically that the signature covers the **original request bytes** of that transaction (not the GUID); keep a copy of each in-flight request **[KP][U]**. Unknown id: `NTE_NOT_FOUND`. A cancelled operation returns `NTE_USER_CANCELLED`.
  The sample treats the platform's cancel as authoritative and completes its pending UI. There is no timeout argument: the platform cancels on browser cancel/timeout **[KP]**.
- **UV** (Windows Hello, done inside the call): fetch `GetUserVerificationPublicKey`; fill `WEBAUTHN_PLUGIN_USER_VERIFICATION_REQUEST` (hwnd = our foreground window or the caller's `hWnd`, `&transactionId`, username, hint); `WebAuthNPluginPerformUserVerification(&req,&cb,&pb)`;
  the returned bytes are a signature by the UV key over `SHA-256(pbEncodedRequest)` of this transaction (ECDSA raw or RSA-PSS); verify it before trusting it; free it. Failure HRESULTs (including user cancel) are returned as-is. Skip it when the user just proved presence another way (sample skips after its own vault unlock) so there is one prompt.
  `lUv`/`dwPinProtocol`/pin-auth are irrelevant: advertise `uv:true` and always verify.
- **MakeCredential response**: authData = `rpIdHash(pbRpId) || flags || signCount(4, =0) || AAGUID(16) || credIdLen(2,BE) || credId || COSE key`, flags `UP|UV|BE|BS|AT` = 0x5D (what the engine returns in `PasskeyCreated.auth_data`; ES256 COSE).
  Fill `WEBAUTHN_CREDENTIAL_ATTESTATION` (format `none`) and call `WebAuthNEncodeMakeCredentialResponse`; the OS is assumed to build the attestation object and the client-data/browser-facing response from it **[U]**. Do not add a CTAP status byte (neither reference does).
  Exclude list hit: return `NTE_EXISTS` (-> InvalidStateError, wa.h:1380). Unsupported algorithm list (no -7): `NTE_NOT_SUPPORTED`.
- **GetAssertion response**: authData = `rpIdHash || flags(UP|UV|BE|BS = 0x1D) || signCount`, signature = DER ECDSA-P256 over `authData || clientDataHash` (the engine returns it ready). Fill `WEBAUTHN_CTAPCBOR_GET_ASSERTION_RESPONSE` with credential id, user handle,
  `pUserInformation{id,name,displayName}`, `dwNumberOfCredentials=1`, `lUserSelected=1` [KP], call `WebAuthNEncodeGetAssertionResponse`. Allow list non-empty: pick the matching credential. Empty allow list with several credentials: the sample shows its own picker; the OS may not say which credential the user chose in its list **[U]**.
- **Errors to use**: `NTE_USER_CANCELLED` (user said no / cancel), `NTE_NOT_FOUND` (no credential), `NTE_NOT_SUPPORTED`, `NTE_EXISTS`, `ERROR_BUSY` (one operation at a time, sample), `HRESULT_FROM_WIN32(ERROR_TIMEOUT)`, `E_FAIL` otherwise. Windows shows its own generic error text whatever you return **[KP]**.
- **GetLockStatus**: `PluginUnlocked` when the app answers `status = unlocked` over the pipe, else `PluginLocked`. Never block or prompt; what the OS does with `Locked` is **[U]**.

## 6. Credential metadata cache

- Purpose: browser autofill and the system sign-in list read this cache; Windows does not call GetAssertion for a site with no cached credential (F8). Create needs no cache.
- Calls: `AddCredentials`/`RemoveCredentials` take arrays but return one HRESULT, so one bad row can fail the batch (KeePassPasskey calls them one row at a time) **[KP]**. Identity of a row = credentialId + rpId; duplicates give `NTE_EXISTS`, missing give `NTE_NOT_FOUND`: treat both as success **[KP]**.
  Changing display fields = remove + add. `GetAllCredentials` + diff is the sync method; `RemoveAllCredentials` was unreliable in the original native sample **[KP]**.
- When: after every create (add that credential), on delete/rename, when the vault unlocks or syncs in new passkeys (`EngineEvent::VaultChanged`), after enable/disable (`State` toggled: clear when disabled), and once after registration.
- Fields per row: credentialId, rpId, rpName (use rpName or rpId), userId (the handle), userName, userDisplayName (use title or rpId) = `PasskeyCandidate` (credentials.rs:66-77) minus `item_id`.
- Size limits: none documented **[U]**. Only metadata crosses (never private keys). The cache lives in the OS, so deleting a passkey must remove its row.
- Caller identity: very likely the registering package **[U]**; KeePassPasskey syncs from the packaged exe (`/synccredential`, started by its KeePass plugin). Assume the NSIS app cannot call these and must launch `lp-winauth-provider.exe --sync-cache` (handoff 4.4 assumed otherwise).

## 7. Pitfalls

- **Linking**: F1. Missing exports on older Windows must degrade to "unsupported OS", not crash at load; use runtime `GetProcAddress`.
- **Apartment/threads**: calls arrive on RPC worker threads; MakeCredential blocks for the whole ceremony and a cancel arrives concurrently on another thread/instance. Use MTA, no UI on RPC threads, a process-wide `transactionId` map, and `CoWaitForMultipleHandles`-style waits only if you pick STA (the sample is STA/WinUI). `CoInitializeSecurity` as the sample does.
- **Identity**: F5. A process started by path has none; `AddAuthenticator` fails there (exact HRESULT **[U]**). Package family name is stored per registration; changing publisher/name = a different PFN = re-register.
- **KeePassPasskey's own check**: it verifies the pipe client's package family name; mirror that idea for LocalPass's pipe (pairing already pins a secret).
- **32/64-bit**: pointer-size structs above are x64; build the provider for the OS arch (`ProcessorArchitecture="x64"`); an arm64 PC needs an arm64 build.
- **Strings**: UTF-16 for `LPCWSTR`; RP ID bytes for hashing are the UTF-8 `pbRpId`, not the UTF-16 copy; logos are base64 text of UTF-8 SVG; the AAGUID in CBOR/authData is RFC 4122 big-endian, not `GUID` field order.
- **Memory**: `response` buffer CoTaskMem only; never return a Rust `Vec` pointer; decoded structs die at `WebAuthNFreeDecoded*`.
- **Foreground rights**: a COM-launched process cannot take focus by itself; use the request `hWnd` as owner and `AllowSetForegroundWindow` handed over by the app **[KP]**. Notifications are hidden by Focus Assist.
- **Profiles/env**: `LOCALPASS_PROFILE` (pipe.rs:9) will not reach a COM-activated process; tests could hit the real app's pipe. Pick the profile from an HKCU value or a file the provider reads.
- **Browser extensions**: a passkey-capable extension (including LocalPass's own with passkey interception on) takes the request before Windows; switch it off for the spike **[KP]**.
- **Debug**: event logs `Plugin-Passkey-Providers/Operational` and `Microsoft-Windows-WebAuthN/Operational` exist and are enabled here [L] (empty today); AppxDeployment-Server for registration; System log DCOM 10010/10016; `reg query HKLM\SOFTWARE\Microsoft\FIDO /s`; log to a file **and** `OutputDebugStringW`; `Get-AppxPackage LocalPass.PasskeyProvider`.
- **Licensing**: KeePassPasskey is GPL-3.0: use it as behaviour documentation only. The Contoso sample is MIT.

## 8. Proposed Rust layout

**Crate** `crates/lp-winauth` (Windows only; `#![cfg(windows)]` modules), binary `lp-winauth-provider` (the packaged exe; no Tauri), a spike example. Windows deps: `windows-core 0.62` with `#[implement]` and the `interface` macro
(`windows-implement 0.60`, `windows-interface 0.59` are already in the cargo registry), `windows` 0.62 features `Win32_System_Com` (CoRegisterClassObject, IClassFactory, CoInitializeEx), `Win32_System_LibraryLoader`; `windows-sys` for CNG
(`Win32_Security_Cryptography`: BCryptImportKeyPair, BCryptVerifySignature, BCryptHash) and DPAPI (already a workspace dep). The `windows` crates have no WebAuthNPlugin* bindings (confirmed by the task and F1), so hand-write them.

| Module | Contents |
|---|---|
| `sys` | `struct WebAuthn { add_authenticator, …: unsafe extern "system" fn(…) }` loaded once via `LoadLibraryExW` + `GetProcAddress` (error `Unsupported` if any export is missing); all `#[repr(C)]` types of section 2 (request, response, cancel, add options/response, update details, credential details, UV request, decoded make/get requests, attestation v8, assertion v6, entity/credential structs, `GET_ASSERTION_RESPONSE`); unit tests pin `size_of`/`offset_of` to the offsets in 2.3/2.4 |
| `com` | `#[interface("d26bcf6f-b54c-43ff-9f06-d5bf148625f7")] unsafe trait IPluginAuthenticator: IUnknown { unsafe fn MakeCredential(&self, req: *const OpRequest, resp: *mut OpResponse) -> HRESULT; … GetAssertion, CancelOperation, GetLockStatus(*mut i32) }`, `#[implement(IPluginAuthenticator)] struct Plugin`, `#[implement(IClassFactory)] struct Factory`, `serve(clsid, handler)` (CoInitializeEx MTA, `CoRegisterClassObject`, idle exit, in-flight map) |
| `register` | add/update/remove/state, AuthenticatorInfo CBOR builder (section 3), logo base64, `fn identity() -> Option<String>` (`GetCurrentPackageFamilyName`) |
| `verify` | op-sign and UV signature check (ECDSA raw r||s, RSA-PSS) from the key blob magic; pure function over bytes, unit-tested with a locally generated key |
| `cache` | add/remove/get-all wrappers + diff |
| `translate` | decoded OS request -> broker request JSON and broker result -> `Encode*` input; pure, tested with fabricated structs |
| `client` (in `lp-broker`) | a Rust broker **client**: handshake (`Hello{kind:"provider"}`, `Challenge/Auth/Ready`, one-time `Pair`) with HMAC labels `lp-broker/client/v1` / `server/v1` (server.rs:41-42, `proof()` :127). Today only the JS extension implements the client side; `nmhost` is a dumb relay |

Pairing secret: pair once from the app's Settings ("Windows passkeys" => runs `lp-winauth-provider.exe --pair`, user approves the 6-digit code in the app), store the secret DPAPI-protected in the provider's own package storage.
Start the app when the pipe is missing (nmhost already has a `launch_app` hook, nmhost.rs:55) or fail with `E_FAIL` quickly.

**Mapping OS requests onto existing ops** (`allowed_for_provider`: Status, Unlock, PasskeyCandidates, PasskeyCreate, PasskeyAssert; protocol.rs:160-169):

| OS call | Broker op | Notes |
|---|---|---|
| MakeCredential | `passkeyCreate{origin, rpId, rpName, userHandle, userName, userDisplayName, algorithms, exclude, uv}` -> `auth_data`, `credential_id` | algorithms from `lAlg`; exclude from `CredentialList` ids |
| GetAssertion, allow list given | `passkeyCandidates{origin, rpId, allow}` then `passkeyAssert{origin, rpId, itemId, clientDataHash, uv}` -> `auth_data`, `signature`, `credential_id`, `user_handle` | candidate carries `userName`, `userDisplayName`, `userHandle` |
| GetAssertion, no allow list | same, plus a picker when more than one candidate | picker op missing |
| GetLockStatus | `status` | no prompt |
| cache sync | per-passkey metadata | **no all-passkeys op exists** |

**Gaps to close** (each needs a broker/engine change plus a test in `crates/lp-broker/tests`):
1. **No origin.** The OS gives no origin; all three engine calls require one and run `validate_rp_id`. Propose a provider path that synthesises `https://{rpId}` (the check then only validates the RP-ID syntax/suffix list) and trust the OS: the signed request (section 5) proves WebAuthn's own client already bound RP ID to the caller. This is a trust shift from "app validated origin" to "Windows validated origin"; document it and keep it behind `client.kind == Provider`.
2. **Double UV.** `verified(uv, …)` (server.rs:442, 554-565) ignores any client-sent `user_verified` and runs the app's own Hello, so OS UV + app UV = two prompts. Decide: (a) new `uv: "preVerified"` accepted only from provider clients after the provider verified the OS UV signature, or (b) let the app's Hello be the only UV and skip the OS call (cheaper, but the OS then has no UV proof and `WebAuthNPluginPerformUserVerification` is the documented contract). Recommend (a).
3. **Interactive unlock / confirm UI.** PasskeyCreate/Assert already `unlock()` when locked (`ensure_unlocked`, server.rs:419). A save/confirm dialog and a credential picker in the app are not implemented.
4. **List all passkeys** op (metadata only, no origin) for cache sync, plus a push or poll on vault change.
5. **AAGUID** parameter in `authenticator_data` if the registered AAGUID is not zero (section 3).
6. **Provider profile** selection without env vars (section 7).

## 9. GO / NO-GO spike

Scope: prove (1) Windows lists LocalPass as a passkey provider and (2) a MakeCredential call reaches our COM server. No engine, no pipe, no Hello. Run as the normal user, shell non-elevated (except Get-Tpm).
Example `crates/lp-winauth/examples/spike.rs`, modes: `register`, `unregister`, `status`, `-ActivateAuthenticator`. Log every line with time, pid, tid, `GetCurrentPackageFamilyName`, to `$env:TEMP\lp-spike.log`, `$env:LOCALAPPDATA\LpSpike\spike.log` and `OutputDebugStringW` (packaged writes may be redirected, see 4.1).

1. Preflight (record output in the result notes):
   `(Get-Item $env:SystemRoot\System32\webauthn.dll).VersionInfo.FileVersion` (>= 10.0.26100.6725; here 7171);
   `(Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock').AllowDevelopmentWithoutDevLicense` (1);
   `(Get-Tpm).TpmReady` in an elevated shell (True; unelevated returns empty);
   `reg query HKLM\SOFTWARE\Microsoft\FIDO /s` baseline (Edge's `{7D6FA3E8-…}` present, `State 0`); Windows Hello PIN configured.
2. Build the example: `cargo build -p lp-winauth --example spike --release` (spike code: loader, `WebAuthNPluginAddAuthenticator` with the section-4 values and a fresh CLSID and AAGUID, class factory + `IPluginAuthenticator` whose MakeCredential logs the request and returns `NTE_USER_CANCELLED`).
3. Layout: `$d="$PWD\target\spike\pkg"; New-Item -ItemType Directory -Force "$d\Assets"; Copy-Item target\release\examples\spike.exe "$d\lp-winauth-provider.exe"; 'StoreLogo','Square44x44Logo','Square150x150Logo' | % { Copy-Item "app\src-tauri\icons\$_.png" "$d\Assets" }`; write the section-4.1 manifest as `$d\AppxManifest.xml` (spike.exe must accept `-ActivateAuthenticator`).
4. Register the package: `Add-AppxPackage -Register "$d\AppxManifest.xml"`; check `Get-AppxPackage LocalPass.PasskeyProvider | fl Name,PackageFamilyName,SignatureKind,IsDevelopmentMode,InstallLocation`. Failure: AppxDeployment-Server event (manifest/asset errors).
5. Register the authenticator **with identity**: `lp-winauth-provider.exe register` through the alias (new terminal; alias dir `%LOCALAPPDATA%\Microsoft\WindowsApps`) or `Invoke-CommandInDesktopPackage`. Expect the log to show a non-empty PFN, `AddAuthenticator hr=0x00000000`, `opSignPubKey cb=72` (ECDSA P-256 blob; RSA would be larger; measured: **104**, P-384), then `state=Disabled`. Then `reg query HKLM\SOFTWARE\Microsoft\FIDO\<SID>\Plugins /s`: a key for our CLSID with our `PackageFamilyName`, `PackageSignatureKind` (record the value), `State 0`.
   **Failure signals:** `E_NOTIMPL` (OS too old), `0x8009....` NTE (TPM/Hello), `E_ACCESSDENIED`/`E_INVALIDARG`/empty PFN (no identity or dev-mode package rejected = F9 NO-GO for model A, try model B once), `GetProcAddress` null (F1).
6. **Human step (you, ~1 minute):** open `ms-settings:passkeys-advancedoptions` (or Settings > Accounts > Passkeys > Advanced options), turn **LocalPass Passkeys** on, pass the Hello prompt. SUCCESS 1: it is listed and the toggle holds; `status` prints `Enabled`; registry `State 1`.
7. Trigger MakeCredential: in Edge/Chrome with the LocalPass extension passkey interception **off**, open https://webauthn.io (or the local relying party `extension/e2e/rp.ts` on localhost), enter a user name, Register. Windows shows its save-passkey sheet; choose Change/other options, pick LocalPass.
   Watch `Get-Content "$env:LOCALAPPDATA\Packages\<PFN>\LocalCache\Local\LpSpike\spike.log" -Wait` (and the TEMP copy; DebugView shows the `OutputDebugStringW` copy).
8. **SUCCESS 2 (the go criterion):** the log contains, in order, a COM activation (`-ActivateAuthenticator` started by the OS, no manual launch), `CoRegisterClassObject ok`, `MakeCredential entered tx=<guid> hWnd=0x… cbReq=… cbSig=…`, `WebAuthNDecodeMakeCredentialRequest hr=0 rpId=webauthn.io alg=[-7,…] user=…`, and request-signature verification `ok` against `GetOperationSigningPublicKey`.
   Optional stage 2: call `WebAuthNPluginPerformUserVerification` (Hello prompt appears), verify the UV signature, then return a fixed software key's response through `WebAuthNEncodeMakeCredentialResponse` and see webauthn.io accept the registration.
9. **NO-GO signals:** (a) step 5/6 fails and model B also fails; (b) listed but toggle cannot be enabled (check the `Plugin-Passkey-Providers/Operational` and `Microsoft-Windows-WebAuthN/Operational` logs); (c) enabled but the OS never activates the server: System log DCOM 10010 (class not registered in time), 10016 (permissions), 0x80080005; absent entries = the OS rejected the CLSID/package; (d) the call arrives but the request signature or decode fails.
10. Cleanup, in this order (the HKLM entry is hard to delete once the package is gone): `lp-winauth-provider.exe unregister` (with identity; `WebAuthNPluginRemoveAuthenticator`), confirm `reg query HKLM\SOFTWARE\Microsoft\FIDO /s` shows only Edge's key, `Get-AppxPackage LocalPass.PasskeyProvider | Remove-AppxPackage`, delete `target\spike`, `$env:TEMP\lp-spike.log`, and `$env:LOCALAPPDATA\LpSpike`; stop any `lp-winauth-provider` process. Leave Edge's registration untouched.

Report to the user: GO if SUCCESS 2 reached under model A (note the exact `SignatureKind`), NO-GO with the failing step's HRESULT and registry/event evidence otherwise. F9 is the deciding unknown for model A; the other **[U]** items (who activates the server, redirected log paths, `pwszPluginRpId` NULL, zero AAGUID) are smaller and show up as specific failures above.

### Spike result (stage A), 2026-10-19: F9 PASSED; stage B (enable + real MakeCredential) is still open, this is NOT a GO

Stage A ran unattended as the normal user (no elevation). Nothing below proves the OS calls our server for a real ceremony: that is SUCCESS 2, stage B.

**What exists on the machine now (end state)**

| Item | Value |
|---|---|
| Code | `crates/lp-winauth` (`src/sys.rs` runtime-bound exports + `#[repr(C)]` types pinned by const asserts, `src/verify.rs`, `src/register.rs`), spike in `crates/lp-winauth/examples/spike.rs` |
| Plugin CLSID (fresh, random) | `{A1E209BF-2035-41D0-831F-B9A5D166DC03}` |
| AAGUID (fresh, random, non-zero) | `a315fc5e-a41e-4d69-9825-83d6c0419893` (RFC 4122 byte order in the CBOR) |
| Package | `LocalPass.PasskeyProvider` 0.1.0.0 x64, `CN=LocalPass Dev`, PFN `LocalPass.PasskeyProvider_rwhsg7s1qxd2y` |
| `Get-AppxPackage` | `SignatureKind None`, `IsDevelopmentMode True`, `Status Ok`, `InstallLocation C:\app\projects\rust\local-pass\target\spike\pkg` |
| Registry (`HKLM\SOFTWARE\Microsoft\FIDO\<SID>\Plugins\{CLSID}`) | `PackageFamilyName` = the PFN, `PackageSignatureKind 0x0` (Edge: 1), `State 0x0` (Disabled), `StateToggled 0`, `AaGuid {A315FC5E-A41E-4D69-9825-83D6C0419893}`, `Name "LocalPass Passkeys (spike)"`, `RpId localpass.invalid`, `SigningKeyAlgorithm ECDSA_P256`, `UvCount 0` |
| Provider exe | `target\spike\pkg\lp-winauth-provider.exe` (the spike, built with `CARGO_TARGET_DIR` elsewhere so the running app is untouched). Replace it only with no provider process running; bump `Version` only if the manifest changes |
| Execution alias | `%LOCALAPPDATA%\Microsoft\WindowsApps\lp-winauth-provider.exe` with modes `status`, `register`, `unregister` (identity, console-subsystem, synchronous, stdout and exit code work; `Invoke-CommandInDesktopPackage` was not needed) |

**Measured results**

| Step | Result |
|---|---|
| Preflight | `webauthn.dll` 10.0.26100.7171; `AllowDevelopmentWithoutDevLicense = 1`; TPM check skipped (known 2.0); baseline of `reg query HKLM\SOFTWARE\Microsoft\FIDO /s` taken (only Edge `{7D6FA3E8-...}`, `State 0`) |
| `status` by path (no identity) | all 24 final-API exports of section 2.4/2.5 present, `EXPERIMENTAL_WebAuthNPluginAddAuthenticator2` absent, `WebAuthNPluginSetAuthenticatorState` present but never bound; `GetAuthenticatorState` = `0x80090011` (not registered) |
| `Add-AppxPackage -Register` of the loose manifest | OK first try (manifest from 4.1 unchanged except the CLSID) |
| `register` through the alias, no `pwszPluginRpId` (NULL) | PFN non-empty (identity works). `WebAuthNPluginAddAuthenticator` hr = **`0x80090027` (NTE_INVALID_PARAMETER)**, response NULL, nothing registered |
| `register --rpid localpass.invalid` (the only change) | **hr = `0x00000000`**, state afterwards `Disabled` (0), `GetOperationSigningPublicKey` ok |
| Round trip | `unregister` hr 0 -> state `0x80090011`; `reg query HKLM\SOFTWARE\Microsoft\FIDO /s` is **byte-identical to the baseline**; `register` again hr 0 (a new op-sign key). Repeated on 2026-10-20 after a reboot (package and HKLM entry had survived it, State still 0): same hrs; the only diff to the baseline was Edge's own DPAPI `AutofillCredentials` blob, which Edge rewrites by itself, so Edge's key is otherwise untouched (`PackageSignatureKind 1`, `State 0`) |
| COM activation probe from an unpackaged PowerShell (`[Activator]::CreateInstance([type]::GetTypeFromCLSID(...))`) | the OS started `lp-winauth-provider.exe -ActivateAuthenticator -Embedding` with the PFN, `CoRegisterClassObject ok`, `IClassFactory::LockServer` / `CreateInstance(IUnknown)` served. Proves the manifest `windows.comServer` class works for loose registration; does not prove the plugin-platform activation path |
| Event logs | `Microsoft-Windows-WebAuthN/Operational`: only `2100/2102 GetPluginAuthenticatorList` pairs (about 17); `Plugin-Passkey-Providers/Operational` and `Synced-Passkey-Provider/Operatonal` (sic) empty; `Microsoft-Windows-AppXDeploymentServer/Operational` (the log that exists; 17 events, none warning/error) and the System log (no DistributedCOM events, so no 10010/10016), both queried from before the first step to after the activation probe |
| Unit tests (`cargo test -p lp-winauth`) | struct sizes/offsets, 88-byte AuthenticatorInfo, and the verifier against CNG-generated P-256, P-384 and P-521 keys and a P-256 signature from the `p256` crate. `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` and `cargo test --workspace` are all clean/green (built with a separate `CARGO_TARGET_DIR`) |

**Corrections to the notes' [U] items**

1. `pwszPluginRpId` NULL is **rejected** (`NTE_INVALID_PARAMETER`); any non-empty RP id was accepted. Logos, AAGUID, name and AuthenticatorInfo were identical in both attempts. The header calls it "required for a nested WebAuthN call"; treat it as required. Edge uses `login.microsoft.com`; the real provider needs a domain it owns or a documented placeholder.
2. AAGUID: a non-zero random one is accepted. A zero AAGUID was not tried, so the engine still needs an AAGUID parameter or the all-zero experiment. The registry `AaGuid` string is the CBOR bytes in RFC 4122 order (Edge's matches its CBOR the same way).
3. **The op-sign key is ECDSA P-384, not P-256**: the blob starts `45 43 53 33` (`ECS3`), `cbKey` 48, **104 bytes** (not 72), although the registry says `SigningKeyAlgorithm ECDSA_P256` for both Edge and us. Which digest Windows signs with (SHA-256, SHA-384) is unknown; `verify_request` reads the curve from the magic, tries SHA-256, SHA-384 and SHA-512 and returns the one that matched, so SUCCESS 2 will record it. Section 5's "ECDSA_P256 / 72 bytes" is wrong for this OS build.
4. Redirected files: `%LOCALAPPDATA%\LpSpike\spike.log` **is redirected** into `%LOCALAPPDATA%\Packages\<PFN>\LocalCache\Local\LpSpike\spike.log` (the real path is never created). `%TEMP%\lp-spike.log` is **not** redirected, so the TEMP copy is the one to watch.
5. Logo: base64 of UTF-8 SVG passed as a wide string was accepted; the registry stores that base64 as `Base64EncodedUtf16{Light,Dark}Logo` plus the decoded UTF-8.
6. Identity run: the execution alias is simpler than `Invoke-CommandInDesktopPackage` (synchronous, exit code, stdout). `CoInitializeEx(MTA)` was called before the registration APIs; whether they need it was not tested.
7. Op-sign key rotation: remove + add produced a different key blob (as section 3 step 5 says).
8. `cargo clean` (or deleting `target\`) deletes the registered loose package's files. Unregister and remove the package first.

**Log files to watch in stage B** (every line has time, pid, tid, `pfn=`): `Get-Content $env:TEMP\lp-spike.log -Wait` (preferred), `$env:LOCALAPPDATA\Packages\LocalPass.PasskeyProvider_rwhsg7s1qxd2y\LocalCache\Local\LpSpike\spike.log`, and DebugView for the `OutputDebugStringW` copy. The provider exits on its own after 150 s with no calls; the OS starts it again on demand.

**Stage B (the user, about 5 minutes)**

1. Switch off passkey interception in the LocalPass browser extension (popup) or disable the extension, so Edge/Chrome asks Windows.
2. Open `ms-settings:passkeys-advancedoptions` (Settings > Accounts > Passkeys > Advanced options), turn **LocalPass Passkeys (spike)** on, pass the Windows Hello prompt. Check: `& "$env:LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe" status` prints `state=1 (Enabled)`; registry `State 0x1`. If it is not listed or the toggle does not hold, copy the two Passkey event logs (`Get-WinEvent -LogName 'Plugin-Passkey-Providers/Operational' -MaxEvents 20`) and stop: that is NO-GO signal (b).
3. In another terminal run `Get-Content $env:TEMP\lp-spike.log -Wait`. In Edge go to https://webauthn.io (or the local RP `extension/e2e/rp.ts`), enter a user name, Register, and when the Windows passkey sheet appears pick **LocalPass Passkeys (spike)** (via "other options"/"Change" if it is not the default). The spike answers `NTE_USER_CANCELLED`, so the page reports "cancelled": that is the expected outcome.
4. SUCCESS 2 = the log shows, in order: `start mode=serve ... -ActivateAuthenticator` started by the OS, `CoRegisterClassObject ok`, `MakeCredential entered tx=... hWnd=... cbReq=... cbSig=... type=1`, the request and signature hex, `op-sign key kind: ECDSA P-384`, `request signature verification: ok (digest Sha...)`, `WebAuthNDecodeMakeCredentialRequest hr=0 rpId=webauthn.io alg=[-7,...] user=[...]`. Report the digest, `hWnd`, `cbReq`/`cbSig`, the `GetLockStatus` lines, and any `verification: FAILED` text.
5. Optionally also try a sign-in on a site (GetAssertion is only called for cached credentials, F8, so expect no call).
6. Then clean up (below) or tell the agent to continue.

**Cleanup: two commands, in this order** (PowerShell, normal user; the first removes the HKLM entry through the package identity, the second removes the package and files; afterwards `reg query HKLM\SOFTWARE\Microsoft\FIDO /s` shows only Edge's key):

```powershell
& "$env:LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe" unregister
Get-Process lp-winauth-provider -ErrorAction SilentlyContinue | Stop-Process -Force; Get-AppxPackage LocalPass.PasskeyProvider | Remove-AppxPackage; Remove-Item -Recurse -Force C:\app\projects\rust\local-pass\target\spike, "$env:TEMP\lp-spike.log", "$env:LOCALAPPDATA\LpSpike" -ErrorAction SilentlyContinue
```

Run the second command **only if the first printed `WebAuthNPluginRemoveAuthenticator hr=0x00000000` and then `GetAuthenticatorState (after) hr=0x80090011`** (the round trip was only tested from the Disabled state; after stage B the authenticator is Enabled, which is untested). If it printed anything else, stop and report instead: with the package gone the per-user HKLM entry cannot be removed without admin. Model B (sparse package + registry CLSID) was not needed and was not tried.

### Spike result (stage B), 2026-10-20: SUCCESS 2 reached, **GO**

The user enabled "LocalPass Passkeys (spike)" in Settings (Windows Hello check passed; registry `State 0x1`, `StateToggled 0x1`) and registered on https://webauthn.io (user `test0101`) in Edge. Log: `%TEMP%\lp-spike.log`.

| Observed | Value |
|---|---|
| Activation | the OS started `lp-winauth-provider.exe -ActivateAuthenticator -Embedding` itself (pid 21256, with the PFN); `CoRegisterClassObject ok` 27 ms later; `LockServer`/`CreateInstance(IUnknown)` then `MakeCredential` on the next RPC thread within 20 ms |
| Request | `tx` GUID, `hWnd=0x508ca`, `cbReq=179`, `cbSig=96` (raw r\|\|s of P-384), `type=1` (CTAP2_CBOR) |
| Signature | verified against `GetOperationSigningPublicKey` (`ECS3`, ECDSA P-384): **digest SHA-256 over `pbEncodedRequest`**, so section 5's "SHA-256" was right and only the curve was wrong |
| Decode | `WebAuthNDecodeMakeCredentialRequest hr=0`, `rpId=webauthn.io` (UTF-8), `rp.name=webauthn.io`, `alg=[-7]`, user id 19 bytes, name and displayName `test0101`, `clientDataHash` 32 bytes. The raw CBOR carries `1:clientDataHash 2:rp 3:user 4:pubKeyCredParams 6:{credProtect:2} 7:{rk:true, uv:true}` and **no excludeList**, no pinUvAuthParam |
| Return | `NTE_USER_CANCELLED` (0x80090036) |
| Retries | after the cancel the platform called `MakeCredential` again **with the same transaction id** 3.4 s and 9 s later (3 calls, `tx=39AA4236...`), then a new transaction at 10:07:03 (second user attempt). Expect repeated calls with one `tx` when returning an error; the real provider must be idempotent per `tx` (do not create two credentials) |
| Not seen | no `GetLockStatus` or `CancelOperation` call in this run; no Hello prompt from us (we did not call UV); the server was still running about a minute after the last call |

Consequences for the design:
1. Model A (full package with `windows.comServer`, Dev-Mode loose registration) is viable for development. A release build needs a signed MSIX (self-signed cert in `TrustedPeople` needs admin once) or the user keeps Developer Mode: decide in 4.5.
2. The OS asks for `rk:true uv:true` and credProtect 2 with ES256 only: matches what LocalPass stores (resident, UV, no extensions). The response may ignore credProtect.
3. Request signature check is cheap and works: always verify before relaying to the app.
4. Keep an in-flight map keyed by `tx` and make `MakeCredential` idempotent for a repeated `tx` (return the first result).
5. Still unknown (test in 4.4): whether the response is accepted with a zero AAGUID, how the OS shows the UV prompt (`WebAuthNPluginPerformUserVerification`), and GetAssertion routing via the credential cache (F8).

Machine state after the spike: authenticator ENABLED (State 1) and the spike server answers every MakeCredential with "cancelled". Disable it in Settings (toggle off) or run the two cleanup commands above when finished; 4.2/4.3 will reuse the same package layout under a new name, so do not leave the spike registered once the real provider exists.

### Corrections from 4.2 (FFI layer), 2026-10-20

Method: `crates/lp-winauth/tools/layout_check.c` is compiled with MSVC `cl` (VS 2022 Community, x64, `/TC`) against SDK 10.0.26100.0 and prints `sizeof`/`alignof`/`offsetof` for 26 structs plus the enum, version and HRESULT constants; its output is `crates/lp-winauth/src/sys/layout_tests.rs` (checked in; the C file says how to regenerate). Types live in `src/sys/types.rs`, the loader `WebAuthn`, wrappers and RAII owners in `src/sys/api.rs`.

1. **No numeric correction was needed.** Every size and offset in sections 2.3 and 2.4 matches the compiler (operation request 56, response 16, cancel 32, add options 72, add response 16, update details 72, credential details 64, UV request 32). New numbers that the notes did not have: RP entity 32, user entity 40, COSE parameter 24, credential 24, credential-EX 32, `WEBAUTHN_CTAPCBOR_AUTHENTICATOR_OPTIONS` 16 (the only 4-byte-aligned struct), ECC public key 48, HMAC salt extension 48, **MakeCredential request 200, GetAssertion request 176, attestation v8 192, assertion v6 168, GetAssertion response 216**. The MakeCredential fields after the entity pointers (all by-value sub-structs are 16 bytes): `WebAuthNCredentialParameters` +48, `CredentialList` +64, `pAuthenticatorOptions` +96, `dwCredProtect` +152, `pbJsonExt` +192. The decoder output matches these offsets at run time (see 4).
2. The `REFCLSID`/`REFGUID` struct fields are plain pointers in C, `HWND` is pointer-sized, and `WEBAUTHN_PLUGIN_REQUEST_TYPE`, `PLUGIN_LOCK_STATUS`, `AUTHENTICATOR_STATE` and `BOOL` are 4 bytes. All `NTE_*` and `HRESULT_FROM_WIN32` values in section 2.1 were re-derived from `winerror.h` and agree.
3. **[U] resolved:** the buffers `WebAuthNEncode*Response` returns are `CoTaskMemAlloc` blocks (`IMalloc::DidAlloc` = 1), so they can go straight into `WEBAUTHN_PLUGIN_OPERATION_RESPONSE`.
4. **Decode and encode work without package identity** (the unit tests run from a plain test exe), so they can be tested off the COM path; only Add/Remove/Update, the cache calls and UV need identity or the user. Decoding the captured 179-byte webauthn.io request gives: version 1, `rpId` webauthn.io (UTF-8 bytes), `alg` -7 with type `public-key`, user id `webauthnio-test0101` (19 bytes), name and displayName `test0101`, `dwCredProtect` 2, options `lUp` 0 (not sent), `lUv` 1, `lRequireResidentKey` 1, empty exclude list (`cCredentials` 0). A hand-built exclude list or allow list decodes to pointers to `WEBAUTHN_CREDENTIAL_EX` (version 1, `dwTransports` 0); empty, truncated or garbage CBOR returns a failure HRESULT without a crash.
5. **What the encoders produce** (both are CTAP2 CBOR maps with no status byte): MakeCredential `{1: "none", 2: authData, 3: {}}`; GetAssertion `{1: {id, type: "public-key"}, 2: authData, 3: signature, 4: {id, name, displayName}, 6: true}` (`lUserSelected` = 1 gives key 6; `dwNumberOfCredentials` = 1 adds no key 5; larger values untested). The encoder only copies `pbAuthenticatorData` through; it did not complain about a fabricated but well-formed authData.
6. The wrapper binds **23** exports. `WebAuthNPluginGetUserVerificationCount` (the 24th in section 2.4) exists but is not bound; `SetAuthenticatorState` and every `EXPERIMENTAL_*`/`*2` name are not bound (a test pins that).
7. Still unverified because it needs identity, a Hello gesture or a state change: `GetAllCredentials` returning `ERROR_FILE_NOT_FOUND` for an empty cache (the wrapper maps it to an empty array, as the Microsoft sample does), `AddCredentials`/`RemoveCredentials` row handling, the status-change callback, `PerformUserVerification` and the UV key, `UpdateAuthenticatorDetails`, `NTE_*` behaviour of `GetOperationSigningPublicKey` for an unknown CLSID. The first provider process (4.3/4.4) should log these.

### Phase 4.3 run log (2026-10-20)

- **Zero AAGUID in `AddAuthenticator`: accepted** (`register --rpid localpass.invalid --zero-aaguid`, hr 0x00000000, state Disabled, AuthenticatorInfo with 16 zero bytes). Whether the OS also accepts an all-zero AAGUID inside a returned authData is untested until the real MakeCredential response (4.4). The engine keeps writing a zero AAGUID.
- **Spike retired**: `unregister` hr 0, `Remove-AppxPackage`, `target\spike`, `%TEMP%\lp-spike.log` and `%LOCALAPPDATA%\LpSpike` removed; `reg query HKLM\SOFTWARE\Microsoft\FIDO\<SID>\Plugins` lists only Edge's `{7D6FA3E8-...}`. The real provider uses a new package (`LocalPass.Passkeys`) and CLSID, so the user must enable it once in Settings (Hello).
- **Design decisions** (user, 2026-10-20): app Hello is the only user verification (no `WebAuthNPluginPerformUserVerification`; if Windows refuses the response without it, ask before adding a provider-only pre-verified flag). The provider sends `https://{rpId}` as the origin: the broker already trusts a paired client's origin claim (the extension does the same), and `validate_rp_id` still checks syntax, IPs and the public-suffix list.

## Phase 4.3 / 4.4 build (2026-10-20)

Built and tested without user gestures; nothing here has seen a real MakeCredential/GetAssertion from Windows yet. The package and authenticator are registered and **disabled**; the checklist at the end is the user's part.

### What exists

| Piece | Where |
|---|---|
| Provider binary (modes `-ActivateAuthenticator`/`-Embedding`, `register`, `unregister`, `status`, `pair [--force]`, `sync-cache`, `cache-selftest`, `clsid`) | `crates/lp-winauth/src/bin/lp-winauth-provider.rs` |
| COM server (interface, factory, idle exit 150 s, response copy into a fresh CoTaskMem block) | `src/com.rs`; wiring of the real OS pieces (`OsVerifier`, `OsCacheSink`, `register`/`unregister`) `src/provider.rs` |
| Operations over plain bytes: signature check, in-flight map, decode, relay, encode, cancel, lock status | `src/handler.rs` (traits `OpVerifier`, `CacheSink`) |
| Pure translation and the HRESULT table | `src/translate.rs` |
| App side: `Relay` trait, `RealRelay` over `lp_broker::client` (one new pipe connection per operation, `CancelToken`, app start, foreground grant, pairing, `probe`) | `src/relay.rs` |
| Credential cache diff and sync (`CacheApi`, `OsCache`) | `src/cache.rs` |
| DPAPI-protected `client.bin`, `profile.txt`, HKCU `AppPath` | `src/store.rs`; log `src/log.rs` |
| Test fakes (`FakeRelay` = an in-memory vault that really creates and signs ES256 passkeys, `FakeVerifier`, `FakeSink`, `MemoryCache`, request builders, response parsers) | `src/testing.rs` (doc-hidden) |
| Real-pipe integration test (starts the `dev_host` example) | `crates/lp-winauth/tests/pipe.rs` |
| COM activation probe (calls the registered server through the OS proxy/stub, no gesture) | `crates/lp-winauth/examples/com_probe.rs` |
| Package manifest (the final class id since 4.5; it was a `@CLSID@` template before) | `crates/lp-winauth/package/AppxManifest.xml` |
| Scripts | `tools/windows-provider/{build,register,unregister,status}.ps1` (+ `common.ps1`) |

The spike example is deleted (superseded). Identities: package `LocalPass.Passkeys` 0.1.0.0 x64, publisher `CN=LocalPass Dev`, PFN `LocalPass.Passkeys_rwhsg7s1qxd2y`, **CLSID `{91E9966B-58AE-455A-AB60-4B2FDB66A730}`** (random, generated once; the exe prints it for `clsid`; since 4.5 the manifest carries it and `build.ps1` only checks that they agree), authenticator name `LocalPass`, `pwszPluginRpId` `localpass.invalid`, zero AAGUID.

`lp-broker` is a dependency with `default-features = false` at the workspace root and `features = ["client"]` in `lp-winauth`; `app/src-tauri` asks for `["server", "client"]`. `cargo tree -p lp-winauth -e normal -i lp-engine` reports "did not match any packages" (no engine, sync or store crate; the provider exe is 1.3 MB). `cargo check -p localpass` still builds.

### Findings (measured on this PC)

1. **The response buffer pointer must never be NULL.** `pluginauthenticator.idl` says `pointer_default(ref)`, so `WEBAUTHN_PLUGIN_OPERATION_RESPONSE.pbEncodedResponse` is a reference pointer and the stub answers a NULL one with `RPC_X_NULL_REF_POINTER` (`0x800706F4`) when it marshals the reply, **whatever HRESULT the method returned**. `com_probe` showed it as an A/B: a forged `MakeCredential` returned `0x800706F4` with `*response = {}` (the handler had run and refused it, see the log) and `0x80090006` (`NTE_BAD_SIGNATURE`) once the failure paths return a 1-byte `CoTaskMemAlloc` placeholder with length 0. This is the likely reason Windows called the spike three times with the same transaction id after it returned `NTE_USER_CANCELLED`: it saw an RPC error, not "cancelled". The advice in sections 2.2 and 5 to zero `*response` is therefore wrong. A NULL `hWnd` marshals fine.
2. **Virtualization can be turned off in a Dev Mode loose package.** `desktop6:RegistryWriteVirtualization` and `desktop6:FileSystemWriteVirtualization` set to `disabled` in `<Properties>` plus `rescap:Capability unvirtualizedResources` were accepted by `Add-AppxPackage -Register`. Result: `provider.log` is at the real `%LOCALAPPDATA%\LocalPass\provider\` and nothing is created under `Packages\LocalPass.Passkeys_*\LocalCache`. So the data directory is a normal folder and the unpackaged scripts can read and write it. (The fallback of keeping everything inside the package's store was not needed.) The `[U]` in section 4.1 about redirected AppData is resolved for this manifest.
3. A packaged process (started through the alias or by COM) can open the app's user-only pipe, pair, and log in (tested against `dev_host` on a private profile). A Hello with an unknown client id gets "unpaired" back and shows nothing in the app, so `status` uses that to say "app reachable" without pairing (it did so against the user's running app). `AllowSetForegroundWindow(app pid)` returned true from the COM-started server (whether the Hello window really comes to the front is for the real test).
4. **Credential cache calls work on a registered, disabled authenticator** (`cache-selftest`): add `S_OK`; add again `NTE_EXISTS`; `GetAllCredentials` returns the row; remove `S_OK`; remove again `S_OK` (not `NTE_NOT_FOUND`); reading an empty cache works (0 rows; the wrapper maps `ERROR_FILE_NOT_FOUND` to empty). Treating `NTE_EXISTS` and `NTE_NOT_FOUND` as success covers both.
5. `UpdateAuthenticatorDetails` with the same CLSID: hr 0 (second `register`: "already registered: details updated, key kept"). Unregister, remove package, register again: the HKLM `Plugins` dump equals the baseline (0 differing lines). Registry of ours: `State 0`, `PackageSignatureKind 0`, `SigningKeyAlgorithm ECDSA_P256` (the key is P-384, see section 9), `AaGuid` all zero, `Name LocalPass`, `RpId localpass.invalid`.
6. `dev_host` prints `READY` as soon as it has *spawned* the pipe server, which can be before the pipe exists. `tests/pipe.rs` knocks until the pipe appears. (`dev_host` is unchanged: the extension e2e does not race on it.)
7. **Real-PC bug 1, Hello shown twice with a locked vault (fixed in `lp-broker`).** `dispatch` ran `Host::unlock` (Hello or master password) and then `verified()` ran `Host::verify_user` (Hello again). `ensure_unlocked` now says whether *that request* unlocked the vault, and `PasskeyCreate`/`PasskeyAssert` count it as the user check (`user_verified = true`); an unlocked vault still asks once, nothing carries to a later request, and a mutex stops a request that only waited for another's unlock from borrowing it. Tests: `lp-broker/tests/server.rs` (counts `unlock`/`verify_user`). **Sign-in** used to be `PasskeyCandidates` (`locked`), `Unlock`, `PasskeyCandidates`, `PasskeyAssert` on separate connections, so the unlock happened in an earlier request and Hello still showed twice. It is now ONE request, `PasskeyAssertAny { origin, rpId, allow, clientDataHash, uv }` (provider clients only; an extension client gets `forbidden`): the app unlocks if needed (that unlock is the user check; nothing carries over), finds the passkeys for the RP and allow list as `PasskeyCandidates` does, then none -> `notFound`, several -> `ambiguous` (nothing signed, nobody asked), exactly one -> Hello (skipped when this request just unlocked) -> sign. The reply is the `PasskeyAssert` payload plus `userName`/`userDisplayName` (`protocol::AssertedPasskey`; the provider has no vault to look them up in). `Relay` has `assert_any` instead of `candidates` + `assert`; `ambiguous` and `notFound` both end as `NTE_NOT_FOUND`. Android's `lp_engine::provider` does not use the broker and is unaffected.
8. **Real-PC bug 2, app not started (`os error 50`, `NTE_DEVICE_NOT_FOUND`).** The COM-activated server sits in a job object (`IsProcessInJob` true; the alias process does not, so `launch-test` through the alias alone does not reproduce it) and refuses the browser helper's start (a `CREATE_BREAKAWAY_FROM_JOB` try, then `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`) with ERROR_NOT_SUPPORTED. Measured in the real COM server (a temporary start-up hook driven by `com_probe`, removed again): legacy flags -> 50; **plain `CreateProcess`** (no creation flags, null stdin/out/err, cwd = the exe's folder) **works**; `ShellExecuteExW` ("open", `--minimized`, `SW_SHOWMINNOACTIVE`, `NOASYNC|NOCLOSEPROCESS|FLAG_NO_UI`) works too. Either way the child had no package identity, was in no job, and outlived the provider (`examples/launch_probe`). `relay::launch_exe` tries `CreateProcess`, then `ShellExecuteExW`, and logs `started <exe> with <method> (pid N)`. Hidden `lp-winauth-provider launch-test [--method createprocess|shell|legacy] <exe> [args]` (through the alias) runs it against a harmless exe: `cargo build --release -p lp-winauth --example launch_probe` (logs to `%TEMP%\lp-launch-probe-<pid>.log`, exits after 12 s).

### Design as built

* **Order of every call** (`handler.rs`): request type, then the request signature against `GetOperationSigningPublicKey` (SHA-256 first, as measured; the verifier also accepts SHA-384/512), refusing with `NTE_BAD_SIGNATURE` before anything is recorded or relayed; then the process-wide in-flight map keyed by transaction id; then decode, translate, relay, check the app's answer (RP id hash, credential id, signature present), encode with `WebAuthNEncode*Response`.
* **Idempotent per transaction.** The first call does the work; a repeated or concurrent call with the same transaction id waits for it and returns a copy of its outcome (failures included, so the retries seen in the spike never show a second Hello prompt). Results are kept 60 s. Every answer to the OS gets its own `CoTaskMemAlloc` copy (the stub frees what it gets, so one block is never returned twice).
* **Cancel.** `CancelOperation` finds the transaction, checks the signature against the stored original request bytes (and, if that fails, against the GUID bytes), logs which one verified, and cancels either way; the pending call returns `NTE_USER_CANCELLED` because the relay future is dropped, which drops its pipe connection, which makes the app withdraw its Hello prompt (lp-broker part A). Unknown transaction: `NTE_NOT_FOUND`; finished transaction: `S_OK`.
* **User verification:** only the app's Windows Hello check (`uv` is sent as required; the app prompts). `WebAuthNPluginPerformUserVerification` is never called. If Windows turns out to insist on it, ask before adding a "pre-verified" flag.
* **Origin** is `https://{rpId}` from the UTF-8 `pbRpId`, only after the RP id passes a host-name check (letters, digits, `.`, `-`, `_`, no empty labels); the engine's `validate_rp_id` then checks the suffix list and IP literals. Locked vault: GetAssertion is one `PasskeyAssertAny` request and the app unlocks inside it (a refusal is `NTE_USER_CANCELLED`); listing for the cache never prompts.
* **GetAssertion** is one `PasskeyAssertAny` request; the app needs exactly one passkey after the RP and allow-list filters. Several (no allow list, or an allow list naming more than one) or none is `NTE_NOT_FOUND`. **Known limit:** there is no picker; the hypothesis is that Windows' own cache list narrows the allow list to the passkey the user chose. Check it in the real test with two accounts on one site.
* **Cache.** After a successful MakeCredential that one row is added (before the response returns). At server start, in a background thread, `sync` mirrors the vault: it never starts the app, never prompts, and when the listing fails (app down, not paired, locked) it changes nothing (a failure is never diffed as an empty vault). Passkeys created elsewhere reach the cache at the next provider start or `sync-cache` run; the app-triggered sync on `VaultChanged` is 4.5.
* **Starting the app.** If the pipe is missing, `MakeCredential`, `GetAssertion` and `pair` start `AppPath --minimized` (`CreateProcess`, else `ShellExecuteExW`, see finding 8) and wait up to 25 s for the pipe; `GetLockStatus`, the cache sync and `status` never do. A profile in `profile.txt` (development only) disables the start. `AppPath` is `HKCU\Software\LocalPass\Provider` (written by `register.ps1`: the running release exe, else `target\release\localpass.exe`).

### HRESULT table (`translate::hresult_for`)

| Situation | HRESULT | Browser sees |
|---|---|---|
| user cancelled, Hello denied or dismissed, vault locked and not unlocked, Windows cancelled | `NTE_USER_CANCELLED` 0x80090036 | NotAllowedError |
| no passkey for the RP / allow list, or several and no way to choose | `NTE_NOT_FOUND` 0x80090011 | NotAllowedError |
| app not running and not startable, pipe is not LocalPass, no vault, or provider not paired | `NTE_DEVICE_NOT_FOUND` 0x80090035 | NotAllowedError |
| exclude list names a passkey we hold | `NTE_EXISTS` 0x8009000F | InvalidStateError |
| ES256 (-7) not offered | `NTE_NOT_SUPPORTED` 0x80090029 | ConstraintError |
| undecodable request, no user, bad RP id, hash not 32 bytes, wrong request type | `NTE_INVALID_PARAMETER` 0x80090027 | NotSupportedError |
| request signature does not verify | `NTE_BAD_SIGNATURE` 0x80090006 | UnknownError |
| `CancelOperation` for an unknown transaction | `NTE_NOT_FOUND` | n/a |
| anything else (app error, bad data from the app, request timeout with the app connected, panic) | `E_FAIL` | UnknownError |

### Store layout (`%LOCALAPPDATA%\LocalPass\provider`)

`client.bin` (DPAPI `CryptProtectData`, current user, entropy `LocalPass Windows passkey provider client v1`; plaintext = version byte, u16 id length, client id, 32-byte secret; refused if altered or protected for another purpose), `profile.txt` (optional dev pipe profile; absent = the real app), `provider.log` (rotated at 2 MB to `provider.log.1`; time, pid, tid, PFN on every line; no secrets, no user names or ids, credential ids shortened to 4 bytes). Registry: `HKCU\Software\LocalPass\Provider\AppPath`.

### Tests

`cargo test --workspace`: 255 pass (201 before). `lp-winauth` has 101 unit tests (translate against the captured 179-byte webauthn.io request decoded by the real `webauthn.dll`; handler with a fake app: create, repeated and concurrent same transaction, failure replayed without a second prompt, TTL expiry, forged signature, wrong request type, exclude hit, cancel mid-flight and with an unverifiable signature, assertion with and without allow list, zero/several candidates, the whole error table, app down; cache diff and sync including "a failed listing changes nothing"; DPAPI round trip and tamper) and 5 integration tests against the real pipe (`dev_host`): pair, create the captured request, same transaction again, list, cache sync, assertion whose signature verifies against the registered public key, exclude hit, locked vault (a sign-in unlocks, listing does not), unknown or damaged pairing, cancel before the request, app killed. `clippy --workspace --all-targets -D warnings` and `fmt` are clean.

OS smoke tests on this PC: register, register again (update), unregister, register again; `status`, `pair`, `sync-cache`, `cache-selftest` from the packaged alias (pair against a `dev_host` on a private profile, then `client.bin` and `profile.txt` deleted); `com_probe` (activation by the OS, `GetLockStatus`, forged `MakeCredential` gives `0x80090006`, `CancelOperation` for an unknown transaction gives `0x80090011`).

### Unverified (only a real OS request can show)

* Windows accepting our `none` attestation response (the `WebAuthNEncodeMakeCredentialResponse` output as the response body) and a zero AAGUID inside authData; the encoded assertion response with `lUserSelected = 1`.
* That Windows accepts no `PerformUserVerification` call from us.
* That the request signature is SHA-256 on every call (spike: yes), and what the cancel signature covers (the log says which).
* GetAssertion routing through the cache, and whether Windows narrows the allow list when several passkeys exist for a site.
* The app's Hello window coming to the front when a browser started the request.
* Whether the 150 s idle exit and on-demand restart behave under real use.
* The app started by the provider on a real create/sign-in with LocalPass closed (the probe test used the same COM server but not the full request path).

### Manual checklist (user, about 10 minutes)

1. `powershell -NoProfile -ExecutionPolicy Bypass -File tools\windows-provider\status.ps1` shows the package registered and `LocalPass ... disabled`.
2. Settings > Accounts > Passkeys > Advanced options: turn **LocalPass** on and pass the Windows Hello check (`status.ps1` then shows `ENABLED`, registry `State 1`).
3. Start the real LocalPass app and unlock it. Then, in a terminal: `& "$env:LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe" pair`. It prints a 6-digit code; approve the same code in the app's dialog ("Windows passkey provider"). `... status` should then say `paired: yes`, `app reachable: yes`, `vault: Unlocked`.
4. Turn the LocalPass browser extension's passkey switch off (popup) so Edge asks Windows.
5. In Edge open https://webauthn.io, enter a name, Register; in the Windows sheet choose LocalPass. Expect the app's Windows Hello prompt (note whether it comes to the front), then success on the page. Then sign in on the same page (choose the LocalPass passkey in the Windows list).
6. Watch the log while doing 5: `Get-Content "$env:LOCALAPPDATA\LocalPass\provider\provider.log" -Wait`. Lines to look for: `MakeCredential tx=...`, `request signature ok (Sha256)`, `the app created credential ...`, `credential cache: added ...`, `tx=... finished hr=0x00000000`; for sign-in `GetAssertion ...`, `signing with credential ...`. Any `hr=` other than 0 and any `CancelOperation ... signature covers ...` line is what to send back.
7. Also try: cancel in the Hello prompt (the page should report a cancel and no second prompt should appear), a locked vault (the app should offer unlock), and the app closed (the provider should start it).
8. Undo: `tools\windows-provider\unregister.ps1` removes the authenticator, the package and `target\provider` (run it before deleting `target\`; the pairing stays unless you pass `-ForgetPairing`).

## Hello focus and startup prompt (2026-10-20)

Two bugs from the first real-PC use: (1) with the app closed, the provider's request showed Windows Hello twice; (2) the Hello dialog did not come to the front (a taskbar button flashed until the user clicked it).

**(1) Cause.** The main window is created with `visible: false`, but its web view still loads, and `Unlock.tsx` offered Hello on mount whenever quick unlock was on. In the app started by the provider (`--minimized`) the unlock screen therefore ran its own `unlock_quick` next to the broker's `AppHost::unlock` -> `quick_unlock`: two Hello operations for one request. The pipe answered about 0.6 s after the launch (provider log 15:00:56.423 -> 15:00:57.038), before the web view mounts, so the broker's prompt probably came first; the order is **not proven** (the app wrote no log then), and either order gives two prompts. `document.visibilityState` is `visible` in a hidden window (measured), so the UI cannot tell by itself. The same gate stops a Hello that popped up by itself after an auto-lock in the tray or after autostart at login.
**Fix.** `unlock_quick(auto)`: the unlock screen passes `auto = true` when it offers Hello by itself; while the main window is hidden or minimized the app answers `"hidden"` (log line `unlock screen: not offering Windows Hello by itself, the window is hidden`) and the broker request owns the prompt. The UI retries once on the `windowShown` event, which is sent only when the user opens the window (tray menu or click, starting the app again); a window shown by the broker (unlock fallback after a cancelled Hello, pairing dialog) does not re-arm it, so a cancelled Hello is not followed by a second one. A visible window at start behaves as before. Verified through the real app's command (`unlock_quick {auto:true}` -> `hidden` while hidden; once the pairing dialog had shown the window it ran on); not verified: the React path itself (it needs quick unlock enabled, which needs a Hello gesture).

**(2) Cause.** The foreground lock: a program that is not in front may not take the foreground, so Windows flashes the taskbar button instead. `SPI_GETFOREGROUNDLOCKTIMEOUT` reads `0x7FFFFFFF` on this PC (registry value 150000). A throwaway program that includes `app/src-tauri/src/focus.rs` (`focus-lab`, with editor, Chrome and the taskbar in front): plain `SetForegroundWindow` is refused (`called=false`); `AttachThreadInput`, `SwitchToThisWindow` and a synthetic Alt key each work on their own. The old `focus_hello_dialog` (find the dialog, `SetForegroundWindow` from the background app) was therefore a no-op exactly when it mattered.
**No owner window.** SDK 10.0.26100 has no HWND interop for `KeyCredentialManager`/`KeyCredential` (only `IUserConsentVerifierInterop::RequestVerificationForWindowAsync(hwnd, message)`, which would also take the site name). Unlock needs the key signature as key material, and `verify_user` uses the same API, so both stay on key signing; moving `verify_user` to `UserConsentVerifier` would be a different consent API and was not done.
**Fix (`focus.rs`).** Before a Hello check for a broker client (`AppHost::unlock` via `quick_unlock(.., for_client)`, `AppHost::verify_user`) an `Anchor` is created: a 1x1 transparent tool window (no taskbar button) on its own thread, brought to the front with `bring_to_front` (`Plain`, `AttachInput`, `SwitchTo`, `AltKey`, in that order, each logged, each given 60 ms to take effect). The process then owns the foreground when Hello starts. `hello::focus_hello_dialog` now watches for the new "Credential Dialog Xaml Host" window (one that was not open when the check started, so the passkey chooser is left alone), raises it with the same steps and logs. When the check ends the anchor gives the foreground back to the window that had it and closes. `show_main` (pairing dialog, unlock fallback, tray) also runs the steps after Tauri's own `set_focus`; in the clean runs Tauri's `set_focus` had already brought the pairing dialog forward (the log says `already in front`), so that part is only a logged backstop. The UI's own Hello (window visible) is unchanged.
**Also found: cancel left the dialog open.** Dropping the broker connection reaches `CancelHandle::cancel` and `IAsyncInfo::Cancel()` returns ok, but the dialog stayed on screen (still there after the whole app was killed). The dialog of the check is now closed 400 ms after a cancel if it is still open. The unlock check (`derive_key`) is still not cancellable at all; a client that goes away during an unlock Hello leaves it to the user.

**Log.** `%APPDATA%\app.localpass\prompts.log` (`<data dir>` of the profile; written by `applog.rs`, only `lp_prompt` events, rotated at 256 KB; window class, exe file name, pid, handle, no titles or vault data). A real test shows, in order: `<why>: foreground before: ...` (who was in front), `step Plain called=false ... not in front` and the step that worked, `focus window in front: true`, `Windows Hello starting; foreground: ...`, `Hello dialog appeared after N ms`, `Hello dialog in front: true`, `300 ms after Windows Hello started: foreground ...; dialog found: true`, `gave the foreground back: true`; on a cancel `Cancel() -> ok` and `Hello dialog still open 400 ms after Cancel(): ...`. Lines for the pairing dialog start with `open the LocalPass window`. `unlock screen: ...` lines are the UI's own Hello.

**What was measured (unattended, throwaway `LOCALPASS_PROFILE`, 6 real Hello dialogs, each cancelled or answered at once).** The old build also got the Hello dialog in front within 60-110 ms in every run (CredentialUIBroker takes the foreground itself when no window is in front, and it was never reproduced failing here), so the user's failure was **not reproduced**; what the runs show for the new build: anchor in front, dialog in front 50-100 ms after it appeared, foreground handed back after the check, cancel closing the dialog, the pairing dialog in front (by Tauri's own `set_focus`). **The one case where the steps did not work:** in two early runs a Windows system dialog (`PickerHost`, class `Shell_SystemDialog`, cause unknown, gone after 2-4 s) had the foreground when the pairing dialog opened, and all four steps (including `AttachInput`) were refused. If the real log shows the same with the OS passkey sheet or another system window in front when Hello starts, the anchor does not help there. The anchor's refused-plain-then-`AttachInput` path was exercised by `focus-lab`, not in the app (in the app the foreground was the app's own window left over from the pairing dialog, so plain succeeded). **Unverified:** the real flow (Edge, the OS passkey sheet, provider-launched app), whether the anchor is what makes the dialog come forward there (the next real test's `prompts.log` answers this), and the UI path of (1). Test tool kept in the repo: `crates/lp-broker/examples/prompt_probe.rs` (pairs as a provider against a `LOCALPASS_PROFILE` app and asks for a passkey; the driving script, like `extension/e2e/app.ts`, starts the exe with a profile and a WebView2 debugging port, approves the pairing and polls `GetForegroundWindow`, and was throwaway). Use a debugging port outside Windows' excluded ranges (`netsh int ipv4 show excludedportrange protocol=tcp`; 9378-9477 is one here).

## Phase 4.5: app integration (2026-10-21)

Settings > **Windows passkeys** (Windows only, `app/src/screens/WindowsPasskeys.tsx`) sets the provider up, pairs it and removes it; the NSIS installer carries it. Distribution is a **Developer Mode loose registration** (`Add-AppxPackage -Register <install dir>\windows-passkeys\AppxManifest.xml`, no certificate). A signed MSIX with a self-signed certificate needs administrator rights (the certificate must go into the machine's trusted people store) and is out of scope. When `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock\AllowDevelopmentWithoutDevLicense` is not 1 the screen shows plain guidance and Set up is disabled (the command also refuses, running nothing). The app's own Windows Hello check stays the only user verification. Turning LocalPass on in Windows settings (Accounts > Passkeys > Advanced options) is the user's own Hello and cannot be done by the app; the button opens `ms-settings:passkeys-advancedoptions` (checked on this PC: it lands on Accounts / Passkeys / Advanced options and lists LocalPass; the fallback `ms-settings:savedpasskeys` is only used if opening the first one fails).

### What exists (`app/src-tauri/src/windows_passkeys/`)

| Piece | File |
|---|---|
| `Status` (what the UI gets) | `mod.rs` |
| identity (package `LocalPass.Passkeys`, exe/alias `lp-winauth-provider.exe`, HKCU key, data folder), `Layout`, the traits `CommandRunner` / `System` / `ClientBook`, the fixed PowerShell texts, parsers | `model.rs` |
| status, setup, pair, remove, app-path refresh, cache sync, written against the traits | `flows.rs` |
| the real runner (child process, `CREATE_NO_WINDOW`, time limit, output cap) and the real registry / file-version reads | `real.rs` |
| debounce rules and the task that follows `EngineEvent`s | `sync.rs` |
| `Manager` (one action at a time, Tauri state) and `start` (called from `lib.rs` setup) | `manager.rs` |
| fakes and tests, plus the ignored real-OS test `real_os` | `tests.rs` |

Commands (`commands.rs`, `lib.rs`, `api.ts`): `windows_passkeys_status|setup|pair|remove(forgetPairing)|open_settings`. Non-Windows builds, and a Windows build run with `LOCALPASS_PROFILE` (a development profile must not change the machine-wide registration), answer `available: false` and the section is not drawn.

* **Status** = `available, supported, windowsVersion, developerMode, providerExeFound, packageRegistered, packageInBundle, registeredInWindows, enabledInWindows, paired, appPathOk, problem`. `supported` = the (build, revision) of the `webauthn.dll` file version >= (26100, 6725), read with `GetFileVersionInfoW` (here 26100.7462). **Only build and revision are compared:** a program without a Windows 10 manifest (a plain test binary) is told `6.2` as major.minor (measured: `6.2.26100.7462` where PowerShell shows `10.0.26100.7462`; the string FileVersion `10.0.26100.7171` is another number again), which the first version of the check read as "too old"; the read-only real-OS test found it. Nothing is run when Windows is too old. Otherwise ONE PowerShell call lists the package (`Get-AppxPackage -Name $env:LP_PACKAGE`, one line `package|full name|version|location|status`), and, when the package and its execution alias exist, the provider's own `status` mode (through the alias, about 140 ms) gives registered / enabled / paired from the official API instead of reading the undocumented `HKLM\...\FIDO\<SID>\Plugins` values. `paired` needs both sides: the provider has `client.bin` AND the app's client list has a `provider` record. `packageInBundle` = the registered location is this app's `windows-passkeys` folder. A registration made by `tools/windows-provider/register.ps1` from `target\provider\pkg` is "another copy": it keeps working, the UI shows a note and a secondary **Set up again**.
* **Set up**: checks first (Windows version, Developer Mode, bundled exe + manifest) and runs nothing if one fails. Then: list the package; if it is registered from another folder, `unregister` + `Remove-AppxPackage` it (one identity cannot live in two folders; Windows' switch resets); write `HKCU\Software\LocalPass\Provider\AppPath` = this exe (BEFORE the register, so the provider can find the app from its first call); `Add-AppxPackage -Register` (skipped when already registered here); wait up to 10 s for the alias; `register` through the alias (idempotent: "details updated, key kept"). Resumable: a failure after the package step leaves the package and the next click continues. `0x80073CFF`/"developer" in the error is mapped to the Developer Mode text; any other error is shown with its first line (300 characters, control characters removed).
* **Pair**: forgets stale `provider` records in the app's client list, then `pair --force` through the alias (the app shows its usual approval dialog for kind `provider`; the command returns when the user answered or after the provider's 3 minute wait). A no-op when already paired; "Set up first" when not registered.
* **Remove**: `unregister` through the alias, and only if that succeeded `Remove-AppxPackage` (after stopping `lp-winauth-provider` processes only), then the `AppPath` value (and the key when empty). If the alias is missing or `unregister` fails NOTHING is removed and the error says so: the app never orphans the HKLM entry. Optional **forget the connection**: the app's `provider` records and `client.bin`.
* **Start-up refresh**: 2 s after the app starts, if the alias exists and the registered package location is this app's own `windows-passkeys` folder, `AppPath` is rewritten when it differs (an app that was moved or reinstalled keeps working). A copy that the package does not belong to (a development build, another install) never rewrites it. No PowerShell starts when no alias exists.
* **No user-controlled strings in command lines.** The program paths are fixed (`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`, the alias), the PowerShell texts are constants with single quotes only (nothing to escape), and the three values (`LP_PACKAGE`, `LP_EXE_STEM`, `LP_MANIFEST`) arrive as environment variables. `-EncodedCommand` is avoided on purpose (it trips anti-malware heuristics). Every command has a time limit (30 s list, 120 s register/remove, 45 s provider, 200 s pair, 60 s cache sync) and runs without a console window.

### Credential cache sync

Windows only signs in with passkeys that are in its cache, and only the packaged provider can write it, so the app asks the provider. `sync.rs` follows the engine's events (`VaultChanged`, `Status(Unlocked)`, a lagged receiver, and a nudge after set up / pair), waits for **3 s of quiet**, runs ONE sync at a time (the task awaits it; `Debounce` also keeps a `running` flag), and drops the pending run when the vault is locked (unlocking raises a new event). A run is `status` then `sync-cache` through the alias, only when the provider is registered, turned on and paired (both sides); otherwise it logs why at debug level. Outcomes: `tracing` info `credential cache synced` with the provider's summary line (counts only), warn on failures; no ids, names or secrets.

### Bundling and installer

* `app/scripts/stage-provider.ts` (last step of `bun run build:all`, which `tauri build` runs): builds `lp-winauth-provider` in release (the target directory is whatever cargo would use, `CARGO_TARGET_DIR` is inherited), verifies that the manifest names the class id the exe prints, and copies exe + `AppxManifest.xml` + the three logos into `app/windows-passkeys/` (git-ignored; its `README.txt` is the only tracked file, so `cargo test`, `clippy` and `tauri dev` still find the folder: tauri-build copies resources on every build and fails on a glob that matches nothing). It skips itself when `TAURI_ENV_PLATFORM` is not `windows` (an Android build started on Windows) or the target triple is not `x86_64-pc-windows-msvc`.
* `app/src-tauri/tauri.windows.conf.json` (merged only for Windows targets) maps `../windows-passkeys/**/*` to the `windows-passkeys` folder next to `localpass.exe` and sets `installerHooks`. The manifest is shipped as it is: the class id is the constant `provider::CLSID` (a unit test in `lp-winauth` and one in the app check the file against the constants; `build.ps1` and the staging script compare it with `lp-winauth-provider clsid`). The old `@CLSID@` placeholder is gone.
* Feature `test-identity` of `lp-winauth` (never shipped) builds a provider with another class id (`{5C1D0A7E-3B94-4F1A-8E62-7D0B9A4C2F18}`), authenticator name `LocalPass (test)`, data folder `LocalPass\provider-test` and registry key `Software\LocalPass\ProviderTest`, so the set-up code can be exercised against a throwaway package next to the real one. In debug builds of the app, `LOCALPASS_PROVIDER_PACKAGE`, `_EXE`, `_REGKEY` and `_DATA` replace the package name, exe/alias name, registry key and data folder (release builds ignore them). The throwaway package MUST have its own exe/alias name: two packages with the same execution alias would fight over the one file in `WindowsApps`.

### Uninstall (and install) hook

`app/src-tauri/windows/hooks.nsh`. `NSIS_HOOK_PREUNINSTALL` sets `LP_DIR` = `$INSTDIR\windows-passkeys` and `LP_ALIAS`, then runs one fixed PowerShell command with a 90 s timeout (nsExec, 64-bit file system view): if `Get-AppxPackage -Name LocalPass.Passkeys` is registered from `LP_DIR` (so uninstalling one copy cannot break a registration that belongs to another), run `<alias> unregister`, stop provider processes, `Remove-AppxPackage`, delete the `AppPath` value. All failures are ignored (an uninstall is never blocked). `NSIS_HOOK_PREINSTALL` kills a leftover `lp-winauth-provider.exe` so an upgrade does not stop on a locked exe.

### Findings while building it

1. **Tauri flattens a glob resource.** `"../windows-passkeys/**/*": "windows-passkeys/"` put `Assets\StoreLogo.png` at `windows-passkeys\StoreLogo.png` (`tauri-utils` `resources.rs`: for a glob, "all globbed paths go under the destination, keeping only the file name"), which would break the manifest's `Assets\...` logo paths. A directory source without `*` keeps the structure: `"../windows-passkeys": "windows-passkeys"` (found by reading the generated `installer.nsi` of the first NSIS build; the second build lists `windows-passkeys\Assets\*.png`).
2. **Version resource lie.** A program without a Windows 10 manifest sees `6.2.x` for the major/minor of system files; only (build, revision) of `webauthn.dll` are compared. Found by the read-only real-OS status test, not by the unit tests.
3. **A manifest for another identity must never be registered.** `setup` reads `Identity Name` from the bundled manifest and refuses when it is not the package this app manages (the debug-only identity override together with the shipped manifest would otherwise register the REAL package from a test folder).
4. **The execution alias is a reparse point.** `Path::exists`/`metadata` follow it and fail; the app uses `symlink_metadata`.
5. The installer's reinstall page runs the OLD uninstaller (`ExecWait '"uninstall.exe" [/UPDATE] [/P] _?=dir'`) when the user chooses "uninstall before installing", so the uninstall hook also runs then (the registration is removed; Set up + Windows' switch again). The default choice on that page does not uninstall. `/UPDATE` (an updater) is skipped by the hook (`$UpdateMode <> 1`). LocalPass has no updater.
6. The hook's 64-bit file-system view matters: the NSIS installer is a 32-bit program, so `$SYSDIR\WindowsPowerShell` would otherwise be the 32-bit PowerShell under `SysWOW64` (`${DisableX64FSRedirection}` around the calls).

### Tests and checks (2026-10-21)

* `cargo test -p localpass --lib windows_passkeys`: 37 unit tests pass, 2 ignored (39 in the lib with the older ones). They use a fake PC (`Fake` in `tests.rs`: PowerShell's package list, the provider behind its alias, registry, files, the app's client list; it records every operation in order): status parsing (real `status` output of this PC), status for each state, **set up writes `AppPath` before `Add-AppxPackage -Register`, and registers before the alias `register`; Developer Mode off, unsupported Windows, missing files and a manifest of another identity run nothing at all**, set up twice, replacing a registration from another folder (unregister, remove, then set up), resuming after a failed step; pair (stale record first, `--force`, no-op when paired); **remove: `unregister` before `Remove-AppxPackage`, nothing removed when `unregister` fails or the alias is missing**, forgetting the pairing on both sides; start-up refresh of `AppPath` (own package only; no process without an alias); the cache sync gate (registered, enabled, paired on both sides); `Debounce` (quiet period, bursts, one run at a time, a change during a run gets its own run, locked vault drops the pending run); the PowerShell texts contain no quotes and no spliced values; the shipped manifest matches the constants (`lp-winauth` has the matching test for the class id).
* **Real OS, read-only:** `cargo test -p localpass --lib real_status_readonly -- --ignored --nocapture` against the real registration: `supported`, `packageRegistered`, `registeredInWindows`, `enabledInWindows` true, `packageInBundle` false (it is registered from `target\provider\pkg`), `problem` none (`paired` is false only because that test has no app to ask for its client list).
* **Real OS, set up -> status -> remove** (`real_os`, throwaway package `LocalPass.PasskeysTest`, provider built with `--features test-identity`, exe/alias `lp-winauth-provider-test.exe`): before: not registered; after set up: package from the bundle folder, registered in Windows, not enabled, `AppPath` written; `reg query HKLM\SOFTWARE\Microsoft\FIDO\<SID>\Plugins` during the run listed exactly Edge's `{7D6FA3E8-...}` (State 0), the real LocalPass `{91E9966B-...}` (State 1) and the test `{5C1D0A7E-...}` (State 0); after remove: not registered. A snapshot script (HKLM plugins with every value including the key blobs, packages, `WindowsApps\lp-winauth*` aliases, `HKCU\Software\LocalPass`, the `%LOCALAPPDATA%\LocalPass` listing, the SHA-256 of `client.bin`, processes, temp leftovers) taken before and after was **identical (0 differences)**; no `LocalPass.PasskeysTest` package, alias, key or folder is left.
* **GUI end to end:** `cd extension && bun run e2e/windows-passkeys.ts` (needs `LP_APP_EXE` = a debug app built with `--features custom-protocol` and `LP_TEST_PROVIDER_EXE` = the test-identity provider): the real app with its own profile and the test identity, Settings > Windows passkeys: initial state, Set up (package location, authenticator in Windows' list, `AppPath`), the state text and buttons afterwards, the Remove dialog, state after removing; 16 checks pass, snapshot identical afterwards. Not clicked: "Turn on in Windows settings" (opens Windows Settings on the screen; the page itself was checked by hand) and Pair (needs the provider enabled, which needs a Windows Hello gesture).
* **Uninstall hook:** the shipped `NSIS_HOOK_PREUNINSTALL` text with only the names swapped for the throwaway package was compiled into a tiny NSIS program (makensis from Tauri's NSIS folder) and run twice: with `$INSTDIR` of another folder it changed nothing (package still registered); with the package's own folder it unregistered the authenticator, removed the package (alias, `AppPath` value) in 2.1 s. This exercises the PowerShell text, the `$$` escapes, `nsExec`, the environment variables and the 64-bit redirection switch. The installer built by `tauri build` (6.2 MiB, `target-app\release\bundle\nsis\LocalPass_0.1.1_x64-setup.exe`) contains the hook (`installer.nsi`) and `windows-passkeys\{lp-winauth-provider.exe, AppxManifest.xml, Assets\*.png, README.txt}`. It was NOT run (instructions: do not install it).
* `cargo fmt --all` (no diff), `cargo clippy --workspace --all-targets -- -D warnings` (clean), `cargo test --workspace` (all pass; localpass 39 incl. 2 ignored, lp-winauth 103 + 5 against the real pipe), `cd app && bun run build`, `cd extension && bunx tsc --noEmit`, `cargo clippy -p localpass --target aarch64-linux-android -- -D warnings` (clean).

### Not verified

* An actual run of the NSIS installer and uninstaller (the hook text and its logic were run in isolation, see above); the reinstall page behaviour is read from the generated script.
* The screen on a PC that never used the provider scripts (this PC has the dev registration, the GUI test used the throwaway identity), and with Developer Mode off (the check is a registry read plus a refusal that runs nothing; unit-tested, not tried by switching Developer Mode off).
* Pairing from the button (`pair --force` through the alias was run by the earlier phases, not by this code), and the cache sync against a live vault (its gate and debounce are unit-tested; `sync-cache` itself was run by hand in 4.3).
* `tauri android build` with `tauri.windows.conf.json` present (the config is chosen by the target triple in `tauri-build`; the Android clippy check passes).
* A newer provider exe inside an already registered package folder: the installer replaces the files in place, the package version stays 0.1.0.0, and Windows keeps using the registration; if the manifest itself changes in a later version, the package version must be raised and the registration redone (Set up again).

## Phase 4.6: assisted test with an independent relying party (2026-10-21)

Windows reserves two things for a person: turning a passkey provider on (Settings, Windows Hello) and the Hello check the app makes for every create and sign-in. So the real-provider test is **assisted**: a script runs everything around those steps and judges the result. Nothing below touched the real registration, `client.bin`, `profile.txt`, Edge's FIDO entry, the user's app or browsers; **no real-provider ceremony was run by the agent** (it would have popped Windows UI on the user's desktop and needs a fingerprint).

### Run it (about 5 minutes, you at the PC)

```
cd extension && bun install        # once
bun run e2e/windows-provider.ts    # create + sign in
bun run e2e/windows-provider.ts --all    # also cancel, locked vault, app closed
```

Flags: `--cancel`, `--locked`, `--app-closed`, `--all`, `--browser edge|chrome` (default Edge, or `BROWSER`), `--port N` (default 8473), `--timeout S` (per human step, default 240), `--user NAME` (default `lp-test-<date>`), `--virtual` (self-test, below), `--help`.

What you see: a preflight (`provider registered and turned on in Windows`, `provider paired with the app`, `app`), then Edge opens `http://localhost:8473/` in a **throwaway profile** (no LocalPass extension, so Windows' own passkey UI is used). For each step the script prints the numbered actions and waits:

1. click **Register**, choose **LocalPass** in the Windows sheet (use "Change" / "Other options" if another provider is offered), accept the LocalPass **Windows Hello** prompt (it may only blink in the taskbar);
2. click **Sign in**, choose the LocalPass passkey, accept Hello.

With `--cancel` you cancel Hello once; with `--locked` you lock the vault when asked (the script waits until `status` says `vault: Locked`); with `--app-closed` you quit LocalPass from the tray when asked (the script never closes it; the provider starts it again and the app stays running afterwards). `...` lines show progress ("the provider received the request from Windows", "still waiting"). When a step does not happen within the timeout it fails with a row and what to look at, later steps that need it are skipped, and everything is cleaned up. Ctrl+C also cleans up.

Expected end (shape; times differ):

```
PASS  provider registered and turned on in Windows   enabled
PASS  provider paired with the app                   yes
INFO  app                                            running, vault Unlocked; credential cache 2 row(s)
PASS  register: registration verified by the RP      6 checks; flags UP=1 UV=1 BE=1 BS=1; counter 0; aaguid 00000000-...; fmt none
PASS  register: provider log: every hr= is 0         MakeCredential 0x00000000
PASS  register: app logged exactly 1 Hello prompt    dialogs 1, starts 1
INFO  register: timing                               page 11.8 s (includes your clicks), provider 3.1 s, RP verify 9 ms, Hello dialog up after 260 ms, ...
PASS  signin: sign-in verified by the RP             6 checks; flags UP=1 UV=1 BE=1 BS=1; counter 0; userHandle ok
PASS  signin: provider log: every hr= is 0           GetAssertion 0x00000000
PASS  signin: app logged exactly 1 Hello prompt      dialogs 1, starts 1
PASS  credential cache grew by one row               2 -> 3
PASS  provider still registered, enabled and paired  app running, vault Unlocked
all checks passed
Delete the test passkey "lp-test-..." (site localhost) in the LocalPass app: it is in your REAL vault.
```

Exit code 1 and `FAIL` rows otherwise. Output folder `target/windows-provider-test/<time>/`: `rp.jsonl` (every ceremony verdict with all checks), `provider-log-new-lines.txt`, `prompts-log-new-lines.txt`, `report.json`.

### What is automated and what is human

| | Automated | Human |
|---|---|---|
| Relying party on `127.0.0.1` (rpId `localhost`), port 8473 (not 1420, not in the reserved ranges), page, SimpleWebAuthn verification, `/state`, JSON-lines log | yes | |
| Throwaway Edge/Chrome profile, start, kill (only what it started), delete the profile, close a leftover "Passkey for localhost" sheet | yes | |
| Preflight and after: read-only `lp-winauth-provider status` (registered, enabled, paired, app reachable, vault, cache rows) | yes | |
| Watching `provider.log` (only lines added during the run: `hr=` values, transaction ids, `CancelOperation`, app start) and `prompts.log` (Hello starts and dialogs per ceremony) | yes | |
| PASS/FAIL table | yes | |
| Turning the provider on in Windows settings (Hello), pairing (approve the code in the app) | | before the run |
| Click Register / Sign in, choose LocalPass in the Windows sheet, the Hello check, cancel, lock the vault, quit the app | | yes |

**The relying party** (`extension/e2e/rp.ts`, also used by `e2e/passkeys.ts` for the extension) in assisted mode asks for a resident key, UV required, attestation `none`, ES256 only, with an exclude list of the credentials it knows. It checks with SimpleWebAuthn: origin, RP id hash, challenge, type, signature, UV; and itself: UP, **UV, BE=1 BS=1** (LocalPass design), format `none`, public key algorithm -7, **counter 0** at registration and at sign-in, **user handle returned and equal to the registered user id** at sign-in, credential known to the site. The AAGUID is logged. `/state` returns `{origin, rpId, credentials, events}`; every event (also what the page saw: `NotAllowedError` etc.) is appended to the JSON-lines file.

**What a ceremony must show to PASS:** the RP verified it with no failed check; the provider log has the transaction with `finished hr=0x00000000` and no other non-zero `hr=` line (a Windows repeat of the same transaction id is shown, not failed); `prompts.log` has exactly one "Windows Hello starting" for it (cancel: one, and none after the cancel; with a locked vault or a closed app no Hello line is a WARN, because a master password can unlock instead); cancel: the page shows `NotAllowedError` and the provider answered `0x80090036`; app closed: provider.log has `LocalPass is not running: starting` and `started ... (pid N)`.

### Verified without a human (2026-10-21)

* `cd extension && bunx tsc --noEmit && bun test`: 56 pass. `test/rp.test.ts` runs the RP against a **software authenticator** (ES256, `none` attestation made with `isoCBOR`): a correct create and two sign-ins are verified and logged; refused or flagged: no UV, no backup flags, wrong origin, wrong RP id hash, a registration counter other than 0, missing and wrong user handle, a counter that moves, unknown credential, signature by another key; options (resident key, UV, `none`, ES256 only, exclude list, switch off) and `/state` and the log file. `test/provider-analysis.test.ts` covers the log parsers (real line formats), the `status` parser (real output) and the PASS/FAIL judgement (every row, the cancel case, locked/closed, timeout, the NTE_NOT_FOUND hint) and the table.
* `bun run e2e/passkeys.ts` (extension, headless Chrome, same RP code): all checks still pass.
* `bun run e2e/windows-provider.ts --virtual --cancel`: the whole runner (RP, headless Edge with a CDP **virtual authenticator**, the phases, `/state`, the table, the clean-up) passes, 10 s; `LP_VIRTUAL_NO_BACKUP=1` (authenticator without BE/BS) fails on the backup-flags check and skips the sign-in; `LP_VIRTUAL_IDLE=1 --timeout 5` (nobody answers) fails with the timeout row; both exit 1. In `--virtual` the provider and Hello rows are SKIP and the counter is only checked for "not going backwards" (the virtual authenticator counts).
* The real runner with nobody at the PC: `LP_BROWSER_ARGS=--headless=new bun run e2e/windows-provider.ts --timeout 8` (hidden Edge, no click): preflight against the real provider (read-only `status`: enabled, paired), the real log tails, start, time-out FAIL row with hints, status after, clean-up; afterwards no process for the profile, no temp folder, no listener on 8473, the real provider log only gained the script's own `start mode=status` lines. (Side observation: when a browser calls WebAuthn at all, Chrome in `e2e/passkeys.ts` here, Windows starts the provider's COM server for a read-only `GetLockStatus` query; that is normal and unrelated to a ceremony. Edge with a virtual authenticator did not trigger it.)

### Not verified (needs you)

* A real Windows sheet with LocalPass in it, the Hello prompt, and the provider/app answers for a script-driven page (the provider and `prompts.log` rows have only been exercised on sample text of the real formats).
* `passkey-sheet.ps1 -Mode cleanup` pressing Cancel on a real sheet (written from an earlier probe of the sheet; run here only with no sheet open: `cancelled: 0`).
* Chrome as the throwaway browser (`--browser chrome`) with a real provider; Edge's own prompts in a fresh profile (the script passes `--no-first-run`).
* `prompts.log` can undercount: the dialog watcher in `hello.rs` ignores a Hello that starts while a previous watcher is still running (the first 0.3 to 5 s), so a very fast second prompt may be missed.

### Decisions and findings

* **Unattended mode (Hello skipped): not built.** The provider-to-app half is already covered without a person: `crates/lp-winauth/tests/pipe.rs` runs the real provider code against `dev_host`, whose host approves every Hello. A skip inside the real app (`AppHost::verify_user` / `unlock`) could be gated by `cfg(debug_assertions)` plus `LOCALPASS_PROFILE`, so a release build and the default profile could never take it, but it would only serve a throwaway `test-identity` provider (own CLSID, own pairing in `LocalPass\provider-test`), which still needs one human Hello in Windows Settings to be turned on and a click in the Windows sheet. The real provider has exactly one `client.bin` and the user's vault and must never skip. A new branch in the user-verification path for that small gain is not worth it.
* **`--auto-sheet` (clicking the LocalPass entry in the Windows sheet by UI Automation) is dropped.** It would still need the fingerprint for Hello, and proving it needs repeated Windows sheets on the user's desktop; the earlier probe only established that the sheet is a `Credential Dialog Xaml Host` window (below `Shell_SystemDim`) with the text "Passkey for localhost" and a `CancelButton`. That is all `passkey-sheet.ps1` uses.
* **A browser that is killed while the sheet is open leaves the sheet on screen**, so after killing the throwaway browser the runner closes sheets whose text is "Passkey for localhost" (and nothing else).
* **GetAssertion needs exactly one LocalPass passkey for `localhost`** (no picker yet, see 4.4). A second run without deleting the first test passkey can answer `NTE_NOT_FOUND` at sign-in; the table says so. Delete `lp-test-*` passkeys after a run.
* Windows' reserved TCP ranges are not fixed (`netsh int ipv4 show excludedportrange protocol=tcp` on 2026-10-21 listed 3143-3242, 5357, 6690-6789, 8859-8958 and 13075-13674, not the 6885-6984 named elsewhere). 8473 is in none of them; the RP falls back to a free port if the wanted one cannot be bound.
* Importing `harness.ts` from the runner is only for `browserPaths` and `sleep`; the runner does not start the dev host.

### Manual checklist (you, about 15 minutes; replaces step 7 of the 4.3/4.4 list)

1. **Enable:** `& "$env:LOCALAPPDATA\Microsoft\WindowsApps\lp-winauth-provider.exe" status` must say `registered: yes, enabled`. If it says disabled or not registered: LocalPass > Settings > Windows passkeys > Set up, then "Turn on in Windows settings" (LocalPass on, Hello).
2. **Pair:** start LocalPass and unlock it; Settings > Windows passkeys > Pair (or `... lp-winauth-provider.exe pair`) and approve the code. `status` then shows `paired: yes`, `app reachable: yes`, `vault: Unlocked`.
3. **Register:** `cd extension && bun run e2e/windows-provider.ts --all`, then follow the printed steps: Register, LocalPass in the Windows sheet, one Hello. The table must show PASS for the registration.
4. **Sign in:** the same run's next step: Sign in, the LocalPass passkey, one Hello.
5. **Cancel:** (`--cancel` step) cancel Hello: the page shows an error, no second prompt appears.
6. **Locked vault:** (`--locked` step) lock the vault, sign in: the app asks to unlock, exactly one Hello.
7. **App closed:** (`--app-closed` step) quit LocalPass from the tray, sign in: the provider starts the app, one Hello, sign-in works.
8. **A real site:** https://webauthn.io in Edge with LocalPass's extension passkey switch off (popup): register and sign in with LocalPass. `Get-Content "$env:LOCALAPPDATA\LocalPass\provider\provider.log" -Tail 40` must show `hr=0x00000000` for both.
9. **Clean up:** delete the `lp-test-*` and webauthn.io test passkeys in the app (Items, kind Passkey). Send back the result table or the `report.json` path and any `FAIL` row.
