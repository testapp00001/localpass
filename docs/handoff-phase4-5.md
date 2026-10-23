# Handoff: Phases 4 and 5 (written 2026-10-19)

Read this file first, then only the notes file for the task you are on. Do not re-read the long
history: everything needed is here, in `CLAUDE.md`, and in the two notes files below.

## Where things stand

| Phase | What | State |
|---|---|---|
| 0 | Shared core: passkey crypto, origin rules, engine credentials API, `lp-broker` | done, tested |
| 1 | Windows broker + native-messaging helper + extension (autofill, save) | done, Chrome + Edge e2e |
| 2 | Passkeys in the extension (WebAuthn wrapper) | done, Chrome + Edge e2e, real-app e2e |
| 3 | Android Credential Manager provider + AutofillService | done, emulator e2e (Chrome 145) |
| 4 | Windows system-wide passkey provider (loose Dev-Mode package, `crates/lp-winauth`) | done, verified by the user on a real PC |
| 5 | Hardening, docs, release builds | **in progress**: 5.1-5.3 done, 5.4-5.6 to do |

## RESUME HERE (state saved 2026-10-22; read this, then only the notes for your task)

**Before any new work, the user wants to agree a cheap working plan** (their usage quota ran out: a 375k-token chat plus many big agents). Suggested: one task per fresh chat; do small edits inline; at most one agent at a time and only for heavy work; no re-running whole e2e suites or cold builds unless the task needs it; tell the user the rough cost first. Do not start anything until they agree.

Done: Phases 0-4; Phase 5 task 5.2 (all fixes, parts A Rust / B extension / C Android; notes sections 5.5, 5.5b, 5.5c in `docs/phase5-hardening-notes.md`).

**5.1 is done (2026-10-22).** Property tests: `crates/lp-broker/tests/{framing_props,protocol_props}.rs`, `crates/lp-core/tests/{origin_props,passkey_parsers,passkey_props,wire_props}.rs`, `crates/lp-engine/tests/{provider_props,webauthn_json_props}.rs`, `crates/lp-sync/tests/replica_props.rs`, `crates/lp-winauth/tests/translate_props.rs`, and proptest blocks in `lp-core/src/materialize.rs` and `lp-engine/src/backup.rs`. A 1000-case run passed everything (279 tests, 47 s, no shrunk failure). No production code changed; the one failure was a wrong expectation in a test, fixed. Results, what each file checks, the mutation checks and the pinned observations: hardening notes, section 7. Commands: `PROPTEST_CASES=1000 cargo test -p lp-core -p lp-broker -p lp-engine -p lp-sync` and `cargo test -p lp-winauth --test translate_props`. Not re-run after 5.1: `cargo test --workspace` as a whole and the extension tests (only test files changed).

**5.3 is done (2026-10-22).** Four read-only reviewers (extension+broker, desktop Rust, Windows provider, Android) found 0 High, 4 Medium, 20 Low and 12 Info; the findings, what was fixed (each with a test) and what was left are in `docs/phase5-security-review.md`, last section "Triage and fixes". Fixed: RA-1 (page CSS/decoys could make a user click an invisible fill button; now Intersection Observer v2 gate, verified in real Chrome), RB-1 (backups lost passkeys), RC-1/RB-5 (pipe squatting by another account), RD-1 (userdebug browser keys trusted by autofill), plus 15 Low/Info items. Workspace: fmt, clippy `-D warnings` clean, 455 tests pass; extension 171 tests; Kotlin 37 tests; `e2e/run.ts` 29 and `e2e/passkeys.ts` 24 pass in Chrome.

**Carry into the next tasks (details in the triage section):** 5.4 README "Security notes and limits" must state the known limits (hostile `.kdbx` KDF cost RB-3, LAN pairing stall RB-4, pipe listener not retried RB-5, Windows passkey cache readable by the same user RC-6, provider files user-writable RC-3, COM server open to same-user processes RC-8, orphan passkey on a last-moment cancel RC-2); 5.5 must fix the NSIS uninstall hook (RB-8/RC-4: browser keys, provider data, do not remove the package when `unregister` failed) and pin/record the trusted-browser list (RD-9: SHA-256 in a test, source and date, drop the unused `assets/extension/` copy); 5.6 assisted run covers RC-5, RC-9, RD-2/RD-5 on a phone, RD-3 (autofill frame/domain), S4 for non-Chrome browsers, the registered Windows provider reaching the app through the new pipe-owner check (first real exercise is the 5.5 build) and `e2e/host.ts` with the release exe. The README must also say that the in-page interface is inert on pages with a `filter`/`opacity` on `<html>` (fails safe, popup still works) and that passkeys travel only in the encrypted backup (the plain export leaves them out and says how many). Not run in 5.3: `e2e/app.ts`, anything Android on a device or emulator, Edge, the Android Rust check. The user's LocalPass (`target\release\localpass.exe`) was running at the start of this session and was not at the end: nothing here stopped it on purpose (cause unknown), and that build predates all of Phase 5 anyway.

