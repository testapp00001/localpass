Status: approved 2026-11-12, in progress; state in docs/handoff-phase8.md.

# Phase 8: one protected LocalPass panel in every browser, Chromium on Linux, smoother unlock

## Context

The user is not happy using LocalPass with Firefox and wants Chrome and the well-known Chromium browsers to work on Linux too. A read of the code, docs and this machine found:

- **Firefox, daily use.** No signed build has the toolbar picker, and neither Firefox profile on this laptop has a LocalPass add-on installed, so it is loaded as a temporary add-on that vanishes at every restart. Firefox's native-messaging file points at `target/debug/localpass`, not `/usr/bin/localpass` (RPM 0.2.1 is installed).
- **Firefox, flow.** The picker opens at the toolbar with a 0.6 s wait. "Save this login?" and passkey prompts open as separate 440x600 windows. After Unlock the panel is gone, the app window stays in front of the browser, and the badge must be clicked again. Whether GNOME gives the raised app window focus was never measured (open item from Phase 6).
- **Chrome on Linux is not wired up.** `native_messaging/linux.rs` writes only Firefox's file, the RPM does not ship the extension folder, the Chrome e2e harness is Windows-only, and the Chrome build says "Windows Hello" on Linux (`__FIREFOX__` is used as "is Linux").
- **Save offers after autofill.** `storeCapture` (`extension/src/background/index.ts`) skips a capture only when username and password both equal the last fill. A password-only step (two-step sign-in) captures an empty username, so it is offered. Nothing asks the app whether a typed login already exists, and a one-time code in a `type=password` box counts as a password.
- **Popup.** Uses `alert()`/`confirm()`, says "Chrome and Edge" in Firefox, has no keyboard shortcut.

Decisions (user, 2026-11-12): all four Firefox problems count. The picker is the **toolbar panel in every browser** (a window near the field is not possible on Wayland). **Chrome loses every in-page control that acts** (menu, save bar, passkey dialogs); only the badge that opens the panel stays in a page. A save is offered **only for a new or changed login**.

## Working rules

Same as Phase 7 (`docs/handoff-phase7.md`): one task per fresh chat, no agents except an independent look, builds and e2e in the background, no commit or push unless asked, experiments with `LOCALPASS_PROFILE=dev`/`dev2`, never start the installed `localpass`, tests never touch the real keyring, `~/.mozilla` or `~/.config/<browser>`. The user's uncommitted mold change in `.cargo/config.toml` is left alone.

## Tasks (in this order)

### 8.0 Plan log
Copy this plan to `docs/plans/2026-11-12-phase8-browser-experience.md` with a status line, add the index row, create `docs/handoff-phase8.md` ("RESUME HERE"), point `CLAUDE.md` "Current work" at it.

