# Handoff: Phase 6, LocalPass on Fedora Linux + Firefox

The approved plan is `docs/phase6-linux-plan.md` (a copy of the plan file; tasks 6.0-6.7, with the design of each). This
file holds the state; do not rely on conversation history.

## RESUME HERE (state saved 2026-10-28)

Decisions (2026-10-28): Firefox (Fedora RPM) as the browser; "is it you?" check = master password only
(no polkit, no keyring quick unlock); RPM built by GitHub Actions plus a Linux CI job; Firefox
extension signed by Mozilla (unlisted, `web-ext sign --channel unlisted`, the user makes the AMO
account and the repo secrets `AMO_JWT_ISSUER`/`AMO_JWT_SECRET`). The Fedora laptop is also the dev
machine for desktop + extension; Android releases stay in CI, Windows checks happen on the work laptop.

Rules: one task per fresh chat; builds in the background; no agents except the security review; never
create the real vault on this laptop before 6.7 (experiments use `LOCALPASS_PROFILE=dev`, the
device-secret format changes in 6.2); tests never touch the real keyring or `~/.mozilla`; never commit
unless asked.

Shell setup for every command: `export PATH=$HOME/.cargo/bin:$HOME/.bun/bin:$PATH`.

## Next: 6.7 (move the home laptop over, assisted) and the open items below, fresh chat

**6.6d is done (2026-10-30)**: security review in `docs/phase6-security-review.md` (no High/Medium; L1-L4 fixed, see 6.6d entry), README
"Firefox on Fedora Linux" + three Security-notes bullets + install text, and the `firefox` job in `release.yml` (pinned `web-ext` 8.10.0 as an extension
devDependency, `web-ext lint`, `web-ext sign --channel unlisted` with the repo secrets as step-level `WEB_EXT_API_*` env, skipped with a warning when
absent, artifact `localpass-firefox-<version>.xpi`, row in the draft's file table, `publish` needs it). Open items before/with 6.7:
- 6.5 still partly done: the FIRST tag push will be the first run of the `linux` and `firefox` jobs (fix whatever breaks; the XPI must then be installed
  in Firefox and survive a restart; the add-on id is permanent once signed); a real `sudo dnf install`/upgrade/remove run with the user.
- Not run for real: L1's path (Firefox closed, app not running, the extension makes the helper start the app: check the app's log does not reach Firefox
  and the extension connects on the first try).
- L5 fixed 2026-10-30 (see the review); I1 (pin `fedora:44` by digest) is open until the first release job has run.
- Release prep 2026-10-30: the `linux` release job used `rustup show`, but Fedora's `rustup` package has only `rustup-init` (would have failed on the first tag): now `rustup-init --default-toolchain none` + PATH. The Firefox e2e (70 checks, passes locally) is a step of the Linux CI job. Re-verified: fmt, clippy `--all-targets`, 321 Rust tests, extension 228 tests + tsc, `web-ext lint` 0/0/0, `bun run tauri build` RPM. Verified 2026-10-31 with podman on this laptop (clean clone of the commits, `fedora:44`): the whole `linux` release job (dnf packages, `rustup-init`, `bun run tauri build`) produces the RPM; `dnf install` of it in a fresh `fedora:44` works, `rpm -V` and `ldd` are clean, `dnf remove` removes it; `cargo clippy --all-targets -D warnings --target x86_64-pc-windows-gnu` (mingw64-gcc in the container) is clean for the lp crates AND for `localpass` after a fix (`UiEvent::VerifyUser*` were dead code on Windows; `VerifyOutcome`/`verify_password` are now `cfg(all(desktop, not(windows)))` so Android does not warn either). Android could not be compiled (no NDK here). Still unrun: the first tag push (signing, publish), the XPI installed in Firefox, a real-desktop `dnf install` (the container install proves packaging, not the tray/GUI), L1's real path. Nothing committed or tagged. The new Firefox e2e CI step has never run on ubuntu-latest (if its Firefox is a snap, the throwaway-HOME native-messaging manifest may not be honoured; it only risks a red CI, release.yml does not depend on it). The `rustup-init` + `rustup show` recipe was run locally with throwaway CARGO_HOME/RUSTUP_HOME and installs the toolchain and the 4 Android targets. The real-keyring test `secret_service_keeps_one_key_per_profile` and the L5 refusal path have not been run since the change (run `cargo test -p lp-engine --features keyring --lib secret_service_keeps_one_key_per_profile -- --ignored` by hand).

## 6.4 assisted checks (done 2026-10-30, the user reported "all work")