Then, in order: **5.4** README "Security notes and limits" + `CLAUDE.md`, **5.5** release builds (NSIS installer, signed Android APK arm64+x86_64 with a version/versionCode bump, extension zip; smoke tests; SHA-256 of each; the user's app runs from `targetelease`, build elsewhere), **5.6** final matrix and report to the user. Ask the user at 5.5: unsigned NSIS (SmartScreen warning) vs a code-signing certificate; side-loaded extension vs store listing; the README trust-boundary sentence (is same-user malware in scope?).

Open follow-ups: Android start-up crashes (secret kept) on a persistent Keystore failure, needs an error screen; `e2e/app.ts` has one check ("browser is not set up yet") that fails on a PC with a real native-messaging registration; `extension/e2e/android-release.ts` was not run since the prompt titles changed; a real QR decode and real-phone behaviour (pairing, autofill, passkeys on Android 14-16, arm64) are untested; the user's `targetelease\localpass.exe` predates the Phase 5 changes (rebuild to see the Replace? dialog and Hello-for-other-site fill).

Machine state at save time (verify, do not assume): the user's app is running (leave it); the Windows provider is registered from `target\provider\pkg`, ENABLED and PAIRED (never re-register: it resets the Windows toggle); Edge's FIDO entry untouched; no emulator, `dev_host` or bun processes of mine running; AVD `lp_api37` restored (PIN 1234, 1 fingerprint); no commits exist in the repo (do not commit unless asked); HKCU Run entry "LocalPass" and the Chrome native-messaging key are the user's own: leave them.

Checks that must stay green (run the ones relevant to what you touched):

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings          # host
cargo test --workspace                                         # 131 tests at the end of phase 3
cd extension && bunx tsc --noEmit && bun test                  # 56 pass
cd extension && bun run build --e2e && bun run e2e/run.ts      # browser e2e (also e2e/passkeys.ts, e2e/app.ts; BROWSER=edge)
```

Android Rust check and the emulator e2e are in `CLAUDE.md` (Android section).

Research notes written ahead of time (read the one for your task, not both):
- `docs/windows-passkey-provider-notes.md` for Phase 4 (SDK headers digested, manifest, spike procedure).
- `docs/phase5-hardening-notes.md` for Phase 5 (attack-surface table, test plan, known issues).

## Working rules (these exist because one long session ran out of room)

1. **One task at a time.** Do a task, verify it, update the status boxes below and the memory file
   (`project-localpass-extension-passkeys.md`), then stop or move on. Never leave the only copy of
   important state in the conversation.
2. **Delegate with the Agent tool (general-purpose)** for: reading large headers or docs, a spike
   with lots of trial and error, a test-debug loop, a security review, drafting docs. Give each agent
   a self-contained prompt: goal, files to read, constraints, the file it must write, and "reply in
   under 200 words". Read the agent's output file section you need, never its transcript.
3. **Keep output small.** Pipe long output through `tail -n 30` or `grep`. Run builds and e2e in the
   background and wait for the notification. Do not poll with sleep. Avoid screenshots (images are
   expensive); read the screen with uiautomator text (`extension/e2e/adb.ts`).
4. **Write big files with the Write tool.** Large bash heredocs break on this machine. A background
   command that ends in `| tail` reports exit code 0 even when the command failed: read the output.
5. **Stop at gates and tell the user.** Phase 4 has a go/no-go spike; do not build the full provider
   until the spike result is reported.
6. **Never commit or push unless asked** (the repo has no commits). If asked, end commit messages
   with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`.
7. **Leave no residue.** Restore registry keys, emulator settings and port forwards; stop the
   emulator you started (the user's AVD is `Pixel_10_Pro_XL`: never touch it); never kill the user's
   running LocalPass (it runs from `target/release`; build release into a separate
   `CARGO_TARGET_DIR` under `$CLAUDE_JOB_DIR/tmp`). Never print or commit secrets
   (`keystore.properties`, `*.jks`).

## Environment facts

- Windows 11 build 26200, Developer Mode ON (loose-file MSIX registration works without a
  certificate), SDK 10.0.26100.0 (makeappx, signtool), TPM 2.0, Chrome 155 and Edge 155.
- Windows reserves TCP ports 6885-6984 (Hyper-V); the Vite dev port is 1420.
- Emulator `lp_api37` (mine): PIN 1234, fingerprint enrolled, Chrome 145. First start after an
  install can raise a false "isn't responding" box (software-rendered WebView).
- The user's HKCU Run entry "LocalPass" points at `target\debug\localpass.exe` (written 10-17, not
  by tests): leave it.
- Tests that touch the registry or settings restore them; check for strays afterwards:
  `HKCU\Software\{Google\Chrome,Microsoft\Edge}\NativeMessagingHosts\app.localpass.host`,
  `%TEMP%\lp-*e2e-*`, stray `chrome`/`dev_host`/`qemu` processes.

## Phase 4: Windows system passkey provider

Goal: LocalPass appears in Windows' passkey provider list, and any app or browser (also without the
extension) can create and use LocalPass passkeys. Private keys stay in the app; the provider only
relays over the broker pipe (client kind `provider` already exists and may only do passkey ops).
The main app stays an NSIS install; only the provider is an MSIX package.

