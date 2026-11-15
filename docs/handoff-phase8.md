# Handoff: Phase 8, one protected panel in every browser, Chromium on Linux, smoother unlock

The approved plan is `docs/plans/2026-11-12-phase8-browser-experience.md` (tasks 8.0-8.6, with the design of each). This file holds the
state; do not rely on conversation history. All plans: `docs/plans/README.md`.

## RESUME HERE (state updated 2026-11-15)

Phase 8 started on branch `phase7` at `c682e84`. Its implementation and this handoff are recorded together in the local Phase 8
commit on `phase7`, requested by the user on 2026-11-15. The user then pushed it and requested merging into `main` (PR #1).
PR #1 merged successfully as `bf57ddd`; the user then requested release 0.3.0, published from `34b7788` on 2026-11-15.
`.cargo/config.toml` is unchanged; never commit, edit or revert the owner's linker settings unasked. Phase 7's release task 7.7 is
complete; its release verification is recorded in `docs/handoff-phase7.md`.

**Where it stands and what to do next**

State on 2026-11-15 (earlier implementation and review details remain under "Status"):
- 8.0 done.
- 8.1, 8.2, 8.3a, 8.3b, 8.3c: coded by agents and checked by the lead (code read, checks re-run; 8.3b and 8.3c also
  mutation-checked). The user accepted the local walkthrough on 2026-11-15; Windows checks remain open.
- 8.4: done on Linux; its assisted check (the Unlock round trip from both browsers with the app in the tray) passed in Firefox and
  Chrome on 2026-11-13. Its Windows paths compiled in merge CI on 2026-11-15; the Android release built for ARM64 and x86_64 in release CI.
- 8.5 (the signed `.xpi` in the RPM): proved by the v0.3.0 release. The signed add-on was downloaded in the Fedora container and
  packaged through the added config; the RPM's XPI is byte-identical to the standalone signed download. Open in the installed app
  has not been tried.
- 8.6: done, including the user's acceptance of the real-screen walkthrough on 2026-11-15:
  - the app walk-through by code and its small fixes, then the fixes of its items 1, 5 and 7 (checked by the lead);
  - the independent review, `docs/phase8-security-review.md`: no High or Medium finding; F1, F2, F4 fixed and checked by the lead on
    2026-11-14 (full check set re-run, mutations run), F3, F5 accepted;
  - the Firefox e2e moved to trusted input after F1, and its old flake found and fixed (a badge click lost in Firefox, `showBadge`);
    both e2e suites passed twice in a row on 2026-11-14;
  - docs: README, `CLAUDE.md`, the plan's "Changes (2026-11-14)", the plan index (2026-11-14).
- `extension/dist` and `extension/dist-firefox` are the normal builds of the current tree (the lead rebuilt them last, 2026-11-14).

Next steps, in this order:
1. **The user's decisions**, list (b) below, remain undecided; acceptance of the walkthrough did not approve these changes.
2. Complete the Windows/Android and release verification in list (c) when the required machine or release is available.
3. PR #1 is merged and v0.3.0 is published. Phase 8 remains in progress only for its remaining manual checks; 8.6 is closed.
   The five usability follow-ups remain undecided. Another release is only made when requested.

**Acceptance and checks (2026-11-15)**
- The user explicitly confirmed Chrome pairing, saving a new login, filling it, and no duplicate offer after autofill or manual
  submission by click and Enter. After receiving the full remaining walkthrough, the user reported everything worked and requested
  a commit. This is overall acceptance of the checks they performed, not an itemized claim that every optional case was run.
- Fresh automated checks: `cargo test --offline --locked --workspace --exclude lp-winauth --no-fail-fast`: 381 passed, 0 failed,
  2 ignored; extension `bun test`: 443 passed, 0 failed; app `bun test`: 21 passed, 9 skipped, 0 failed; both TypeScript checks and
  `cargo fmt --all -- --check` passed. Socket-dependent tests first failed under sandbox restrictions, then passed with socket access.
- Browser e2e and clippy were not rerun in the initial local acceptance checks; subsequent merge CI and release results are below.
  Android compilation and signed Firefox RPM packaging are now verified by release 0.3.0. Decisions in list (b) remain open.
- During the requested merge to `main` (PR #1), the first Linux CI run passed Clippy but its three helper-process tests failed:
  Cargo 1.100 runs the test executable from a nested `build/<crate>/<hash>/out/` folder, so the old two-parent lookup missed the
  `nmhost` example. The test now searches ancestor folders for the built example, and both CI jobs build examples before tests.
  Reproduced locally by relocating the old test executable (failed), then reran the corrected broker suite (109 passed) and all
  three tests from the nested layout (3 passed); formatting and diff checks passed. No production behavior changed.
- The cancelled Windows web job also exposed shared-global test leakage: `content-fill.test.ts` installs a passkey bridge, then
  `content-passkeys.test.ts` installs another on the same window, producing duplicate decisions and leaking keep-alive timers into
  later files. Running those files in that order reproduced the same four failures locally. `extension/package.json` now runs
  `bun test --isolate`, and CI uses `bun run test`; all 443 extension tests passed with per-file isolation. No assertion was removed
  or weakened. Use `bun run test` for future full-suite checks.
- The Windows web job remained stalled after per-file isolation. Timer-driven deadline assertions now await a tagged settlement
  before applying the same synchronous checks, keeping successful values distinct from rejections and avoiding a pending async
  matcher. All 103 decision tests and extension TypeScript checking passed locally. The extension CI step also has a five-minute
  limit so any remaining runner stall yields a completed log instead of waiting indefinitely. The cancelled isolated Windows run
  had no assertion failures and stopped at the deadline test, confirming that the earlier duplicate-listener failures were gone.
  The updated assertions still need confirmation on Windows in PR #1.

- Merge CI on `d2e8784`: Linux completed successfully, including Clippy, the workspace tests and Firefox e2e; Windows Clippy,
  example builds and workspace tests passed. The final Windows provider cancellation test exposed the same two-parent helper lookup
  in `lp-winauth/tests/pipe.rs`; it now finds `dev_host.exe` from either Cargo layout. The actual lookup function compiled and found
  fixtures in legacy and nested paths on Linux; the Windows integration run remains CI's responsibility. No provider test was skipped.

**Release 0.3.0 (2026-11-15)**
- Published: https://github.com/testapp00001/localpass/releases/tag/v0.3.0 (source `34b7788`, annotated tag `v0.3.0`).
- Release workflow `43704939727` passed all jobs; CI `43704939634` also passed for the release commit. Windows installer, Fedora RPM,
  signed Android APK (ARM64 and x86_64), Chromium ZIP, signed Firefox XPI and checksums are available.
- All five download hashes matched `SHA256SUMS.txt`. All six attestations verified against the release workflow, tag and source
  digest. Android's APK v2 signature uses the pinned release certificate; both app ABIs are present.
- 8.5's signed path is proved: the RPM metadata is `localpass` / `0.3.0` / `x86_64`, and
  `/usr/lib/localpass/firefox/localpass-firefox.xpi` is byte-identical to the standalone signed XPI. The unsigned workflow branch was
  not exercised by this signed release. Installed-app Open, Windows browser checks and platform runtime smoke checks were not run
  on this Linux host; no installed app or real vault was started.

**For the user: one list**

(a) Assisted checklist reference (user acceptance above; individual results were not itemized). Builds: the app from
`LOCALPASS_PROFILE=dev bun run tauri dev`; Firefox: about:debugging, This
Firefox, Load Temporary Add-on, `extension/dist-firefox/manifest.json`; Chrome: chrome://extensions, Developer mode, Load unpacked,
`extension/dist` (or **Reload** if it is loaded already). Both builds must be the normal ones (not `--e2e`).
- 8.1 (dev app + Chrome, `extension/dist`): Settings → Browser extension → Set up, load the extension, pair, fill, save.
- 8.2 (either browser): sign in with autofill on a one-step and a two-step site (no offer); type an existing login and send it with
  a click or Enter (no offer); type a changed password (update offer).
- 8.3 (Firefox `dist-firefox` and Chrome `dist`): pick, save and a passkey with the vault locked and unlocked; does a real Esc in the
  panel dismiss a save offer or passkey request (or does the browser close the panel first and leave it waiting); does a real click
  outside leave a save offer waiting with the badge; does the panel reopen by itself after the real app's Unlock; (Chrome) does
  clicking the badge while an earlier panel is still closing make `openPopup` fail so that the pick ends silently (seen in the lead's
  review, not measured).
- 8.3c keyboard (both builds): `Alt+Shift+L` opens the popup (or the waiting decision's panel); in the popup the first Fill for the
  page has the focus and Enter fills it; in a panel Up/Down move between the rows and Enter picks; a panel or popup the page caused
  holds 0.6 s (nothing greyed); one the user opened with the icon or the shortcut acts at once; a login for another site asks
  "Fill anyway" inline in the popup.
- New since F2 (both builds): a request the page left waiting is held on its first showing even from the toolbar icon. To try it:
  start a passkey request (or a save) in a tab that is not in front (for example a second window), then click the LocalPass icon in
  that tab: the panel's buttons wait 0.6 s; close it and click the icon again: now they act at once.
- New, the badge fix (Firefox `dist-firefox`): with the browser window not focused (click another window first), click the badge in
  a login field once: the panel opens on that one click.
- New, app (dev app): deleting a folder asks first and names the number of items; the folder's rename and delete buttons are reached
  with Tab; Esc with stacked dialogs (for example "Confirm it is you" over a pairing request) closes only the top one; Tab wraps
  inside the top dialog; "Browser connected" appears only when the pairing really succeeded (approve one that has already ended: an
  error toast instead); busy buttons (Add device, Move to trash/Restore/Delete forever, folder Save, Empty trash) cannot be pressed
  twice.
- 8.4 (optional): Firefox case E (the Unlock round trip with the app open behind Firefox) with the default `remap`.

(b) Decisions only the user can make (from the walk-through's list in the 8.6 entry; nothing changed yet):
- Items of a deleted folder keep its id (`app/src/screens/ItemEditor.tsx:245`, `Engine::delete_folder`): the select shows "No
  folder" but a save keeps the stale id. Clear unknown ids on load, or have the engine treat them as none?
- Folders exist only in the wide sidebar (`app/src/screens/Shell.tsx:155`, `hidden md:flex`): a narrow window or the phone cannot
  open or manage folders. A folder picker in the mobile nav or the Vault header?
- "Keep running in the tray" on GNOME without the AppIndicator extension hides the window with no visible icon
  (`app/src/screens/Settings.tsx:165-170`; starting LocalPass again shows it). A Linux hint, or a check?
- Disconnect a browser acts at once (`app/src/screens/BrowserExtension.tsx:164`; the browser must pair again). Ask first or not?
- Known small gap in the dialog Tab wrap (`app/src/components/ui.tsx`, `FOCUSABLE`): hidden controls (`display: none`) are listed,
  so when a dialog's last listed control is hidden, Tab from the last visible one leaves the dialog. Rare. Fix now or leave?
(The line numbers are those of the walk-through, before the small fixes; look the code up again before acting.)

(c) Needs another machine or a release:
- Windows compilation/workspace tests and Android ARM64/x86_64 release builds passed. These build checks are complete; they do
  not claim platform runtime coverage beyond the tests that actually ran.
- Windows checks of the panel in Chrome and Edge (focus rule, shortcut assignment, `openPopup`-to-attach order, `e2e/app.ts`), on
  the Windows work laptop.
- **Open** in the installed app (Nautilus selecting the signed add-on) still needs an assisted check. The first signed release
  path for 8.5 passed; the unsigned workflow branch was not exercised by this release.
- Release 7.7 / 0.3.0 is complete. Windows installer and Android runtime smoke checks were not rerun on this Linux host.

Rules:
- One task per session, in plan order.
- **Coding is done by subagents**, one task per agent, briefed with the plan, the files and the hard rules. **Since 2026-11-13 (user
  instruction): Opus at medium effort, at most two agents at once** (before: Opus at high effort or Sonnet at xhigh, one at a time); **the main session plans, briefs and checks every result against the code before the next task starts** (user instruction of
  2026-11-12, which replaces the earlier "main session codes, agents only for an independent look" rule; see `CLAUDE.md`, "Working with agents").
  A review agent for an independent look is still briefed without the main session's conclusions and is read-only.
- Builds and e2e run in the background. Never commit or push unless the user asks. Experiments use `LOCALPASS_PROFILE=dev` (or `dev2`).
  **Never start the installed `localpass` binary, even for `--version`**: it is the real app and opens the real profile.
- Tests never touch the real keyring, `~/.mozilla` or `~/.config/<browser>`: they use a throwaway HOME, profile and runtime folder.
- Shell setup for every command: `export PATH=$HOME/.cargo/bin:$HOME/.bun/bin:$PATH`.
- Keep the plan log current: when a task or phase finishes, update its status in `docs/plans/README.md` and here. An approved plan file is never
  rewritten; append a dated "Changes" section instead.

### How a session works (lead and agents)

- The main session is the lead: it reads this file, writes one brief per task, starts agents as **Opus at medium effort, at most two at
  once** (user instruction of 2026-11-13; before: one coding agent at a time, Opus at high effort or Sonnet at xhigh), and when the agent reports it reads the changed code and re-runs the checks itself before the next task starts. It does
  not write code or project docs itself (user instruction of 2026-11-12).
- Every brief carries these hard rules: the PATH line; never commit, push, tag or stash, and build on the uncommitted work in the tree; do
  not edit `.cargo/config.toml` and use the cargo environment override from "Machine facts", keeping it set for e2e runs; never start the
  installed `localpass` or the GUI app; tests never touch the real keyring, `~/.mozilla`, `~/.config/mozilla` or `~/.config/<browser>`; no
  further agents; only the named task; an approved plan file is never edited; a final report that lists files changed, every check with
  its real result, decisions taken, what a page can now do that it could not before (for extension work), and what was not done or not
  verified.
- A brief names the goal and the rules that must not weaken, points at the files, asks the agent to measure browser behaviour before
  designing on it, and asks for mutation checks of new guards. It does not paste the lead's conclusions as facts the agent must not
  question.
- **Usage limits:** the 5-hour limit cut agents off twice (8.3a after reading, 8.3b before its final run). An interrupted agent can be
  continued only from the chat that started it. From a new chat, start a fresh agent and give it the handoff entry and the working tree as
  its starting state. Prefer smaller tasks (one of a, b, c at a time), and have the agent write its handoff entry as it goes, not at the
  end.
- **Timeouts (lesson of 2026-11-13/14):** an agent waited about 10 hours for the notice of a background check run that had already
  ended, and had to be stopped before it reported. Every long command in an agent brief gets a `timeout` (builds, e2e, the full check
  set), and when a background notice does not come in the expected time the lead checks `ps` for the run instead of waiting.
- The lead's checks so far used one background command with the cargo environment override: `cargo fmt --all -- --check`, clippy,
  `cargo test --workspace --exclude lp-winauth --no-fail-fast`, `cargo build -p lp-broker --examples`, then in `extension/`:
  `bunx tsc --noEmit`, `bun test`, `bun run build --e2e && bun run e2e/run.ts && bun run e2e/passkeys.ts`,
  `bun run build --firefox --e2e && bun run e2e/firefox/run.ts`, `bun run build`, `bun run build --firefox`,
  `bunx web-ext@8 lint -s dist-firefox`, and `cd app && bunx tsc --noEmit`. The two `--e2e` builds are for tests only; always finish with
  the two normal builds so `dist/` and `dist-firefox/` are the ones a person loads.

User decisions (2026-11-12, also in the plan):
- The login picker is the **toolbar panel in every browser** (a window near the field cannot be placed on Wayland).
- **Chrome loses every in-page control that acts** (menu, save bar, passkey dialogs); only the badge that opens the panel stays in a page.
- A save is offered **only for a new or changed login**.

Machine facts found 2026-11-12 (Fedora laptop, GNOME on Wayland):
- Firefox 160 (RPM) keeps its profiles in `~/.config/mozilla/firefox` (XDG); `~/.mozilla` holds only `extensions/` and `native-messaging-hosts/`.
  Neither profile lists a LocalPass add-on (the user loads `extension/dist-firefox` as a temporary add-on).
- `~/.mozilla/native-messaging-hosts/app.localpass.host.json` names `/home/nimda/projects/localpass/target/debug/localpass`, not
  `/usr/bin/localpass` (RPM `localpass-0.2.1-1` is installed). Left as it is.
- Google Chrome 157 (RPM) is installed and `~/.config/google-chrome` exists. Its `NativeMessagingHosts` folder exists but is **empty**; it was
  created at 21:49:35, the time of Chrome's first start (the profile folder is from the same minute), before any test of this phase ran.
- **`clang` and `mold` are not installed**, so with the owner's `.cargo/config.toml` lines every Rust link step on this laptop fails
  (`error: linker 'clang' not found`, already in build scripts, so `cargo check/clippy/test/build` all fail). Install them
  (`sudo dnf install clang mold`) or drop the lines before Rust can be built here. The `dev_host`/`nmhost` examples in `target/debug/examples`
  were built at 21:29, before the lines were added, and are current for 8.1 (8.1 changed only a test in `lp-broker`).
- Until then cargo works on this laptop with an override of those lines (checked by the lead on 2026-11-12), with its own warm target folder:
  `export CARGO_TARGET_DIR=/home/nimda/projects/localpass/target/phase8-check CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc RUSTFLAGS=""`.
  The e2e harnesses (`extension/e2e/harness.ts`, `extension/e2e/firefox/harness.ts`) take `dev_host`/`nmhost` from `$CARGO_TARGET_DIR/debug/examples`
  when that variable is set (since 8.2), else from `target/debug/examples`.
- On 2026-11-13 at 08:44 the owner's two `.cargo/config.toml` lines (clang + mold) were found removed from the working tree: the file
  equals the last commit again. Who removed them is not known; `clang` and `mold` are still not installed. The cargo override above
  keeps working either way. `.cargo/config.toml` was not touched.

## Status

- [x] 8.0 Plan log (2026-11-12, docs only): plan copy with status line (made before this session), index row in `docs/plans/README.md`, this
  file, `CLAUDE.md` ("Current work" pointer; "Working with agents" rewritten to the new rule), a dated note under the agent rule in
  `docs/handoff-phase7.md` (the old text kept).
- [ ] 8.1 Chromium browsers on Linux (2026-11-12, coded; Rust compiled and tested by the lead the same day; **see "Open"**).
  - `app/src-tauri/src/native_messaging/linux.rs`: a browser table (`BROWSERS`: Firefox, Google Chrome `google-chrome`, Chromium `chromium`,
    Brave `BraveSoftware/Brave-Browser`, Microsoft Edge `microsoft-edge`, Vivaldi `vivaldi`) and a `Places { home, config }` (`config` =
    `$XDG_CONFIG_HOME` when absolute, else `~/.config`; only `Places::from_env` reads the environment). Firefox keeps
    `~/.mozilla/native-messaging-hosts` with `allowed_extensions` and is always written (as before); a Chromium browser gets
    `<config>/<folder>/NativeMessagingHosts/app.localpass.host.json` with `allowed_origins: [nmhost::allowed_origin()]`, only when
    `<config>/<folder>` exists (nothing is created for an absent browser). Write to `.part`, then rename. `remove` deletes all six files.
    `status` gives per browser `present` (Chromium: config folder exists; Firefox: `~/.mozilla/firefox` or `<config>/mozilla/firefox` exists, the
    two profile roots, since `~/.mozilla` alone may only hold our own folder), `registered` (manifest `path` equals this exe) and `other_copy`
    (manifest names another exe). 8 unit tests (both manifest kinds, XDG rule, only present browsers get a file and no folder is made, status per
    browser, Firefox legacy root, another copy and "use this copy", remove).
  - **Firefox with an XDG profile reads only `~/.mozilla/native-messaging-hosts`** (per-user) and the system folder: `toolkit/xre/nsXREDirProvider.cpp`,
    `XRE_USER_NATIVE_MANIFESTS` ("Keep forcing the legacy path for compatibility", `GetUserDataDirectoryHome(.., aForceLegacy = true)` then
    `.mozilla`; with `aForceLegacy` `GetLegacyOrXDGHomePath` returns `$HOME` even when `~/.mozilla` does not exist), and
    `toolkit/components/extensions/NativeManifests.sys.mjs` (`XREUserNativeManifests`, `XRESysNativeManifests`, nothing else). Read from the
    `mozilla-firefox/firefox` GitHub mirror, `main`, last change to that file 2026-10-08. So no second Firefox location is written.
  - Chrome on Linux: `chrome/common/chrome_paths.cc`, `DIR_USER_NATIVE_MESSAGING` = `DIR_USER_DATA` + `NativeMessagingHosts`;
    `chrome_paths_linux.cc`: the default user data dir is `$CHROME_CONFIG_HOME`, else `$XDG_CONFIG_HOME`, else `~/.config`, plus the browser
    folder. `$CHROME_CONFIG_HOME` is not followed by the app (rare).
  - `commands.rs`: `BrowserStatus` has `browsers: [{name, present, registered}]` and `otherCopy` (the first other exe any manifest names) in
    place of `chrome`/`edge`/`firefox`. Windows fills Chrome and Edge from its registry check with `present: true` and `otherCopy: null`.
  - `app/src/lib/api.ts` (`BrowserInfo`, `BrowserStatus`), `app/src/screens/BrowserExtension.tsx`: step 1 names the browsers found ("your
    browsers" when none), the status line lists the set-up ones (and on Linux "Not yet for X" with a Set up button when a present browser has no
    file), `otherCopy` shows "set up for another copy of LocalPass: <path>" with **Use this copy** (the same install); step 2 on Linux shows the
    Firefox `.xpi` steps and the Chrome/Chromium/Brave/Edge/Vivaldi Load unpacked steps with the folder and Open; a muted line says Flatpak and
    Snap browsers cannot reach LocalPass. Windows wording is unchanged ("Chrome and Edge").
  - `tauri.linux.conf.json`: `bundle.resources` `"../../extension/dist/**/*": "extension/"`. `extension_folder()` uses Tauri's `resource_dir()`:
    for `/usr/bin/localpass` that is `/usr/lib/localpass` (`../lib/<productName>`, tauri-utils `platform.rs`), so `/usr/lib/localpass/extension`;
    in a cargo build it is `target/debug` and tauri-build copies the resources to `target/debug/extension` at build time (stale files there are
    not deleted). `build:all` (the RPM's `beforeBuildCommand`) already builds the extension, and `release.yml`'s `linux` job installs its packages
    first. Consequence: every Linux cargo build of `localpass` now needs `extension/dist` built (as on Windows); CI's Linux job builds it first.
  - `crates/lp-broker/src/nmhost.rs`: test `linux_chrome_invocations_are_recognised_and_checked` (`[exe, chrome-extension://<id>/]` is an
    invocation and an allowed caller; another id or our id without the slash is not allowed).
  - Extension: `src/shared/platform.ts` (`checkName`, `osFromNavigator`, `platformOs` via `chrome.runtime.getPlatformInfo` or, in a content
    script, the isolated `navigator`; `browserName` from the user agent and its brands, now also used by `background/broker.ts` for the client
    name). Popup: the "is it you?" name comes from it (no more `__FIREFOX__` for that) and the privacy note names the running browser. Content
    passkey dialog: "Confirm with <check> if it asks.", worked out per request. Tests: `test/platform.test.ts` (new), `popup.test.ts` (Linux and
    Windows wording, privacy note), `content-passkeys.test.ts` (Linux and Windows line); `test/chrome.ts` fakes `getPlatformInfo` (`fake.os`).
  - `extension/e2e/harness.ts`: `BrowserSetup` (work folder, dev host and browser env, helper registration, launch, cleanup) used by `Harness`
    and now also by `e2e/run.ts` (which had its own copy). Linux: `dev_host`/`nmhost` without `.exe`, browser from `BROWSER`
    (chrome|chromium|edge|brave|vivaldi, first existing path), manifest in `<throwaway profile>/NativeMessagingHosts`, throwaway HOME, XDG
    folders and runtime folder (the broker socket), `--password-store=basic` (no keyring), a check that the real
    `~/.config/<browser>/NativeMessagingHosts` did not change; `LP_CHROME_HEADED=1` runs headed. Windows: registry as before.
  - README: "Browser extension" (title, Chromium on Fedora paragraph, Flatpak/Snap), "Firefox on Fedora Linux" (the "not set up" sentence
    gone, XDG note), "Installing and updating" (registrations, reloading the unpacked extension after an RPM update).
  - Checks run 2026-11-12: `cargo fmt --all` clean; `cd app && bunx tsc --noEmit && bun run build` pass; `cd extension && bunx tsc --noEmit`
    pass, `bun test` 262 pass 0 fail, `bun run build`, `bun run build --firefox`, `bunx web-ext@8 lint -s dist-firefox` 0 errors/0 notices/
    0 warnings. **Chrome 157 on Linux, headless:** `bun run build --e2e && bun run e2e/run.ts` all 30 checks pass (incl. "the real browser
    config was not touched"), `bun run e2e/passkeys.ts` 25 checks pass, with the 21:29 examples.
  - Rust, checked by the lead on 2026-11-12 in `target/phase8-check` with the override under "Machine facts": `cargo clippy --workspace
    --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast` 355 passed, 0 failed, 2 ignored.
  - **Open:** the Windows and Android builds of `commands.rs`; `tauri build` (the RPM with the extension folder); the Settings screen in
    `tauri dev`; the assisted check (Set up in the dev app, load `extension/dist` in the real Chrome, pair, fill, save).
- [x] 8.2 Offer a save only for a new or changed login (coded and checked; local walkthrough accepted by the user on 2026-11-15,
  see "RESUME HERE" for the limits of that acceptance and the earlier "Open" notes for historical context).
  - Broker: request `checkLogin {origin, username, password}` (`protocol.rs`) answers `{status: "new" | "update" | "unchanged"}` from
    `Engine::plan_save_login` (`server.rs`), so the same input checks as `saveLogin` (https origin, password not empty; no length limit of its
    own beyond the 1 MiB frame, as `saveLogin`). Not interactive (a locked vault answers `locked`, no unlock, no prompt), not for a provider
    client (`forbidden`). The comment where it is declared says why it gives a paired extension nothing new (`findLogins` already lists user
    names for any site and `fillLogin` returns the password of a login saved for it). Tests: `protocol.rs` (wire form, classification),
    `tests/protocol_props.rs` (exhaustive table, 14 kinds), `tests/hardening.rs` (new/update/unchanged incl. case, spaces and another
    address of the site, nothing written or asked; locked without an unlock prompt; refused for the provider).
  - Extension background (`background/index.ts`): `storeCapture` drops a capture whose password hash equals the tab's last fill when the
    user name matches as the engine compares it (trimmed, ASCII case ignored) or is empty. New message `seenUser {username}`: the content
    script sends it when a form with a user name box and no password box (the account step that `detect.ts` already finds by
    `autocomplete="username"`/`email`) is submitted, clicked or sent with Enter; the worker checks it like a capture (content script only,
    string, at most `MAX_CAPTURE_CHARS`, not blank) and keeps `seenUser:<tab>` = {origin from `sender`, username, at} in session storage
    (removed with the tab). A password-only capture on the same origin within 5 minutes takes that name. `pendingSave` (serialized per
    tab) asks `checkLogin` once per capture only when the connection is already `ready` (a page load still never connects), with a 5 s
    limit: `unchanged` drops the capture silently, `update` marks it (`PendingSave.update`), `new` leaves it; locked, not connected, no
    answer, an unknown status or an older app (`unsupported`) offer as before and ask again at the next page load. Firefox marks the capture
    offered only when it is offered.
  - **Follow-up (lead, same day): a sign-in the user really sent may connect; a page load still never does.** The content script marks a
    `capture` `byUser` only for a trusted click or a trusted Enter key (`sentByUser`); a submit event never counts, because
    `form.requestSubmit()` and `button.click()` from a page script fire a submit event with `isTrusted === true` (measured in Firefox 160 and
    Chrome 157 with a probe page). A capture the worker keeps as pending (not one it drops) and that is `byUser === true` calls
    `broker.connect()` (no pairing, not awaited) when the phase is `disconnected` or `appNotRunning`; unpaired, denied, untrusted, error and
    pairing states are left alone. This can start the app when it is not running, like the connect at browser start. `seenUser` never
    connects (a newsletter box with `autocomplete="email"` is the same kind of form). `pendingSave` still never starts a connection; while one
    is `connecting` it waits for it through `whenReady`, at most 2 s, then asks `checkLogin` or offers as before. The header comment of
    `background/index.ts` states the rule.
  - **Measured (Firefox 160, harness, 2026-11-12):** with the native port open and no extension page open, the event page was NOT suspended
    after 75 s idle (the `nmhost` helper process stayed). The earlier claim that it is suspended in daily use was an inference from the
    Phase 6 note (measured there without a native port) and was wrong for at least 75 s; longer idle times, browser or app restarts and
    Chrome's worker were not measured. The harness can stop the background (`FirefoxHarness.terminateBackground`, what about:debugging's
    "Terminate background script" does): the helper exits and the restarted background stays disconnected until something connects.
  - Wording (`shared/shown.ts` `saveOffer`): "Save this login to LocalPass?" / Save, or "Update the saved password in LocalPass?" / Update,
    in the Chrome save bar (`content/ui.ts`) and the Firefox decision window (`DecisionInfo.saveLogin.update`, `background/decide.ts`,
    `decide/index.ts`). The app's own "Replace?" dialog on Save is unchanged.
  - `content/detect.ts` `isOtpPasswordBox`: a `type=password` box is a one-time-code box (not a login password, so never captured) when it
    has no `current-password`/`new-password` token and either `autocomplete="one-time-code"`, or a strong code name (`otp`, one-time,
    2fa, two-factor, totp, authenticator, verification/security/sms code; not passcode, token, pin or a bare code) on a short box
    (maxlength 4-10 or `inputmode="numeric"`) whose name/id does not look like an account name. Side effect: `isOtpField` uses the same
    strict rule for password boxes, so a password box named for example `passcode` with a maxlength is no longer also taken as the form's code box.
  - Tests: `test/background.test.ts` (filled + empty or differently cased user name, another account still captured, popup `fillTab`,
    `seenUser` use, other site/tab/expiry, validation, `checkLogin` unchanged/update/new, locked/older app/no status, no connection = no
    request and no connect, Firefox once-only), `detect.test.ts` (code boxes, a password with a code box beside it, six real password boxes
    kept), `content-fill.test.ts` (account step sends `seenUser`, code box not captured, password step), `decide.test.ts`,
    `decide-page.test.ts`, `ui-guard.test.ts` (both wordings); `test/fakeapp.ts` `checkLoginAnswer`. Mutation-checked (guard removed, tests
    failed, guard restored): empty-user-name drop, user name normalisation, the `unchanged` drop, the `ready`-only rule, the seen-user origin
    check, the OTP filter in `findLoginForms`, the "current-password wins" rule, and in Rust letting `checkLogin` prompt to unlock.
  - E2E: `e2e/run.ts` (Chrome) and `e2e/firefox/run.ts` gained a two-step sign-in (`/two-step`, `/two-step/password`): filled by LocalPass
    on both steps (no offer), typed with a new account (offered with the first step's name, then Not now); a saved login typed by hand, also
    as " Alice" (no offer); a changed password typed by hand (update offer, Update, the vault login changed and no second login made). The
    harnesses take the examples from `$CARGO_TARGET_DIR/debug/examples` when that is set. The Firefox run also stops the background
    page and checks that a sign-in the page itself submitted (`requestSubmit`) is offered unchecked (it may not connect), and that a saved
    login sent with a real click after another stop reconnects and is not offered. The Chrome run has no restart check.
  - README "Browser extension": when a save is offered.
  - Checks run 2026-11-12 (cargo with the override under "Machine facts"): `cargo fmt --all` clean; `cargo clippy --workspace --all-targets
    -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast` 359 passed, 0 failed, 2 ignored; `cargo build -p
    lp-broker --examples` ok; `cd extension && bunx tsc --noEmit` pass, `bun test` 287 pass 0 fail, `bun run build`, `bun run build
    --firefox`, `bunx web-ext@8 lint -s dist-firefox` 0 errors/0 notices/0 warnings; Chrome 157 headless `bun run e2e/run.ts` 39 checks
    pass, `bun run e2e/passkeys.ts` 25 pass; Firefox `bun run e2e/firefox/run.ts` 100 checks pass; `cd app && bunx tsc --noEmit` pass.
    After the follow-up (extension only, no Rust change): `bunx tsc --noEmit` pass, `bun test` 297 pass 0 fail, `bun run build`,
    `bun run build --firefox`, `web-ext lint` 0/0/0, Chrome `e2e/run.ts` 39 checks pass, Firefox `e2e/firefox/run.ts` 103 checks pass.
    New unit tests (`background.test.ts`): a user-sent capture with no connection connects and the page load drops a saved login (no pair
    message); captures with `byUser` false/missing/"true"/1 never connect; `pendingSave` alone never connects; dropped captures (filled,
    never-save) do not connect, with a control that a kept one does; an unpaired browser and an app that failed its proof are left alone;
    the bounded wait (handshake answered after 300 ms: checked; never answered: offered after 1.9-3 s). `content-fill.test.ts`: submit never
    counts even when trusted, a trusted click or Enter does, a page's earlier submit does not hide the user's. Mutation-checked: the `byUser`
    guard, the "only wait for `connecting`" rule, connecting only for kept captures, the reconnect-state list, "submit never counts".
  - Still offered (not fixed): a typed login that is saved already while the vault is locked, when the app does not answer within 2 s of
    the user's sign-in (for example while it starts), or when the sign-in was submitted by the page's script and nothing is connected; a fill
    in one tab and the sign-in in another (records are per tab, `checkLogin` still catches it when connected); a site that changes the user
    name or password after the fill (formatting, client-side hashing) is offered as new or update; a capture from a button click before the
    real submit (show-password toggles) or a failed sign-in with a wrong password is offered as an update; an account step without
    `autocomplete="username"`/`email` is not seen, so its password step is offered with "(no username)"; change-password forms without a
    user name box are offered as a new login with "(no username)"; a code box of `type=password` without strong hints is still a password.
  - **Open:** the assisted check with the user (one-step and two-step sign-in with autofill: no offer; an existing login typed: no offer; a
    changed password: update offer), in Firefox and Chrome. Windows not run.
  - Checked by the lead: re-ran `cargo fmt --all`, the lp-broker tests, `bunx tsc --noEmit` and the extension tests (297 pass after the
    follow-up).
- [ ] 8.3 One protected panel for every decision, both browsers.
  - [ ] 8.3a Panel for all four decisions (2026-11-13, coded and checked here; **open:** see "Open" at the end of this entry). Firefox only;
    the Chrome build behaved as before (8.3b moved Chrome onto it).
    - `background/decide.ts` (`Decisions`): all four decisions use the toolbar panel when `panelSupported()` (an `action.openPopup`
      function and, through `runtime.getBrowserInfo`, Firefox 149+; nothing else assumes Firefox: without `getBrowserInfo` it answers
      false, which 8.3b changes into a feature check for Chrome), else the separate window exactly as before (closing it is "no", a save
      window closed is Not now). The tab's popup is pointed at the decision's panel (`setToolbar`: per-tab `setPopup`, and for a save or
      passkey request a badge "1"; `decidePanel:<tab>` in session storage remembers it) and `openPopup` opens it.
    - Lifetimes: a login pick ends when its panel closes (as before). A save offer and a passkey request are "pendable": a closing
      panel (port disconnect), a switch to another tab or a focus change only hides the panel (`hide`, `pend`); the request waits
      with the badge, the tab's popup stays the decision, so a click on the toolbar icon loads the same page again and it attaches
      again (one attached page at a time; a closed one can attach again). They end by an answer (Not now/Esc/Never, Cancel/Esc/Use
      another method), `decideCancel` (page abort), a navigation (passkeys: `status loading` or a URL change), the tab closing, a newer
      request from the tab, or their life: `decisionTiming.lifeMs` = 2 min from the request for passkeys, the capture's own 2 min for a
      save; the life only ends a request that no panel shows and nothing works on (`expire`, checked again in `pend`). The outcome a
      page hears is the one of a cancel today (NotAllowedError, or the last failure).
    - Saves: the site shown and saved is the capture's (`PendingSaveInfo.origin`, which the browser reported for the sign-in), not the
      tab's. `checkPanelTab` skips the site check for a save only (comment there: a save sends nothing to the page; the tab must still be
      the same tab, active in its focused window, as before). A save survives navigations: `onTabUpdated` sets the popup and badge again
      (Firefox resets per-tab action state when a tab loads a page). **Choice for point 6 (keep-alive after a navigation):** every page of
      the tab asks `pendingSave` (the Firefox gate "offered once" moved from `offerPending` to `Decisions`: the capture records `offered`
      and the `decisionId` through the new `markOffered` dep, serialized in the per-tab `offering` queue) and sends `decide` again; `open`
      finds the open decision for the same capture and **adopts** it (the earlier page's request is rejected, the new one gets the
      outcome and pings), so the background stays awake and nothing opens twice. Reason: the page that asked is gone after the sign-in's
      redirect, and only a live content script can keep the event page awake; no new message kind was needed for the take-over.
      **After a background restart** (`Decisions.restore`, run at start-up in the Firefox build, everything else awaits it): every tab whose
      popup a forgotten decision changed gets its normal popup and no badge, then every capture that was offered and is still valid waits
      again under its **old decision id** (no panel by itself), so a panel the toolbar still shows for it keeps working; a capture offered
      before but without a live decision (the restart raced a page) also waits without opening. A panel whose request is gone turns
      itself into the normal popup (`decide/index.ts` `toPopup`, `location.replace(popup.html)`), so there is never a dead panel. Login
      picks and passkey requests are not restored: their content script's `decide` call ends with the old background anyway.
    - Point 4 (work in progress): while an answer is carried out (`busy`) or the app's Unlock asked from the panel runs (`unlocking`), a
      closing panel and focus changes do nothing; a tab switch still ends a login pick (as before; a login is never handed to a page
      the user left). A login that was fetched is delivered only when its tab is the active tab of the focused window again, on the same
      site (`whenInFront`, polls every 250 ms for up to `refocusMs` = 60 s; the old immediate re-check would have failed while the app
      still had the focus after Hello/Unlock). If carrying out fails with no panel attached: a login pick ends with that error; a save or
      passkey request waits again with the badge and `decisionInfo.failed` (shown as text, "The last try did not work: ...") when shown
      again. A panel shown again during the work gets `working: true` and shows "Waiting for LocalPass…" until the work ends (polls
      `decisionInfo` every 500 ms; on success the background ends the decision and the panel closes with its port).
    - Point 5 (Unlock): the panel's Unlock sends `decisionUnlock {id}` (authorized like every decision message); the background asks the
      app and follows it (a second Unlock joins the first). Refused or timed out (the broker answers `locked`): a login pick is cancelled,
      a save or passkey request waits. Unlocked with no panel attached: the panel opens again by itself once (`tryReopen`, on the focus
      returning to the decision's window or the tab being activated) when the tab is the active tab of the focused window and, but for a
      save, still on the decision's site; within `reopenMs` = 60 s, after which a login pick ends and the others only wait. A login pick
      whose tab is no longer the active one when the Unlock ends is cancelled. A focus change to "no browser window" (the app) while
      waiting to reopen cancels nothing.
    - Point 6 (keep-alive): `content/ask.ts` `askDecision` sends `decidePing {ref}` every 10 s while its `decide` call is open (the
      background answers whether that frame's request is open; Chrome refuses it as every decision message); used by the picker,
      the save offer and both passkey calls.
    - Point 7: every end goes through `finish` → `close` → `clearToolbar` (in the popup queue; the popup is reset to the manifest's
      `default_popup` only when it is still this decision's URL, the badge is cleared then or when no other decision holds the tab) and the
      restart restore. Point 8: `openPopup` throwing, or the tab not in front when the request comes: a login pick ends with the existing
      error; a save or passkey request waits with the badge (no window as fallback).
    - `decide/index.ts`: in the panel a blur no longer answers a save offer (the browser closes the panel on a click outside; the
      background decides); a window keeps the old rule. Shown-again views (`working`, `failed`), `toPopup`. `decide/timing.ts` `pollMs`.
    - `shared/messages.ts`: `decidePing`, `decisionUnlock`, `DecisionInfo.working/failed`. `background/index.ts`: `markOffered`,
      `decisionView` (life, offered, id), `offeredCaptures` + `decisions.restore` at start-up (Firefox), the two messages, `decidePanel:`
      removed with the tab.
    - `crates/lp-broker/examples/dev_host.rs`: `hold-unlock` (an unlock prints `UNLOCK?` and waits), `unlock yes|no`, `auto-unlock`, for
      the e2e of a slow Unlock.
    - Tests: `decide.test.ts` (the toolbar fake now has per-tab popups and badges; 25 new tests under "every decision in the panel
      (8.3a)", one for older Firefox), `decide-page.test.ts` (panel blur does not answer a save, Unlock goes by `decisionUnlock`, stale
      panel becomes the popup, shown again while working, failure line as text), `background.test.ts` (once-per-capture semantics
      changed: a later page gets the capture again to take the offer over; the panel opens once and the next page adopts it; pings;
      restart restore incl. an expired capture), `content-passkeys.test.ts` (pings while the call is open and stop after), `chrome.ts`
      `installToolbar`. `content-passkeys.test.ts` also clears leftover in-page hosts and loads its own copy of the bridge first: bun
      runs the files in the folder's order (btrfs: changes when a file is rewritten), and after `ui-guard.test.ts` and `rp.test.ts` the
      file's 38 tests failed for that reason alone (a pre-existing order dependence, not a code change).
    - Mutation-checked (each guard changed, tests failed, guard restored), 21 in all: busy/unlocking never cancels on close; busy
      ignores focus changes; save waits on close; passkey waits on close; the popup is restored; the badge is cleared; reopen only for
      the same active tab in the focused window; a second attach is refused; site check relaxed for saves only; a save still needs its
      tab active in the focused window; a fetched login waits for its tab to be in front; refused Unlock (pick cancelled, others wait);
      a waiting request ends after its life (the suite hung, i.e. caught); a failure while closed (pick ends, others wait); the panel opens
      by itself once (offered capture waits); a later page takes the save over; restart clears forgotten panels; content pings; panel
      blur is not Not now; stale panel becomes the popup; the hold on acting controls.
    - E2E `e2e/firefox/run.ts`: harness `clickToolbarButton` (pins the action button and clicks it with BiDi input in the browser
      window's chrome context; Firefox opens the tab's popup), `pressKey` in a panel now dispatches a trusted key through the panel
      page's own `nsITextInputProcessor` (measured: BiDi key input never reached a panel's page, not even a letter, so the old "Esc closes
      the chooser" check only exercised Firefox closing the popup). The old-Firefox path is forced by replacing the background's
      `chrome.runtime.getBrowserInfo` through `runtime.getBackgroundPage()` from the extension's popup tab (restored afterwards). An outside
      click is still the native `hidePopup()` path (BiDi clicks bypass Firefox's popup handling, Phase 7 note).
    - Changed or removed e2e checks: "closing the save window with X counts as Not now" and "nothing saved after X" (now: the closed
      panel waits with a badge, the next page does not open it, the toolbar icon shows it again, Save there; the "saved"/"in the
      vault"/"closes after about two seconds" checks now run on that re-shown panel instead of a third offer); "closing the window with X
      refuses the request (NotAllowedError)" (now: waits, completes when shown again; Esc refuses); "no more than eight LocalPass windows
      are open at once" (now: no more than eight requests open, the ninth refused, closing the tabs ends them); the locked picker (was:
      Unlock in the open panel, list, Cancel; now: held Unlock, panel closed, reopens by itself, a pick fills; a refused Unlock ends the
      pick). Unchanged in what they check otherwise.
    - Checks run 2026-11-13 (cargo with the override under "Machine facts"): `cargo fmt --all -- --check` clean; `cargo clippy --workspace
      --all-targets -- -D warnings` clean; `cargo test -p lp-broker` 107 passed, 0 failed (the workspace tests were not rerun: only the
      `dev_host` example changed); `cargo build -p lp-broker --examples` ok. `cd extension && bunx tsc --noEmit` pass; `bun test` 332 pass,
      0 fail; `bun run build`, `bun run build --firefox`, `bunx web-ext@8 lint -s dist-firefox` 0 errors/0 notices/0 warnings. Chrome 157
      headless: `e2e/run.ts` 39 checks pass, `e2e/passkeys.ts` 25 pass (unchanged counts). Firefox 160 headless: `e2e/firefox/run.ts` 149
      checks pass (twice in a row after the last harness fix). `SLOW=1` was not run.
    - **Open / not verified:** a person's real click outside the panel (the harness uses the native `hidePopup()` path); a real keyboard
      Esc in the panel (the harness sends a trusted key inside the panel's page, which reaches the page's handler; whether Firefox's own
      popup handling can close the panel on a real Esc before the page sees it was not measured: then Esc on a save or passkey panel would
      act like a click outside, i.e. leave it waiting); the real app's Unlock taking the focus on GNOME and the panel reopening with real
      focus events (the harness hides the panel and the window keeps the focus); a real Firefox below 149 (forced by replacing
      `getBrowserInfo`); whether Firefox resets a tab's popup and badge on a navigation (handled either way; in the run the panel stayed
      open across a cross-origin redirect); a tab moved to another browser window while its request waits (the decision keeps its old
      window id, so its panel then says the page is no longer active; not handled); the assisted check of 8.3 with the user.
    - Checked by the lead on 2026-11-13: re-ran `bunx tsc --noEmit`, the extension unit tests (332), the Firefox e2e (149 checks pass) and
      the Chrome run (no failures).
  - [ ] 8.3b Chrome uses the panel; in-page controls go (coded on 2026-11-13; the lead ran the checks and reviewed the code against it,
    no defect found; mutation-checked, the test gaps closed by tests on 2026-11-13; **open:** its assisted check and Windows, see "Open" at
    the end of this entry).
    - **Measured first (Chrome 157.0.8103.78, Linux; probe extension and puppeteer in a throwaway profile, harness `BrowserSetup`):**
      1. `action.openPopup({windowId})` works **without a user gesture** (asked from the service worker on a content script's message
         at page load). It needs the window to be the active (focused) one: otherwise `"Cannot show popup for an inactive window. To show
         the popup for this window, first call chrome.windows.update with focused set to true."`; without `windowId` and no focused
         window, or while a popup is already open, `"Could not find an active browser window."`. It opens the popup of the window's
         **active** tab (asked for a background tab, it opened the active tab's popup): `checkPanelTab` before `openPopup` stays needed.
      2. While the popup has the keyboard focus (`document.hasFocus()` true in it): `windows.get(id).focused` is **false**,
         `windows.onFocusChanged` fires the window id and then `-1` (`WINDOW_ID_NONE`) as the popup opens, `windows.getLastFocused()`
         still names the browser window, `tabs.query({active: true, lastFocusedWindow: true})` still gives the tab.
         `runtime.getContexts({contextTypes: ["POPUP"]})` lists the popup with its exact URL (query included) but `windowId: -1`,
         `tabId: -1`. **Rule built (Chromium only, told apart by the absence of `runtime.getBrowserInfo`):** the window counts as
         focused when `windows.get().focused` is true, or when this decision's own panel is open (`getContexts` has its exact URL with
         the decision id) and `getLastFocused()` is the decision's window (`Decisions.inFront`/`panelShown`, used by `checkPanelTab`
         and the wait before delivering a login). Sound because Chrome opens a toolbar popup only in the focused window and closes it
         when it loses the focus. A focus change to `-1` ends nothing in Chromium while that panel is open (or being opened; looked at
         again after `settleMs` = 300 ms); Chrome's closing of the panel ends a pick through the port as before. Firefox keeps the
         plain rule (an unfocused window is refused even with the panel open; unit test).
      3. The popup page's `sender`: no `tab`, no `frameId` (absent), `url` the exact page URL with query, `origin`
         `chrome-extension://<id>`, `id` the extension id. Same as Firefox: the panel check is unchanged.
      4. Per-tab `setPopup`/`setBadgeText` work; `getPopup({tabId})` returns the **full** `chrome-extension://` URL (also for the
         default popup), so `clearToolbar`'s comparison holds. Chrome **resets a tab's popup and badge on every cross-document
         navigation** (same- and cross-origin; not on `pushState`): `onTabUpdated` already sets them again for a save (and the next
         page's `decide` adopts it). `tabs.get().url` is **null** without the `tabs` permission or host permissions (content-script
         matches do not grant it), which broke `checkPanelTab`: the Chrome manifest now asks for `tabs`
         (`management.getPermissionWarningsByManifest`: no new warning, the list stays "Read and change all your data on all
         websites", native apps, privacy settings; `host_permissions` gave the same warnings but more power). The Firefox manifest
         drops `tabs` (Firefox reads the URL already; `tabs` would add a prompt there).
      5. Lifetime: not measured beyond the e2e. Relied on Chrome's documentation (an extension event or API call resets the 30 s idle
         timer; `connectNative` keeps the worker alive while the port is open, Chrome 105+) and on the content script's 10 s
         `decidePing` and the panel's 10 s `decisionInfo`. All listeners are registered synchronously at the top level of
         `background/index.ts` (the `__FIREFOX__` block is gone). Seen in the e2e: with tab and window listeners the worker is started
         again by Chrome for ordinary tab events right after DevTools stops it. Not measured: Chrome's 5-minute limit for a single
         message that has no reply yet (a login pick open longer than that); if the worker is stopped then, the pick ends (the content
         script's call fails, the panel turns into the popup).
      6. Puppeteer reaches the open action popup as a `page` target (`target.asPage()`) **in headless Chrome**, and its clicks there
         are trusted (`isTrusted` true); `keyboard.press("Escape")` reaches the page's handler. **Headed is not usable unattended on
         this GNOME Wayland laptop:** a window started by the harness never gets the focus (Wayland and XWayland, also after
         `windows.update({focused: true})`), so `openPopup` fails. The Chrome e2e runs headless.
    - `background/decide.ts`: `panelSupported()` true in Chromium when `action.openPopup` is a function (Firefox: 149+ as before);
      `chromium()`, `panelShown`, `inFront` (rule above), `leftUnlessPanel`; `checkRequest` (a content script's `decide` request is page
      input: only known shapes and fields, `content/validate.ts`'s limits and relying-party rule against the frame's host, `newPassword`
      only when `=== true`, a save id a string of at most 64); `fits` (Generate only for `newPassword`, a passkey in a pick only with
      `passkeys`); the pick's outcome is now `ChooseOutcome` = `{fill}` | `{generated}` | `{passkey}`, each delivered only after
      `whenInFront`; `decisionCandidates` also for a pick with `passkeys`; `info` tells the panel `newPassword`/`passkeys`.
    - `background/index.ts`: decisions in both builds (`asker` and the listeners and `restore` without `__FIREFOX__`); handlers of the
      in-page controls removed; anything unknown answers `forbidden` (table below); `findLogins` for a content script answers
      `SiteStatus {status, hasLogins}`; `pendingSave` answers `{id}`; `unlock` popup-only.
    - Content: `content/ui.ts` is only the badge (`armOpener`, host element as before); `arm`, the IntersectionObserver guard,
      `showMenu`, `showSaveBar`, `showSheet` and their styles are gone. `content/index.ts`: the badge asks `decide` `chooseLogin` with
      `newPassword` (from `detect.ts`) and, in Chrome, `passkeys` (the pending conditional request's checked parameters); the outcome
      fills this form, its new-password boxes, or answers the page's pending request (`resolveConditional`, only if it is still the same
      request id). `offerSave` always asks `decide`. `content/passkeys.ts`: create and modal sign-in always through `decide`; Chrome parks a
      conditional request (as before), Firefox hands it to the browser at once (as before). `generate` and the passkey messages are no
      longer sent. Lines of source: `content/ui.ts` 426 → 104, `content/passkeys.ts` 276 → 172,
      `content/index.ts` 312 → 240, `background/index.ts` 581 → 564, `background/decide.ts` 726 → 864, `decide/index.ts` 347 → 368: about
      520 lines removed (menu, save bar, sheets, the visibility guard, their handlers), about 160 added, net about −360.
    - Decision page (`src/decide/`, now `public/decide.html|css`, built for both): the picker lists the page's pending passkeys first
      ("Passkey: <name>"), then the logins, then "Generate a strong password / Fills the new password boxes" for a new-password form;
      with no login it still offers Generate. Rows are held like every acting control; nothing that acts has the focus. The waiting line
      is "Confirm with Windows Hello if it asks." or "Confirm with your master password if it asks." (`shared/platform.ts`); the page
      names no browser. `build.ts` bundles `decide` for both builds; `public-firefox/` is gone. `manifest.json`: `minimum_chrome_version`
      127, `tabs`.
    - Messages after 8.3b (a "page" is a web page's content script; "panel" the decision panel or window authorized by `authorize()`):

      | kind | who may send it | what it returns |
      |---|---|---|
      | `findLogins` | page | `{status, hasLogins}` for the frame's site (no titles or user names); `interactive` may connect |
      | `capture` | page | nothing (stores a capture after the 8.2 checks) |
      | `seenUser` | page | nothing |
      | `pendingSave` | page (top frame) | `{id}` of the tab's capture to offer, or null |
      | `passkeysEnabled` | anyone of the extension | the popup's switch (boolean) |
      | `decide` | page | the outcome of the user's decision: `PasskeyCreated`, `PasskeyAssertion`, `ChooseOutcome` or `SaveOutcome` (request checked by `checkRequest`) |
      | `decideCancel`, `decidePing` | page | nothing / whether that frame's request is open |
      | `decisionInfo` | panel | what the panel shows (site, user name of a save, passkey request, `newPassword`, `passkeys`, `working`, `failed`) |
      | `decisionLogins` | panel (pick) | titles and user names of the site's logins |
      | `decisionCandidates` | panel (passkey sign-in, or pick with `passkeys`) | the site's passkey candidates |
      | `decisionUnlock`, `decisionAnswer` | panel | Unlock's result / `{message?}`; the secret goes to the page's `decide` |
      | `state`, `connect`, `unlock`, `loadLogins`, `cachedLogins`, `fillTab`, `getPrivacy`, `setPrivacy` | popup only | as before |
      | `fill`, `savePending`, `dismissPending`, `passkeyCandidates`, `passkeyCreate`, `passkeyAssert`, `generate`, anything else | nobody | `forbidden` |
    - Tests: `ui-guard.test.ts` (the badge only: host element, a made-up click ignored, a real one only calls the opener, the shadow
      root holds only the badge and the module exports only `guard`/`showBadge`/`hideBadge`, it follows its field);
      `content-passkeys.test.ts` (limits and only known fields go to `decide`, nothing drawn, outcomes, withdraw, pings, switch,
      conditional: parked with checked parameters, answered once and only for the same request, withdrawn or replaced, Firefox fallback;
      the V13 display tests moved to `decide-page.test.ts`); `content-fill.test.ts` (badge from the status alone, `chooseLogin` without
      and with `newPassword`, a fill, a generated password into both new boxes, a cancelled pick, a conditional passkey from the
      picker); `decide.test.ts` (request checks, Generate and passkey rows and their refusals, the Chromium focus rule incl. six
      refusals and "Firefox is not loosened", focus to the panel keeps a pick, a fetched login waits while neither window nor panel
      has the focus); `decide-page.test.ts` (rows, hold, wording on Windows and Linux, no browser named, V13); `background.test.ts`
      (every closed kind refused for a page with nothing asked or stored, gone for the popup too, `unlock` popup-only, `findLogins`
      without names, `pendingSave` only the id, saves through the decision, decisions in both builds; the 8.2 tests read the user name
      from session storage); `manifest.test.ts` (`tabs`, 127, Firefox without `tabs`); `test/chrome.ts` (decision listeners and
      `windows.get` in the base fake), `test/setup.ts` (no `FakeVisibility`).
    - Mutation checks (repeated on 2026-11-13 by a separate agent; one change at a time, whole `bun test`, original text restored, tree
      checked against checksums afterwards), 58 in all, 46 caught, 12 survived. Caught: `background.test.ts`: each closed kind given a
      handler again (`fill`, `savePending`, `dismissPending`, `passkeyCandidates`, `passkeyCreate`, `passkeyAssert`, `generate`), the
      final `forbidden` turned into an answer, `unlock` without `popupOnly`, `findLogins` answering the full reply, `pendingSave`
      answering the whole capture; `authorize()` skipped in `decisionInfo`, `decisionLogins`, `decisionCandidates`, `decisionAnswer`,
      `decisionUnlock` (`background.test.ts` and/or `decide.test.ts`); `decide.test.ts`: the panel's no-`tab`/`frameId` check,
      `checkRequest` (unknown kind passed through, `checkCreate`/`checkGet` skipped (limits), relying-party id taken from the request
      instead of the frame's host, `newPassword` truthy instead of `=== true`, save id without the 64 limit, save id not a string),
      `fits` (Generate without `newPassword`, a passkey in a pick without `passkeys`), `decisionCandidates` for a pick without `passkeys`
      and for any decision, `{fill}` delivered without `whenInFront`, `chromium()` always true, `panelShown` (any decision's panel, any
      panel URL, no `getContexts` check (failed, then the suite hung), `getLastFocused` not compared), `inFront` (no panel check, tab
      not required active), a focus change to -1 always ending (no `leftUnlessPanel`), the -1 rule also for Firefox (test timed out),
      `leftUnlessPanel` never ending (test timed out); `content-passkeys.test.ts` (+ `content-fill.test.ts`): the parked request's
      parameters with extra fields, `resolveConditional` answering whatever request is pending now, or without the id check, the modal
      sign-in passing all of the page's fields, Firefox parking a conditional request; `ui-guard.test.ts`: `armOpener` without the
      trusted-click check, a second element (a menu) drawn next to the badge. **Survived** (no test fails; not fixed, as briefed):
      (1) `asker()` without `fromPage` (`background/index.ts` 465): a test would send `decide` from an extension tab (`sender.tab` set,
      extension URL) and expect `forbidden`; (2-4) `checkRequest` keeping unknown fields (`...s` / `...value` for `passkeyGet`,
      `saveLogin`, `chooseLogin`): a test would check the stored request (e.g. through `decisionInfo` or what reaches the app) carries
      no field the page added; today such fields are inert; (5) `run()` generating without `spec.newPassword` (`decide.ts` 795): the
      second check behind `fits`, unreachable while `fits` holds; (6, 7) `{generated}` and `{passkey}` delivered without `whenInFront`
      (`decide.ts` 798, 802): a test like the `{fill}` one ("a fetched login waits until its tab is in front again...") for Generate and
      for a passkey picked in the pick; (8) `leftUnlessPanel` without the second look after `settleMs` (567), (9) without
      `await d.opening` (565), (10) without the `busy`/`unlocking`/`reopenUntil` re-check after the wait (569): tests where the panel
      appears only after the first look, while `openPanel` is still running, and where an answer or Unlock starts during the 300 ms;
      (11) `content/index.ts` 69 resolving the request pending now instead of `pendingPasskey.id` (`finish()` in
      `passkeys.ts` still only answers a known id; a `content-fill` test would replace the page's request while the picker is open and
      expect the new one unanswered); (12) an extra action dispatched by the badge's click after `action()` (the test checks only that
      the opener is called).
      Gaps closed on 2026-11-13 (step 1(c), tests only, no source change; each new test passes on the unchanged code and fails under its
      mutation, run with the same apply/run/restore script): (1) `background.test.ts` "an extension page shown in a tab (sender.tab set,
      extension URL) may not ask for, withdraw or ping a decision" catches `asker()` without `fromPage`; (2-4) `decide.test.ts` "fields
      the page added are dropped: the checked request has only the known fields" (calls the exported `checkRequest` directly) catches all
      three `...s`/`...value` mutations; (6, 7) `decide.test.ts` ("the picker's own rows (8.3b)") "a generated password and a passkey
      picked in the pick also wait until the tab is in front again (like a fetched login)" catches `{generated}` and `{passkey}` delivered
      without `whenInFront`; (8-10) in "Chrome: the panel takes the focus from its window (8.3b)": "focus to -1 keeps a pick when its
      panel shows up only on the second look (after settleMs)" catches the missing second look (8; the test below fails under it too),
      "focus to -1 while the panel is still opening (openPanel running) keeps the pick" catches the missing `await d.opening` (9), "an
      answer or an Unlock that starts during the settleMs wait is not ended by the focus change" catches the missing
      `busy`/`unlocking`/`reopenUntil` re-check (10; each of its two cases alone fails under it); (11) `content-fill.test.ts` "Chrome: a
      passkey chosen in the picker does not answer a request the page made after the picker opened" catches `content/index.ts`
      answering the request pending now. Each mutation was then run again against the whole `bun test` with all new tests in place: each
      fails (only the tests named above; the second-look mutation (8) also fails the answer/Unlock test). New totals: 58 mutations, 56
      caught, 2 survive and are left without a test on purpose: (5) `run()` generating without `spec.newPassword`, a second check behind
      `fits` that cannot be reached while `fits` holds, and (12) an extra action after the badge's `action()`, an artificial change.
      Also added: an `afterEach` in `decide.test.ts` that ends every decision a test left open (`onTabRemoved` for each), test-side only.
      Before it, a failing `decide.test.ts` test left its decision open, and the run never ended (seen with `panelShown-noContexts`: still
      running with the CPU busy after 280 s, all tests already reported); with it, `panelShown-noContexts` and `focus-none-firefox` fail
      and the whole `bun test` ends in about 22 s. `bun test` after step 1(c): 356 pass, 0 fail (7 new tests); `bunx tsc --noEmit` pass;
      `extension/src` unchanged (checksums).
    - Chrome e2e (`e2e/run.ts`, `e2e/passkeys.ts`, headless, through the panel: `Panels`, `panelClick`, `panelShows`, `toolbarBadge`,
      `inPageControls` in `harness.ts`). Changed checks: "the menu lists the saved login" (now the panel lists it); the fills (a row in
      the panel); "no save prompt after signing in with a filled login" and the 8.2 "not offered" checks (no panel opens within 2.5 s,
      and the toolbar badge is empty); "a new login is offered for saving" and the update and two-step offers (in the panel, Save,
      Update and Not now there); the locked vault (Unlock in the panel, then the pick fills); the attack pages: "page CSS cannot hide the
      interface" and "keeps its own display..." now about the badge, plus "its click only opens the panel and nothing is filled"; the
      three decoys: "painted over the interface keeps it from reacting", "a click through ... opens nothing", "with the decoy gone the
      interface reacts again" and "then a click opens the menu" are **removed** (there is no visibility guard any more) and replaced by
      "a click through <decoy> at most opens the panel, and fills nothing" and "the page still has no LocalPass control but the badge".
      Passkeys: the create, refusal, chooser, conditional, locked, Esc and switch checks now read and click the panel; "closing the
      dialog refuses" is now "Esc in the panel refuses"; the worker check is now "stopped" (DevTools reports `stopped`; Chrome may start
      it again at once for a tab event since the worker listens to them) and "a passkey request to a stopped worker still gets the
      LocalPass panel", plus "and the sign-in through it works". The harness lets a worker run that DevTools holds after `stopWorker`
      (`Runtime.runIfWaitingForDebugger`; measured: puppeteer did not, and the restarted worker stayed "starting"). New: the only LocalPass
      control in a page is the badge (also during a save offer: none), the panel's rows are held when it opens, a content script's own
      `fill`/`generate` are refused and its `findLogins` names no login (from the content script's own isolated world over CDP), the
      toolbar badge for a waiting save, Generate (both boxes, not the user name, never in the panel; no Generate row on a sign-in form),
      the panel closes after a pick and after a save, a passkey request draws nothing and shows the toolbar badge.
    - Firefox e2e: a sign-up page and "Generate fills both new password boxes, not the user name", "the picker closes after Generate".
    - Windows-only scripts (not run here): `e2e/app.ts` now drives the panel (badge → panel row; Create passkey and the sign-in row in the
      panel; the locked report in the panel). `e2e/host.ts`, `e2e/windows-provider.ts`, `e2e/provider-analysis.ts` and
      `test/provider-analysis.test.ts` do not touch the extension's page UI; unchanged.
    - README ("Browser extension": the panel paragraph for every browser, the filter limit gone; passkeys; "Firefox on Fedora Linux"
      shortened to what differs; "Security notes and limits": the badge bullet for every browser, the `filter`/`opacity` bullet removed).
      `CLAUDE.md`: "The extension wraps WebAuthn in the page", a new "One decision panel in every browser" bullet, "Firefox build",
      "Extension trust rules".
    - Checks run by the lead on 2026-11-13, once each, with the cargo environment override: `cargo fmt --all -- --check` clean; `cargo
      clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast` 359 passed, 0
      failed, 2 ignored; `cargo build -p lp-broker --examples` ok; extension `bunx tsc --noEmit` pass; `bun test` 349 pass, 0 fail; Chrome
      157 headless `e2e/run.ts` 47 checks pass, `e2e/passkeys.ts` 28 pass; Firefox 160 `e2e/firefox/run.ts` 151 pass; `bun run build` and
      `bun run build --firefox` ok; `web-ext lint` 0 errors, 0 notices, 0 warnings; `cd app && bunx tsc --noEmit` pass. Not run: each e2e
      a second time, `cd app && bun run build`, `SLOW=1`. Repeated on 2026-11-13 (mutation-check agent), two passes each, all
      passing: Chrome 157 headless `e2e/run.ts` 47 and 47 checks, `e2e/passkeys.ts` 28 and 28; Firefox 160 `e2e/firefox/run.ts` 151 and
      151; then `bun run build` and `bun run build --firefox` ok.
    - What a page can do now that it could not before: open LocalPass's panel through the badge even when it hid or covered the badge
      (before, Chrome ignored a click it could not prove visible); claim a new-password form (adds the Generate row; Firefox had no
      Generate at all). What it can no longer do: get any LocalPass control that acts drawn into it (menu rows, Save, Create passkey,
      a passkey row), so no click on a hidden, covered or restyled control fills, saves or signs; a content script can no longer get a
      password (`fill`), a signature (`passkeyAssert`/`passkeyCreate`), a save or dismissal, a generated password, the app's Unlock
      prompt, or the titles and user names of the site's logins and the user name of a capture by a message of its own.
    - **Open / not verified:** Windows (Chrome and Edge: the focus rule rests on the Linux measurement; on Windows the popup is its
      own top-level window, so `getLastFocused` and `getContexts` behaviour there is assumed, not measured; `e2e/app.ts` unrun); a
      person's real clicks and focus in headed Chrome on GNOME (Windows Hello or the app's Unlock taking the focus closes Chrome's panel:
      the reopen after Unlock and the wait before a fill are unit-tested and run headless only); Chrome's 5-minute single-message limit
      for a long-open pick; the worker now wakes for every tab update in Chrome (`tabs.onUpdated` has no filter there), measured only as
      "works"; a tab moved to another window while its request waits (as in 8.3a); `SLOW=1`; the assisted check of 8.3 with the user.
    - **Lead review (step 1(a), 2026-11-13), all against the code:** every message kind is sent only by who the table says (page kinds
      through `fromContentScript`/`asker()`, popup kinds through `popupOnly()`, panel kinds through `authorize()`, the rest `forbidden`);
      `checkRequest` builds the request again from known fields with `content/validate.ts`'s limits and the relying-party rule; `fits`;
      the Chromium focus rule only replaces "the window has the focus" (the tab must still be the active tab of the decision's window);
      Firefox keeps the plain rule; `content/ui.ts` lost only the menu, save bar, sheets and visibility guard, and the badge keeps the
      trusted-click opener. The `tabs` install warning was checked in Chromium's source
      (`chrome/common/extensions/permissions/chrome_permission_message_rules.cc`, `main`, read 2026-11-13): `kTab` is in the suppressed
      list of the `IDS_EXTENSION_PROMPT_WARNING_ALL_HOSTS` rule, so with the content scripts' `https://*/*` match "Read your browsing
      history" is not shown, which agrees with the agent's measurement. No defect needing a code change was found. One edge to try in the
      assisted check: clicking the badge while an earlier panel is still closing could make `openPopup` fail and that pick end silently
      (not measured).
  - [ ] 8.3c Picker and popup polish. Split by the lead: part 1 (shortcut, keys in the decision panel, the hold without
    dimming and without a hold when the user opened the panel) and part 2 (the popup: first Fill focused, inline messages).
    - **Part 1 (2026-11-13, coded by an agent; all its checks pass; checked by the lead).**
    - **Measured first** (probe extension with `commands._execute_action`, a per-tab popup and a background log; throwaway HOME and
      profile; Chrome 157.0.8103.78 headless through puppeteer, Firefox 160.0 headless through the Firefox harness; deleted afterwards):
      1. **`Ctrl+Shift+L` is not assigned in Chrome 157 on Linux:** `commands.getAll()` gives `_execute_action` with `shortcut: ""`
         (a conflict leaves it empty). Other suggested keys in fresh profiles: `Ctrl+Shift+Y`, `Ctrl+Shift+U`, `Ctrl+Shift+K`,
         `Ctrl+Shift+E` and `Alt+Shift+L` are assigned; `Ctrl+Shift+P` and `Alt+Shift+P` are empty too. **Firefox 160 assigns
         `Ctrl+Shift+L`** (and `Alt+Shift+L`). The manifest keeps the plan's `Ctrl+Shift+L` (see the decision below).
      2. With a per-tab popup set, **Firefox opens that popup for a real toolbar click and for the shortcut** (BiDi key input into the
         browser window's chrome context; the same keys into the page's context opened nothing), and **nothing reaches the
         background** for either: no `action.onClicked`, no `commands.onCommand` (`_execute_action` is never sent there). With no
         popup at all, the click and the shortcut both fire `action.onClicked` (still no `onCommand`). Chrome headless: the shortcut
         cannot be driven (it is unassigned, and CDP key events go to the page, not to the browser's accelerators), and puppeteer
         cannot click the toolbar; Chrome documents the same silence (`onClicked` does not fire when a popup is set).
      3. **Telling the background's own `openPopup` from a user opening:** time from the `openPopup` call to the panel page's port
         reaching `runtime.onConnect`, 8 openings each: Chrome 99, 64, 47, 25, 31, 26, 40, 44 ms (`openPopup` resolved 1-2 ms after the
         connect in each case); Firefox 56, 51, 37, 42, 45, 22, 18, 20 ms (`openPopup` resolved 8-35 ms after the call, i.e. **before**
         the connect). So "the promise has resolved" does not mean the panel has attached, and any time window would be a guess that a
         busy machine (or a page flooding the background with messages) could stretch. The design below uses no time window.
      4. **Focus at open, both browsers:** the panel page starts with `document.hasFocus()` false, then gets a `focus` event 25-60 ms
         later (Chrome also `visibilitychange` to visible: it starts `hidden`; Firefox starts `visible`), the same for a panel opened by
         `openPopup` and (Firefox) by the toolbar click. So a user-opened panel would be held by its own first focus event if every
         `focus` started the hold.
    - **Changed:**
      - `manifest.json`: `commands._execute_action` with suggested key `Ctrl+Shift+L` (`Command+Shift+L` on a Mac); `manifest-firefox.ts`
        carries it over unchanged (its `...rest`).
      - `decide/index.ts`: the hold no longer sets `disabled` (no greying): acting controls get `aria-disabled="true"` for
        `timing.holdMs`, and every way to act goes through `act()`, which does nothing while `held`: their click handler, and a
        document `keydown` handler for Enter and Space on an acting control, which always calls `preventDefault()` (so the browser's
        own activation, Enter on key down or Space on key up, can never act; `keyup` of both is stopped too) and acts only for a fresh,
        plain key (`repeat`, a modifier or a composition never acts). Safe buttons (Cancel, Not now, Use another method, Close) are not
        held and keep their native behaviour; Esc unchanged. Up/Down move the focus between the rows (`button.row`; from anything else
        Down goes to the first row and Up to the last; no wrap); moving does not act, so it works during the hold, and Enter on the row
        still waits for it. They do not reach the buttons (Tab does). Focus at each view: still the first safe button, also in a panel
        the user opened (Down then Enter picks).
      - **The user-opened signal:** `background/decide.ts` counts its own `action.openPopup` calls per browser window (`ownOpens`,
        incremented right before the call, since Chrome's panel can connect before the call returns; part 1 took it back when the
        call threw, assuming such a call shows nothing; **part 2 keeps it** (the safe side: the next opening in that window is held
        once). `attach()` takes one back when a panel page of a panel decision attaches in that window; when there is none
        to take, nothing of the background's opened this panel, so the user did (toolbar icon or shortcut), and it posts
        `PanelNote {userOpened: true}` on that page's port. A separate window never gets it. A refused attach takes nothing. A panel that
        closed before attaching leaves its count: the next panel in that window is then held once (safe side). The panel page starts held
        as before and only `note.userOpened === true` on its own port, only in a panel, ends the hold (`userOpened`). It holds again
        after it first loses the focus (`blur`) or is hidden (`visibilitychange`), as every window did; its own first `focus` and
        `visibilitychange` to visible at open (item 4) are not a return. Reason for that choice: a panel that lost the focus and got
        it back may meet a user who was typing elsewhere, the same reason as before; in practice both browsers close a panel that loses
        the focus, so this mostly concerns the app's Hello/Unlock prompt.
      - `shared/messages.ts`: `PanelNote`. `public/decide.css` untouched (no style for the hold: it is not shown).
      - Harnesses: `e2e/harness.ts` `panelClick` and `e2e/firefox/harness.ts` `clickWindow` wait for `aria-disabled` to go (the
        Firefox helper's synthetic click would otherwise "click" a held control and do nothing); Firefox `pressKey` now also sends
        Enter, ArrowUp and ArrowDown (same text-input-processor path in a panel, WebDriver code points in a window).
    - **Tests:** `decide-page.test.ts`: the old hold tests now check `aria-disabled` and that nothing is ever `disabled`; new
      "keys and the hold without dimming (8.3c)" (11): Down/Up between rows and their ends, Enter and Space pick (Space's key up is
      stopped), a held row ignores click/Enter/Space while the focus may move onto it, an acting button the same and safe ones are
      not held, `repeat`/modifier/composition never act and the default is stopped, a user-opened panel acts at once and its first
      focus and visibility do not hold it (focus still on Cancel), the note before or after the first view, holding again after blur
      or hidden, a page-caused panel held at open and by its first focus and not freed by any note but exactly `{userOpened: true}`,
      a separate window never listens, Up/Down do nothing without rows. `decide.test.ts`: "a separate window is always held (8.3c)"
      and "a panel the user opened skips the hold (8.3c)" (8): own opening held and the same request shown again by the user not
      (save, create, sign-in), a pick always held, a panel that attaches while `openPopup` still runs held (first opening and the
      reopen after Unlock), a failed opening taken back (part 2: kept, the test now checks that the next attach is held once), an opening whose page never attached holds the next attach, refused attaches
      (extension tab, frame, content script, other extension) use nothing up, a request with `userOpened`/`presentation` fields and a
      newer request are still held, the count is per window. `manifest.test.ts`: the shortcut, and the Firefox manifest carries it.
    - **E2E added:** Chrome `e2e/run.ts`: a keyboard pick in the badge's panel (Enter during the hold fills nothing, Down focuses
      the first row, Down then Enter fills, the panel closes) and the hold check now "held (not greyed out)". Firefox
      `e2e/firefox/run.ts`: the same keyboard pick; the save offer shown again from the toolbar icon has Save free at once (7 ms after
      its text showed, in the first run); the waiting passkey sign-in shown again from the toolbar icon is not held and Down, Enter
      signs in; the held checks read `aria-disabled`. Not possible headless in Chrome: a user opening (no toolbar click, the
      shortcut unassigned and not reachable through CDP), so Chrome's no-hold path is covered by the unit tests only.
    - **Mutation checks** (one change at a time, the named test file, original text restored, checksums of both source files checked
      afterwards; helper `mut.py`), 23 in all, **23 caught**. `decide/index.ts` (with `decide-page.test.ts`): hold never on (10 tests
      fail); `act()` without the `held` check, i.e. click and Enter (5); row click bypassing `act` (3); Enter/Space bypassing `act` (3);
      `repeat` allowed (1); `preventDefault` only after the repeat check (1); Space's key up not stopped (1); the user-opened flag
      ignored (2); `blur` not counted as away (1); hidden not counted as away (1); any truthy `userOpened` accepted (1); the note
      listened to in a separate window (1); focus on open moved to the first row or primary button (7); Down from outside going to
      the last row (4); the hold greying again with `disabled` (10). `background/decide.ts` (with `decide.test.ts`): own opening not
      counted (8); every panel told it was user-opened (8); never told (3); a failed `openPopup` not taken back (1; obsolete since
      part 2, which keeps the count on purpose); the note also to a
      separate window (1); counted only after `openPopup` returned (1: the reopen-after-Unlock test); one count for all windows (1).
      `manifest-firefox.ts` (with `manifest.test.ts`): the Firefox manifest dropping `commands` (1).
    - **Checks run 2026-11-13** (one background command with the cargo environment override, in this order, all exit 0): `bunx tsc
      --noEmit` pass; `bun test` 378 pass, 0 fail (356 before: +22); `bun run build --e2e`; Chrome 157 headless `e2e/run.ts` 51 checks
      pass (47 before), `e2e/passkeys.ts` 28 pass; `bun run build --firefox`, `bun run build --firefox --e2e`; Firefox 160
      `e2e/firefox/run.ts` 156 pass (151 before); `bun run build`, `bun run build --firefox`; `web-ext lint` 0 errors, 0 notices,
      0 warnings. An earlier run of the same e2e (first pass) also passed everything but the Firefox check "dist-firefox differs from the
      tested build only in the shadow-root mode", because the normal `dist-firefox` was still the old build: build `--firefox` before
      the Firefox e2e. In both runs the "Enter during the hold" e2e checks ran (the hold was still on when Enter arrived). Rust not
      touched, not run. `SLOW=1` not run.
    - **Decisions beyond the plan:** (1) the manifest keeps `Ctrl+Shift+L` although Chrome 157 on Linux does not assign it (the user
      can choose a key at `chrome://extensions/shortcuts`; README says so); `Alt+Shift+L` would be assigned by both browsers here, for
      the lead to decide. (2) Up/Down move only between the rows, not onto the buttons, and do not wrap. (3) The focus on open stays on
      a safe button also in a panel the user opened. (4) A user-opened panel holds again after it once lost the focus or was hidden.
      (5) A panel whose background opening never attached makes the next attach in that window held (no time window).
    - **What a page can now do that it could not before:** nothing new that acts. The hold no longer greys the controls, which is
      only visual. A page cannot make a panel skip its hold: the signal is the background's own count of its `openPopup` calls and
      reaches the panel only on the panel's own port; every way a page has to get a panel shown (badge, save offer, passkey call, a
      newer request) goes through `openPopup` and is counted. The one thing a page could do is make the next user-opened panel hold
      once (a page-caused panel the user closes before its page connects leaves its count), which only costs the user 0.6 s.
    - **Open / not verified:** a real person's toolbar click and shortcut in Chrome (headless cannot click the toolbar, the shortcut is
      unassigned there and not reachable through CDP: Chrome's user-opened path is unit-tested only); a real keyboard in either
      browser's panel (Firefox keys go through the panel's text input processor, Chrome's through puppeteer into the popup page);
      whether a screen reader announces `aria-disabled` during the hold; whether `openPopup` can ever show a panel and still throw
      (assumed not: the failures measured in 8.3b are checks before showing); Windows (Chrome/Edge: shortcut assignment and the
      openPopup-to-attach order not measured there); the popup (part 2).
    - **Part 2 (2026-11-13, coded by an agent; all its checks pass; checked by the lead).**
    - **The shortcut is `Alt+Shift+L`** (user decision of 2026-11-13, from part 1's measurement: Chrome 157 on Linux leaves
      `Ctrl+Shift+L` empty, both browsers assign `Alt+Shift+L`). `manifest.json` `commands._execute_action.suggested_key` is
      `{default: "Alt+Shift+L"}`; the `mac` entry went (LocalPass ships no macOS build). `manifest-firefox.ts` carries it unchanged.
    - **The popup concern holds** (checked against the code): `openPanel` points the tab's popup at the panel (`setToolbar`), then
      awaits `checkPanelTab` (`tabs.get`, `windows.get`, and in Chrome `runtime.getContexts` and `windows.getLastFocused`) before
      `action.openPopup`. Chrome resets a tab's popup on every cross-document navigation (measured in 8.3b, item 4 of its
      measurements), and a save passes `checkPanelTab` after a navigation (no site check), a passkey or pick after a same-site one;
      the page chooses when it navigates. `openPopup` then shows `popup.html`. A second path needs no navigation: a panel whose
      request ended before its page connected replaces itself with `popup.html` (`toPopup` in `decide/index.ts`), inside the
      popup surface the background opened. In both, the old popup's later "first Fill focused" would have made Enter fill in a
      moment the page chose.
    - **Built:** the popup asks the background who opened it, as a panel does. It connects a port named `popup:<its window id>`
      (`POPUP_PORT` in `shared/messages.ts`; the window from `chrome.windows.getCurrent()`), and `Decisions.attachPopup` answers it
      only for the manifest's popup page shown as a toolbar popup (`sender.id` this extension, `sender.url` exactly the popup URL,
      no `sender.tab`, no `frameId`, a numeric window id), else disconnects and takes nothing. It takes one of the window's own
      openings (`ownOpens`) if there is one (then says nothing: held), otherwise posts `PanelNote {userOpened: true}`. In the popup
      (`popup/index.ts`): acting controls (every button and the two switches) are held for `timing.holdMs` (0.6 s, the panel's
      value) after each view and each return to the window, with `aria-disabled` and no greying; a capture-phase `click` listener
      on the document drops every click on an acting control while held (also the browser's own activation by Enter or Space),
      Enter/Space key downs on an acting control are cancelled while held or when repeated, Space's key up while held. Only
      exactly `{userOpened: true}` on the popup's own port ends it; then nothing is held and the first Fill of the tab's own site
      gets the focus (also when the note arrives after the view, unless the focus is already on an acting control). Without the
      note the focus goes to `main` (focusable, acts on nothing). As in the panel, after a `blur` or a hidden page, a return holds
      again and the focus is not put on a Fill again. Rows of "Show all my logins" never get the focus.
    - **Inline instead of `alert`/`confirm`:** a login saved for another site first opens a step in its row ("Saved for a different
      website than <host>. LocalPass will ask for <check> before it fills it here.", **Fill anyway** and **Back**, Back focused so a
      second Enter does not confirm; Back restores the Fill and focuses it); the app's own user check is unchanged. A failed fill is
      shown in its row (`textContent`, class `error`), a failed Unlock under the Unlock button, a failed change of the browser's
      password saving in that setting's note. `alert`/`confirm` are no longer called anywhere in `src/`.
    - **Part 1's signal, changed to the safe side:** `openPanel` no longer takes its count back when `openPopup` throws; the next
      opening in that window is held once (0.6 s). Comment on `ownOpens` updated.
    - **E2E harness changes:** the popup page opened in a tab is now held 0.6 s too (it never gets the note: it has a `sender.tab`), so
      every "Connect" click in it waits for the hold (`panelClick` in `harness.ts`, `run.ts`, `app.ts`; `clickWindow` in
      `firefox/run.ts`, `firefox/assisted.ts`), and `passkeys.ts` waits before clicking the passkeys switch (also after
      `bringToFront`, a return to the tab). Puppeteer's `waitForFunction` on that background tab needs `polling: 100` (its
      default polls on animation frames, which a background tab never runs: the first run timed out there). Firefox harness:
      `toolbarPopup()` finds `popup.html` in a toolbar panel as a `panel:` context.
    - **Tests:** `popup.test.ts` (28, 12 before): `alert`, `confirm` and `prompt` are stubs that record and throw, checked empty after
      every test; listeners of earlier imported popups are removed after each test (as in `decide-page.test.ts`). New: the port name
      `popup:20`; the user's popup focuses the first own-site Fill and Enter fills it (`force: false`); a late note moves the focus
      there, but not away from a control the user moved to; a key repeat never fills; no focus for other-site rows or without own
      logins; after a blur a return holds again and a new view (`brokerState`) does not focus a Fill; a background-shown popup
      (no note) focuses nothing that acts, marks Fill `aria-disabled` without `disabled`, and Enter/Space/click on Fill and
      "Show all my logins" do nothing during the hold, the switches are held too, after the hold a click works and the focus stays
      off; only exactly `{userOpened: true}` frees it (6 other notes); the inline other-site step (text, Back focused, Enter on Back
      goes back and refocuses Fill, Fill anyway sends `force: true`); a failed fill shown in its row as text (markup not parsed),
      a failed Unlock under its button, a failed privacy change in its note. The two platform-wording tests read the inline text.
      `decide.test.ts` (85, 79 before): the failed-opening test now expects the next attach held once; new "the normal toolbar popup
      is told who opened it (8.3c)" (6): no own opening gives the note; an own `openPopup` that showed `popup.html` after the
      tab's popup was reset gives none, the popup takes the count and the user's next panel gets the note; a panel refused because
      its request ended leaves the count to the popup; a failed `openPopup` keeps the count for the popup; refused popup ports (in a
      tab, a frame, a content script, another extension, a decision page, a query on the URL, window ids `abc` and empty) are
      disconnected and take nothing; the count is per window. `manifest.test.ts`: `Alt+Shift+L` without `mac`, Firefox the same.
    - **E2E added:** Chrome `e2e/run.ts` (58 checks, 51 before): `commands.getAll()` gives `_execute_action` `Alt+Shift+L`; the popup
      page in a tab focuses no Fill and is held; a fill into a tab gone to `about:blank` shows "this page can't be filled" in the
      row with no dialog; the normal popup opened by `action.openPopup()` from the extension's popup tab (not the background:
      what the background sees for a user opening, the only one headless Chrome allows) focuses the first Fill with nothing held,
      Enter fills alice, the popup closes. Firefox `e2e/firefox/run.ts` (161 checks, 156 before): the shortcut is `Alt+Shift+L`;
      a real click on the toolbar button opens the normal popup, the first Fill has the focus with nothing held, Enter (through the
      panel's text input processor) fills alice, the popup closes. Not possible headless: a background `openPopup` that shows the
      normal popup (the navigation race is not deterministic): unit-tested only.
    - **Mutation checks** (one at a time, original text restored and checked by `mut.py`, checksums afterwards), 21 in all, **21
      caught**. `popup/index.ts` with `popup.test.ts`: focus on a Fill whatever the opener (4 tests fail); focus also after `away`
      (1, after the test added for it); hold ignoring `away` (1); any truthy `userOpened` accepted (1); the click guard off (3);
      repeats allowed (1); the other-site row filling without the inline step (4); Back not focused (1); the "show all" list
      focusing its first Fill (1); the switches not held (1); a late note always taking the focus (1, after the test added for it).
      `background/decide.ts` with `decide.test.ts`: the failed `openPopup` taken back again (2); popup always told (5); popup reading
      the count without taking it (4); popup in a tab accepted (1); with a `frameId` (1); URL prefix instead of exact (1); `Number`
      on the window id (1); another extension's sender (1); the `popup:` port not routed (5). `manifest.json` with
      `manifest.test.ts`: the key back to `Ctrl+Shift+L` (2). Not mutated on purpose: the keydown `held` part of the popup's
      Enter guard (the capture `click` guard stops the same activation, so that mutant would survive by design; the two layers are
      belt and braces).
    - **Checks run 2026-11-13** (one background command with the cargo environment override, in this order, all exit 0): `bunx tsc
      --noEmit` pass; `bun test` 400 pass, 0 fail (378 before: +22); `bun run build --e2e`; Chrome 157 headless `e2e/run.ts` 58 checks
      pass, `e2e/passkeys.ts` 28 pass; `bun run build --firefox`, `bun run build --firefox --e2e`; Firefox 160 `e2e/firefox/run.ts`
      161 pass; `bun run build`, `bun run build --firefox` (so `dist/` and `dist-firefox/` are the normal builds); `web-ext lint`
      0 errors, 0 notices, 0 warnings. **Shortcut measured in both browsers' throwaway profiles** through `commands.getAll()` with the
      final manifest: `_execute_action` is `Alt+Shift+L` in Chrome 157 and Firefox 160 (now an e2e check in both runs). Rust not
      touched, not run. `SLOW=1` not run.
    - **Decisions beyond the plan:** the key is `Alt+Shift+L` with no `mac` entry (user decision); a failed `openPopup` keeps its
      count; the normal popup takes part in the count and is held like a panel (0.6 s after each view and each return, buttons and
      the two switches, no greying) unless the background says the user opened it; only then does the first own-site Fill get the
      focus (not after the popup once lost the focus, never a row of "Show all my logins"); without the note the focus goes to
      `main`; the other-site step focuses Back, not "Fill anyway"; a failed Unlock no longer redraws the popup (the error stays under
      the button; a state change from the background redraws it anyway).
    - **What a page can now do that it could not before:** nothing new that acts. Before part 2 an own `openPopup` that showed the
      normal popup (after a navigation reset the tab's popup, or a panel turned into the popup) gave a popup whose buttons acted at
      once; now that popup is held and focuses nothing that acts, and so is a user-opened popup in the rare case a stale count is
      left (a page can cause that by making a panel open and get closed before it connects, or an `openPopup` fail: the cost is one
      0.6 s hold, nothing acts). The window id in the port name comes from the popup page itself (an extension page); a content
      script cannot open a port that passes `attachPopup` (its sender has a tab and a web URL), so it cannot take or skip a count.
    - **Open / not verified:** a real person's keyboard and toolbar click in Chrome (headless: the user path is simulated with
      `openPopup` from an extension page; the shortcut itself was not pressed in Chrome); Firefox's shortcut was not pressed in this
      part either (the toolbar click opens the same popup, part 1 showed both open it); a background `openPopup` that really shows
      the normal popup in a browser (the navigation race) is covered by unit tests only; whether `windows.getCurrent()` in a
      Chrome toolbar popup always names the window of the toolbar (worked in the e2e: the note arrived); screen readers and
      `aria-disabled` on the switches; Windows (Chrome/Edge: shortcut assignment not measured there).
    - **Follow-up (lead's review of part 2, 2026-11-13, coded by an agent).**
    - **Found (held, checked against the code):** `toPopup` (`decide/index.ts`, from `start()` when the first `decisionInfo`
      answers `notFound`, and from `keepAlive`) replaced a panel with `popup.html`. A panel that the background's own
      `openPopup` showed (at a moment the page chose, e.g. a passkey request) and that attached has used up that window's
      `ownOpens` count in `attach()`. If its request then ended (the page aborts it) and `toPopup` won the race with the port's
      `onDisconnect -> window.close()` (`close()` first awaits `windowReady`, and a linger delays the disconnect), the popup's
      `attachPopup` found no count and posted `{userOpened: true}`: no hold, and the first Fill focused at a moment the page chose.
    - **Fix:** `toPopup` loads `popup.html?from=panel`; the popup reads `from=panel` from its own `location.search`
      (`fromPanel`) and then never opens its `popup:<window>` port (`listen()` returns), so it never gets a note: held 0.6 s after
      each view and return, no Fill focused, like any popup the background showed. `attachPopup`'s exact-URL rule is unchanged,
      so such a port would be refused and take no count anyway (test added). Only extension pages choose the popup's URL, and
      the marker can only take trust away. Other URL comparisons checked: `clearToolbar` compares the tab's popup setting with
      the panel URL, `defaultPopup` is the manifest's, `panelShown` compares `getContexts` with the panel URL: none is affected.
      The Firefox harness's `toolbarPopup()` now also finds `popup.html?from=panel`.
    - **Tests:** `popup.test.ts` (+4, 32): with `?from=panel` the popup never asks (no port) even where the fake background
      answers `{userOpened: true}` on any port, stays held, focuses no Fill, Enter and click fill nothing; a note is not
      believed; after the hold a click works and the focus stays off the Fill; without the marker or with another `from`
      (`toolbar`, `panelx`) the popup still asks and focuses its first Fill. `decide-page.test.ts` (+1): the 8.3a stale-panel
      test now expects exactly `.../popup.html?from=panel`; new: a panel whose request ends while shown (keep-alive) turns into
      the marked popup. `decide.test.ts`: `popup.html?from=panel` added to the refused popup senders (disconnected, takes nothing).
    - **E2E (`e2e/firefox/run.ts`, 166 checks, 161 before):** there was no Firefox e2e check of a stale panel turning into the
      popup. Tried: pointing the tab's popup at `decide.html?id=gone&presentation=panel` and clicking the toolbar button; in both
      runs Firefox closed the panel (refused port, `onDisconnect`) before `toPopup` ran, so that stays an `info` line, not a
      check. Checked instead: the tab's popup set to `popup.html?from=panel` and opened with a real toolbar click (the background
      has no count there, so it would tell an unmarked popup the user opened it): it opens in the toolbar panel, carries the
      marker, its Fill buttons are `aria-disabled`, after 0.8 s no Fill has the focus, Enter fills nothing. The user-opened popup
      check that follows still passes (first Fill focused, Enter fills).
    - **Mutations** (helper `mut.py`, one at a time, original text restored and checksums checked): `toPopup` without the marker
      (caught by `decide-page.test.ts`, 2 tests); the popup ignoring the marker, i.e. `if (fromPanel) return;` removed (caught by
      `popup.test.ts`, 3 tests); the marker read as `from=toolbar` (caught, 4 tests). **3 of 3 caught.**
    - **Checks run 2026-11-13** (one background command with the cargo environment override, in this order, all exit 0): `bunx tsc
      --noEmit` pass; `bun test` 405 pass, 0 fail (400 before: +5); `bun run build --e2e`; Chrome headless `e2e/run.ts` all
      checks pass, `e2e/passkeys.ts` all pass; `bun run build --firefox`, `bun run build --firefox --e2e`; Firefox
      `e2e/firefox/run.ts` 166 pass; `bun run build`, `bun run build --firefox`; `web-ext lint` 0 errors, 0 notices, 0 warnings.
      (A first run with the stale-panel attempt as a check failed exactly that check, as described above.)
    - **Not verified:** the real race (a panel attached, its request ends, `toPopup` wins) in a browser: not reproducible
      headless; covered by unit tests.
    - **Checked by the lead on 2026-11-13.** Read parts 1, 2 and the follow-up against the code (snapshots taken before each agent
      started). The review of part 1 found that a failed `openPopup` gave its count back, which rests on the unverified assumption
      that a failed call never shows a panel; part 2 changed it to keep the count. The review of part 2 found that a panel which had
      attached (using up the count) could turn into the normal popup (`toPopup`) and be told it was user-opened; the follow-up closed
      it (`popup.html?from=panel` never asks and is always held). The lead also confirmed the part 2 concern (an own `openPopup` can
      show the normal popup). Re-run by the lead after part 1 (378 unit tests, Chrome `e2e/run.ts` 51, `e2e/passkeys.ts` 28, Firefox
      `e2e/firefox/run.ts` 156, all passing) and after the follow-up, with the cargo environment override: `bunx tsc --noEmit` pass;
      `bun test` 405 pass, 0 fail; Chrome 157 headless `e2e/run.ts` 58 checks pass, `e2e/passkeys.ts` 28 pass; Firefox 160
      `e2e/firefox/run.ts` 166 pass; `bun run build` and `bun run build --firefox` ok; `web-ext lint` 0 errors, 0 notices, 0
      warnings. The shortcut key `Alt+Shift+L` is the user's choice of 2026-11-13 (asked by the lead after part 1 measured that
      Chrome 157 on Linux leaves `Ctrl+Shift+L` unassigned).
- [x] 8.4 Unlock and confirm without losing the browser (app, Linux).
    - **Coded by agents on 2026-11-13** (app side, then the remap and Chrome-reopen follow-ups and the assisted-script fixes);
      **checked by the lead** (code read against the snapshots, the full check set re-run after the first part and after both
      follow-ups: Rust 379 tests passed / 0 failed with fmt and clippy clean, extension 420 unit tests, Chrome headless
      `e2e/run.ts` 63 checks and `e2e/passkeys.ts` 28 checks, Firefox headless `e2e/firefox/run.ts` 166 checks on a clean run,
      normal builds and `web-ext lint` 0/0/0; the lead also re-ran the extension checks after the script fixes); assisted runs 1
      and 2 done, see the status line at the end of the entry.
    - **Built:** `app/src-tauri/src/prompt_window.rs` (new, `cfg(target_os = "linux")`): `PromptWindows<W: MainWindow>` with a
      `Prompt` RAII guard per broker prompt. The first prompt to open reads the window (`is_visible`, `is_minimized`,
      `is_focused`) and stores `Hidden`/`Minimized`/`Showing`; nested and overlapping prompts only count up. When the last one
      ends (answered, refused, timed out, or its future dropped: the guard's `Drop`), `decide()` restores: hidden → `hide()`,
      minimized → `minimize()`, on screen → nothing; nothing at all when the span never raised the window (quick unlock) or the
      user opened the window themselves meanwhile (`show_main_for_user`, i.e. tray icon, tray "Open", starting the app again;
      only for that span). `broker.rs`: `AppHost::prompt_window(kind)` for `unlock`, `verify_user` (non-Windows),
      `confirm_overwrite`, `approve_pairing`; the pairing takes its guard before its own `unlock`, so there is no hide between
      the unlock and the pairing dialog. The guards are declared before the dialog guards (`OpenApproval`), so the dialog
      closes first, then the window hides. Off Linux `PromptWindow` is a wrapper whose `raise()` is the old `show_main`:
      Windows and Android behave as before.
    - **Which browser waits:** `Host::unlock(&self, client: &str)` (trait and all four impls). `ensure_unlocked` gets the
      `ClientRecord` and passes `display_text(&client.name)` (control and invisible/bidi characters dropped, trimmed, 60 chars);
      its queueing and `unlock_failures` logic are unchanged. Pairing names are now cleaned with the same `display_text` (before:
      control characters only, no bidi filter, no trim), so the pairing dialog gets the cleaned name too. The app's
      pairing passes `prompt.name` to its own unlock. `AppHost` keeps `UnlockWaiting` (newest waiting client shown, a version
      that grows on every change), with a `Waiting` RAII guard that adds/removes and emits `UiEvent::UnlockWaiting {version,
      name}`. The unlock screen asks `broker_unlock_waiting` on mount and listens to `lp-unlock-waiting` (App.tsx re-dispatches
      it); the higher version wins, so neither an event sent before the screen listened nor a slow first answer is lost.
      It shows "<name> is waiting: unlock LocalPass to continue." as JSX text (`role="status"`).
    - **Instrumentation (Linux, prompt log `prompts.log`, target `lp_prompt`):** once per start `display backend` (backend
      inferred like GTK 3: `GDK_BACKEND`'s first entry, else `wayland` if `WAYLAND_DISPLAY`, else `x11`; plus the raw
      `GDK_BACKEND`, `XDG_SESSION_TYPE`, `XDG_CURRENT_DESKTOP`) and the raise method; per prompt `browser prompt opened; window
      state found` (kind, visible, minimized, focused, before), `raising the main window` (how), `main window focus changed
      during a browser prompt` (focused, ms since the span opened; from `WindowEvent::Focused` of `main`), `the user opened
      the window...`, `browser prompt ended; window restored` (kind, outcome, ms, restore). No site, user or client names.
      Debug builds only: `LOCALPASS_RAISE=remap` = hide, 100 ms pause, unminimize, show, set_focus; anything else (and every
      release build) = `present` = unminimize, show, set_focus. (This changed after run 1: the default, in release builds too, is
      now `remap`, see the sub-entry "Change after run 1" below.)
    - **Found in the sources (tao 0.37.1, tauri-runtime-wry 2.12.1):** `is_minimized` is only GDK's `ICONIFIED` window state
      (`connect_window_state_event`); xdg-shell has no minimized state event, so on Wayland it should stay false (GTK's C source
      is not on this laptop: from knowledge, not read). Then a minimized window reads as `Showing` and is left alone: it is
      never hidden into the tray. `is_visible` is the GTK `visible` property (false after `hide`, true while minimized).
      `set_focus` is `gtk_window_present_with_time(GDK_CURRENT_TIME)` and is skipped when the window is minimized or not yet
      visible at the moment the request is handled (`show` and `set_focus` are separate messages; tao handles one per GTK
      iteration, so `show_all` should run first). On Wayland, `present` asks the compositor through xdg-activation without a
      user-input serial, which Mutter may refuse ("is ready" notification): this is what the measurement decides.
      Getters wait for the main thread, so `PromptWindows` reads the window only from runtime threads; the main thread (tray,
      window events) touches only atomics and a small mutex never held while waiting.
    - **Tests:** `prompt_window` 12 unit tests (hidden → hide, on screen stays, minimized → minimize never hide, nested, overlapping
      in either order, dropped future, user opened meanwhile and only for that span, quick unlock never raised, state read once per
      span, remap passed through, the decide table); `broker::tests` +1 (newest waiting name, versions); `lp-broker`
      `tests/server.rs` +1 `the_unlock_prompt_is_told_which_client_is_waiting` (the connection that just paired, another client
      logging in with a different hello name, a fresh connection; a bidi/control name cleaned at pairing; a stored name with a
      bidi override cleaned again before `unlock`).
    - **Checks run 2026-11-13** (one background command, cargo environment override, all exit 0): `cargo fmt --all -- --check`;
      `cargo clippy --workspace --all-targets -- -D warnings`; `cargo test --workspace --exclude lp-winauth --no-fail-fast` (all
      pass, 0 failed); `cargo build -p lp-broker --examples`; `cd app && bunx tsc --noEmit && bun run build`. `extension/` not
      touched, not run.
    - **Mutations** (helper script, one at a time, original restored and sha256 checked), **14 of 15 caught**: restore after the
      inner of nested/overlapping prompts (3 tests); ignore the user's opening (1); hide a window that was on screen (3);
      hide a minimized window (2); restore although never raised (2); a dropped guard does not end (9); the user's opening
      sticks across spans (1); minimized not read (1); state re-read by a later prompt (2); waiting shows the oldest (1);
      removing does not bump the version (1); broker passes no name (1); broker passes the raw stored name (1); pairing name
      cleaned as before (1). **Survived:** the app's pairing passing `""` to its own unlock (`approve_pairing`): `AppHost` needs a
      real `AppHandle`, so it is covered only by reading and the assisted check.
    - **Assisted measurement (open):** `LOCALPASS_PROFILE=dev2 bun run tauri dev` (and once with `LOCALPASS_RAISE=remap` in
      front), Firefox via `extension/e2e/firefox/assisted.ts`. Cases: app in the tray (hidden), app open but behind the
      browser, app minimized. Each case: an unlock from the panel, "Confirm it is you", a "Replace?", a pairing on a locked
      vault. Watch `~/.local/share/app.localpass/profiles/dev2/prompts.log`: `display backend` (expect `backend=wayland`), per
      prompt `window state found` (does `minimized=true` ever appear on Wayland?), `raising ... how=`, the `focus changed ...
      focused=true ms=` line after the raise (none = the raise did not get the focus), and `window restored restore=Hide` with
      the browser then focused again (does the panel reopen by itself?). Also check the unlock screen shows "Firefox extension
      is waiting" and drops it when the request ends.
    - **Not verified:** Windows and Android builds (only read: the non-Linux `PromptWindow`, `PromptKind` with `dead_code`
      allowed off Linux, `verify_user`'s `cfg(not(windows))` block on Android); the GUI app was not started; GTK's Wayland
      behaviour for `ICONIFIED` and focus is from sources and knowledge, not measured.
    - 8.4 assisted scripts (2026-11-13, an agent; extension e2e only, no extension source changed): `extension/e2e/firefox/assisted.ts`
      (extended) and `extension/e2e/assisted.ts` (new, Chrome; `BROWSER=chromium|brave|vivaldi|edge`), shared code in `extension/e2e/assist.ts`;
      `e2e/harness.ts` `BrowserSetup` takes `{ appExe, profile }` for an assisted run (helper = the app binary, headed, real
      `XDG_RUNTIME_DIR`, `defaultViewport: null`; HOME, XDG folders and user data dir stay throwaway) and exports `browserKind`.
      Steps: A pairing, B/C "Show all my logins" with the window hidden/open, D/E the panel's Unlock round trip with the app hidden in
      the tray / open behind the browser, each followed by y/n questions (window in front, cursor in the password field, GNOME
      "is ready" notification, window hidden again or kept, browser back in front, panel back) and the new `prompts.log` lines
      (`~/.local/share/app.localpass/profiles/<profile>/prompts.log`, printed as is; `LP_PROMPTS_LOG` overrides). The script sees: the
      decision pages (Firefox: `h.find("decide.html")`, panels through the chrome scope; Chrome: `decide.html?` targets), their text,
      the Unlock press (a capture listener the script adds to each panel writes `localStorage["lp-assist-unlock"]`, read from the
      control tab, so it survives the panel closing), the vault status (the popup's `state` from a background popup tab), the fill
      (the test page's fields) and the toolbar badge; it prints a timeline and "Unlock -> panel closed / vault unlocked / panel back /
      filled". The answers are printed at the end as a list for this file. Locking the vault is the person's job (the protocol has
      `lock`, but the extension has no message for it). Run: terminal 1 `cd app && LOCALPASS_PROFILE=dev2 bun run tauri dev`;
      terminal 2 `cd extension && bun run build --firefox --e2e && bun run e2e/firefox/assisted.ts`, or
      `bun run build --e2e && bun run e2e/assisted.ts` (Chrome; click into its window once, GNOME may open it without focus); both
      take `[profile] [app binary]` (default `dev2`, `target/debug/localpass`). Checked: dry start without the app stops with "the app
      is not listening" before a browser opens (both); the drivers and the watcher were run headless against the dev host from a
      scratch script (lock, Unlock in the panel, panel closed, Unlock answered, panel back, pick, fill: both browsers);
      `bunx tsc --noEmit`, `bun test` (405 pass), `e2e/run.ts`, `e2e/passkeys.ts`, `e2e/firefox/run.ts` pass. Not verified: a headed
      run (not opened by the agent); whether the person can use Chrome's toolbar panel in the puppeteer window.
    - **Assisted run 1 (2026-11-13)**, GNOME on Wayland, the user at the screen, the real dev app (`LOCALPASS_PROFILE=dev2`). Run
      order (from `prompts.log`): Chrome first with the app started without `LOCALPASS_RAISE` (= `present`), 13:15-13:40 UTC; then
      Firefox with `LOCALPASS_RAISE=remap`, 13:45-13:53 UTC. Full log: `~/.local/share/app.localpass/profiles/dev2/prompts.log`.
      - Lead's reading of `prompts.log`, time from the prompt opening to `focused=true` of the main window:
        - present, window hidden in the tray: 67-94 ms (8 prompts), every one then `restore=Hide`.
        - present, window open behind the browser: 2789-4062 ms (4 prompts): the person's click, not automatic; the person saw
          GNOME's "LocalPass is ready" notification instead (Chrome C and E).
        - remap, window hidden: 171-212 ms (6 prompts), every one then `restore=Hide`.
        - remap, window open behind the browser: 143 ms (1 prompt, Firefox E); the person still saw a "LocalPass is ready"
          notification.
        - remap during a pairing that first needed an unlock (13:45:56): the second raise (the pairing dialog after the unlock)
          blinked the window that already had the focus: `focused=false ms=6161`, `focused=true ms=6260`.
        - Firefox C was meant to start with the window open but the log shows `before=Hidden` (the window was in the tray): it
          repeats B. Its answers "window stayed open: no, browser back by itself: no" with `restore=Hide`: probably the terminal
          (where the person answers the script) got the focus back, since the script sends B/C's request itself; not a confirmed
          defect.
      - Checked against the full log by the app agent (2026-11-13): the reading holds. The counts are a little higher because the log
        also has the unscripted tries of 13:15-13:35: present/hidden 10 spans at 67-109 ms, present/behind 10 spans at 1293-6157 ms
        (none automatic), remap/hidden 7 spans at 171-212 ms, remap/behind 1 span at 143 ms. The blink (13:45:56) and Firefox C's
        `before=Hidden` are as read. One more fact: at 13:23:07.030 a prompt that opened 1.6 ms after the previous one's
        `restore=Hide` read `visible=false focused=true` (GTK still called the just-hidden window active), and its `present` then
        logged `focused=false` and `true` again (58 and 109 ms).
      - Firefox, verbatim summary of `extension/e2e/firefox/assisted.ts`:
        - 8.4 assisted, Mozilla Firefox 160.0, 2026-11-13, profile dev2, LOCALPASS_RAISE=remap, app /home/nimda/projects/localpass/target/debug/localpass
          - A pairing:
            - seen by the script: paired
            - Did the app's pairing approval come to the front by itself? **yes**
            - Did the browser come back to the front by itself afterwards? **yes**
          - B confirm, window hidden:
            - seen by the script: reply after 7.2 s: {"ok":true,"logins":8}
            - seen by the script: prompts.log: 2026-11-13T13:46:35.413550Z  INFO browser prompt opened; window state found kind="verify" visible=false minimized=false focused=false before=Hidden / 2026-11-13T13:46:35.413786Z  INFO raising the main window kind="verify" how="remap" / 2026-11-13T13:46:35.589711Z  INFO main window focus changed during a browser prompt focused=true ms=176 / 2026-11-13T13:46:42.610966Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=7197 restore=Hide
            - Did the LocalPass window come to the front with "Confirm it is you"? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **no**
            - After you finished in the app, did the LocalPass window go away again (back to the tray)? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **yes**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - C confirm, window open:
            - seen by the script: reply after 5.4 s: {"ok":true,"logins":8}
            - seen by the script: prompts.log: 2026-11-13T13:48:10.676924Z  INFO browser prompt opened; window state found kind="verify" visible=false minimized=false focused=false before=Hidden / 2026-11-13T13:48:10.677032Z  INFO raising the main window kind="verify" how="remap" / 2026-11-13T13:48:10.856806Z  INFO main window focus changed during a browser prompt focused=true ms=179 / 2026-11-13T13:48:16.046331Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=5369 restore=Hide
            - Did the LocalPass window come to the front with "Confirm it is you"? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **no**
            - After you finished in the app, did the LocalPass window stay open? **no**
            - Did the browser come back to the front by itself afterwards (no click needed)? **no**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - D unlock, app hidden in the tray:
            - seen by the script: filled; Unlock -> first panel closed at once (< 0.4 s); Unlock -> vault unlocked 5.2 s; vault unlocked -> panel back at once (< 0.4 s) (panel); Unlock -> filled 7.2 s (includes your pick in the panel)
            - seen by the script: prompts.log: 2026-11-13T13:52:18.432794Z  INFO browser prompt opened; window state found kind="unlock" visible=false minimized=false focused=false before=Hidden / 2026-11-13T13:52:18.433276Z  INFO raising the main window kind="unlock" how="remap" / 2026-11-13T13:52:18.631578Z  INFO main window focus changed during a browser prompt focused=true ms=198 / 2026-11-13T13:52:23.653793Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=5220 restore=Hide
            - Did the LocalPass window come to the front with the unlock screen? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **no**
            - After you finished in the app, did the LocalPass window go away again (back to the tray)? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **yes**
            - Did the LocalPass panel come back in the browser by itself, showing the login? **yes**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - E unlock, app open behind the browser:
            - seen by the script: filled; Unlock -> first panel closed at once (< 0.4 s); Unlock -> vault unlocked 5.4 s; vault unlocked -> panel back at once (< 0.4 s) (panel); Unlock -> filled 8.4 s (includes your pick in the panel)
            - seen by the script: prompts.log: 2026-11-13T13:53:41.168507Z  INFO browser prompt opened; window state found kind="unlock" visible=true minimized=false focused=false before=Showing / 2026-11-13T13:53:41.169013Z  INFO raising the main window kind="unlock" how="remap" / 2026-11-13T13:53:41.311522Z  INFO main window focus changed during a browser prompt focused=true ms=143 / 2026-11-13T13:53:46.390115Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=5221 restore=Nothing("it was on screen")
            - Did the LocalPass window come to the front with the unlock screen? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **yes**
            - After you finished in the app, did the LocalPass window stay open? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **no**
            - Did the LocalPass panel come back in the browser by itself, showing the login? **yes**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - Set-up:
            - Which LOCALPASS_RAISE did you start the app with (present, remap, or Enter for none)? **remap**
      - Chrome, verbatim summary of `extension/e2e/assisted.ts`:
        - 8.4 assisted, chrome: Google Chrome 157.0.8103.78, 2026-11-13, profile dev2, LOCALPASS_RAISE=none, app /home/nimda/projects/localpass/target/debug/localpass
          - A pairing:
            - seen by the script: paired
            - Did the app's pairing approval come to the front by itself? **yes**
            - Did the browser come back to the front by itself afterwards? **yes**
          - B confirm, window hidden:
            - seen by the script: reply after 4.8 s: {"ok":true,"logins":8}
            - seen by the script: prompts.log: 2026-11-13T13:36:11.854599Z  INFO browser prompt opened; window state found kind="verify" visible=false minimized=false focused=false before=Hidden / 2026-11-13T13:36:11.854867Z  INFO raising the main window kind="verify" how="present" / 2026-11-13T13:36:11.943939Z  INFO main window focus changed during a browser prompt focused=true ms=89 / 2026-11-13T13:36:16.636944Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=4782 restore=Hide
            - Did the LocalPass window come to the front with "Confirm it is you"? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **no**
            - After you finished in the app, did the LocalPass window go away again (back to the tray)? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **yes**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - C confirm, window open:
            - seen by the script: reply after 9.9 s: {"ok":true,"logins":8}
            - seen by the script: prompts.log: 2026-11-13T13:37:08.812843Z  INFO browser prompt opened; window state found kind="verify" visible=true minimized=false focused=false before=Showing / 2026-11-13T13:37:08.813240Z  INFO raising the main window kind="verify" how="present" / 2026-11-13T13:37:11.809184Z  INFO main window focus changed during a browser prompt focused=true ms=2996 / 2026-11-13T13:37:18.671888Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=9859 restore=Nothing("it was on screen")
            - Did the LocalPass window come to the front with "Confirm it is you"? **no**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **yes**
            - After you finished in the app, did the LocalPass window stay open? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **no**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - D unlock, app hidden in the tray:
            - seen by the script: filled; Unlock -> first panel closed 6.4 s; Unlock -> vault unlocked 6.4 s; vault unlocked -> panel back not seen; Unlock -> filled 16.0 s (includes your pick in the panel); no second panel: the first one stayed open or the fill came another way
            - seen by the script: prompts.log: 2026-11-13T13:38:29.592764Z  INFO browser prompt opened; window state found kind="unlock" visible=false minimized=false focused=false before=Hidden / 2026-11-13T13:38:29.593236Z  INFO raising the main window kind="unlock" how="present" / 2026-11-13T13:38:29.686811Z  INFO main window focus changed during a browser prompt focused=true ms=94 / 2026-11-13T13:38:35.917259Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=6324 restore=Hide
            - Did the LocalPass window come to the front with the unlock screen? **yes**
            - Was the cursor already in the master password field, so you could type at once without clicking? **yes**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **no**
            - After you finished in the app, did the LocalPass window go away again (back to the tray)? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **yes**
            - Did the LocalPass panel come back in the browser by itself, showing the login? **no**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **none**
          - E unlock, app open behind the browser:
            - seen by the script: filled; Unlock -> first panel closed 18.5 s; Unlock -> vault unlocked 9.1 s; vault unlocked -> panel back not seen; Unlock -> filled 34.3 s (includes your pick in the panel); no second panel: the first one stayed open or the fill came another way
            - seen by the script: prompts.log: 2026-11-13T13:39:55.361292Z  INFO browser prompt opened; window state found kind="unlock" visible=true minimized=false focused=false before=Showing / 2026-11-13T13:39:55.361664Z  INFO raising the main window kind="unlock" how="present" / 2026-11-13T13:39:58.151140Z  INFO main window focus changed during a browser prompt focused=true ms=2789 / 2026-11-13T13:40:04.396408Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=9035 restore=Nothing("it was on screen")
            - Did the LocalPass window come to the front with the unlock screen? **no**
            - Was the cursor already in the master password field, so you could type at once without clicking? **no**
            - Did GNOME show a "LocalPass is ready" notification (instead of, or before, the window)? **yes**
            - After you finished in the app, did the LocalPass window stay open? **yes**
            - Did the browser come back to the front by itself afterwards (no click needed)? **no**
            - Did the LocalPass panel come back in the browser by itself, showing the login? **no**
            - Anything else you saw (wrong window raised, a second prompt, a delay)? **no answer**
          - Set-up:
            - Which LOCALPASS_RAISE did you start the app with (present, remap, or Enter for none)? **none**
      - **Change after run 1 (2026-11-13, an agent, app side; not yet checked by the lead).** `app/src-tauri/src/prompt_window.rs`,
        `app/src-tauri/src/broker.rs`:
        - **Default on Linux is now `remap`, in release builds too** (`mode_from`: a release build ignores `LOCALPASS_RAISE`). Each
          raise reads the window (`seen()`) and `steps()` picks: on screen with the focus (`focused && visible && !minimized`) →
          **nothing** (no blink; `visible` is part of it because of the stale `visible=false focused=true` above); on screen without
          the focus, or minimized → **re-map**: `hide`, 100 ms pause, `unminimize`, `show`, `set_focus`; hidden → `unminimize`,
          `show`, `set_focus` with no hide and no pause (a hidden GTK window is unmapped already, so its `show` is the fresh map a
          re-map aims at; the extra hide + pause only added time: 171-212 ms against 67-109 ms, and the hidden window got the focus
          with every method).
        - **No blocked runtime thread:** `Prompt::raise` is `async`; the pause is `tokio::time::sleep` between the hide and the show
          (the broker awaits `window.raise().await`; off Linux `PromptWindow::raise` is an `async fn` around the old `show_main`, so
          Windows and Android behave as before). Order: every `WebviewWindow` call off the main thread posts one message to tao's
          event channel (`send_user_message` → `EventLoopProxy::send_event`, an unbounded crossbeam FIFO; tauri-runtime-wry 2.12.1),
          and tao (0.37.1) turns `set_visible`/`set_minimized`/`set_focus` into requests on one glib FIFO channel, so hide, show
          and focus are handled in the order sent. One caveat, read in tao: `set_focus` checks `get_visible()` when its message is
          handled and drops the request while the window is not yet visible; tao handles one user event per GTK iteration and the
          show's GTK request is normally dispatched in between, so this is very likely but not guaranteed.
        - **A re-map cut off in its pause** (the client leaves while the window is unmapped: the broker drops the future): the span
          remembers `unmapped`, and the last prompt's end shows the window again without the focus unless it is to be hidden anyway.
          The end log line has `unmapped=`.
        - **Variants for the next assisted run** (debug builds, `LOCALPASS_RAISE=`): `remap` (the default), `present`, and the new
          `remap-no-focus` (the same re-map without the final `set_focus`). The log line `raising the main window` now carries
          `how`, `steps` (`Nothing`, `Show { focus }`, `Remap { focus }`), `visible`, `minimized`, `focused`; a re-map adds `main
          window shown again after the re-map focus=`.
        - **Tests:** `prompt_window` 18 unit tests (was 12; the old ones now run on the default `remap`): a focused window is never
          re-mapped by any method; a window behind the browser is re-mapped in order (hide, show with or without focus), `present`
          only shows; a hidden window is shown with no hide first, also with a stale `focused=true`; the nested pairing does not
          blink; the pause does not block (a 1 h pause polls `Pending` at once) and a re-map cut off in its pause shows the window
          again, without focus; a cut-off re-map of a window that was hidden stays hidden; the `steps` table; `mode_from`
          (release always `remap`).
        - **Checks:** one command, cargo environment override, 2026-11-13: `cargo fmt --all -- --check` pass; `cargo clippy --workspace --all-targets -- -D warnings` pass; `cargo test --workspace --exclude lp-winauth --no-fail-fast` pass (34 test binaries, 379 tests, 0 failed); `cd app && bunx tsc --noEmit` pass; `bun run build` pass. `extension/` not touched, not run.
        - **Mutations:** 15 of 15 caught (helper script, one at a time, original restored, sha256 checked): no focused check (3 tests); focused check without `visible` (2); without `!minimized` (1); a hidden window re-mapped too (10); always `set_focus` (3); `present` re-maps (3); a cut-off re-map never shown again (1); shown again even when it is to be hidden (1); `unmapped` never cleared (3); never set (1); a release build reads `LOCALPASS_RAISE` (1); debug default `present` (1); re-map without the hide (7); without the pause (2); `raised` not set (9).
        - **Next assisted run:** the same cases with each of `remap` (no variable), `LOCALPASS_RAISE=remap-no-focus` and, if needed,
          `present`, mainly E (window open behind the browser) and a pairing on a locked vault. Compare: does the window come to the
          front and take the typing focus by itself; does the "LocalPass is ready" notification appear (if it appears with `remap`
          and not with `remap-no-focus`, the final `set_focus` causes it and can go, provided the window still gets the focus);
          does the pairing dialog after its unlock blink (expected no: `steps=Nothing` in the log); `ms=` of `focused=true`.
        - **Not verified:** a GUI run (not started by the agent); Windows and Android builds (only read: the off-Linux
          `PromptWindow::raise` is now `async`, its callers await it in code that compiles on every target).
    - **Chrome: panel reopen after Unlock (2026-11-13)** (an agent, extension only; not yet checked by the lead).
      - **Cause (confirmed in the code):** in Chrome on Wayland the panel stayed open behind the app during the Unlock. So when the
        Unlock answer came, `afterUnlock` (`extension/src/background/decide.ts`) found `d.port` still set and returned ("a panel that
        is still open carries on by itself") without arming `reopenUntil`. The panel closed only afterwards (D: Chrome's window back in
        front; E: the person's click into Chrome), its port's `onDisconnect` ran `panelClosed` → `pend`, and a login pick is not
        pendable, so `pend` cancelled it. `tryReopen` (from `onFocusChanged`) did nothing since `reopenUntil` was 0. Every deliberate
        answer in the panel page (Esc, Cancel/Close, Not now, Never, Use another method, a pick) is already a `decisionAnswer` message;
        the only "port closing only" case is such an answer refused by `authorize` (e.g. `checkPanelTab` says not in front), after
        which the page closes itself.
      - **Built (`decide.ts`):** `Decision.graceUntil`. `afterUnlock(ok)` with the panel still attached sets it to now +
        `reopenMs` (Chromium only; Firefox closes its panel during the Unlock, measured, and keeps its rules). `panelClosed` during the
        grace (not busy/unlocking) turns it into the normal reopen: `armReopen(d, graceUntil)` (the old after-Unlock code, factored
        out; the reopen time is the rest of the original window, not a fresh one), which runs `tryReopen` at once (covers a focus
        event for Chrome's window that came while the port was still open) and once more after `settleMs` (covers the window reporting
        `focused` only a moment later). Ends: at its time (`inGrace` compares with `Date.now()`), at any `decisionAnswer` from the
        decision's own panel page (`answered()`: same extension, exact panel URL, no `sender.tab`, no `frameId`; checked before
        `authorize`, so a refused Esc also ends it; ending it can only mean fewer reopens), when the background hides the panel
        (`hide`: user left), and when the panel closes (the reopen takes over once; the reopened panel's attach clears `reopenUntil`).
        `leftUnlessPanel` treats the grace like `reopenUntil` (a -1 focus change while the panel is still open is not the user
        leaving). The reopen goes through `tryReopen`/`checkPanelTab`/`openPanel` unchanged: same tab active in its focused window,
        same site (a pick on a tab that went elsewhere is cancelled), the background's own `openPopup` (counted in `ownOpens`, so
        held, no `userOpened`). A refused Unlock arms nothing. `extension/src/decide/` unchanged.
      - **What a page can do now that it could not before:** only in Chromium, only after the user's successful Unlock in the
        panel and before any answer there, within `reopenMs`: if the page makes the panel close (e.g. takes the focus), the panel
        opens again once, held, under the `tryReopen` rules, as it already could while the panel closed during the Unlock.
      - **Tests:** `extension/test/decide.test.ts`, new block "a panel still open when the Unlock worked, closed afterwards (Chrome on
        Wayland, 8.4)", 14 tests: closing 0 s / 1 s / 10 s after (clock shifted) reopens a pick, a save, a passkey create and get,
        with the focus event before or after the close, held, once; E (closed while the app is in front, -1 meanwhile, reopened on
        the later focus); the Unlock answer after the close; a -1 focus change during the grace; an Unlock during the `settleMs`
        wait; the window reporting focus late; Esc/Cancel, a pick, Not now, Never, another method end it with no reopen; a refused
        answer still ends the grace; answers from a content script, an extension tab, a frame, another extension or the popup do not;
        another site / inactive tab / another tab / another window cancel the pick, a hidden save does not reopen; the grace ends with
        the reopen time; a late close gets only the rest of it; a reopened panel ends it; a refused Unlock gives none; Firefox
        unchanged. `extension/e2e/run.ts` 5b (Chrome headless, dev host): lock, Unlock in the panel, the list shows, the harness closes
        the panel page → a new panel opens by itself, its rows held, the pick fills; and Esc after the Unlock → closed, no new panel.
      - **Mutations** (one at a time, original restored, sha256 checked): **15 of 15 caught**: no grace set (7 tests); Chromium gate
        dropped (1); `panelClosed` ignores the grace (7); grace never expires (1); answers do not end it (1); `answered` without the
        URL / tab / frame / sender-id check (1 each); `hide` keeps it (1); `leftUnlessPanel` without it (2); no `settleMs` retry (1);
        grace kept after the conversion (2); fresh reopen time on conversion (1); grace also after a refused Unlock (1). A check in
        `onFocusChanged` and one in `attach` were found redundant by mutation (survived) and removed.
      - **Checks:** `bunx tsc --noEmit` pass; `bun test` 420 pass, 0 fail; `cargo build -p lp-broker --examples` pass;
        `bun run build --e2e && bun run e2e/run.ts` all pass (incl. 5b); `e2e/passkeys.ts` all pass; `bun run build --firefox --e2e
        && bun run e2e/firefox/run.ts`: first run "1 check(s) failed" (which one not captured: the run was cut to its last 3 lines), then 3 further runs all
        pass (the Firefox path is unchanged: the grace is Chromium only; looks flaky, not caused here); `bun run build`, `bun run build --firefox`, `bunx web-ext@8 lint -s dist-firefox`
        0 errors, 0 warnings.
      - **Not verified:** a real Chrome on Wayland (the next assisted run: `extension/e2e/assisted.ts` D and E should now show "panel
        back"); Edge/Brave/Vivaldi; whether Chrome reports the window focus at all after E's click (if not, the reopen waits for the
        next tab or focus event, as before).
    - **Assisted run 2 (2026-11-13)**, 15:37-15:42 UTC, GNOME on Wayland, the user at the screen: Google Chrome 157.0.8103.78 with
      `cd extension && bun run build --e2e && bun run e2e/assisted.ts` against `cd app && LOCALPASS_PROFILE=dev2 bun run tauri dev`
      **with no `LOCALPASS_RAISE`**, i.e. the new default `remap` (app built 22:23:33 local, after "Change after run 1"; extension
      `dist-e2e` built 22:37:12 local, after "Chrome: panel reopen after Unlock"). The vault was locked before step A. The
      `prompts.log` lines below were checked against `~/.local/share/app.localpass/profiles/dev2/prompts.log` (identical, 30 lines).
      - **Results per step** (time to `focused=true` from the log):
        - **A, pairing on a locked vault:** the unlock nested in the pairing was raised with `steps=Show { focus: true }` and had the
          focus after 69 ms; the pairing dialog after it logged `steps=Nothing` (the window already had the focus): **no blink**
          (run 1's remap blinked here); then `restore=Hide`. The approval came to the front by itself and the browser came back.
        - **B, "Show all my logins" with the window hidden:** `Show { focus: true }`, focus after 102 ms, the cursor in the field, no
          "LocalPass is ready", `restore=Hide`, Chrome back in front by itself.
        - **C, the same with the window open behind Chrome:** `Remap { focus: true }`, re-mapped and focused after 168 ms (run 1 with
          `present`: 2996 ms, the person's click after a "LocalPass is ready" notification), cursor in the field, **no "LocalPass is
          ready"**; the window stayed open (`restore=Nothing("it was on screen")`, by design) and Chrome did not come back by itself.
        - **D, the panel's Unlock with the app hidden in the tray:** `Show { focus: true }`, focus after 90 ms, `restore=Hide`, Chrome
          back by itself, **the panel came back by itself** 0.2 s after the unlock (run 1 in Chrome: not back), filled.
        - **E, the same with the app open behind Chrome:** `Remap { focus: true }`, focused after 148 ms, cursor in the field, no "is
          ready"; the window stayed open (by design), the person clicked Chrome, and **the panel came back by itself** (panel #2 2.1 s
          after the unlock, the `graceUntil` reopen), filled.
        - Between D and E a "Confirm it is you" at 15:40:56 with the window open (`Remap`, focus after 148 ms, refused or cancelled
          after 3.0 s) was started by the person's own click, not by the script.
        - `minimized=true` never appeared; the window was never minimized in this run.
      - **The person's answers**, the script's output as pasted (verbatim, until the hang; "(script bug)" marks the empty
        `prompts.log` print explained below):
        ```
        LocalPass assisted check: chrome: Google Chrome 157.0.8103.78, 2026-11-13 22:37:13
        app binary (native-messaging helper): /home/nimda/projects/localpass/target/debug/localpass (built 2026-11-13 22:23:33)
          ? Which LOCALPASS_RAISE did you start the app with (present, remap, or Enter for none)? -> (Enter: none)
        >>> A. Pairing
          prompts.log, new lines during A: (pairing kind, nested unlock raised with steps=Show { focus: true }, focused=true ms=69)
          ? Did the app's pairing approval come to the front by itself? y
          ? Did the browser come back to the front by itself afterwards? y
        the vault is unlocked and has a login for http://localhost:42105
        >>> B confirm, window hidden
          reply after 6.2 s: {"ok":true,"logins":8}
          prompts.log, new lines during B: none   (script bug, see above)
          ? Did the LocalPass window come to the front with "Confirm it is you"? y
          ? Was the cursor already in the master password field, so you could type at once without clicking? y
          ? Did GNOME show a "LocalPass is ready" notification? n
          ? After you finished in the app, did the LocalPass window go away again (back to the tray)? y
          ? Did the browser come back to the front by itself afterwards (no click needed)? y
          ? Anything else you saw? (none)
        >>> C confirm, window open
          reply after 3.9 s: {"ok":true,"logins":8}
          prompts.log, new lines during C: none   (script bug)
          ? Did the LocalPass window come to the front with "Confirm it is you"? y
          ? Was the cursor already in the master password field? y
          ? Did GNOME show a "LocalPass is ready" notification? n
          ? After you finished in the app, did the LocalPass window stay open? y
          ? Did the browser come back to the front by itself afterwards (no click needed)? n
          ? Anything else you saw? (none)
        >>> D unlock, app hidden in the tray
            +  0.0 s  watching (vault locked)
            + 11.2 s  decision panel #1 opened
            + 11.2 s  #1 shows: LocalPass localhost LocalPass is locked. Unlock it to see your logins. Unlock Close Esc to close
            + 12.5 s  Unlock pressed in the panel
            + 16.8 s  #1 shows: LocalPass localhost Looking for your logins… Cancel Esc to close
            + 16.8 s  vault: locked -> unlocked
            + 17.0 s  decision panel #2 opened
            + 17.0 s  decision panel #1 closed
            + 17.0 s  #2 shows: LocalPass localhost Fill a login for localhost 123 123 Cancel Esc to close
            + 18.9 s  decision panel #2 closed
            + 18.9 s  page filled (user name "123", password of 3 characters)
          result: filled; Unlock -> first panel closed 4.4 s; Unlock -> vault unlocked 4.3 s; vault unlocked -> panel back at once (< 0.4 s) (panel); Unlock -> filled 6.4 s (includes your pick in the panel)
          ? Did the LocalPass window come to the front with the unlock screen? y
          ? Was the cursor already in the master password field? y
          ? Did GNOME show a "LocalPass is ready" notification? n
          ? After you finished in the app, did the LocalPass window go away again (back to the tray)? y
          ? Did the browser come back to the front by itself afterwards (no click needed)? y
          ? Did the LocalPass panel come back in the browser by itself, showing the login? y
          ? Anything else you saw? (none)
        >>> E unlock, app open behind the browser
            +  0.0 s  watching (vault locked)
            +  6.8 s  decision panel #1 opened
            +  6.8 s  #1 shows: LocalPass Connecting…
            +  7.0 s  #1 shows: LocalPass localhost LocalPass is locked. Unlock it to see your logins. Unlock Close Esc to close
            +  7.9 s  Unlock pressed in the panel
            + 13.2 s  #1 shows: LocalPass localhost Fill a login for localhost 123 123 Cancel Esc to close
            + 13.2 s  vault: locked -> unlocked
            + 15.3 s  decision panel #2 opened
            + 15.3 s  decision panel #1 closed
            + 15.3 s  #2 shows: LocalPass localhost Fill a login for localhost 123 123 Cancel Esc to close
            + 17.9 s  page filled (user name "123", password of 3 characters)
          result: filled; Unlock -> first panel closed 7.5 s; Unlock -> vault unlocked 5.3 s; vault unlocked -> panel back 2.1 s (panel); Unlock -> filled 10.0 s (includes your pick in the panel)
          ? Did the LocalPass window come to the front with the unlock screen? y
          ? Was the cursor already in the master password field? y
          (hang: the next questions were answered in the chat, see above)
        ```
        E's remaining answers, given in the chat (the script never asked them): "LocalPass is ready" notification **no**; the window
        stayed open **yes** (log `restore=Nothing("it was on screen")`); the browser came back by itself **no** (the person clicked
        Chrome); the panel came back by itself, showing the login **yes**.
      - `prompts.log` lines of the run:
        ```
        2026-11-13T15:37:21.998676Z  INFO browser prompt opened; window state found kind="pairing" visible=false minimized=false focused=false before=Hidden
        2026-11-13T15:37:21.998851Z  INFO browser prompt opened while another is open; the window state of the first counts kind="unlock" open=2
        2026-11-13T15:37:22.000440Z  INFO raising the main window kind="unlock" how="remap" steps=Show { focus: true } visible=false minimized=false focused=false
        2026-11-13T15:37:22.067764Z  INFO main window focus changed during a browser prompt focused=true ms=69
        2026-11-13T15:37:27.422566Z  INFO browser prompt ended; another is still open kind="unlock" outcome="unlocked" open=1
        2026-11-13T15:37:27.422852Z  INFO raising the main window kind="pairing" how="remap" steps=Nothing visible=true minimized=false focused=true
        2026-11-13T15:37:29.767322Z  INFO browser prompt ended; window restored kind="pairing" outcome="approved" ms=7768 restore=Hide unmapped=false
        2026-11-13T15:37:57.740765Z  INFO browser prompt opened; window state found kind="verify" visible=false minimized=false focused=false before=Hidden
        2026-11-13T15:37:57.742100Z  INFO raising the main window kind="verify" how="remap" steps=Show { focus: true } visible=false minimized=false focused=false
        2026-11-13T15:37:57.843269Z  INFO main window focus changed during a browser prompt focused=true ms=102
        2026-11-13T15:38:03.972126Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=6231 restore=Hide unmapped=false
        2026-11-13T15:38:42.147225Z  INFO browser prompt opened; window state found kind="verify" visible=true minimized=false focused=false before=Showing
        2026-11-13T15:38:42.149087Z  INFO raising the main window kind="verify" how="remap" steps=Remap { focus: true } visible=true minimized=false focused=false
        2026-11-13T15:38:42.250831Z  INFO main window shown again after the re-map kind="verify" focus=true
        2026-11-13T15:38:42.315870Z  INFO main window focus changed during a browser prompt focused=true ms=168
        2026-11-13T15:38:46.021064Z  INFO browser prompt ended; window restored kind="verify" outcome="verified" ms=3873 restore=Nothing("it was on screen") unmapped=false
        2026-11-13T15:39:41.717424Z  INFO browser prompt opened; window state found kind="unlock" visible=false minimized=false focused=false before=Hidden
        2026-11-13T15:39:41.718128Z  INFO raising the main window kind="unlock" how="remap" steps=Show { focus: true } visible=false minimized=false focused=false
        2026-11-13T15:39:41.807437Z  INFO main window focus changed during a browser prompt focused=true ms=90
        2026-11-13T15:39:45.935567Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=4217 restore=Hide unmapped=false
        2026-11-13T15:40:56.031374Z  INFO browser prompt opened; window state found kind="verify" visible=true minimized=false focused=false before=Showing
        2026-11-13T15:40:56.033204Z  INFO raising the main window kind="verify" how="remap" steps=Remap { focus: true } visible=true minimized=false focused=false
        2026-11-13T15:40:56.134741Z  INFO main window shown again after the re-map kind="verify" focus=true
        2026-11-13T15:40:56.179451Z  INFO main window focus changed during a browser prompt focused=true ms=148
        2026-11-13T15:40:59.061012Z  INFO browser prompt ended; window restored kind="verify" outcome="refused or timed out" ms=3029 restore=Nothing("it was on screen") unmapped=false
        2026-11-13T15:41:59.269632Z  INFO browser prompt opened; window state found kind="unlock" visible=true minimized=false focused=false before=Showing
        2026-11-13T15:41:59.271221Z  INFO raising the main window kind="unlock" how="remap" steps=Remap { focus: true } visible=true minimized=false focused=false
        2026-11-13T15:41:59.371887Z  INFO main window shown again after the re-map kind="unlock" focus=true
        2026-11-13T15:41:59.417864Z  INFO main window focus changed during a browser prompt focused=true ms=148
        2026-11-13T15:42:04.490337Z  INFO browser prompt ended; window restored kind="unlock" outcome="unlocked" ms=5220 restore=Nothing("it was on screen") unmapped=false
        ```
      - **The hang and its fix** (2026-11-13, an agent, `extension/e2e/assist.ts` only): the script hung in E after "Was the cursor
        already in the master password field" (the person's "y" got no response). Cause, confirmed in the code: `Person.line()`'s
        timer, when it fired, set the current handler to undefined whatever wait it belonged to, and `until()` and `cancelLine()` ended
        a wait without clearing its timer; 180 s after E's "waiting for the app to report a locked vault" ended early, its stale timer
        wiped the handler of the question asked then, and the typed line went to nobody (the same could end `pressed()`). Fix: every
        wait owns its timer and handler; ending a wait (a line, its own timeout, `cancelLine`, a newer `line()`) clears its timer,
        and a timer ends only its own wait. Second bug: `confirmTrial` and `unlockRoundTrip` called `log.since()` (which moves the
        mark) and then `log.print(step)` (which read again), so every step after A printed "none" (the summary notes had the lines);
        `print(step, lines)` now prints the lines already read. `Person` takes its input stream (default `process.stdin`, the
        scripts unchanged). Proof without a person or a browser: `extension/test/assist.test.ts` (8 tests: an answer typed after a
        finished `until()`'s timeout would have fired still reaches the question, also after an `until()` skipped by Enter and after
        `cancelLine()`; a stale timer does not end `pressed()`; timeouts and a newer `line()` end only their own wait; the lines
        read are the lines printed; a rotation to `prompts.log.1`). Against the old `Person` the four `Person` tests fail, against the
        old `print` the print test fails; with the fix all 8 pass. Both assisted scripts dry-started without the app stop with "the
        app is not listening" (exit 2) before any browser opens.
      - **Known, not caused by 8.4: a flake of the headless Firefox e2e.** `e2e/firefox/run.ts` sometimes stops right after "a click
        the page made up on the badge opens nothing" with "timed out waiting for a decision window" (`e2e/firefox/harness.ts`
        `decisionWindow`): 1 of 4 runs by the 8.4 agent, 1 of 2 by the lead with the current code, and 1 of 4 with `decide.ts` as it
        was before the Chrome fix (built from a scratch copy; the other three passed 165 checks), so it predates 8.4. Not fixed here.
        **Outcome (2026-11-14):** found and fixed in 8.6 (a badge click lost in Firefox, `showBadge`; sub-entry "8.6 Firefox e2e
        after F1").
    - **Status (2026-11-13): 8.4 done on Linux.** The plan's assisted check, the Unlock round trip from both browsers with the app in
      the tray, passed: Firefox in run 1, Chrome in run 2 (and "Confirm it is you" and a pairing on a locked vault in both). Open:
      the Windows and Android builds of the changed `cfg` paths were not compiled here (Windows behaviour unchanged by design).
      Optional: Firefox E with the new default (Firefox D/E passed in run 1 with the earlier debug `remap`); the Firefox e2e flake
      above.
- [x] 8.5 Firefox install (signed `.xpi` in the RPM) (2026-11-13, coded by an agent; **done in the tree and checked by the lead**
  (night of 2026-11-13, see "Lead's check" at the end of this entry); **open: proven only by the first real release run; Open in the
  real app**).
  - Where: the RPM carries the signed add-on at `/usr/lib/localpass/firefox/localpass-firefox.xpi`, **beside** the Chrome folder
    `extension/`, not inside it (deviation from the plan's wording "into the RPM's `extension/` resources": "Load unpacked" loads
    `extension/` as-is, so an `.xpi` there would become part of the unpacked Chrome extension).
  - How: tauri-build fails on a resource whose file is missing or whose glob matches nothing (`tauri-utils` 2.10.1 `resources.rs`:
    `ResourcePathNotFound`, `GlobPathNotFound`; an existing empty folder is skipped), so `tauri.linux.conf.json` does not name it.
    `release.yml`: the `firefox` job gets an output `signed` (`true` only after Mozilla signed and the file was copied; step id
    `sign`); `linux` now `needs: [check, firefox]`, downloads the `firefox` artifact only when `signed == 'true'`, writes
    `$RUNNER_TEMP/firefox-addon.json` = `{"bundle":{"resources":{"<abs .xpi>":"firefox/localpass-firefox.xpi"}}}` and runs
    `bun run tauri build --config <that file>`; "Collect files" lists the RPM (`rpm -qlp`) and fails when `signed` is true but the
    file is missing, else prints a warning. Upload `if-no-files-found` unchanged (`firefox`: ignore, `linux`: error). The `publish`
    job (checksums, attestations, notes) is unchanged except the RPM row of the notes table ("includes the signed Firefox add-on when
    this release has one"); the `.xpi` stays a release file of its own.
  - App: `commands.rs` `FIREFOX_ADDON`, `firefox_addon_in(resources)` (pure, `is_file`), `firefox_addon(app)` (`resource_dir()`),
    `BrowserStatus.firefox_addon`, new command `open_firefox_addon` (opener's `reveal_item_in_dir`: on GNOME Nautilus'
    `org.freedesktop.FileManager1.ShowItems` with the file selected, fallback the portal's `OpenDirectory`), registered in `lib.rs`,
    `api.ts` `firefoxAddon` + `openFirefoxAddon`. `BrowserExtension.tsx` (Linux): with the file, "pick this file (signed by Mozilla, it
    came with LocalPass):" + path + Open; without it (dev builds, a release without the secrets) the old text pointing at the release.
    README: "Firefox on Fedora Linux" step 1, "Installing and updating" (Fedora), "Making a release".
  - Checks (2026-11-13, cargo override): `cargo fmt --all -- --check` clean; clippy `--workspace --all-targets -D warnings` clean;
    `cargo test -p localpass` 43 passed, 1 ignored (new: `the_firefox_addon_is_found_only_where_the_rpm_puts_it`); `cd app && bunx
    tsc --noEmit && bun run build` pass; the workflow parses (bun `yaml`) and `actionlint` 1.7.7 (downloaded binary) reports nothing.
    RPM locally (`bun run tauri build --bundles rpm`, release, 9 min cold): with `--config` and a placeholder `.xpi` (as CI) the RPM
    lists `/usr/lib/localpass/firefox/localpass-firefox.xpi` and the 12 `extension/` files; without `--config` (stale copy left in
    `target/.../release/firefox` on purpose) the RPM has no `firefox/` entry and the same 12 files; the cargo builds (test/clippy) pass
    with no `.xpi` anywhere. The CI check script against both RPMs: signed RPM + `signed=true` ok, unsigned RPM + `signed=true` fails,
    unsigned + empty warns. Mutations (each caught by the new test): `Some(file)` without `is_file`, `exists()` for `is_file()`, the
    path inside `extension/`. Not covered by a test: the screen's two branches (the app has no UI test set-up; tsc only).
  - Open: the first real release run (download-artifact in the Fedora container, `--config` merge in CI's tauri CLI, the signed
    and unsigned paths); Open in the real app (Nautilus selecting the file); the Windows build of `commands.rs` (the field is always
    null there).
  - Lead's check (night of 2026-11-13): read the `release.yml` diff (the `firefox` job's output `signed` is set only after the `.xpi`
    is copied; `linux` `needs: [check, firefox]`; `download-artifact` pinned like `publish`; the `--config` file adds
    `firefox/localpass-firefox.xpi`; the Build step passes `--config` only when the file exists; "Collect files" lists the RPM and
    fails when `signed` is true but the file is missing, else warns; the notes table's RPM row), `commands.rs` (`FIREFOX_ADDON`,
    `firefox_addon_in` with `is_file`, `firefox_addon` from `resource_dir`, `open_firefox_addon` with the opener's
    `reveal_item_in_dir`, which compiles on every platform and answers `UnsupportedPlatform` on Android per tauri-plugin-opener
    2.7.1's source), the two branches of `BrowserExtension.tsx` and the README lines. Re-ran (cargo override): `cargo fmt -p localpass
    -- --check` ok; `cargo clippy -p localpass --all-targets -- -D warnings` clean; `cargo test -p localpass` 43 passed, 0 failed,
    1 ignored; `cd app && bunx tsc --noEmit` ok. Accepted, with the location beside `extension/` recorded in the plan's "Changes
    (2026-11-14)".
- [x] 8.6 App walk-through, independent review, docs (review, fixes and docs checked by the lead; local walkthrough accepted by the
  user on 2026-11-15, see "RESUME HERE"). The code-only walkthrough and open notes below record the earlier state.
  - 8.6 app walk-through (by code) (2026-11-13, agent; done by reading code, the GUI was not started; **the walk-through of
    the real screens with the user is still open**). Only `app/src/` changed; no Rust touched. **Checked by the lead (night of
    2026-11-13):** read the diffs of `Shell.tsx` (the folder delete's text is true: `Engine::delete_folder` only writes
    `FolderSet {name: None}`), `StrengthMeter`, `ItemDetail`, `Vault`, `Onboarding`, `VerifyDialog`; re-ran `cd app && bunx tsc
    --noEmit` ok, `bun run build` ok, `bun test test/duplicates.test.ts test/scan.test.ts` 13 pass, 0 fail. Of the list below the
    lead confirmed items 1 and 5 in the code and sent 1, 5 and 7 to the small-fix agent; 2, 3, 4 and 6 are for the user.
    - Fixes (each small and local):
      - `screens/Shell.tsx`: deleting a folder now asks first (`ask` from the dialog plugin, as Vault/ItemDetail/Devices do):
        "Delete the folder “X” on all your devices? It holds N items. Items are not deleted with it: they stay in All items,
        outside any folder." (true per `Engine::delete_folder`: it writes `FolderSet {name: None}` only; items keep their
        folder field, the folder is no longer listed, items still show in All items). A failure now shows a toast instead
        of an unhandled rejection. Rename/delete were `svg` elements with `onClick` inside the folder's `<button>` (not
        focusable, no name, nested in a button): now two real `<button>`s with `aria-label` "Rename folder X"/"Delete folder
        X", placed over the count (named group `group/folder`), shown on hover and on `focus-within`, so Tab reaches them.
        Both Lock buttons catch a failed `api.lock()`.
      - `screens/Vault.tsx`: Empty trash catches errors (toast) and is busy while running (no double press); search field
        has `aria-label`.
      - `screens/ItemDetail.tsx`: Move to trash, Delete forever, Restore, favorite star and Open in browser catch errors
        (toast; trash/delete no longer report success after a failure); Show/Hide buttons say Hide when shown.
      - `screens/ItemEditor.tsx`: `aria-label` on the website and custom-field inputs (placeholder only before).
      - `screens/Settings.tsx`: Remove vault catches errors; `aria-label` on the three selects; Releases link catches.
      - `screens/Devices.tsx`: `inviteCancel` rejection caught; `aria-label` on the address field.
      - `screens/Unlock.tsx`, `components/VerifyDialog.tsx`: `aria-label` "Master password"; VerifyDialog shows
        `errorText(e)` instead of `String(e)` ("Error: ..." before). `App.tsx`: same for the app-info toast.
      - `screens/Onboarding.tsx`: on Join, the pairing code keeps the focus (the master password field stole it).
      - `components/StrengthMeter.tsx`: a failed estimate is caught and counts as score 0 (cannot leave an old high score).
      - `components/PasswordGenerator.tsx`, `components/TotpCode.tsx`: Copy failures shown; the length slider has a label.
      - `components/TotpQrTools.tsx`: "Win+Shift+S on Windows" (the hint read as Windows-only on Linux).
      - `screens/BrowserExtension.tsx`: Disconnect button is labelled with the browser's name (no logic change).
    - Larger things for the user (not changed):
      1. `App.tsx:167-175` + `commands.rs:929` `broker_respond`: "Browser connected" is toasted even if the request had
         already ended (`BrokerHost::respond` returns a bool that is dropped). Return it and toast only on true.
         (fixed, see below)
      2. `screens/ItemEditor.tsx:245` (and `Engine::delete_folder`): items of a deleted folder keep its id; the select shows
         "No folder" but a save keeps the stale id. Clear unknown folder ids on load, or have the engine treat them as none.
      3. `screens/Shell.tsx:155` (aside `hidden md:flex`): folders exist only in the wide sidebar; a narrow window or the phone
         cannot open or manage folders. Decide whether the mobile nav or the Vault header gets a folder picker.
      4. `screens/Settings.tsx:165-170`: "Keep running in the tray" on GNOME without the AppIndicator extension hides the
         window with no visible icon (starting LocalPass again shows it through single-instance). A Linux hint, or check.
      5. `components/ui.tsx:219` `Modal`: each open modal listens for Esc on `window`, so one Esc answers "no" to every
         stacked dialog (e.g. VerifyDialog over a pairing request); no focus trap. Stack-aware Esc and a focus trap.
         (fixed, see below)
      6. `screens/BrowserExtension.tsx:164`: Disconnect acts at once (the browser must pair again). Confirm or not.
      7. `screens/Devices.tsx:110`: Add device can be pressed again while `inviteStart` runs; ItemDetail's trash button
         likewise. Busy states, if wanted. (fixed, see below)
    - Checks (2026-11-13): `cd app && bunx tsc --noEmit` clean; `bun run build` built; `bun test test/duplicates.test.ts
      test/scan.test.ts` 13 pass, 0 fail. The built CSS contains the `group/folder` hover and focus-within rules. No new
      unit test: the fixes are UI wiring (covered by tsc only). Not verified: anything visual or by keyboard in the real app.
  - 8.6 small app fixes (after the walk-through) (2026-11-13, agent; items 1, 5 and 7 of the list above; done by code, the
    GUI was not started). **Checked by the lead (night of 2026-11-13):** code read (`broker_respond` -> `bool` via
    `try_state(..).is_some_and(respond)`, `api.ts`, `App.tsx` `respondBrowser`, `modalStack.ts`, `ui.tsx` `Modal`); re-ran `cargo fmt
    -p localpass -- --check` ok, clippy `-p localpass` clean, `cargo test -p localpass` 43 passed, 1 ignored, app tsc and `bun run
    build` ok, the three app test files 21 pass, 0 fail. **Known small gap (lead's reading, not fixed):** the Tab wrap's
    `FOCUSABLE` list in `ui.tsx` also takes hidden controls (`display: none`), so when the last listed control of a dialog is hidden,
    Tab from the last visible one leaves the dialog. Rare; for the user's walk-through.
    - "Browser connected" only when it is true: `commands.rs` `broker_respond` now returns the `bool` of
      `BrokerHost::respond` (false: the request had already ended, or on Linux an approval of a check that needs the
      password; also false when the broker is not running), `api.ts` `brokerRespond` is `invoke<boolean>`. `App.tsx`
      `respondBrowser` toasts "Browser connected" / "Windows passkey provider connected" and dispatches
      `lp-browser-paired` only on true; on false: "That request had already ended, so the browser [or: Windows
      passkey provider] was not connected. Try again." (error toast); a failed call shows `errorText`. Unchanged on purpose:
      `respondOverwrite` and `VerifyDialog`'s cancel (no success message follows either answer). Same command on every
      target (the `broker` module and the command are not `cfg`-gated).
    - Esc and Tab act on the topmost dialog only: new pure `app/src/lib/modalStack.ts` (`createModalStack`, `wrapFocus`);
      `components/ui.tsx` `Modal` registers in one module-level stack while open (latest `onClose` kept in a ref, so a
      re-render does not re-register). One `keydown` listener on `window` in the capture phase (present only while a
      dialog is open): Esc calls only the top dialog's `onClose` and stops the event (so the vault's Esc "stop
      selecting" no longer fires under a dialog), Tab/Shift+Tab wrap inside the top dialog and pull focus in from
      outside. "Top" is the open dialog latest in document order (all share `z-40`, so it is the one drawn on top; in
      `App.tsx` `VerifyDialog` is rendered before the pairing/"Replace?" dialogs, so open order would be wrong there),
      open order when unknown. No other Esc handling exists outside `Modal` (only `Vault.tsx`'s selection Esc;
      `ScanOverlay` is Android's camera view, no Esc).
    - No double presses: `Devices.tsx` Add device is `busy` while `inviteStart` runs; `ItemDetail.tsx` Move to trash,
      Restore and Delete forever share an `acting` busy state (Delete forever from the confirmation through the
      call; Restore moved from an inline promise into `restore`); `Shell.tsx` folder Save is busy and `saveFolder`
      ignores a second call (Save or Enter) while one runs (it could create a new folder twice).
    - Checks (2026-11-13, cargo override): `cargo fmt --all -- --check` clean; `cargo clippy -p localpass --all-targets
      -D warnings` clean; `cargo test -p localpass` 43 passed, 1 ignored; `cd app && bunx tsc --noEmit` clean, `bun run
      build` built; `bun test test/duplicates.test.ts test/scan.test.ts test/modal-stack.test.ts` 21 pass, 0 fail (new
      `app/test/modal-stack.test.ts`, 8 tests). Mutations of `modalStack.ts`, each caught by one failing test: top by
      open order only, the remove function not idempotent, Tab wrapping one control early.
    - Open: the real screens (Esc with stacked dialogs, Tab wrap, the toasts, busy spinners) need the user's look;
      Android target not compiled (only the command's return type changed). Not changed (each behind a native
      confirmation or harmless when repeated): Devices Remove, Settings Export unencrypted and Remove vault, Shell
      folder delete, BrowserExtension Disconnect (a second press can only toast an error).
  - 8.6 review fixes (F1, F2, F4) (2026-11-13, agent; **checked by the lead on 2026-11-14**). The review itself and the reasons
    for each decision: `docs/phase8-security-review.md`. The agent was stopped before it reported (it had waited about 10 hours on a
    check run that had already ended), so its own check results were never seen; its code and this sub-entry are in the tree, and
    the lead re-ran everything:
    - Code read: `index.ts` (`Pending.byUser` stored from `byUser === true`; `offerPending` asks `savedState`/`checkLogin` only
      when `pending.byUser === true`), `decide.ts` `attach` (own-open count taken either way; `first = !d.shown`; `d.shown =
      true`; `userOpened` only when `!own && !first`), `server.rs` `is_invisible` and its test, `test/display-text.test.ts`.
    - Full check set (cargo override): `cargo fmt --all -- --check` ok; `cargo clippy --workspace --all-targets -- -D warnings`
      clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast` all ok, 0 failed; `cargo build -p lp-broker --examples`
      ok; extension `bunx tsc --noEmit` ok, `bun test` 441 pass, 0 fail; Chrome headless `e2e/run.ts` and `e2e/passkeys.ts` all
      passed; normal `bun run build`, `bun run build --firefox`, `web-ext lint` 0/0/0; app tsc and build ok.
    - Firefox headless `e2e/firefox/run.ts` failed twice the same way (97 ok, then "a saved login typed by hand is not offered",
      "... with other case and spaces ...", "a changed password is offered as an update", then a timeout waiting for "Update"):
      those cases sent the sign-in with a page script's `requestSubmit()` (`byUser` false), which F1 deliberately no longer
      checks. Chrome's e2e uses real input and passed. Fixed in the test, next sub-entry.
    - Mutations by the lead on a scratch copy of `extension/` (the tree untouched, files compared byte-identical after): F1 guard
      removed, 4 background tests fail; F1 `byUser === true` -> `!!byUser`, 2 fail; F2 `!own && !first` -> `!own`, 3 decide tests
      fail; F2 `first = false`, 3 fail; F2 own-open count taken only when shown, 5 fail. All caught. F4 (Rust) not mutated; its test
      asserts each added range.
    - F1 (`checkLogin` as a guess oracle): `extension/src/background/index.ts`: the stored capture (`Pending`) keeps
      `byUser` (only a real `true` counts, as for the reconnect); `offerPending` asks the app (`savedState`) only for a
      capture with `byUser === true`. Any other capture (a page's `requestSubmit()`, a submit event, a capture stored by
      an older worker without the field) is offered as before 8.2: never dropped as `unchanged`, never marked `update`.
      A later capture in the tab replaces the earlier one with its own `byUser`; the password step of a two-step sign-in
      uses its own `byUser` (the name from `seenUser` is not required to be user-sent: a page can change its own boxes
      during the user's click or Enter anyway); the fill-hash drop in `storeCapture` is unchanged; `restore` reads only
      `offered` captures and needs no app answer.
    - F2 (a page-staged decision opened by the user's toolbar click acted at once): `extension/src/background/decide.ts`:
      a decision has `shown` (a panel page attached to it once). `attach` takes the own-open count as before (none is
      taken when there is none) and posts `{userOpened: true}` only when the opening was not its own **and** the decision
      was shown before. So a decision's first showing is always held 0.6 s, also from the toolbar icon or `Alt+Shift+L`
      (e.g. a passkey request or save that could not open by itself and waited behind the badge); a reshow after closing
      acts at once; the normal popup is unchanged. A login pick always opens by itself first (its own count), so it was
      held already. A panel that closes before attaching does not count as shown (held again, the safe side); a decision
      recreated after a worker restart (`resume`) is held once again. `CLAUDE.md` bullet "One decision panel" updated.
    - F4 (invisible characters): `crates/lp-broker/src/server.rs` `is_invisible` now covers every Cf character of
      Unicode 17 (checked against Python's `unicodedata` 17.0.0 list) plus U+034F, U+2028/U+2029 and the Hangul fillers
      U+115F, U+1160, U+3164, U+FFA0; the extension's `displayText` (`extension/src/shared/limits.ts`) the same set
      (its `CONTROL` already turned U+2028/U+2029 into spaces), as a `u` regex. New tests: Rust
      `display_text_drops_tags_separators_fillers_and_other_format_characters`, `extension/test/display-text.test.ts`.
  - 8.6 Firefox e2e after F1 (2026-11-14, agent; **checked by the lead on 2026-11-14**: read `showBadge` against `armOpener`,
    which has no timing guard of its own (only "ignore an untrusted click" where required), so keeping the button weakens nothing
    and the click still only asks for the panel, which holds; re-ran extension `bunx tsc --noEmit` ok, `bun test` 443 pass, 0 fail,
    Firefox headless `e2e/firefox/run.ts` passed twice in a row, Chrome headless `e2e/run.ts` 63 ok twice, `e2e/passkeys.ts` passed,
    then the normal `bun run build` and `bun run build --firefox` and `web-ext lint` 0/0/0).
    - Why: after F1 the lead's run of `e2e/firefox/run.ts` failed "a saved login typed by hand is not offered", "... with
      other case and spaces ..." and "a changed password is offered as an update" (shown: "Save this login"), because the
      test sent those sign-ins with a page script's `requestSubmit()`, which F1 rightly no longer checks with the app.
    - Changed (test only, `extension/e2e/firefox/run.ts`, step 4b): new helper `typeAndSend(user, pass, "click" | "enter")`
      types the values with a script and sends the sign-in as the user through WebDriver BiDi `input.performActions` (the
      harness's `clickElement` on Sign in, or `pressKey("Enter")` after focusing the password box); `capturedHere()` reads
      the background's capture for the active tab (`storage.session` `pending:<tab>`, from the extension's popup tab,
      without the password). The "already saved, not offered" cases now use a real click (`alice`) and a real Enter
      (` Alice`); the update case a real click. New checks pin F1 in the real browser: the saved `alice` login and the
      changed `bob` password sent by a page script (`requestSubmit()`) are offered as "Save this login" (not dropped, not
      "Update"), with `byUser: false` and no `update` in the capture; each is answered "Not now" and nothing is saved.
      Unchanged: section 4's save-offer UI cases (new logins, offered either way), "a page's own submit must not
      reconnect" and the two-step cases (all pass as they are).
    - Measured in Firefox 160 (headless, BiDi): a real Enter key in the password box gives a capture with
      `byUser: true` (new login `mia`; `update: false`, i.e. the app was asked and said "new"); a real click on Sign in
      gives `byUser: true, update: true` for the changed password; `requestSubmit()` gives `byUser: false`.
    - Runs (cargo override, `bun run build --firefox --e2e`): run 1 stopped at the known flake after 7 checks (below);
      run 2 **174 ok, 0 failed** (the whole run, with the 8 new checks; the lead's run had stopped at the missing
      "Update" button after 97 ok); runs 3 and 4 (same code, with part 2's temporary instrumentation) 174 ok each.
    - **Part 2: the flake "timed out waiting for a decision window" is found and fixed (a real content-script race).**
      - Cause: when a tab or window gets the focus back, Firefox fires `focus` on the field again; `onFocus`
        (`extension/src/content/index.ts`) asks the worker `findLogins` and then `showBadge`, which removed the badge and
        drew a **new** button. When that happened between the mousedown and the mouseup of a click on the badge, the
        click had no common target and was never dispatched, so nothing opened. In the e2e, `trustedClick` first
        activates the login tab (it was behind the popup tab), which is exactly that case. A person hits it too: a
        click on the badge in a window that did not have the focus (the click itself focuses it) can be lost.
      - Evidence (temporary instrumentation of that step in `run.ts`, now removed: a MutationObserver on the badge's
        shadow root and capture-phase pointer/focus listeners in the page, times in ms after the tab's activation):
        passing runs show `focus u` at 11-34, the badge replaced at 20-48, then pointerdown 3-23 ms later and a click on
        the new badge. The failing run (loop run 9 of 12, old build) shows `28 pointerdown badge muted`, `28 mousedown
        badge muted`, `29 mut -1` / `29 mut +1`, `30 mouseup badge` and **no click**. Also seen without instrumentation
        in full run 1 (stopped after 7 checks).
      - Fix (`extension/src/content/ui.ts` `showBadge`): the same field again (same anchor, button still in the page)
        updates the existing button in place (its muted class and its opener, which the click handler reads through
        the current badge record); only another field (or a removed host) gets a new button. `place` stops when its own
        badge record is no longer the current one. New tests in `extension/test/ui-guard.test.ts`: "the same field
        again keeps its button ..." and "another field gets a new button and the old one goes away". Mutation check (5
        mutations, each caught): no reuse; reuse for any field; class not updated; opener not updated; opener bound at
        creation.
      - After the fix: 12 of 12 loop runs to that step passed with no badge replacement at all (old build: 1 of 12
        failed, plus 1 of 4 full runs here); full Firefox runs 5 and 6 had 173 ok and failed only the step-0 check
        "dist-firefox differs ... only in the shadow-root mode" because `dist-firefox` was still the build from before
        the `ui.ts` change; after `bun run build --firefox`, run 7 **174 ok, all passed**. Chrome `e2e/run.ts` (`ui.ts`
        is shared): 63 ok, all passed. `bunx tsc --noEmit` clean, `bun test` 443 pass (441 + the 2 new), normal builds
        `dist/` and `dist-firefox/` rebuilt, `web-ext lint` 0 errors, 0 warnings.
      - Not verified: the lost click with a real mouse in a real (non-headless) window; Windows; `e2e/passkeys.ts` and
        the Firefox assisted scripts were not run (the change touches only the badge button's reuse).
  - 8.6 docs (2026-11-14, agent; docs only, no code changed). New `docs/phase8-security-review.md` (scope, F1-F5 with severity,
    scenario, decision and outcome, the badge fix, "checked and sound", "not covered"). README: the hold's exception for a request
    the page left waiting, save offers checked only for a sign-in the user sent, a "what a page can and cannot get" note under
    "Security notes and limits", the Linux window behaviour around browser prompts under "Firefox on Fedora Linux". `CLAUDE.md`:
    "Current work", the Linux native-messaging files, the save-offer rule and the badge in "Extension trust rules", new bullets for
    the Linux prompt window (8.4), the signed add-on (8.5) and the dialog stack. The plan: "Changes (2026-11-14)". The plan index row.
    This file: the status lines above and "RESUME HERE".