Reported passing: 1 tray (menu, close-to-tray, Quit), 2 browser registration (Set up writes the file, Remove deletes it), 4 clipboard
paste and clear, 5 autostart toggle + single instance. Still open: the Wayland-focus check (needs a broker request while the window is
hidden; do it in 6.6c with the harness), the "Confirm it is you" dialog (same), and the `Gtk-CRITICAL` log line (the user did not paste it).
AMO secrets `AMO_JWT_ISSUER`/`AMO_JWT_SECRET` are set in the repo (2026-10-30); the add-on id `localpass@localpass.local` is CONFIRMED by the user
(2026-10-30; permanent once signed). The user saw no `Gtk-CRITICAL` line, so item 7 is closed (nothing to explain). Original list:

6.4 CODE is done (2026-10-29, see Status). What is left of 6.4 needs the user at the screen:
`LOCALPASS_PROFILE=dev2 bun run tauri dev` (vault password `test123!@#`), then check and write the result into the 6.4 entry:
1. Tray icon visible (AppIndicator extension), menu "Open LocalPass" works, close-to-tray, Quit.
2. Settings > Browser extension: "Set up" writes `~/.mozilla/native-messaging-hosts/app.localpass.host.json` (CAUTION: that is the real
   Firefox folder; the user may prefer to run it with `HOME=<tmp>` or remove it right after), status shows "Set up for Firefox", Remove deletes it.
3. The "Confirm it is you" dialog: no browser exists yet, so trigger it from the `lp-broker` client or wait for 6.6's harness; until then
   the dialog is verified only by types/build. (Idea: a debug-only `LOCALPASS_DEBUG_VERIFY` is NOT worth adding; use the 6.6 e2e.)