- [x] **4.0 Read the notes** (`docs/windows-passkey-provider-notes.md`) and decide the spike plan. Main session, short. Plan: split at the human step. Stage A (agent, unattended): build, loose-register, `AddAuthenticator` with identity, unregister/re-register round trip, model B once. Stage B (user): enable in Settings with Hello, trigger MakeCredential. GO only after stage B (SUCCESS 2).
- [x] **4.1 SPIKE (agent, then GATE).** **RESULT 2026-10-20: GO (SUCCESS 2 reached, user's log).** The user enabled the spike provider in Settings; webauthn.io MakeCredential reached our COM server (started by the OS), request signature verified (ECDSA P-384, SHA-256), `WebAuthNDecodeMakeCredentialRequest` ok, returned `NTE_USER_CANCELLED`. Details: notes file, section 9, "Spike result (stage B)". Machine state now: package registered, authenticator ENABLED (State 1) with the spike server that always cancels: disable or clean up (commands in the notes) before the user relies on passkeys elsewhere. **Stage A detail:** F9 PASSED (logged, repeated after a reboot). `AddAuthenticator` accepted the Dev-Mode loose package (hr 0, `PackageSignatureKind 0`, PFN `LocalPass.PasskeyProvider_rwhsg7s1qxd2y`), but only with a non-NULL `pwszPluginRpId` (NULL gives `0x80090027`); the op-sign key is 104-byte ECDSA P-384, not 72-byte P-256. Evidence, the stage B steps and the two cleanup commands: notes file, section 9, "Spike result (stage A)". Machine state: package registered and authenticator registered DISABLED (State 0) for stage B; code in `crates/lp-winauth` (compiles, clippy and fmt clean, `cargo test -p lp-winauth` passes), layout in `target/spike/pkg`. NOT a GO: stage B (user enables it in Settings with Hello, triggers MakeCredential, SUCCESS 2) decides. Smallest experiment: loose-file MSIX with a COM server and
  `WebAuthNPluginAddAuthenticator`. Success = LocalPass shows up in Windows' passkey provider
  settings and a MakeCredential request reaches the COM server (log it). Write the result and
  evidence into the notes file. **Report GO or NO-GO to the user. On NO-GO stop Phase 4**, record
  why in the notes and README ("not possible because ..."), and go to Phase 5.
  Risks the research found (details in the notes file):
  1. Unknown whether `WebAuthNPluginAddAuthenticator` accepts a Developer-Mode loose registration (this is the spike's main question). Plan B is Edge's model: a sparse package with an exe manifest and a registry CLSID.
  2. The plugin functions are exported by `webauthn.dll`, not by the SDK `.lib`; bind them with `LoadLibrary`/`GetProcAddress`.
  3. OS requests carry no origin, but `lp_engine::credentials` requires one: the broker/engine protocol needs a provider path that takes RP id only (the OS has already bound the request to the RP). The broker also runs its own Windows Hello, which would double the OS user-verification prompt; skip it for provider calls.
  4. Windows only routes sign-in to credentials in its cache, and the cache calls probably need package identity, so the NSIS-installed app may not be able to push to it; the provider process (with identity) may have to do it, and the broker needs a "list all passkeys" op for that.
  5. Cancel-signature details come from KeePassPasskey (not Microsoft docs); the response buffer must be CoTaskMem-allocated; whether a zero AAGUID is accepted is untested.
- [x] **4.2 FFI layer (agent).** Hand-written `#[repr(C)]` types and extern fns for the plugin API in
  a new crate (suggest `crates/lp-winauth`), with tests that pin struct sizes and offsets.
  **DONE 2026-10-20.** `crates/lp-winauth/src/sys/` holds `types.rs` (26 final-API structs, `Default` = zeroed), `api.rs` (`WebAuthn::load()` binds 23 `webauthn.dll` exports with `LoadLibraryExW` + `GetProcAddress`; a missing export is `LoadError::Unsupported(name)`, `WebAuthn::missing_exports` lists them all; safe wrappers for state/add/update/remove, op-sign and UV keys, `perform_user_verification`, the credential cache, status callback, decode and encode; RAII owners `OsBytes`, `AddResponse`, `CredentialDetailsArray`, `DecodedMakeCredential`/`DecodedGetAssertion` (with accessors for rp id, client-data hash, user, algorithms, exclude/allow ids, options), `CoTaskMemBuf` (`write_to`/`into_raw` hand the allocation to the COM response)) and the tests. No `SetAuthenticatorState`, `EXPERIMENTAL_*` or `*2` binding (a test pins that). **Layout numbers are not from the notes:** `crates/lp-winauth/tools/layout_check.c` compiled with MSVC `cl` against SDK 10.0.26100.0 generates `src/sys/layout_tests.rs` (size, align and every field offset of all 26 structs, plus enum, version and HRESULT constants); it matched the notes everywhere, and the regenerated output was diffed against the checked-in file. `cargo test -p lp-winauth`: 52 pass (workspace 183), clippy `-D warnings` and fmt clean. Decode and encode run against the real `webauthn.dll` without package identity: the captured 179-byte webauthn.io request decodes to rpId, alg -7, user and a 32-byte hash (also credProtect 2, rk/uv options, exclude and allow lists built by hand, malformed input is an error), `none` attestation and assertion responses encode to CTAP2 CBOR in a `CoTaskMem` block (`IMalloc::DidAlloc` = 1). `examples/spike.rs` and `register.rs` now use the new layer (the registered spike exe was not rebuilt or touched; everything was built in a separate `CARGO_TARGET_DIR`). **Unverified** (needs identity, Hello or a state change, so not exercised): the credential-cache calls (and the `GetAllCredentials` empty-cache mapping), status callback, `PerformUserVerification` and UV key, `UpdateAuthenticatorDetails`, `GetOperationSigningPublicKey` errors for an unknown CLSID, and an encoded response being accepted by the OS. Corrections: notes file, "Corrections from 4.2".
- [ ] **4.3 Provider process and pipe client (agent).** The COM server relays each OS request to the
  app over `\\.\pipe\localpass-broker-v1-<SID>` (profile suffix as in `lp-broker`), pairs once as client kind
  `provider` (approval in the app, secret in DPAPI like the extension). Add any missing broker ops
  with tests in `crates/lp-broker/tests`.
  **Part A DONE 2026-10-20 (broker side; part B = provider binary, translate, cache, scripts is still open, so not ticked).**
  `lp-broker` has features `server` (engine, pipe listener, nmhost) and `client` (both default; the provider wants
  `default-features = false, features = ["client"]`, whose only internal dependency is `lp-core`; to allow that, part B must set
  `default-features = false` on the root `[workspace.dependencies]` entry and give `app/src-tauri` `features = ["server"]`, or Cargo ignores the member's opt-out).
  `lp_broker::client`: `Client::{connect, pair(on_code), request, request_as, send_request/response, into_stream}`, `Options`, `Credentials`
  (secret zeroized, Debug hides it), `ClientError {Unpaired, Denied, Busy, Locked, ServerProofMismatch, Io, Timeout, Remote, Protocol, Closed}`,
  `connect_pipe`/`pair_pipe(profile, ..)` (`pipe::pipe_name_for(profile)`; `None` = default pipe, env not read). Request DTOs/results and `VaultStatus` moved to `lp-core`
  (re-exported by `lp-engine`); `Request` is now Serialize too. New `Request::PasskeyListAll` / `Engine::passkey_list_all` (metadata only, never prompts, `locked` if locked).
  Pairing dialog and `UiEvent::ExtensionPairing` carry `kind`; Hello gets the site as `reason` (logged only: the Hello API has no text field).
  A dropped connection now aborts its pending unlock/Hello/pairing prompts (JoinSet; pairing selects on the reader), and `AppHost` cancels the WinRT Hello operation and closes the pairing dialog
  (`extensionPairingCancelled`): **open one connection per OS operation and drop it to cancel**. Unverified on a real Hello dialog.
  **Part B BUILT 2026-10-20, awaiting the user's real-OS check (box stays open).** `crates/lp-winauth`: COM server + modes `register|unregister|status|pair|sync-cache|cache-selftest|clsid`
  (`src/bin/lp-winauth-provider.rs`, `com.rs`, `provider.rs`), `relay.rs` (real relay: one pipe connection per operation, app start, pairing), `store.rs` (DPAPI `client.bin`, `profile.txt`, HKCU `AppPath`),
  package `LocalPass.Passkeys` (`package/AppxManifest.xml`, file/registry virtualization off) and `tools/windows-provider/{build,register,unregister,status}.ps1`. Registered and **disabled** on this PC
  (CLSID `{91E9966B-58AE-455A-AB60-4B2FDB66A730}`). `lp-broker` is now `default-features = false` at the root; the provider uses `client`, the app `server`+`client`; the provider links no engine/sync/store crate.
  Key finding: the response buffer pointer must be non-null even on failure (`pointer_default(ref)`, else `0x800706F4`), probably what made Windows retry the spike. Details, HRESULT table, store layout and the manual checklist: notes file, "Phase 4.3 / 4.4 build".
- [ ] **4.4 MakeCredential / GetAssertion / Cancel (agent).** User verification with the plugin UV
  call, exclude/allow lists, authenticator data and signature encoding, `UP`/`UV`/`BE`/`BS` flags as
  in `lp_core::passkey`. The app pushes credential metadata to Windows when the vault changes
  (`EngineEvent::VaultChanged`).
  **BUILT 2026-10-20 (same change as 4.3 part B), awaiting the user's real-OS check (box stays open).** `src/handler.rs` + `translate.rs`: request signature checked first, in-flight map per transaction id (repeats and concurrent
  duplicates get the first result, kept 60 s), `CancelOperation` drops the pending connection (Hello withdrawn), app Hello is the only user verification (no `PerformUserVerification`), `none` attestation and assertion encoded
  with `WebAuthNEncode*`, credential added to Windows' cache after a create, GetAssertion needs exactly one matching passkey (no picker yet; several = `NTE_NOT_FOUND`). Tested without gestures: 255 workspace tests (lp-winauth 101 unit +
  5 against the real pipe via `dev_host`, assertion signature verified against the registered key), COM probe through the OS proxy/stub, register/unregister round trip. Open until a real webauthn.io create and sign-in
  succeed. The app-triggered cache sync on `VaultChanged` is not built (4.5); until then new passkeys reach the cache at the next provider start or `lp-winauth-provider sync-cache`.
  **Real-PC test 2026-10-20 (create and sign-in on webauthn.io work), two bugs fixed, awaiting a re-check:** (1) a locked vault showed Windows Hello twice: the broker counts the unlock done inside a
  passkey request as its user check, and GetAssertion is now ONE broker request (`PasskeyAssertAny`, provider clients only: unlock if needed, pick the single matching passkey, `notFound`/`ambiguous` otherwise, Hello only when it was already unlocked),
  so create and sign-in both ask once (notes finding 7); (2) with the app closed the provider could not start it (`os error 50`, the COM server sits in a job object and the browser helper's `DETACHED_PROCESS` flags are refused):
  `relay::launch_exe` now uses plain `CreateProcess`, then `ShellExecuteExW` (both verified from the real COM server with a probe exe; finding 8). The package was found unregistered at the start of that work and was registered again
  from the new build, then the exe was replaced in place (no second re-register; `status.ps1` showed LocalPass ENABLED afterwards, the user had turned it on in Settings); `client.bin` (pairing) is untouched.
  **Hello focus and startup prompt (2026-10-20, awaiting a re-check):** (1) two Hello prompts when the provider had to start the app came from the app's own unlock screen, which offered Hello by itself in the hidden window next to the broker's unlock: `unlock_quick(auto)` now refuses while the window is hidden, and the UI offers it when the user opens the window; (2) the Hello dialog did not come to the front because a tray app may not take the foreground: `app/src-tauri/src/focus.rs` brings a 1x1 `Anchor` window (and then the Hello dialog) to the front with measured steps, the main window gets the same steps as a logged backstop after Tauri's own `set_focus`, and every attempt is logged to `%APPDATA%\app.localpass\prompts.log`. Details and what the log shows: notes file, "Hello focus and startup prompt".
- [x] **4.5 App integration.** Settings section "Windows passkeys": register/unregister the package,
  show status, link to Windows settings; scripts under `tools/windows-provider/` (build, loose
  register, unregister).
  **DONE 2026-10-21 (details: notes file, "Phase 4.5").** `app/src-tauri/src/windows_passkeys/` (status / set up / pair / remove behind a `CommandRunner` + `System` + `ClientBook` trait,
  PowerShell only for Get/Add/Remove-AppxPackage with values in environment variables, never in command lines; commands `windows_passkeys_status|setup|pair|remove|open_settings`),
  `app/src/screens/WindowsPasskeys.tsx`, Developer-Mode loose registration (guidance instead of a failure when it is off). Bundling: `app/scripts/stage-provider.ts` (last step of `build:all`) fills `app/windows-passkeys/`,
  `tauri.windows.conf.json` ships it as the `windows-passkeys` resource folder; the manifest now carries the final class id (no placeholder). Credential cache sync from the app: debounced 3 s after `VaultChanged` / unlock,
  one at a time, only unlocked + registered + enabled + paired. Uninstall: `app/src-tauri/windows/hooks.nsh` (`NSIS_HOOK_PREUNINSTALL` unregisters through the alias, then `Remove-AppxPackage`, only for a package registered from this install folder).
  Tests: 37 unit tests with fakes (setup order AppPath < register < alias register, Developer Mode off runs nothing, remove unregisters before removing, debounce, parsers) + two ignored real-OS tests, `extension/e2e/windows-passkeys.ts` (the Settings screen against the throwaway package, 16 checks) and the uninstall hook text run in a tiny NSIS program; the real run of
  setup -> status -> remove against a throwaway package (`LocalPass.PasskeysTest`, feature `test-identity` of `lp-winauth`) left registry, packages, aliases and files exactly as before. A read-only status against the real
  registration found a bug (a binary without a Windows 10 manifest reads `webauthn.dll` as version 6.2.x; only build and revision count now). **Not verified:** an NSIS install/uninstall run (hooks), the Settings screen on a clean PC
  without the dev registration, Developer Mode off on a real PC, the whole flow with a user who has never used the provider scripts.
- [x] **4.6 Tests (agent).** Local relying party (`extension/e2e/rp.ts`) opened in Edge/Chrome with the
  extension's passkey switch off; a test mode that skips the Windows Hello gesture (like `dev_host`
  does) so it can run unattended; a short manual checklist for the user (real Hello gesture,
  webauthn.io).
  **DONE 2026-10-21 (details, expected output, checklist: notes file, "Phase 4.6"); the real-provider run itself is the user's and has NOT been done.**
  `extension/e2e/rp.ts` (assisted RP: resident key, UV required, `none`, ES256, exclude list; SimpleWebAuthn plus UP/UV/BE/BS, counter 0, user handle, AAGUID; `/state`, JSON-lines log),
  `extension/e2e/windows-provider.ts` (ASSISTED runner: `bun run e2e/windows-provider.ts [--all|--cancel|--locked|--app-closed]`; throwaway Edge profile without the extension; watches `/state`, only the new lines of `provider.log` and the app's `prompts.log`, read-only `lp-winauth-provider status` before/after; PASS/FAIL table; cleans up),
  `extension/e2e/provider-analysis.ts` (its pure part) with 39 new `bun test` tests (`test/rp.test.ts` uses a software authenticator, `test/provider-analysis.test.ts`; 56 pass),
  `tools/windows-provider/passkey-sheet.ps1` (closes a leftover "Passkey for localhost" sheet). Self-tested without a person: `--virtual` (CDP virtual authenticator, passes; two deliberately broken variants fail as they should), a hidden-Edge run of the real
  runner with nobody clicking (preflight, time-out row, clean-up), `e2e/passkeys.ts` still passes. **No unattended Hello-skipping mode was built** (finding in the notes: the throwaway test identity would still need a human to turn it on, the real provider must never skip),
  and `--auto-sheet` was dropped. **Still open for the user:** the checklist run on the real PC (a real Windows sheet, Hello, cancel, locked, app closed, webauthn.io); send back the table or `report.json`.
- [x] **4.7 Docs and memory.** DONE 2026-10-21: README section "Windows passkeys", `CLAUDE.md` desktop bullet and Commands lines, notes file sections for 4.1-4.6, memory file updated. Phase 4 is complete (user verified create + sign-in, closed app, locked vault on a real PC; the assisted test `extension/e2e/windows-provider.ts` is the repeatable check).

## Phase 5: hardening, docs, release builds

- [x] **5.1 Property and negative tests** (DONE 2026-10-22, notes section 7) from `docs/phase5-hardening-notes.md` section 3:
  frame reader, JSON protocol, CBOR/authenticator-data/COSE parsers, origin/PSL matching, RP-id
  validation, `webauthn_json`, the Android JSON entry. Plain `cargo test` with `proptest`
  (cargo-fuzz does not run on Windows MSVC).
**Decisions by the user for 5.2 (2026-10-21):** (1) V5: "force fill" (a login saved for another site) and searching all items need the user's Windows Hello (or an in-app confirmation) every time; same-site fill is unchanged. (2) V7: keep "any process may ask, the user approves the 6-digit code" (the dialog names the client kind), no pairing switch. (3) V11/V22: ask first before a save changes an existing login's password or replaces a passkey; sibling subdomains are never overwritten silently. (4) V25: Android passkeys are refused on phones with no screen lock/biometric (message: set one up). Still open (decide in 5.5/5.4, ask the user then): unsigned NSIS vs code-signing certificate, side-loaded extension vs store listing, the README trust-boundary sentence (same-user malware).

- [x] **5.2 Fix the known issues** (DONE 2026-10-22: parts A Rust, B extension, C Android; V7 kept by decision, V22/V23 not changed; see the three status paragraphs below and notes sections 5.5, 5.5b, 5.5c. Open follow-up: on a persistent Android Keystore failure start-up now crashes with the secret kept instead of wiping it: give it a proper error screen.) listed in the notes (verified ones first), each with a test.
  The issues V1-V18 in the notes were found by a research agent reading the code: re-check each
  against the code before fixing. Do the security-relevant ones first: V1 (the extension skips
  the server-proof check on a fresh service worker, `extension/src/background/broker.ts`),
  V4 (forgetting a browser does not end its open session), V5 (`searchItems` and forced
  `fillLogin` reach any item), V15 (backup KDF parameters used before authentication), V8 and V10
  (broker resource limits), then V2 (Android autofill trusts `webDomain` from any app; **fixed 2026-10-21**, see `BrowserTrust.kt` and the README section), V3 (`validate_rp_id` accepts
  `1.10` for `https://192.168.1.10`, `origin.rs`) and V26 (the broker secret is plaintext in
  `chrome.storage.local`, which content scripts can read by default).
  **Part A (Rust: broker, engine, core, app dialog) DONE 2026-10-21; part B (extension TS: V1, V6, V9, V13, V16, V17, V26 plus the follow-ups below) and part C (Android Kotlin: V13, V14, V18, V19, V24, V25) remain, so the box stays open.**
  Fixed with tests (test names, numbers and the protocol notes: notes file, section 5.5): V3, V4, V5, V8, V10, V11, V12, V15, V20, V21; V22 and V7 left as decided.
  New: `Broker::revoke` + `BrokerState::forget_client` (every "forget" path uses it); `Host::confirm_overwrite` (a new required method of `Host`) with the in-app "Replace?" dialog (`UiEvent::ConfirmOverwrite`, `App.tsx`, answered through `broker_respond`);
  `Engine::{plan_save_login, create_login, update_login_password, passkey_would_replace, search_logins, fill_login_anywhere, login_label}`; `KdfParams::validate`; `framing::read_message_timed`.
  What the extension must follow (all backward compatible on the wire): `searchItems` and a forced `fillLogin` now show a Windows Hello prompt on every call, so the popup must search once per explicit action, not per keystroke;
  `saveLogin` and `passkeyCreate` may wait up to 2 minutes for the Replace? dialog and answer `denied` if the user declines; new error codes `tooLarge`, `tooMany`, `unpaired`; a connection idle for 15 minutes is closed.
  Android: `saveLogin` through the JSON entry no longer updates a login saved for another host of the site (it makes a new one). Not verified by me: the dialog on a real desktop (focus, wording), the extension e2e against the new prompts (see the notes).
  **Part B (extension TS) DONE 2026-10-22** (details: notes file, section "5.5b"; part C Android Kotlin may still be open, so the box stays open). Fixed with tests: V1, V6, V9 (extension side), V13 (dialog), V16, V17, V26, plus the part A follow-ups (popup lists all logins once per click, `denied`/`tooMany`/`tooLarge`/`unpaired` handled, reconnect after idle close or a forgotten client shows "Connect again"). 158 `bun test` tests (new: `extension/test/{broker,storage,background,content-passkeys,content-fill,inject,popup,manifest}.test.ts` with the fakes `chrome.ts`, `fakeapp.ts`); e2e `run.ts` (Chrome and Edge) and `passkeys.ts` pass, `run.ts` now also checks that a real content script cannot read `storage.local`. `e2e/app.ts` (new `LP_APP_EXE`, built in a separate target dir) passes except "the browser is not set up yet", which cannot hold on a PC with a real registration (the script saved and restored both HKCU keys); its passkey step needed a change: the real app asks Windows Hello unless the site asks `userVerification: "discouraged"` (rp.ts got that option), so it was not unattended before. Not verified: that the popup really closes when Windows Hello takes the focus (the list is kept two minutes in `storage.session` either way), and an impostor pipe in a real browser (unit tests only).
  **Part C (Android Kotlin + provider op) DONE 2026-10-22** (details, test names and how the no-screen-lock check was run: notes file, section "5.5c"; the box stays open until the remaining checks of 5.2 are confirmed). Fixed: V11 (Android: provider op `passkeyWouldReplace`, "Replace your passkey?" before the biometric prompt), V13 (prompts show the engine-checked relying party id first, page-supplied names are cut and cleaned), V18 (`DeviceSecretFailure`: only a permanent failure deletes the device secret), V19 (FileProvider shares only `Pictures/` of the app's external files), V24 (`UnlockCarry`: a single-use token issued only by the picker's own unlock step), V25 (passkeys refused without a screen lock, warning row in Settings); V14 is not an issue on Android. Emulator `lp_api37`: `android.ts`, `android-save.ts`, `android-setup.ts` and the new `android-nolock.ts` pass; the AVD was restored to PIN 1234 and the enrolled fingerprint (disk files copied back, then `android.ts` run). The debug APK has these changes; the release APK still has to be rebuilt in 5.5. Passkey prompt titles changed ("Create a passkey for <site>", "Sign in to <site> with a passkey"): `android-release.ts` was updated to match but not run.
- [x] **5.3 Security review** (DONE 2026-10-22; `docs/phase5-security-review.md`). Review by area in the order of the notes' checklist,
  report findings, fix confirmed ones.
- [ ] **5.4 Threat model docs.** Update README "Security notes and limits" and `CLAUDE.md`.
- [ ] **5.5 Release builds.** NSIS installer (`bun run tauri build`), signed release APK
  (arm64 + x86_64, see `CLAUDE.md`), extension zip. Smoke test each (installer in a throwaway
  profile, APK on `lp_api37`). Build into a separate target dir if the user's app is running.
- [ ] **5.6 Final verification matrix and report** to the user: what was tested where, what only the
  user can test (real phone, real Windows Hello, real sites such as webauthn.io), known limits
  (Android native apps get no passkeys/autofill, ES256
  only, Chrome-only tested on Android).

- [x] **Optional: saving a new password from Android's autofill prompt.** DONE 2026-10-21. `LpAutofillService` puts a `SaveInfo`
  (username + password, password required, `FLAG_SAVE_ON_ALL_VIEWS_INVISIBLE`) on every response for
  a form with both fields, also a response with only the `SaveInfo` when no login matches (the
  response an unlock or login pick returns replaces the old one, so it repeats it). `onSaveRequest`
  reads the typed values with `AutofillParser.typed` (website from the same place as filling, so the
  unverified-`webDomain` limit V2 applies equally) and calls the existing provider op `saveLogin`
  (`Engine::save_login`: new item titled with the site, password updated in place, or unchanged;
  never a duplicate). Locked vault: the login waits in `PendingSaves` (memory only, random token,
  2 minute life, wiped on use), `SaveCallback.onSuccess(IntentSender)` (API 28+) opens
  `AutofillActivity` (`ACTION_SAVE_LOGIN`), which unlocks with the usual `Unlocker` and then saves;
  backing out saves nothing, below API 28 a toast says nothing was saved. Tests: Rust
  `a_login_saved_from_the_autofill_prompt_is_stored_once_and_updated_in_place`
  (`crates/lp-engine/tests/provider.rs`), emulator `extension/e2e/android-save.ts` (run on `lp_api37`,
  Chrome 145, all checks pass: save, No thanks, changed password updates in place with history,
  locked vault unlocked by fingerprint then saved, by master password then saved, backing out of
  the unlock saves nothing, a single-page login, a search box is not offered; run it with
  `cd extension && bun run e2e/android-save.ts`, `LP_SKIP_INSTALL=1` reuses the installed build).
  Not verified: Android below 28 (toast path), browsers other than Chrome, a real phone.
  Test gotcha: `adb shell input text` loses characters while the autofill dropdown is open, so the
  e2e types through DevTools key events.

- [x] **Android set-up screen, autofill trust (V2) and the signed release APK.** DONE 2026-10-21.
  (1) Settings > "Autofill and passkeys" (Android only): `app/src/screens/AndroidSetup.tsx`, Kotlin
  `LpMobilePlugin.setupStatus/openSetup`, commands `android_setup_status`/`android_open_setup`. Autofill status is real
  (`AutofillManager.hasEnabledAutofillServices`); the button opens Android's chooser (on Android 17 the "Preferred service"
  list, which sets autofill AND the primary credential provider). Whether LocalPass is the passkey provider CANNOT be read
  (`credential_service` throws "is not readable ... system apps only"), so that row shows no status, only "Open settings and
  switch LocalPass on". Local-network permission state is shown (Android 17) with an Allow button (app settings). The status
  re-reads on `visibilitychange`/`focus`. Test: `extension/e2e/android-setup.ts` (debug build).
  (2) V2: `BrowserTrust.kt` (package + signing-cert SHA-256 against `privileged_browsers.json`, pure `parse`/`isTrusted`,
  11 JUnit tests in `app/src/test`) gates `LpAutofillService` fill/authenticate/save; `AutofillActivity.checkedForm` re-checks
  from the `AssistStructure` the system adds to the intent. Manifest `<queries>` makes https browsers visible. Negative run
  (asset temporarily renamed Chrome): no dropdown, no Save sheet, logcat "not a trusted browser: com.android.chrome"; then restored.
  `android.ts` and `android-save.ts` pass (both showed one intermittent timing failure around the locked-vault/biometric steps on
  re-runs; they passed on the next run, not tied to the change).
  (3) Release: `bun run tauri android build --target aarch64 x86_64 --apk --ci` -> `target/android-release/localpass-0.1.0-universal-release.apk`
  (+ `.sha256`); `apksigner` verifies v2, not debuggable, minSdk 24, arm64-v8a + x86_64, both services. `extension/e2e/android-release.ts`
  smoke-tests it on `lp_api37` (clean install, vault, set-up screen, save, fill, passkey create + sign-in). User guide: `docs/android-setup.md`.
  Not verified: a real phone, other browsers, arm64 execution (only x86_64 runs on the emulator).

- [x] **Known issue fixed in 0.1.1 (versionCode 1001): stuck on the QR scanning screen.** DONE 2026-10-21. Reported on a real phone:
  editing a login > "Scan setup QR code" opened the camera, and neither the on-screen Cancel nor the system Back closed it. Cause:
  `tauri-plugin-barcode-scanner` 2.5.2 (`BarcodeScannerPlugin.kt`) `cancel()` runs `destroy()` (clears `savedInvoke`) before
  `savedInvoke?.reject("cancelled")`, so the pending `scan()` promise never settles and `scanQr`'s `finally { setScanning(false) }` never ran
  (the page stayed see-through). Fix: `app/src/lib/scan.ts` `cancelScan()` also resolves a local `cancelled` promise that `scanQr()` races against
  `scan()`. Same helper for `Onboarding.tsx` (join by scanning) and `ItemEditor.tsx`.
  Tests: (1) `cd app && bun test test/scan.test.ts` (8 tests, fake plugin that drops the pending call like the real one; run against the pre-fix
  `scan.ts` the Cancel/Back/reopen tests fail, with the fix all pass; also covers the success path and scan errors). (2) `extension/e2e/android-scan.ts` on
  `lp_api37`: edit a saved login > Scan setup QR code > Cancel, Back, reopen (x2 each) > Back leaves the app as before; and Join > Scan QR code instead > same.
  Debug run (DevTools): 52 checks ok, and it first shows the bug (the plugin's `cancel` alone leaves `html.scanning` on), then that the fixed Cancel
  recovers from that state. Release run (`--release <apk>`, uiautomator only): all ok. `android-release.ts` on the 0.1.1 release APK: all ok.
  0.1.0 -> 0.1.1 `adb install -r` over the release key works (same signer cert, versionCode 1000 -> 1001).
  Release file: `target/android-release/localpass-0.1.1-universal-release.apk` (+ `.sha256`; 0.1.0 files kept): versionName 0.1.1, versionCode 1001,
  minSdk 24, arm64-v8a + x86_64, v2-signed, not debuggable. Versions bumped to 0.1.1 in root `Cargo.toml`, `tauri.conf.json`, `app/package.json`,
  `extension/manifest.json`, `extension/package.json` (NOT the Windows provider's `AppxManifest.xml`, a separate identity).
  **Emulator limits (not a bug in the fix):** the plugin decodes with Google's ML Kit barcode module that Play services must download, and `lp_api37`
  cannot ("Waiting for the barcode module to be downloaded"), so the camera never opens and **a successful scan (a QR code decoded on the phone) was
  not tested on a device**, only in the unit test. CameraX also fails its camera check if no front camera exists (`hw.camera.front=none` while the system
  declares one): start the emulator with `-camera-front emulated` (command line only; the AVD files were not changed). `-camera-back imagefile:<png>`
  would show a QR image to the camera. Lesson: a WebView's accessibility tree (uiautomator) does not follow a scroll (cut-off nodes keep a zero-height box),
  so release-build e2e steps must not depend on scrolling; submit forms with Enter.

## Prompt template for agents

> Goal: ... Context: LocalPass, Rust workspace at C:\app\projects\rust\local-pass; read CLAUDE.md
> and `docs/handoff-phase4-5.md` (rules) first. Read only: <files>. Do: <numbered steps>.
> Constraints: no commits; no secrets; restore any system state you change; keep output small.
> Write results to <file>. Reply in under 200 words: what you did, the file, what failed.

## Gotchas learned so far

- A passkey sign-in on Android: cancelling in LocalPass's prompt fails the request at once with
  `NotAllowedError`; the system sheet title says "Google Password Manager" regardless of provider.
- Chrome on Android only uses a third-party autofill service after Chrome Settings > Autofill
  services > "Autofill using another service" (and a restart).
- `content.js` runs at `document_start`: rescan forms on demand and wait for `DOMContentLoaded`
  before offering a save; the MV3 service worker sleeps, so passkey calls must be allowed to connect.
- Rebuild `cargo build -p lp-broker --examples` after engine changes: a stale `dev_host` silently
  lacks new JSON fields.
- `serde` `rename_all` on a tagged enum renames variants only; use `rename_all_fields` for fields.
- Android autofill datasets use deprecated builders because minSdk is 24; that is intended.
