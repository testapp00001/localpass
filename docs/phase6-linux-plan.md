# Phase 6: LocalPass on Fedora Linux (home laptop) + Firefox

## Context

The home laptop now runs Fedora 44 (GNOME 50, Wayland). The work laptop stays on Windows 11 and the phone on Android. LocalPass has to run on Fedora as a full peer: it syncs with the phone and the work laptop, fills from Firefox and signs passkeys. This laptop is also the development machine from now on (desktop and extension; Android release builds stay in GitHub Actions, and Windows checks happen on the work laptop).

**User decisions (2026-10-28):**
1. **Browser: Firefox** (Fedora's RPM, 158), so the extension needs a Firefox port.
2. **"Is it you?" check: master password only.** No polkit, no keyring quick unlock. This laptop has no fingerprint reader.
3. **Packaging: an RPM built by GitHub Actions**, plus a Linux CI job.
4. **Firefox install: Mozilla unlisted signing.** The user makes an AMO developer account. Each release XPI is signed with `web-ext sign --channel unlisted` and installed by hand.

## Review: where the code stands on Linux

The core is already portable: lp-core, lp-store, lp-sync, lp-engine, lp-import and the React UI. The engine's platform label is `std::env::consts::OS`, which gives `"linux"`, and `Devices.tsx` already shows a laptop icon for it. The non-Windows fallbacks already hide the Windows Hello, Browser extension and Windows passkeys sections. `lp-winauth` compiles to nothing off Windows. Nobody has built it on this machine yet (no `target/`, and no Rust or bun installed).

Machine facts checked:
- The AppIndicator tray extension is enabled.
- GNOME Keyring serves `org.freedesktop.secrets`.
- The firewalld zone is FedoraWorkstation, which allows ports 1025-65535, so sync port 47821 and mDNS are fine.
- Intel GPU, so the WebKitGTK DMABUF/NVIDIA issue does not apply.
- `/dev/tpmrm0` is only usable by the `tss` group, so TPM is not used.
- No Chrome or Edge is installed.

Gaps, in order of weight:

| # | Gap | Where | Effect today on Linux |
|---|-----|-------|-----------------------|
| G1 | Device secret and `protect_local` | `crates/lp-engine/src/secret.rs` (`#[cfg(not(windows))]` is a passthrough) | Network/Noise keys and paired-client secrets sit in plain files |
| G2 | Broker transport | `crates/lp-broker/src/pipe.rs`, `client.rs`, `nmhost.rs`; `app/src-tauri/src/broker.rs:361` | No listener; the native-messaging helper exits with "only available on Windows" |
| G3 | User check | `AppHost::verify_user` (`broker.rs:280`) returns `false` off Windows | Other-site fills and most passkey operations are always `denied` |
| G4 | Browser registration | `native_messaging.rs` (registry), `commands.rs:683-735` | Settings > Browser extension is hidden |
| G5 | Firefox extension | `extension/` targets Chrome MV3 | See the Firefox section |
| G6 | Clipboard | `clipboard.rs` (`#[cfg(not(windows))]` is a plain `set_text`) | No history exclusion. The X11/XWayland value may vanish when `Clipboard` drops |
| G7 | Packaging/CI | `tauri.conf.json` targets `nsis` only, and CI is Windows only | No Linux artifact, and nothing catches Linux breakage |
| G8 | Small items | `suggested_name()` (no `HOSTNAME` in GUI sessions), Wayland focus rules, tray click events (Linux gives menu only) | Cosmetic or UX |

Firefox findings that need design rather than a copy:
- **F1 (the biggest):** Firefox has no IntersectionObserver v2 `isVisible`. `content/ui.ts` `arm()` (the RA-1 fix) then keeps every in-page control inert: the fill button, account menu, save bar and the passkey dialog's primary button (`ui.ts:391`). That fails closed, which is safe, but passkeys and saving cannot work in Firefox as built.
- **F2:** `storage.local.setAccessLevel` is likely missing in Firefox, which breaks the 5.2 rule "the pairing secret is unreadable by content scripts".
- **F3:** `sender.origin` may be missing. `background/index.ts:63` already falls back to `sender.url`, but sandboxed iframes need a check.
- **F4:** Firefox starts the native host as `[exe, <manifest path>, <add-on id>]`, not `[exe, chrome-extension://...]`. Its manifest uses `allowed_extensions` and lives in `~/.mozilla/native-messaging-hosts/`.
- **F5:** The manifest needs `background.scripts`, because Firefox MV3 has no service worker. The fixed id goes in `browser_specific_settings.gecko.id`. MAIN-world content scripts need Firefox 128 or later.
- **F6:** The e2e harness drives Chrome over CDP. Firefox needs WebDriver BiDi.

## Working rules (same as `docs/handoff-phase4-5.md`)

- One task per fresh chat.
- Builds in the background.
- No agents except a security review.
- Record state in a new `docs/handoff-phase6-linux.md` ("RESUME HERE") and a Linux section in `CLAUDE.md`.
- **Do not create the real vault on this laptop before 6.7.** Experiments use `LOCALPASS_PROFILE=dev`, because the device-secret format changes in 6.2.
- Tests must never touch the user's keyring entries or `~/.mozilla/native-messaging-hosts`. Use profiles, throwaway `HOME`, and throwaway keyring attributes.
- 5.6 (the Windows final matrix) stays open and is done on the work laptop. Phase 6 does not block on it.

## Tasks

### 6.0 Fedora dev setup and baseline (no product code)
- Packages:
  - `sudo dnf install webkit2gtk4.1-devel openssl-devel curl wget file libxdo-devel libayatana-appindicator-gtk3-devel librsvg2-devel`
  - `sudo dnf group install c-development`
  - rustup (the pinned toolchain from `rust-toolchain.toml`) and bun.
- Baseline:
  - `cargo test --workspace`, clippy `-D warnings`
  - `cd app && bun install && bun run build`
  - `cd extension && bun install && bun test`
  - `LOCALPASS_PROFILE=dev bun run tauri dev`: the window renders, the tray shows, and the vault can be created and synced against `lp-cli` on 127.0.0.1.
- Fix only what breaks the Linux build.
- Then add a **Linux job to `.github/workflows/ci.yml`** (fmt, clippy, workspace tests, extension tests, app build) so Windows-side changes cannot break Linux silently.

### 6.1 Firefox spike (risk first; needs only the built extension)
Load `extension/dist` with a hand-made Firefox manifest as a temporary add-on in Fedora's Firefox. Check and record:
- MV3 `background.scripts` with `type: module`
- The MAIN-world `inject.js` and the `postMessage` bridge (`event.source === window` under Xray wrappers)
- IOv2 (expected absent, F1)
- `storage.local.setAccessLevel` (F2) and `sender.origin` (F3)
- `privacy.services.passwordSavingEnabled`
- `windows.create({type: "popup"})`

**Gate:** report to the user and confirm the F1 design below before 6.6.

### 6.2 Linux local protection (G1): `crates/lp-engine/src/secret.rs`
- Introduce a small `LocalProtect` trait with three implementations:
  - `Dpapi` (Windows, the current code)
  - `Keyring` (Linux, new, behind a cargo feature)
  - `Plain` (others, tests, `lp-cli`; the current passthrough)
- Add `FileSecretStore::with_protector(path, protector)`. `FileSecretStore::new` keeps today's platform default, so tests and `lp-cli` are unchanged and never touch the keyring.
- How `Keyring` works:
  - One random 32-byte key is held in the Secret Service, with attributes `{application: app.localpass, kind: local-key, profile: <profile|default>}`.
  - Blob format: `"LPL1" || nonce || XChaCha20-Poly1305(data, aad = purpose)`, using lp-core's `crypto.rs`.
  - It must work from plain threads and from inside the Tauri tokio runtime. Use the `secret-service` blocking API, or run it on a dedicated thread.
  - No Secret Service: fail with a clear message. There is no silent plain fallback in the app.
- App wiring (`app/src-tauri/src/lib.rs:383`, `broker.rs:43/69`): on Linux, pass the Keyring protector to the device-secret store and to the paired-clients file.
- Tests: protector crypto with an in-memory key source (round trip, purpose binding, bad magic, tamper). One `#[ignore]` test against the real Secret Service with a throwaway attribute that deletes its item.
- Check the locked-keyring case (auto-login): the keyring unlock prompt appears at start, and sync starts after it.

### 6.3 Broker over a Unix socket (G2): `crates/lp-broker`
- `pipe.rs` gets a `#[cfg(target_os = "linux")] mod platform`.
- Socket path: `$XDG_RUNTIME_DIR/localpass/broker-v1[-<profile>].sock`. Refuse if `XDG_RUNTIME_DIR` is unset.
- Directory: created 0700, and refused if not owned by us or wider than 0700. Socket: 0600.
- Server:
  - A live server already answering means refuse to start (the `first_pipe_instance` equivalent).
  - A stale file (connect gives `ECONNREFUSED`) is unlinked first.
  - Every accepted connection is dropped unless `peer_cred().uid() == getuid()`.
- Client: refuses a server whose `peer_cred` uid differs. This is the RC-1 pipe-owner check.
- `app_not_running` becomes `NotFound | ConnectionRefused`.
- Add a `pipe::Stream` alias (`NamedPipeClient` / `UnixStream`). `client.rs::connect_pipe/pair_pipe` and `nmhost::platform` become `cfg(any(windows, target_os = "linux"))`. `Broker::serve` and `nmhost::relay` are already transport-generic, so reuse them unchanged.
- Turn on `lp-broker/tests/nmhost.rs` (`#![cfg(windows)]`) and `server.rs:945` for Linux.
- New tests: stale socket replaced, live socket refused, dir permission and owner check, uid check (a pure function).
- `app/src-tauri/src/broker.rs:361` starts the listener on Linux too.
- `native_host.rs::launch_app` on Linux uses `CommandExt::process_group(0)`, so Firefox killing the helper does not kill the app.

### 6.4 Linux desktop integration (G3, G4, G6, G8): `app/src-tauri`
- **Master-password check (G3).**
  - New `Engine::verify_password(&str) -> Result<bool>` (`crates/lp-engine/src/lib.rs`, next to `unlock`). It reuses `derive_kek` + `keys::open_vault_keys` without changing state.
  - On Linux `AppHost::verify_user` emits a new `UiEvent::VerifyUser { request_id, reason }`, calls `show_main`, and waits, reusing the `OpenApproval`/`approvals()` pattern of ConfirmOverwrite. A client that leaves withdraws the dialog.
  - New command `broker_verify(request_id, password)`: Argon2 runs off the UI thread. A wrong password keeps the dialog open with an error; 5 failures or Cancel means `denied`.
  - A dialog in `App.tsx` shows the site (the broker's cleaned `reason`).
  - Three edits per command: the function, `generate_handler!`, and `app/src/lib/api.ts`.
  - Quick unlock stays `kind: "none"` on Linux. The UI already hides it.
- **Native-messaging registration (G4)**: split `native_messaging.rs` into `windows` and `linux` modules.
  - Linux writes `~/.mozilla/native-messaging-hosts/app.localpass.host.json` with `allowed_extensions: [FIREFOX_EXTENSION_ID]` and `path` = the current exe.
  - `is_registered` reads the file and compares `path`. `remove` deletes it.
  - Optional and cheap: the same for `~/.config/{google-chrome,chromium,microsoft-edge}/NativeMessagingHosts/` with `allowed_origins`.
  - `BrowserStatus` gains `firefox`. `BrowserExtension.tsx` shows the Firefox steps (about:addons > Install Add-on From File).
- **Helper invocation (F4)** in `lp_broker::nmhost`:
  - Add the constant `FIREFOX_EXTENSION_ID`.
  - `is_native_host_invocation` and the origin check also accept `[exe, <…/app.localpass.host.json>, FIREFOX_EXTENSION_ID]`.
  - Unit tests cover both forms and a wrong id.
- **Clipboard (G6):**
  - Use `arboard::SetExtLinux::exclude_from_history()`.
  - Keep one `Clipboard` alive for the process on Linux, so the value survives on X11/XWayland.
  - GNOME has no data-control protocol, so the XWayland path is the real one.
  - Test paste into Firefox and a terminal, and the clear-after timer.
- **Small items (G8):**
  - `suggested_name()` falls back to `/etc/hostname`.
  - The UI wording for Linux.
  - Check autostart (`~/.config/autostart`, `--minimized`), single instance (D-Bus), close-to-tray with the AppIndicator extension, and that the tray menu "Open LocalPass" works. Linux sends no tray click events.
- **Wayland focus check (assisted, short):** a browser request while the window is hidden (unlock, verify, pairing, Replace?) will probably give GNOME's "LocalPass is ready" notification instead of raising the window. Record what happens. Keep it unless it is unusable: no always-on-top hacks. `focus.rs` stays Windows only.

### 6.5 Packaging and release (G7)
- New `app/src-tauri/tauri.linux.conf.json`:
  - `bundle.targets: ["rpm"]`
  - `bundle.linux.rpm.depends` including `libayatana-appindicator-gtk3` (loaded at runtime for the tray)
  - No Windows resources.
- Check `rpm -qpR` and `rpm -qpl`: binary `/usr/bin/localpass`, a `.desktop` file, icons.
- Install and upgrade: `sudo dnf install ./LocalPass-X.Y.Z-1.x86_64.rpm` upgrades in place, and per-user files are untouched. `dnf remove` leaves `~/.local/share/app.localpass`, the native-messaging JSON and the autostart entry. Document this, plus "Remove in Settings first".
- `.github/workflows/release.yml`: a `linux` job in a `fedora:44` container builds the RPM. A Firefox job runs `web-ext lint` and then `web-ext sign --channel unlisted` with the repository secrets `AMO_JWT_ISSUER/AMO_JWT_SECRET` (skipped when the secrets are absent). Both artifacts go into `SHA256SUMS.txt` and attestations. The user sets up the AMO account and secrets; that is outward-facing.
- `scripts/release.ts`: no new version file (the Firefox manifest is generated from `extension/manifest.json`).

### 6.6 Firefox extension port (G5, F1-F6): `extension/`
- **Build:**
  - `bun run build --firefox` writes `dist-firefox/` with a manifest generated from `manifest.json`: `background.scripts`, `browser_specific_settings.gecko {id, strict_min_version: "128.0"}`, and no `key` or `minimum_chrome_version`.
  - A unit test ties the gecko id to `lp_broker::nmhost::FIREFOX_EXTENSION_ID`, like the Chrome `key`/`EXTENSION_ID` tie.
  - `minify` stays off, so AMO needs no source upload.
- **F1, the decision surface (recommended, confirmed at the 6.1 gate):** on Firefox, every decision that an IOv2-gated in-page control makes opens an extension-owned window instead. That window is a new `src/decide/` page opened with `windows.create({type: "popup"})`. Page CSS and script cannot cover it, and the clicks are real.
  - Covers: passkey create and sign-in (account choice), save or update a login, and choosing a login from the in-page icon.
  - The toolbar popup fill already works.
  - Conditional mediation (the username-field menu) is not offered on Firefox: the page's conditional request is left to the browser.
  - Chrome keeps today's in-page UI unchanged.
- **F2:** on Firefox, keep the pairing secret in the background page's IndexedDB (extension origin; content scripts cannot reach it) instead of `storage.local`. Update `background/storage.ts`, `background/broker.ts` and `test/storage.test.ts` accordingly.
- **F3:** the content script (isolated, trustworthy) reports an opaque `self.origin`, and the worker refuses fill and passkeys for sandboxed frames.
- **WebAuthn wrapper:** check that the `inject.ts` objects behave in Firefox (`PublicKeyCredential` prototype, `toJSON`, `getClientExtensionResults`, `isConditionalMediationAvailable`), using `tools/passkey-verify` (SimpleWebAuthn).
- **E2E (F6):** new `extension/e2e/firefox/` harness over WebDriver BiDi:
  - Firefox with `--remote-debugging-port`, a throwaway `-profile`, and `HOME=<tmp>` so the test host manifest never touches the real `~/.mozilla`.
  - The extension is installed with `webExtension.install` (temporary) against the `dev_host` example on its own `LOCALPASS_PROFILE` socket.
  - Port the scenarios of `e2e/run.ts` (fill, save, decoys) and `e2e/passkeys.ts` (register, sign in, locked vault means the master-password dialog, wrong site, fallback).
- **Security review** (one reviewer, like 5.3) of the Linux broker, the protector, the verify dialog and the Firefox port. Fix the Medium findings and add README "Security notes and limits" bullets:
  - Linux: the keyring is readable by same-user programs (out of scope, decision 3).
  - Master-password checks.
  - No OS passkey provider on Linux.
  - The Wayland focus behaviour.
  - No in-page controls in Firefox.

### 6.7 Move the home laptop over (assisted, on the real devices)
- Install the release RPM and the signed XPI.
- In the onboarding, choose Join and pair with the phone over the home LAN (code or QR).
- Revoke the old Windows home-laptop device in Devices (on the phone or the work laptop).
- Check sync home↔phone↔work: the phone relays an edit made on Fedora to the work laptop.
- In Firefox:
  - Connect and pair (6-digit code).
  - Fill and save on a real site.
  - Passkey on webauthn.io.
  - Locked vault and an other-site fill each give the master-password dialog.
- Autostart at login, close to tray, quit.
- Write the Linux part of the final report (alongside 5.6).

## Critical files

- `crates/lp-engine/src/secret.rs`, `crates/lp-engine/src/lib.rs`
- `crates/lp-broker/src/{pipe,client,nmhost}.rs`, `crates/lp-broker/Cargo.toml`, `crates/lp-broker/tests/{nmhost,server}.rs`
- `app/src-tauri/src/{lib,broker,commands,native_messaging,native_host,clipboard}.rs`, `app/src-tauri/Cargo.toml`, new `app/src-tauri/tauri.linux.conf.json`
- `app/src/App.tsx`, `app/src/lib/api.ts`, `app/src/screens/BrowserExtension.tsx`
- `extension/build.ts`, `extension/manifest.json` (source), `extension/src/background/{index,storage,broker}.ts`, `extension/src/content/{ui,index}.ts`, new `extension/src/decide/`, new `extension/e2e/firefox/`
- `.github/workflows/{ci,release}.yml`, `README.md`, `CLAUDE.md`, new `docs/handoff-phase6-linux.md`

## Verification

- **Every task:** `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` on Fedora.
  - The Linux CI job is green, and the Windows CI job stays green (shared code changes in 6.2, 6.3, 6.4).
  - After changes to shared Rust, check Android with `cargo clippy -p localpass --target aarch64-linux-android` (CI, or locally if the NDK is set up).
- **6.2:** protector unit tests, plus the ignored keyring test run once by hand. Confirm with `secret-tool search application app.localpass` that only the expected item exists.
- **6.3:** `cargo test -p lp-broker` (whole crate, so the `nmhost` example is built), including the newly enabled nmhost tests over the socket.
- **6.4:** with `LOCALPASS_PROFILE=dev bun run tauri dev`:
  - The verify dialog shows and passes or denies with the `dev_host`/`prompt_probe` examples.
  - The Firefox registration file is written and removed in a throwaway `HOME`.
  - Clipboard paste and clear work.
  - Autostart, tray and single instance behave.
- **6.5:** a CI run on a test tag (draft release) produces an RPM that installs into a fresh `fedora:44` container (`dnf install ./*.rpm`, `rpm -V`) and starts. The signed XPI installs into Fedora's Firefox and survives a restart.
- **6.6:** `cd extension && bunx tsc --noEmit && bun test`, the new Firefox BiDi e2e (fill, save, decoys, passkeys) and `tools/passkey-verify`. The Chrome build is unchanged (`bun run build` output diff, plus the Chrome unit tests).
- **6.7:** the assisted checklist above on the real phone, the work laptop and this laptop.