4. Clipboard: copy a password from an item, paste into Firefox and a terminal, check it clears after the delay.
5. Autostart toggle in Settings creates `~/.config/autostart/*.desktop` with `--minimized`; second launch focuses the first (single instance).
6. Wayland focus: with the window hidden, what a raised dialog does (probably GNOME's "LocalPass is ready" notification). Record it.
7. The `Gtk-CRITICAL` message in the dev log: find the cause or note it as harmless.

## Status

- [x] 6.0 Fedora dev setup and baseline (2026-10-28). Packages installed by the user (gcc, rustup,
  webkit2gtk4.1-devel, openssl-devel, libxdo-devel, libayatana-appindicator-gtk3-devel, librsvg2-devel,
  patchelf, rpm-build), Rust via rustup-init (toolchain from `rust-toolchain.toml`, cargo 1.99.1), bun 1.4.4.
  Baseline on Fedora 44: `cargo test --workspace --exclude lp-winauth` 295 pass, 0 fail (the Windows
  count is higher because the broker/nmhost tests are Windows only until 6.3); `cargo fmt --check` clean;
  app `bun run build` ok; extension `tsc` clean and 171 tests pass; `cargo clippy --workspace --lib --bins
  --tests -D warnings` clean. **Known failure:** `cargo clippy --all-targets` fails only on
  `crates/lp-broker/examples/dev_host.rs` (panics "needs Windows named pipes"; fixed by 6.3, then switch the
  Linux CI clippy step to `--all-targets`). `LOCALPASS_PROFILE=dev bun run tauri dev` builds (2 min cold)
  and starts (data in `~/.local/share/app.localpass/profiles/dev`, status NoVault); window and tray
  checked by the user: (fill in). Linux job added to `.github/workflows/ci.yml` (ubuntu-latest, apt
  packages; not run yet, uncommitted).
- [x] 6.1 Firefox spike (2026-10-29), Firefox 158.0.1 headless, throwaway profile + HOME (the real
  `~/.mozilla` was verified untouched). Driver and probe kept as the seed of the 6.6 harness:
  `extension/e2e/firefox/spike/{run.ts,probe/}` (`bun run.ts <ext dir> [--real]`; Bun WebSocket to
  Firefox's BiDi endpoint `--remote-debugging-port`, `webExtension.install {type:"path"}`, a local server
  on 127.0.0.1:8089 with a CSP-sandboxed `/frame`; no geckodriver needed). Results:
  - MV3 `background.scripts` + `type: module` with top-level await: works.
  - MAIN-world content script (`world: "MAIN"`) works; the `window.postMessage` bridge between MAIN and
    isolated worlds works both ways with `event.source === window`. The real `inject.js` installed
    `__localpassPasskeys` in the page and in the iframe (real dist + derived manifest). `chrome`/`browser`
    are undefined in the MAIN world.
  - **F1 confirmed:** `IntersectionObserverEntry` has no `isVisible` (entries carry only time, rootBounds,
    boundingClientRect, intersectionRect, isIntersecting, intersectionRatio, target), so the `arm()` gate
    keeps every in-page control inert in Firefox.
  - **F2 confirmed:** `chrome.storage.local.setAccessLevel` is `undefined`, and a content script can read
    and write `storage.local`. `storage.session` exists.
  - F3: `sender.origin` is present ("http://localhost:8089"); for a CSP-sandboxed frame it is the string
    `"null"` while `sender.url` is the real URL and `frameId` is a large number. `originOf()` already turns
    `"null"` into null, so it fails closed (good). `location.origin` inside the sandboxed frame still shows
    the URL origin; `self.origin` is the document origin.
  - **New F7:** `background/index.ts:74` `fromContentScript` only excludes `chrome-extension://` URLs;
    Firefox extension pages are `moz-extension://`. Add it (6.6).
  - `privacy.services.passwordSavingEnabled.get` works (controllable_by_this_extension).
  - `windows.create({type:"popup", url: extension page})` works, `windows.remove` works (headless too).
  - `connectNative` does not throw for a missing host; it returns a port and then `onDisconnect` fires with
    "No such native application app.localpass.host".
  - Content scripts ran with NO `host_permissions` in the manifest for a temporary install. Not verified for
    an AMO-signed install (Firefox grants MV3 host permissions at install since 127; check in 6.5/6.7, and
    add `host_permissions` to the Firefox manifest only if the scripts do not run).
  - `sender.documentLifecycle` is null in Firefox (the extension does not use it).
  - **Gate closed (2026-10-29):** the user chose the F1 design "separate LocalPass window": decisions that the IOv2-gated in-page controls make (choose a login, save/update a login, passkey create and sign-in) open an extension-owned `windows.create({type:"popup"})` page (`src/decide/`) in Firefox; no conditional-mediation menu on Firefox; Chrome keeps its in-page UI.
- [x] 6.2 Linux local protection (keyring), done 2026-10-29. End-to-end checked in the real app with profile `dev2`: vault created, `device.key` is 76 bytes starting `LPL1`, exactly one keyring entry `{application: app.localpass, kind: local-key, profile: dev2}`, no warnings in the log. Not yet checked: restart with a LOCKED keyring (the unlock prompt at start), and `dev2` is a throwaway profile (password `test123!@#`).
  `crates/lp-engine/src/secret.rs`: trait `LocalProtect` (`protect/unprotect(purpose, data)`), `PlatformProtect`
  (DPAPI on Windows, bytes unchanged elsewhere; what `protect_local`/`FileSecretStore::new` use, so tests
  and `lp-cli` never touch a keyring), `KeyedProtector<S: LocalKeySource>` (`"LPL1" || XChaCha20-Poly1305`,
  aad = purpose, key read once and cached; a plain/foreign blob is refused by name, never read as data),
  `SecretServiceKeys` (cargo feature `keyring`, Linux only; crate `secret-service` 5.2 with
  `rt-async-io-crypto-rust`, blocking API on a dedicated `lp-keyring` thread; entry attributes
  `{application: app.localpass, kind: local-key, profile: <LOCALPASS_PROFILE|default>}`, label "LocalPass
  local data key"; locked default collection is unlocked (user prompt); an entry that is not a 32-byte key is
  an error and is never replaced; two racing creators agree on the oldest entry).
  `FileSecretStore::with_protector`. App: `local_protector()` in `lib.rs` (Linux = keyring, else platform),
  `ClientFile::load(path, protector)` / `AppHost::new` / `broker::start` take the protector;
  `app/src-tauri/Cargo.toml` enables `lp-engine/keyring` on Linux only. Tests: 6 new unit tests in
  `secret.rs` (round trip, purpose/key binding, foreign blob + every-byte tamper + truncation refused, key
  asked once, failing source = error not a new key, store uses its protector) and the `#[ignore]` real
  keyring test `secret_service_keeps_one_key_per_profile` (ran by hand OK: same key per profile, other
  profile differs, works from inside a tokio runtime, both throwaway entries deleted, `secret-tool search`
  shows none left). Workspace: 301 tests pass, clippy (lib/bins/tests) clean, fmt clean.
  **Not verified:** the Windows and Android builds of these changes (no mingw gcc here for a Windows
  `cargo check`; `sudo dnf install mingw64-gcc` + `rustup target add x86_64-pc-windows-gnu` would allow
  `cargo clippy -p lp-engine -p lp-broker --lib --target x86_64-pc-windows-gnu`; CI will run it).
  **Profile note:** the `dev` profile made before 6.2 has a plain `device.key`; the new build refuses it
  with "not protected by the system keyring". Use a new profile (`dev2`) or delete
  `~/.local/share/app.localpass/profiles/dev`.
- [x] 6.3 Broker over a Unix socket (2026-10-29). New `crates/lp-broker/src/pipe/unix.rs` (Linux only):
  socket `$XDG_RUNTIME_DIR/localpass/broker-v1[-<profile>].sock` (no `XDG_RUNTIME_DIR` / not absolute = error,
  nothing invented); the folder is created 0700 and refused if it is a link, not ours, or group/other can
  enter; the socket is 0600; the server refuses to start when a socket already answers (500 ms probe, `AddrInUse`),
  replaces a stale socket nothing answers on (only a socket of ours, anything else is never deleted), removes its
  socket on a clean stop, and drops connections whose `SO_PEERCRED` uid is not ours; clients check the folder
  and the server's uid before the handshake. `pipe::Stream` is `NamedPipeClient` on Windows and `UnixStream` on
  Linux; `client::{connect_pipe,pair_pipe}` and `nmhost` (`run`, `connect_or_start`) now compile for
  `any(windows, linux)`; `app_not_running` is `NotFound | ConnectionRefused`. App: `broker::start` listens on
  Linux; `native_host::launch_app` puts the started app in its own process group on unix. `libc` added
  (workspace dep, Linux target of lp-broker) for `geteuid`. Tests: 12 unit tests in `pipe/unix.rs` (path rules,
  folder facts, peer uid, private folder + 0600 socket, wide folder refused, link folder refused, live socket
  refused and left intact, stale socket replaced, a foreign file never deleted, app-not-running cases) and the
  Windows-only integration tests now run on Linux too: `tests/server.rs`
  `the_real_named_pipe_works_and_cannot_be_squatted` and all 3 in `tests/nmhost.rs` (the helper as a real child
  process over a real socket; each test uses a runtime folder of its own, never the user's). `dev_host` example
  works on Linux. Linux CI clippy is now `--all-targets` (the 6.0 known failure is gone). Workspace: 317 tests
  pass, 0 fail; clippy `--all-targets -D warnings` and fmt clean. Not verified: Windows/Android builds (CI).
- [x] 6.4 Linux desktop integration. CODE done 2026-10-29, assisted checks passed 2026-10-30 except the items listed under "6.4 assisted checks". `cargo test --workspace --exclude lp-winauth --no-fail-fast`: 321 pass, 0 fail (317 + 4 new);
  clippy `--workspace --all-targets -D warnings`, fmt and app `bun run build` clean. Not verified: Windows/Android builds (CI).
  - Master-password check: `Engine::verify_password(&str) -> Result<bool>` (no state change; scenario test covers locked/unlocked,
    right/wrong). `AppHost::verify_user` (non-Windows desktop) opens an approval, emits `UiEvent::VerifyUser {request_id, reason}`
    (+ `VerifyUserCancelled`), `show_main`, waits 110 s. `AppHost::verify_password(request_id, pw)` only accepts ids of open verify
    requests (`verifying` map, never a pairing/overwrite id), counts the try BEFORE the Argon2 check, 5 wrong = refused; command
    `broker_verify` (cfg desktop non-Windows) returns `accepted|wrong|refused`; Cancel/backdrop = `broker_respond(id,false)`.
    `broker_respond` goes through `AppHost::respond`, which refuses a YES for an open verify request (only `verify_password`
    can accept one; found by the advisor, otherwise any IPC call could skip the password).
    UI: `app/src/components/VerifyDialog.tsx`, mounted in `App.tsx`. The `OpenApproval` guard now also clears the `verifying` entry.
  - Registration: `native_messaging.rs` is now a thin parent; `native_messaging/windows.rs` (old code, unchanged) and
    `native_messaging/linux.rs` (`~/.mozilla/native-messaging-hosts/app.localpass.host.json`, `allowed_extensions`, atomic write,
    2 unit tests with a throwaway HOME). `BrowserStatus` gained `os` and `firefox`; `BrowserExtension.tsx` words Firefox
    (about:addons > Install Add-on From File) and hides the folder row on Linux (6.5 should show the XPI instead).
    Chrome/Chromium on Linux is NOT registered (optional in the plan; skipped).
  - Helper argv: `lp_broker::nmhost::FIREFOX_EXTENSION_ID = "localpass@localpass.local"` (the gecko id 6.6 must put in the Firefox
    manifest and test against), `is_native_host_invocation` also accepts `[exe, <...app.localpass.host.json>, <id>]`,
    new `is_allowed_caller` (used by `run`) checks the Chrome origin or the Firefox id; unit test for both forms and wrong ids.
  - Clipboard: process-wide `arboard::Clipboard` on Linux (`CLIPBOARD` static, `with_clipboard`), `SetExtLinux::exclude_from_history`.
  - `suggested_name()` falls back to `/etc/hostname` (first label); unit test `pretty_name`.
  - `FIREFOX_EXTENSION_ID` is permanent once 6.5 signs the add-on on AMO: the user confirms `localpass@localpass.local` before the first `web-ext sign`.
  - Also fixed: `crates/lp-core/tests/origin_props.rs` unwrapped `psl::domain_str` for generated hosts that are themselves public suffixes
    (`a.r.appspot.com`), a rare existing flake (the new `origin_props.proptest-regressions` file keeps the case).
  - `native_messaging.rs` re-exports with `self::windows::*` (the `windows` crate name would be ambiguous). Windows build unverified (CI).
  - Autostart / single instance / tray: the tauri plugins and `setup_tray` already compile for Linux; only the assisted check is open.
- [~] 6.5 Packaging and release (2026-10-30). DONE: `app/src-tauri/tauri.linux.conf.json` (`productName: "localpass"` so the package, binary and
  desktop file are all `localpass`; targets rpm; `depends: libayatana-appindicator-gtk3`; `desktopTemplate: linux/localpass.desktop`
  with `Name=LocalPass`, `StartupWMClass=localpass`). `bun run tauri build` (8 min cold) gives `target/release/bundle/rpm/localpass-0.2.0-1.x86_64.rpm`
  (8.8 MiB): files `/usr/bin/localpass`, `/usr/share/applications/localpass.desktop` (desktop-file-validate OK), 3 icons; requires
  `libayatana-appindicator-gtk3`, `libwebkit2gtk-4.1.so.0`, `libgtk-3.so.0`, `libayatana-appindicator3.so.1`; `dnf install --assumeno` resolves.
  Wayland app_id is the binary name `localpass` (no `enable_gtk_app_id`), which matches the desktop file. `release.yml` has a `linux` job
  (`fedora:44` container, dnf deps, same steps as Windows) feeding `publish` (needs + table row); README "Installing and updating" has the
  Fedora paragraph. NOT DONE: (a) the Firefox XPI job (`web-ext lint` + `web-ext sign --channel unlisted`, secrets `AMO_JWT_ISSUER/SECRET`, skipped
  when absent) waits for 6.6's `bun run build --firefox`; (b) an actual `sudo dnf install` / upgrade / remove run (needs sudo, do it with the
  user); (c) the `linux` release job has never run (first tag push), the container needs `rustup` from dnf to honour `rust-toolchain.toml`;
  (d) Settings > Browser extension shows no XPI path on Linux yet (6.6).
- [~] 6.6 Firefox extension port + security review. **6.6a-d done**; **6.6a done 2026-10-30** (Firefox 158 headless, throwaway profile/HOME):
  - `bun run build --firefox` (add `--e2e` for `dist-firefox-e2e`) writes `extension/dist-firefox/`: `manifest-firefox.ts`
    derives the manifest from `manifest.json` (`background.scripts` + `type: module`, `gecko {id, strict_min_version "128.0",
    data_collection_permissions {required: ["none"]}}`, no `key`/`minimum_chrome_version`); the gecko id is read from
    `lp_broker::nmhost::FIREFOX_EXTENSION_ID` in `crates/lp-broker/src/nmhost.rs` by `build.ts` and `test/manifest.test.ts`
    (so they cannot drift). `bunx web-ext@8 lint -s dist-firefox`: 0 errors/notices/warnings (needs the data-collection key). The
    real `dist-firefox` installs in Firefox and its `inject.js` runs in page and iframe. `__FIREFOX__` is a build-time define
    (false in unit tests, `test/setup.ts`). Dirs `dist-firefox/`, `dist-e2e/`, `dist-firefox-e2e/` are git-ignored.
  - Chrome build: `diff -r` of `dist/` before/after changes only `background.js` (dead IndexedDB code behind `if (true)`
    and the F7 line); popup/content/inject/manifest are byte-identical.
  - **F2 (done):** `background/storage.ts` has an `Area` interface (`get/set/remove`); `local()` is the Chrome path
    (`setAccessLevel` gate, fail closed) or, in the Firefox build, `indexedDbArea(indexedDB)` (database `localpass`, store `kv`, in the
    extension origin; never falls back to storage.local). Probe results: a content script's `indexedDB.open("localpass")` creates
    an empty page-origin DB (cannot see the background's); **`chrome.storage.session` is undefined in Firefox content scripts**, so
    the captured password in `pending:<tab>` is not exposed and F2 needs no more than `storage.local`; `return true` +
    async `sendResponse` round-trips; `sender.origin` is `"null"` for a sandboxed frame while `sender.url` is real.
    The popup's passkey switch used to write `chrome.storage.local` directly (it would have split from the worker's copy
    in Firefox): it now uses `local()` too, and the scan test in `test/storage.test.ts` covers `src/{background,popup,shared}`
    (no `chrome.storage.local`/`storage.onChanged` outside `storage.ts`). Tests: `test/storage.test.ts` (IndexedDB area via
    `fake-indexeddb`, a new devDependency; pairing through `BrokerClient` with `__FIREFOX__` on leaves `storage.local` untouched).
    **Only fakes and the hand-written probe exercised this**: the shipped Firefox background has NOT been seen running
    in a real Firefox. BiDi cannot navigate a tab to `moz-extension://` ("not allowed in this context"), so the popup
    cannot be driven that way; `run.ts --ext-page <id>` (real dist-firefox, page `navigator.credentials.get`) ended in
    Firefox's own `NotAllowedError` after 15 s, which is consistent with the fallback path but proves nothing. 6.6c must
    show a real round trip (e.g. a decide window the background opens itself, or a page-visible marker).
  - **F7 (done):** `fromContentScript` compares with `chrome.runtime.getURL("")` (covers `moz-extension://`); **F3 (pinned):**
    a sandboxed frame (`sender.origin "null"`, real `sender.url`) is refused (`invalid`) by `findLogins` and `fill`
    (`test/background.test.ts` "Firefox senders"). The earlier plan said the content script reports `self.origin`; the worker's
    refusal made that unnecessary for fill/passkeys.
  - Extension tests: 183 pass; `tsc` clean. The spike probe (`e2e/firefox/spike/probe`) now also checks storage.session,
    IndexedDB and async replies; `run.ts` needs an ABSOLUTE extension path.
  - **6.6b done 2026-10-30 (the decision window, F1).** `src/background/decide.ts` (`Decisions`), `src/decide/index.ts` +
    `public-firefox/decide.{html,css}` (built only by `--firefox`), `src/shared/shown.ts` (texts shared with the in-page UI).
    Flow (Firefox build only; Chrome refuses every `decide*` message): a content script sends `decide {ref, request}`
    (`passkeyCreate|passkeyGet|saveLogin|chooseLogin`); the background builds an `Asker` from `sender` (origin by `frameOrigin`,
    tab, frame), opens `windows.create({type:"popup", url: decide.html?id=<uuid>})` and answers the content script's pending
    message with the outcome (PasskeyCreated/PasskeyAssertion/LoginFill/SaveOutcome) or a rejection (`fallback` = "use another
    method", `cancelled`, or the last error). The window gets `decisionInfo/Logins/Candidates/Answer` by id and **never the secret**
    (a fill goes to the asking content script). `authorize()` accepts only the window it opened: sender id, URL = `getURL("decide.html")+"?"`
    with its own id, `sender.tab.windowId` = the created window. Answers must fit the decision (`ANSWERS`), one at a time (`busy`), a failed
    answer leaves the window open and "Close" reports that failure. One window per tab (newer replaces older), max 8, window/tab close
    or the page's `cancel` = no (a save window closed = "Not now": pending dropped; Save shows the result 1.8 s then closes). A save is offered
    ONCE per capture on Firefox (`Pending.offered`). Content: passkey create/get call `viaWindow` after the `usable()` check; **conditional
    `get` returns `fallback` at once**; the badge uses `armOpener` (trusted click only, no visibility proof; it merely asks for a window);
    sheets/menus/save bar never appear on Firefox. Known Firefox limits: no "generate a strong password" row, no username-field passkey menu.
  - **Measured in real Firefox 158** (`e2e/firefox/spike/decide.ts`): a trusted badge click opens the window (`moz-extension://<uuid>/decide.html?id=...`),
    the whole path content -> background -> window -> `decisionInfo/Logins` works (this also shows the background, F7 and the IndexedDB area running for
    real: from inside the window, same origin as the background, `indexedDB.databases()` lists `localpass` v1 with store `kv`; note this used the
    E2E build `dist-firefox-e2e`, not the shipped `dist-firefox`), Close closes it. **An event page is suspended after ~30 s idle even with a `runtime.connect` port open** (no native port is open when the app is
    not running): the decision was forgotten after 75 s. Fix: the window pings `decisionInfo` every 10 s (`keepAlive`), re-measured OK at 75 s
    (`WAIT_MS=75000 bun decide.ts <abs dist-firefox-e2e>`). With the app connected the native port should also keep it awake; 6.6c can confirm.
  - **Intended behaviour for the 6.6d review (advisor-driven):** (1) the window disables every acting control (action buttons, passkey and login
    rows, Unlock, Never) for `timing.holdMs` = 600 ms after each view and each window focus/visibility, and focuses nothing that acts, because a page
    chooses when the window opens (script-called passkey, replacement request, a badge click it hid) and a typing user must not land on "Create passkey" or
    a login row; Cancel/Close/Use another method/Not now work at once (`src/decide/timing.ts`, tests set it to 0). (2) the keep-alive ping
    (`timing.keepAliveMs` 10 s) pauses while an answer is in flight and after it worked, otherwise the background's `notFound` (it forgets a decision when it
    finishes) would overwrite "Saved to LocalPass." during the 1.8 s linger; a failed answer resumes pings. (3) `armOpener` has no visibility proof, so a
    page can get a hidden badge clicked and open a window: bounded by one window per tab, 8 total, the hold above, and the window showing only the browser-
    reported site. Not exercised for real yet: the window's X button (`windows.onRemoved`; the probe used Close), Wayland placement/focus of the popup.
  - Tests: 228 pass (`decide.test.ts` 18, `decide-page.test.ts` 14, plus Firefox cases in `content-passkeys`, `ui-guard`, `background`); `tsc` clean;
    `web-ext lint` clean; Chrome `dist/` differs from the 6.6a baseline only in background/content/popup (refactors + dead `__FIREFOX__` branches).
  - **6.6c done 2026-10-30: the Firefox e2e harness.** `extension/e2e/firefox/{harness.ts,run.ts,assisted.ts}`; run `cargo build -p lp-broker --examples`,
    `cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts` (about 90 s; `SLOW=1` adds a 75 s wait in a decision window).
    70 checks pass in headless Firefox 158.0.1 (an earlier 58-check version passed 6 runs in a row incl. one with SLOW; the 70-check version once, rerun before relying on it). Set-up: a throwaway HOME/XDG dirs/profile/runtime dir
    (`mkdtemp` 0700), the NM manifest `<HOME>/.mozilla/native-messaging-hosts/app.localpass.host.json` (`allowed_extensions` = the gecko id, `path` = the
    `nmhost` example), `dev_host` listening on the throwaway `$XDG_RUNTIME_DIR/localpass/broker-v1-e2e<pid>.sock`, Firefox started with the same
    `LOCALPASS_PROFILE`, so the REAL helper chain runs: Firefox -> `nmhost` (argv `[exe, <manifest>, <id>]` accepted by `is_allowed_caller`) -> socket ->
    broker handshake -> pairing. The harness asserts the real `~/.mozilla/native-messaging-hosts` is untouched. BiDi cannot navigate a tab to
    `moz-extension://`: the harness opens the popup by evaluating `chrome.tabs.create({url: getURL("popup.html")})` inside a decision window (an
    extension page; `--remote-allow-system-access`), and keeps that popup tab as a control surface (the passkeys switch). New tabs use
    `referenceContext` of the first tab, because `browsingContext.create` fails ("browsingContext is null") while a popup window has the focus.
    Checked for real (dist-firefox-e2e): a shipped-vs-E2E build diff (only `content.js` differs, only `mode: "closed"` -> `"open"`; so
    pairing/background/decide/popup/inject of the SHIPPED `dist-firefox` are byte-identical to what ran); unpaired = the window says how to connect;
    pairing name "Firefox extension"; a page-made `.click()` on the badge opens nothing; a trusted click opens one window showing the browser-reported site
    and the login without its password; a second click replaces it (one per tab); the 600 ms hold (row and "Create passkey" disabled right after a
    `focus` event, Cancel enabled); fill via a row click; a filled login is not offered for saving; save window: Not now, X, Save ("Saved to LocalPass."
    then closes by itself after ~1.8 s, the password is in the vault), Never for this site; CSP-sandboxed frame (`sender.origin "null"`) = no LocalPass
    window and the browser's own error; passkeys against SimpleWebAuthn (e2e/rp.ts): register, exclude list (`InvalidStateError`), sign in, conditional
    mediation left to Firefox at once, locked vault unlocked in the window, Cancel and X = `NotAllowedError`, wrong RP id = `SecurityError`, security-key
    request opens no window, the popup switch off/on, 9 tabs = at most 8 windows, after 75 s in a window the request still works (keep-alive).
    **Decoys (evidence for the 6.6d review of `armOpener`, 6.6b item 3):** ported `/attack-style` and `/attack-cover` of the Chrome e2e. CSS hiding
    (`opacity/display/visibility/transform ... !important` on `html > div`) does NOT hide the badge, because its host sets `all: initial` and the rest inline with
    `!important`, which beats a page's `!important` rule; a click on it opened a window. The see-through decoy (`pointer-events: none`, opaque white, last
    child of `<html>`) lets a real click reach the badge, so a window opens (Chrome's IOv2 proof would have refused this click). That is the accepted residual
    of Firefox: the window shows `localhost | Fill a login for localhost | Local site / alice | Cancel` only, nothing is filled (username and password stay
    empty) until a row is clicked INSIDE the window. **Frames:** a login form in a same-site frame gets the badge (control), one in a CSP-sandboxed frame
    (origin null) does not; a passkey request from the sandboxed frame is refused by the page-side wrapper itself (`sameSiteAsTop`, it never reaches the
    content script; the wrapper is installed there), opens no window and gets Firefox's own `NotAllowedError`; the background's `sender.origin "null"` refusal
    is pinned only by `test/background.test.ts`. **WebAuthn wrapper shape (plan 6.6, via the RP page, not `passkey-verify`):** the sign-in result is
    `instanceof PublicKeyCredential` and `Credential`, `type "public-key"`, `rawId`/response buffers are `ArrayBuffer`s, the response is an
    `AuthenticatorAssertionResponse`, `toJSON()` has `authenticatorAttachment, clientExtensionResults, id, rawId, response, type`,
    `getClientExtensionResults()` is `{}`, `isConditionalMediationAvailable()` answers `true`; the registration result is an
    `AuthenticatorAttestationResponse` with `getPublicKey()`, `getPublicKeyAlgorithm() = -7`, `getAuthenticatorData()`, `getTransports() = ["internal","hybrid"]`.
    `tools/passkey-verify` (checks the Rust-made fixture, NOT the extension wrapper) on Linux: all checks pass. Unit tests 228, `tsc` and `web-ext lint` clean. **Bugs found and fixed by this task:**
    (1) the pairing prompt in the app said "Chrome extension" for Firefox (`background/broker.ts` `browserName`, now "Firefox extension"); (2) the popup
    said "Windows Hello" on Firefox/Linux (`popup/index.ts` `CONFIRM`: "your master password" in the Firefox build). Remaining Windows-wording:
    `content/passkeys.ts` sheet text (Chrome path only, Windows only). **Assisted part (user, 2026-10-30): passed** (hidden window, dialog opened, master password accepted; details above). Before that, `dev_host` approved every user check and `e2e/firefox/assisted.ts` had only been run as far as its "app not listening" refusal (it leaves a paired "Firefox extension" client in the `dev2` profile: forget it in Settings > Browser extension afterwards); the
    native port keeping the event page awake was not isolated (the window's own ping covers a decision; the 75 s test has the app connected).
    `Gtk-CRITICAL`: closed (the user saw none). **Open:** the Firefox e2e is not in the Linux CI job (ubuntu-latest has Firefox; it needs
    `cargo build -p lp-broker --examples`, `bun run build --firefox --e2e`, `bun run e2e/firefox/run.ts`): add it in 6.6d if the user wants it. Both bug fixes
    also change the Chrome build (`background.js`, `popup.js`); the Chrome unit tests pass but the Windows Chrome e2e was not rerun.
  - **6.6d done 2026-10-30.** Review report `docs/phase6-security-review.md`. Fixed after verifying each against the code: L1 `native_host::launch_app` gives
    the started app null stdin/stdout/stderr (it inherited Firefox's native-messaging pipes); L2 `AppHost::verify_user` opens and marks the master-password check
    under the `verifying` lock (a `broker_respond(id, true)` could win the gap); L3 the passkey reason is `Create a passkey for <site id> ("<name>")`
    (`lp-broker` `create_reason`, also the Windows Hello text; two tests updated); L4 the data directory is chmod 0700 at start (unix, not Android).
    `cargo test -p lp-broker -p localpass` and `cargo clippy -p lp-broker -p localpass --all-targets -D warnings` pass; Windows/Android builds unverified (CI).
    Extension: `web-ext` is now an exact-pinned devDependency (bun blocks its postinstall; nothing needs it).
- [ ] 6.7 Move the home laptop over (assisted)