### 8.1 Chromium browsers on Linux (app and RPM)
- `app/src-tauri/src/native_messaging/linux.rs`: a browser table. Firefox keeps `~/.mozilla/native-messaging-hosts` (`allowed_extensions`). Chromium family gets `<config>/NativeMessagingHosts/app.localpass.host.json` with `allowed_origins: [nmhost::allowed_origin()]` for `google-chrome`, `chromium`, `BraveSoftware/Brave-Browser`, `microsoft-edge`, `vivaldi` (`<config>` = `$XDG_CONFIG_HOME` if absolute, else `~/.config`). A Chromium file is written only when that browser's config folder exists; nothing is created for absent browsers. `install`, `remove`, `is_registered` work per browser; keep the write-then-rename.
- Check against Firefox's source where a Firefox with an XDG profile (`~/.config/mozilla`, the case on this laptop) reads user host manifests; write that location too if it is read.
- `commands.rs` `BrowserStatus`: replace the `chrome`/`edge`/`firefox` booleans with `browsers: [{name, present, registered}]` plus `otherCopy` (a manifest exists but names another exe). Windows fills Chrome and Edge as today. Update `app/src/lib/api.ts` and `screens/BrowserExtension.tsx`: Linux shows the Firefox `.xpi` steps and the Chromium "Load unpacked" folder with Open, lists which browsers are set up, offers "Use this copy" for `otherCopy`, and says Flatpak/Snap browsers are not supported.
- `tauri.linux.conf.json`: `bundle.resources` `"../../extension/dist/**/*": "extension/"` so `extension_folder()` finds it in the RPM.
- `crates/lp-broker/src/nmhost.rs`: `is_allowed_caller` already accepts Chrome's origin argument; add a test for the Linux argv shape.
- `extension/e2e/harness.ts`: run on Linux (browser paths per OS, no `.exe`, host manifest written into the throwaway `--user-data-dir`'s `NativeMessagingHosts`, never the real config). `e2e/run.ts` and `e2e/passkeys.ts` on this laptop's Chrome 157 are the proof and the baseline for 8.3.
- New `extension/src/shared/platform.ts` (`chrome.runtime.getPlatformInfo`): "Windows Hello" on Windows, "your master password" elsewhere; the popup's privacy note names the running browser. Replaces the `__FIREFOX__`-as-Linux checks in `popup/index.ts` and `content/passkeys.ts`.
- README: "Browser extension" covers Chromium on Linux.

### 8.2 Offer a save only for a new or changed login
- Broker: new request `checkLogin {origin, username, password}` → `{status: "new" | "update" | "unchanged"}` from `Engine::plan_save_login`. Never prompts and never unlocks (answers `locked`); extension clients only (`protocol.rs`, `server.rs`, tests). It reveals no more than `fillLogin` already gives a paired extension for a matching site; the 8.6 review checks that.
- `extension/src/background/index.ts`:
  - `storeCapture`: a capture whose password hash equals the tab's last fill is dropped when the username matches **or the captured username is empty**.
  - A username-only step records `seenUser:<tab>` (origin, username, time); a password-only capture on the same origin within 5 minutes uses it.
  - `pendingSave`: when connected and unlocked, call `checkLogin`: `unchanged` drops the capture silently, `update` offers "Update the saved password?", `new` offers "Save this login?". Locked or unreachable keeps today's offer (it cannot be known), and an app without `checkLogin` is treated the same.
- `extension/src/content/detect.ts`: an OTP field of `type=password` is not a login password.
- Tests: `background.test.ts`, `detect.test.ts`, lp-broker; the Firefox e2e gets a two-step sign-in page and a "typed an existing login" case.

### 8.3 One protected panel for every decision, both browsers
The decision page (`src/decide/`, `background/decide.ts`) becomes the only place where the user picks, saves or confirms.

- **8.3a Panel for all four decisions.** `Decisions.present` uses the toolbar panel when `action.openPopup` works (Firefox 149+, Chrome 127+ by feature check; `minimum_chrome_version` → 127) and the separate window otherwise. Lifetimes:
  - choose a login: closing the panel cancels (as now).
  - save: closing is not "Not now". The offer waits for its 2 minute life with a badge on the toolbar icon, whose click reopens it. Not now, Esc and Never answer it.
  - passkey create / sign-in: an outside click leaves the request pending (badge, click to reopen) until Cancel, Esc, "Use another method", a page abort, navigation, or 2 minutes.
  - a panel that closes while an answer is carried out, or while the app's Unlock is on screen, never cancels. After Unlock the panel reopens by itself once the browser window has focus and the tab and origin are unchanged.
  - the asking content script pings every 10 s while its `decide` call is open (the background must not sleep while the panel is closed).
  - authorization stays as it is (exact page URL and id, no `sender.tab`, tab active in the focused window, same origin, re-check after fetching a credential).
- **8.3b Chrome uses it; in-page controls go.** Remove the `__FIREFOX__` branches in `content/index.ts` and `content/passkeys.ts` so both builds call `decide`. Delete `arm`, the IntersectionObserver guard, `showMenu`, `showSaveBar`, `showSheet` from `content/ui.ts`; the badge with `armOpener` stays. `decide.html/css` move to `public/`, `build.ts` bundles `decide` for both. The picker gains two rows the Chrome menu had: "Generate a strong password" (hint from the form, value goes to the content script) and, in Chrome, passkeys for a pending conditional request. Firefox keeps handing conditional requests to the browser.
- **8.3c Picker and popup polish.** Up/Down/Enter in the picker. The 0.6 s hold stays for page-triggered openings but no longer dims the rows, and is skipped when the user opened the panel from the toolbar icon or the shortcut. Manifest `commands._execute_action` with `Ctrl+Shift+L`; the popup focuses the first Fill, so shortcut then Enter fills. Popup `alert`/`confirm` become inline messages.
- Tests: `decide.test.ts`, `decide-page.test.ts` (pending, badge, reopen, keep-alive, new rows, keys), `ui-guard.test.ts` reduced to the opener, `content-passkeys.test.ts` and `popup.test.ts` updated; `e2e/run.ts`, `e2e/passkeys.ts` and `e2e/firefox/run.ts` drive the panel. First step of 8.3b: check that Chrome opens the panel under the harness (headless may not; then headed, or the window fallback for e2e).

### 8.4 Unlock and confirm without losing the browser (app, Linux)
- `app/src-tauri/src/broker.rs`: a prompt raised for a broker client (`unlock`, `verify_user`, `confirm_overwrite`, pairing) notes whether the main window was showing (`main_window_showing`). When the prompt ends and it was not, the window hides again so the browser gets focus back. `cfg(target_os = "linux")` only; Windows is unchanged until it can be tested there.
- The unlock screen says which browser is waiting when a browser raised it.
- Assisted measurement on GNOME Wayland (`LOCALPASS_PROFILE=dev2 bun run tauri dev` + `extension/e2e/firefox/assisted.ts`): does the raised window get focus when hidden, and when open but behind? If not, fix in this order: re-map the window (hide then show); a desktop notification that names the request. Record the result in the handoff.

### 8.5 Firefox install
- `release.yml`: the `linux` job takes the signed `.xpi` from the `firefox` job into the RPM's `extension/` resources; Settings shows that file with Open. Without signing secrets the text points at the release page as today.
- The permanent fix for the temporary add-on is a release (7.7, 0.3.0), built only when the user asks.

### 8.6 App walk-through, independent review, docs
- Walk every app screen in `LOCALPASS_PROFILE=dev bun run tauri dev`; fix small things (known: deleting a folder in `Shell.tsx` has no confirmation and no error handling) and list larger ones for the user.
- One read-only review agent on the 8.2-8.4 diff (the decide surface is now the only one in Chrome; `checkLogin`); check each finding against the code; write `docs/phase8-security-review.md`.
- README "Security notes and limits" and "Firefox on Fedora Linux" rewritten for the single panel; `CLAUDE.md` architecture bullets updated; plan index and handoff closed.

## Verification

```sh
export PATH=$HOME/.cargo/bin:$HOME/.bun/bin:$PATH
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude lp-winauth
cd app && bunx tsc --noEmit && bun run build
cd extension && bunx tsc --noEmit && bun test && bun run build && bun run build --firefox && bunx web-ext@8 lint -s dist-firefox
cargo build -p lp-broker --examples
cd extension && bun run build --e2e && bun run e2e/run.ts && bun run e2e/passkeys.ts          # Chrome on Linux
cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts                   # Firefox
```

Assisted, with the user, per task: 8.1 Set up in the dev app, load `extension/dist` in the real Chrome, pair, fill, save. 8.2 sign in with autofill on a one-step and a two-step site (no offer), type an existing login (no offer), a changed password (update offer). 8.3 pick, save and a passkey in Firefox and Chrome, with the vault locked and unlocked, mouse and `Ctrl+Shift+L`. 8.4 the unlock round trip from both browsers with the app in the tray. Windows (Chrome/Edge with the single panel) can only be checked on the work laptop and stays open until then.

## Changes (2026-11-13)

Things decided while carrying the plan out.

- **Working rules.** Since the user's instruction of 2026-11-12, coding and project docs are done by subagents (Opus at high effort or Sonnet at xhigh) and the main session plans, briefs and checks. This replaces "no agents except an independent look" in "Working rules" above.
- **Machine.** The owner's uncommitted `.cargo/config.toml` lines need `clang` and `mold`, which are not installed. All cargo commands of this phase ran with an environment override and the target folder `target/phase8-check` (see the handoff).
- **8.1.** Firefox reads per-user host manifests only from `~/.mozilla/native-messaging-hosts`, also with an XDG profile (its source forces the legacy folder), so no second location is written. The status also reports a registration that names another copy of LocalPass (`otherCopy`), and Settings offers "Use this copy".
- **8.2.** Beyond the plan, a sign-in the user sends with a real click or Enter may connect to the app so the "already saved?" check can be made. A page load never connects, and a form's submit event never counts as the user's (a page script can trigger a submit the browser marks as trusted; measured in Firefox 160 and Chrome 157). The account name of a two-step sign-in's first step is remembered for 5 minutes per tab. The update wording is "Update the saved password in LocalPass?" with an Update button.
- **8.3a.** A save offer is the one deliberate exception to "the tab must still show the same site": a sign-in is followed by redirects, a save sends nothing to the page, and the panel shows the site of the capture. A login that was fetched while the tab was not in front is held for up to 60 seconds and delivered only when the tab is again the active tab of the focused window on the same site. A save offer is taken over by the next page of the tab and restored after a background restart.
- **8.3b.** Beyond the plan, a content script can no longer get a password, a signature, a save, a generated password or the app's Unlock prompt by a message of its own (those come only as the outcome of a decision taken in the panel), and it learns only whether the site has logins, not their names. Chrome's manifest asks for `tabs` (needed to read the tab's address for the panel check) and `minimum_chrome_version` is 127. Chrome reports its window as unfocused while its own panel has the focus, so for Chromium only an open panel of this very decision counts as focus. Passkeys for a page's pending conditional request are listed in the picker in Chrome only; Firefox keeps handing those requests to the browser.
- **State on 2026-11-13.** 8.0 done; 8.1, 8.2 and 8.3a coded and checked by the lead; 8.3b coded with its automated checks passing and its review open; 8.3c to 8.6 not started; all assisted checks with the user open. Details and next steps: `docs/handoff-phase8.md`, "RESUME HERE".
- **8.3c.** The shortcut is `Alt+Shift+L`, not `Ctrl+Shift+L` (user decision of 2026-11-13: Chrome 157 on Linux leaves `Ctrl+Shift+L` unassigned, both browsers assign `Alt+Shift+L`; no `mac` key, LocalPass ships no macOS build). Up/Down reach only the panel's rows (not its buttons) and do not wrap. A panel's focus stays on a safe button when it opens, also when the user opened it. A panel the user opened holds again after it lost the focus once. The background tells a panel, and the normal toolbar popup, that the user opened it only when it has no pending own `openPopup` in that window (a count per window, no time window; a failed `openPopup` keeps its count, so the next opening there is held once): an own `openPopup` can show the normal popup (Chrome resets a tab's popup on a navigation, and a panel whose request is gone turns into the popup), so the popup is held like a panel and focuses its first Fill only when the user opened it. The popup's confirmation before filling a login saved for another site, and its errors, are inline (no `alert`/`confirm`). A popup that a panel turned into is always held.
- **8.4.** Beyond the plan, on Linux only, the app raises its window by re-mapping it (hide, a 100 ms pause, show, focus) when the window is on screen but lacks the focus; a hidden window is just shown, a focused one is left alone (measured: GNOME on Wayland gives no focus to a `present` of a mapped window and shows "LocalPass is ready" instead). `Host::unlock` gets the waiting client's cleaned name, so the unlock screen names the waiting browser, and pairing names are cleaned like site text. In Chromium a panel that outlived a successful Unlock and closes before an answer reopens once (`graceUntil`). A window that was open behind the browser stays open afterwards (as the plan says), so the person clicks back into the browser.

## Changes (2026-11-14)

Things decided while carrying out 8.5 and 8.6.

- **8.5, where the `.xpi` goes.** The RPM carries the signed add-on at `/usr/lib/localpass/firefox/localpass-firefox.xpi`, beside the Chrome folder `extension/`, not inside it as the plan says: "Load unpacked" loads `extension/` as it is, so an `.xpi` there would become part of the unpacked Chrome extension. `tauri.linux.conf.json` cannot name the file (tauri-utils 2.10.1 fails a build on a missing resource, and dev builds have none), so the release workflow's `linux` job waits for the `firefox` job and, when Mozilla signed the add-on, adds it with `tauri build --config`. Without the signing secrets the RPM has no add-on and Settings points at the release page as before. Proven only by the first real release run.
- **8.6, the independent review** (`docs/phase8-security-review.md`): no High or Medium finding. F1 and F2 (Low) and F4 (Info) were fixed; F3 and F5 (Info) accepted. Two behaviour changes follow:
  - F1: a save offer is checked with the app (`checkLogin`) only for a sign-in the user sent with a real click or Enter. A sign-in a page's own script sends is offered plainly as "Save this login?", even when it is saved already, so the offer cannot tell a page whether a guessed login is in the vault. This narrows 8.2's "`unchanged` drops the capture silently" to user-sent sign-ins.
  - F2: a decision's first showing is always held 0.6 s, also when the user's click on the toolbar icon or `Alt+Shift+L` shows it (a request the page left waiting behind the icon). This changes 8.3c's "skipped when the user opened the panel from the toolbar icon or the shortcut" for that first showing; a reopen after the user closed it acts at once, and the normal popup is unchanged.
- **8.6, the app walk-through** was done by an agent by reading the code (the GUI was not started); the walk-through of the real screens with the user is still to be done, and 8.6 stays open until then. Small fixes made: deleting a folder asks first and reports errors, the folder buttons are real buttons Tab reaches, error toasts instead of unhandled failures on several screens, labels for screen readers, busy states against double presses, Esc and Tab act on the topmost dialog only, and "Browser connected" is shown only when the request was still waiting. The larger items are left for the user (handoff, "RESUME HERE").
- **8.6, the badge.** While moving the Firefox e2e to trusted input after F1, a real cause of its known flake was found: Firefox fires `focus` on the field again when its window gets the focus back, and the badge was redrawn between the mousedown and the mouseup of a click on it, so the click was lost (a person can hit this too). `showBadge` now updates the existing badge for the same field in place.
- **Working rules.** Agents run as Opus at medium effort, at most two at once (user instruction of 2026-11-13). Every long command in an agent brief gets a `timeout`, and the lead checks running processes when a background notice does not come: one agent waited about 10 hours on a check run that had already ended.

## Changes (2026-11-15)

- **Local acceptance.** After the guided walkthrough, the user reported everything worked and requested a local commit. This closes
  8.6's real-screen walkthrough; individual optional test results were not supplied. Fresh automated checks passed (Rust workspace,
  excluding `lp-winauth`: 381 passed, 2 ignored; extension: 443 passed; app: 21 passed, 9 skipped; both TypeScript checks and Rust
  formatting). Browser e2e and clippy were not rerun in this session.
- **Remaining work.** Windows/Android checks and release verification remain open. The folder, narrow-window navigation, Linux tray,
  browser disconnect and hidden-control focus decisions remain undecided; no implementation of those follow-ups was approved.
- **Commit scope.** Record the existing Phase 8 implementation, tests and documentation together on `phase7`. Leave
  `.cargo/config.toml` unchanged. Push, merge, tag and release only when separately requested.
