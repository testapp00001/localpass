# Handoff: Phase 7, secure files, vault tools, faster builds

The approved plan is `docs/plans/2026-10-31-phase7-files-tools-builds.md` (tasks 7.0-7.7, with the design of each). This file holds the
state; do not rely on conversation history. All plans: `docs/plans/README.md`.

## RESUME HERE (state updated 2026-11-15)

**Additional completed task (2026-11-08): Firefox toolbar password picker.** See the results section at the end and
`docs/plans/2026-11-08-firefox-toolbar-picker.md`. Secure-file storage was subsequently removed (2026-11-09). The toolbar changes are included in the local commit requested on 2026-11-09.

**Phase 7 is complete.** Phases 0-6 are done except 5.6 (the Windows final matrix, on the work laptop;
`docs/handoff-phase4-5.md`). The latest release is [v0.3.0](https://github.com/testapp00001/localpass/releases/tag/v0.3.0), published
on 2026-11-15. Phase 7 started from a clean tree at `d4eb731`; secure-file storage (7.6) was dropped at the user's request.

**Branch:** Phase 7 and Phase 8 were merged into `main` through PR #1 (`bf57ddd`). Release commit `34b7788` bumps the versions and
changelog to 0.3.0; annotated tag `v0.3.0` points to that commit. The user explicitly requested the commit, merge and release.
The remaining Phase 8 manual checks and usability decisions are in `docs/handoff-phase8.md`.

Rules (the plan's "Working rules"; the agent rule itself is in `CLAUDE.md`, "Working with agents"):
- One task per fresh chat. 7.0-7.3 are small and independent; do the later tasks in the order of the plan.
- No agents by default. One only for an independent look (audit, review of a finished task, second opinion), briefed with the goal, the files or
  the diff and the questions, not your conclusions; read-only; check every finding against the code before acting.
  *(Note 2026-11-12: this rule changed for Phase 8 at the user's request: subagents now do the coding, one task per agent, and the main session
  plans, briefs and checks. The new rule is in `CLAUDE.md`, "Working with agents", and `docs/handoff-phase8.md`.)*
- Builds and e2e run in the background. Never commit or push unless the user asks. Experiments use `LOCALPASS_PROFILE=dev` (or `dev2`).
  **Never start the installed `localpass` binary, even for `--version`: it is the real app and opens the real profile** (`localpass --version` hung instead
  of printing a version, so it was starting the app; it was stopped and left no process, the vault files were not touched). Tests never touch the real keyring or `~/.mozilla`.
- Shell setup for every command: `export PATH=$HOME/.cargo/bin:$HOME/.bun/bin:$PATH`.
- Keep the plan log current: when a task or phase finishes, update its status in `docs/plans/README.md` and here. An approved plan file is never
  rewritten; append a dated "Changes" section instead.

User decisions for Phase 7 (2026-10-31, also in the plan): secure-file content syncs to every device including the phone; the largest file is 50 MB before
compression; desktop QR sources are a pasted clipboard image (main path), "Scan QR on screen" and an image file; `www.` is ignored when looking for duplicates and
other subdomains stay separate unless the option is on.

## Status

- [x] 7.0 Agent rule, plan history and phase docs (2026-10-31, docs only, nothing committed). `docs/plans/` (index, Phase 6 plan moved with a "Changes" note, Phase 7 plan
  copy), `CLAUDE.md` ("Current work", "Working with agents", "Plan history"), the agent lines in `docs/handoff-phase4-5.md` and `docs/handoff-phase6-linux.md`, Phase 6
  closed in its handoff, this file, two feedback memories. Deviation from the plan text: the old agent line in the Phase 6 plan file was left as it was and a dated
  "Changes (2026-10-31)" section was appended instead, because the plan's own rule forbids rewriting an approved plan.
- [x] 7.1 Faster Rust builds (2026-10-31): branch applied, linker and LTO decided, numbers below. The user confirmed mold is not wanted and the Windows `rust-lld.exe` change was skipped at first and **checked by the user on Windows on 2026-11-01 ("work"), so it is now in `.cargo/config.toml`** (details below).
- [x] 7.2 Firefox decision window: Esc and click outside (2026-10-31, code and automated checks done; **the user confirmed on 2026-11-01 on GNOME that Esc and a click outside close the window**, tested with `extension/dist-firefox` loaded as a temporary add-on in their real Firefox; see the note at the end of this entry).
  `extension/src/decide/index.ts`: each view's way out is the button flagged `dismiss` (Cancel, Close, Not now; the two "request is gone" Close buttons are now `safe` too); Esc
  (plain key only: not a repeat, not during IME composition, no Ctrl/Alt/Meta/Shift) takes it at once, even during the 600 ms hold, and only once (`dismissed` latch). A blur closes the window only for
  `chooseLogin` and `saveLogin`, only after the window has had focus once (`hadFocus`, so a window Wayland opened unfocused is left alone), not while an answer is carried out or the
  app's Unlock is on screen (`working`), and only if `document.hasFocus()` is still false `timing.outsideMs` = 250 ms later. Passkey windows close with Esc only. A muted "Esc to close" line (`.hint`,
  `decide.css`) shows under views that have a way out. Tests: `test/decide-page.test.ts` 12 new (240 in all: Esc incl. during the hold and in every view, ignored keys, hint, waiting view, blur
  closes chooser and save offer, focus returning, never-focused window, blur during carry-out and during Unlock (armed again after), passkey windows, before the decision is known);
  mutation-checked (six guards removed one at a time, each caught). The tests drop the page listeners between pages (`dropListeners`), because every imported page adds listeners to the shared window/document.
  e2e: `e2e/firefox/run.ts` presses a real Esc (`h.pressKey`, BiDi) on the chooser: it closes and fills nothing, the badge then opens a fresh one; 72 checks pass on Firefox 159.0.
  **Firefox 159 needs `MOZ_REMOTE_ALLOW_SYSTEM_ACCESS=1` for scripts in extension pages (the `--remote-allow-system-access` flag alone worked in 158): the harness now sets both**
  (an earlier run failed at its first check with "System access is required"). `bunx tsc --noEmit`, `web-ext lint` (0/0/0) clean; Chrome `dist/` is byte-identical to a build without the change (only the version string differed
  from a 0.2.0 build that was lying around). Consequence to know: clicking outside a save offer counts as Not now, and a save is offered once per capture on Firefox, so a stray click drops that offer (decided in the plan).
  Assisted check (2026-11-01, user): Esc and click outside both close the chooser on GNOME, and the window stays open while the app's Unlock dialog is used.
  **Lesson:** the user's first try used the installed signed XPI (0.2.1 in `extensions.json`), which has no Esc code, and saw nothing change. Before an assisted check of extension code, say which build to load: about:debugging > This Firefox > Load Temporary Add-on > `extension/dist-firefox/manifest.json` (gone after a Firefox restart, same add-on id, so the pairing was kept).
  The installed signed XPI (0.2.2) does not have this yet; it ships with the next release.
- [x] 7.3 Secure notes hidden until shown (2026-10-31, code and build done; **the user confirmed on 2026-11-01 that hide/show works in `tauri dev`**).
  Only for `kind === "note"`; notes on logins and cards are unchanged. `screens/ItemDetail.tsx`: the Notes row shows `••••••••••••` until the Eye button is pressed (`shown.notes`, reset by the
  existing `setShown({})` on item switch; the text then shows with `whitespace-pre-wrap`), plus a Copy button (`api.copyField(id, "notes")`: `Engine::field_secret` already copies any text field, so no
  Rust changed, and the clipboard auto-clears). `components/ui.tsx`: new `SecretTextArea` (Show/Hide `IconButton` inside the box, `spellCheck`/`autoCorrect` off, masked with the new `.mask-text` class in
  `index.css`, `-webkit-text-security: disc`, which WebView2, WebKitGTK and Android WebView support); `screens/ItemEditor.tsx` uses it for a note's body (starts masked, also for a new note, like
  `PasswordInput`). Checks: `bunx tsc --noEmit` and `bun run build` clean; the rule is in the built CSS. The notes text still reaches the webview in `ItemView` (the masking is a display
  guard against shoulder-surfing, not a Rust-side secret like passwords in lists). The app has no component-test setup and none was added for this.
  Not run: the visual pass. Assisted: in `tauri dev` make a secure note with two lines; the detail shows dots, Show reveals the text with its line breaks, Hide and switching to another item hide it again,
  Copy puts the note on the clipboard and toasts "Note copied · clears in Ns"; in Edit the body is dots, Show reveals it, typing and Enter in the masked box work, Save keeps the text.
- [x] 7.4 2FA QR on desktop (2026-11-01, code and automated checks done; **the user ran their assisted passes on 2026-11-01 and reported "all work"** without naming items; on Windows the user later reported that **the QR functions work** (paste, and "Scan QR on screen" counted in that; per-item results were not given, e.g. whether the two-monitor case was tried)).
  Rust, `app/src-tauri` (desktop only: `#[cfg(desktop)]`, the deps sit in the non-Android target section, so the APK gets none of it):
  - `src/qr.rs`: `from_rgba` / `from_gray` / `from_file` -> `Found { uri, issuer, account }` (`Debug` hides the link) or a `QrError` with the user-facing text (no code, not a 2FA code,
    `otpauth-migration://`, HOTP / bad secret via `TotpSpec::parse`, too big, wrong file type). `rqrr` 0.11 on a grayscale copy (transparent pixels count as white). Limits: file 20 MB (read through
    `take`, regular files only), 50 MP checked from the header before decoding, only PNG / JPEG / BMP / WebP by content, not by extension. Pixel buffers and file bytes are `Zeroizing`. When a picture holds
    several codes, the first usable 2FA one wins and the most specific complaint is reported otherwise. `from_screen`: **Windows** GDI copy of the whole virtual screen (all monitors, nothing on disk);
    **Linux** `ashpd` 0.13 Screenshot portal (`interactive`, `modal`; `Ok(None)` when the picker is closed), the returned `file://` path is read with `from_file` and **deleted at once**.
  - `clipboard.rs`: `read_image()` (arboard `image-data`) and `clear_if_image()`. **Measured on GNOME Wayland (2026-11-01): `clear()` alone does NOT remove a picture that a Wayland program
    (Firefox, `wl-copy`, the screenshot tool) put on the clipboard; owning the clipboard with empty text, waiting 300 ms and then `clear()` does** (without the pause it still failed). Text we copied ourselves was
    never affected. So the clear on Linux is set-then-pause-then-clear.
  - `commands.rs`: `totp_qr_from_clipboard`, `totp_qr_from_file(path)`, `totp_qr_from_screen` (the window minimizes, waits 450 ms, captures, comes back); registered in `lib.rs` under `#[cfg(desktop)]`.
    Returns `{ uri, issuer, account, clipboardCleared }`. No capability changes (dialog open and `core:default` events were already granted).
  - New deps: `rqrr` 0.11 (no default features), `image` 0.25 (png, jpeg, bmp, webp; arboard already pulled it with png), `ashpd` 0.13 (Linux; tokio + screenshot), arboard feature `image-data`;
    dev-deps `qrcode` and `tempfile`. 17 new crates in `Cargo.lock`.
  Frontend: `components/TotpQrTools.tsx` wraps the 2FA field on desktop (Android keeps its camera button): "Paste QR image", "Scan QR on screen", "From image file…", Ctrl+V (a window `paste` listener
  that acts only when focus is inside the block **and the paste carries no text**, so pasting a key or link works as before), a picture file dropped on the block (`onDragDropEvent`, position checked
  against the block), "Found: Issuer (account)" while the field still holds the scanned link, errors inline. `lib/api.ts` has the three wrappers. README "Security notes and limits" has a new bullet
  (a screenshot of the QR is the secret; clipboard managers, clipboard history and the screenshot tool's own file may keep a copy).
  Checks: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast`: 334 pass, 0 fail, 2 ignored (321 before + the 13 new; the ignored ones are the assisted clipboard test and one older test); `bunx tsc --noEmit` and `bun run build` clean. 13 unit tests in `qr.rs` (round trips from RGBA, PNG, JPEG, BMP, WebP; a
  small code in a 1920x1080 picture; transparent background; two codes where the first is not 2FA; every error; size and type limits; the portal URI parser), the alpha test mutation-checked (alpha ignored ->
  both alpha tests fail). One `#[ignore]` assisted test, `cargo test -p localpass --lib -- --ignored picture_another_program` (`wl-copy` puts a PNG on the clipboard like GNOME's screenshot tool, then read,
  decode, clear, and `wl-paste --list-types` must show no image): passes here. It replaces the clipboard.
  **Windows: the user reported on 2026-11-01 that the QR functions work there** (before that it was compile-checked only): `from_screen` was built alone against `windows` 0.62 for `x86_64-pc-windows-gnu` (the app crate itself needs a MinGW C compiler for a dependency, so a
  full cross check was not possible); it had never run before that report. Android is untouched (CI's Android job still has to build it).
  Not run (assisted, all in `LOCALPASS_PROFILE=dev bun run tauri dev`, new login, 2FA block): (1) Firefox screenshot "Copy" then Ctrl+V in the field and the Paste QR image button; GNOME PrtSc
  to clipboard (Ctrl+PrtSc / the screenshot UI's copy), also while the window is hidden or another window has focus; the clipboard is empty afterwards ("Nothing is copied" in `wl-paste`) and a text
  key pasted with Ctrl+V still works; (2) "Scan QR on screen" with a QR visible in Firefox: the window steps aside, GNOME's picker appears, a selection fills the field, the window returns with focus, and
  no file is left in `~/Pictures/Screenshots` (find where the portal wrote it; closing the picker leaves the form as it was); (3) a PNG from the file dialog and one dropped on the block (drop events need
  `core:event`, so check it really fires on WebKitGTK and WebView2); (4) Windows: Win+Shift+S then Ctrl+V, and "Scan QR on screen" on two monitors; (5) a non-2FA QR, a Google Authenticator export QR, a
  HOTP QR and a picture without a code give the right message.
- [x] 7.5 Duplicate logins (2026-11-01, done; the user checked the screen in `LOCALPASS_PROFILE=dev bun run tauri dev` with the seed below and said it all worked as expected). Done in the same chat as the fixes after 7.4, at the user's word "start next".
  Engine: `crates/lp-engine/src/duplicates.rs`, `Engine::find_duplicate_logins(include_subdomains) -> Vec<DuplicateGroup>` (re-exported with `DuplicateItem`). Live logins only (not trashed, `kind=login`) with a non-empty password
  and at least one readable website; username trimmed and compared without case (an empty username matches another empty one on the same site), password exactly; union-find over shared
  (username, password, site) so an item with several sites lands in one group; the grouping keys borrow the passwords from the vault state, so no secret is copied. `DuplicateGroup { sites (shared by 2+ copies),
  username (as the newest copy has it), items: [ItemSummary + urls, hasNotes, customFields, updatedBy] }`, newest first; no password, note text, 2FA secret or custom value in the output (a test serializes it and looks).
  The site name comes from the new `lp_core::origin::site_key(saved_url, include_subdomains)`: parsed as `match_login` parses (a shared `parse_saved_url`), scheme ignored, lower case, no trailing dot, one leading `www.` dropped, `:port`
  when not the default, with the option the registrable domain from the public suffix list; IP addresses, `localhost` and single labels keep their host. **Behaviour change in `match_login` (tightening):** `parse_saved_url` no longer
  prepends `https://` to text that already contains `://`, so a saved `https://` alone is not read as the host `https`.
  App: command `find_duplicates(include_subdomains)` (`commands.rs`, registered in `lib.rs`, wrapper `api.findDuplicates`), remembered setting `duplicatesIncludeSubdomains` (`settings.rs`, `api.ts`, default false),
  `screens/Duplicates.tsx` (Shell page `duplicates`, opened by "Find duplicate logins" in a new Settings section "Vault tools"; the Settings tab stays highlighted): a toggle "Count subdomains as the same site", per group a
  radio list (the newest copy is kept by default) with title, websites, "Changed ... on device" and badges (2FA, Notes, N custom fields, Conflict), and "Move the other copies to trash". It asks first, with the system dialog,
  when a copy going to the trash has a 2FA secret, notes or more custom fields than the kept one (`lib/duplicates.ts` `lostWithTrash`); otherwise it trashes at once (the Trash can restore). It reloads on `lp-vault-changed`.
  The screen says "Logins without a website are not checked" (static text; the plan's `Vec` signature has no count).
  Tests: 12 engine tests in `crates/lp-engine/tests/duplicates.rs` (subdomain vs option, www, different passwords/users/case of the password, username case and spaces, empty usernames, trash/other kinds/no site/no password, IP and ports and localhost,
  public suffix list incl. github.io, several sites and transitive grouping, newest first and the extras, no secret in the JSON, locked vault), 1 `site_key` test in `origin.rs`, 5 in `app/test/duplicates.test.ts`.
  Mutation-checked (8 changes made one at a time to `duplicates.rs`/`origin.rs`): 7 caught; the guard `.filter(|p| !p.is_empty())` is not testable through `save_item` (a blank password is stored as no password), it only defends against an
  empty text written by another device. Checks: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast`: 347 pass, 0 fail, 2 ignored (334 + 13);
  `bunx tsc --noEmit`, `bun run build`, `bun test test/duplicates.test.ts` clean. Android compile of the shared Rust is not run (CI).
  Checked by the user on 2026-11-01 ("everything works right as expected"). The assisted pass was: import `/tmp/claude-1000/-home-nimda-projects-localpass/eeeb639a-6cc1-4033-a2e8-a6cdfe5963a5/scratchpad/duplicates-seed.csv` into a `LOCALPASS_PROFILE=dev` vault (Settings > Import), then Settings > Find duplicate logins.
  Expect with the toggle off: two groups, `ann@mail.test` on `example.com` (the root and the www copy) and `admin` on `192.168.1.10` (NAS and NAS again; the :8443 one stays out); with it on: the example.com group gains "Example (sub a)" and a
  `carl` group on `example.co.uk` appears. Trashing with the root kept must warn about notes (the www copy) and, with the option on, about a 2FA secret (the sub a copy). The other password, other user and "Lonely" logins never appear.
- [dropped] 7.6 Secure-file storage: removed at the user’s request on 2026-11-09, including the 7.6a-7.6d implementation. The results below are historical; 7.6e and 7.6f will not be implemented.
- [x] 7.7 Release 0.3.0 (2026-11-15, explicitly requested by the user): published as the latest release at
  https://github.com/testapp00001/localpass/releases/tag/v0.3.0 from `34b7788`, tag `v0.3.0`.
  Release workflow `43704939727` and the release commit's CI `43704939634` passed. Assets: Windows installer, Fedora RPM,
  Android universal APK (ARM64 and x86_64), Chrome/Chromium extension ZIP, Mozilla-signed Firefox XPI, and `SHA256SUMS.txt`.
  All five download hashes matched; all six build attestations verified against the repository, release workflow, exact tag and
  source commit. Android's v2 signature matched the pinned release certificate. The RPM contains a byte-identical copy of the
  signed standalone XPI. Windows installation, Android runtime and the installed app's file-manager Open action were not run here;
  the existing Windows final matrix and Phase 8 manual checks remain separate. No installed app or real vault was started.

## 7.6a results (2026-11-01, this laptop: Fedora 44, WebKitGTK 2.54.2, GStreamer 1.28.8 with libav/openh264/dav1d/fdk-aac, rustc 1.99.1)

The harness is kept in `docs/spikes/7.6a-webview/` (README: what it checks and how to run it on another machine). It ran inside the real app binary with the production CSP;
all spike code was removed from the tree afterwards (`git status` shows only `docs/spikes/`), the `spike` profile folder was deleted, and `target/debug/localpass` was rebuilt without it.

**1. zstd: Go on Linux; the Windows and Android builds are not checked** (no NDK or MinGW here; the first CI Android run after 7.6b adds the `zstd` crate to `lp-engine` is the check,
and `zstd-sys` builds C with the `cc` crate, which the NDK clang handles; if it ever fails, `ruzstd` decodes but only compresses weakly). Measured with the `zstd` 0.13 crate, one level per process,
`set_pledged_src_size` (the window shrinks to the file), 50 MB of source text (`.rs/.md/.toml/.json` from the cargo registry) and 6.3 MB of real PDFs (all the PDFs on this laptop, cat-ed; no bigger real PDF was
at hand, so 50 MB of PDF is an extrapolation of about 8x). Peak RSS is the whole process (the 52 MB input is in it):

| level | text 50 MB: time | size | peak RSS | PDFs 6.3 MB: time | size |
|---|---|---|---|---|---|
| 3 | 0.26 s | 15.3% | 63 MB | 0.03 s | 53.3% |
| 9 | 1.1 s | 12.3% | 73 MB | | |
| 12 | 2.2 s | 12.0% | 102 MB | 0.19 s | 50.9% |
| 15 | 9.9 s | 11.6% | 126 MB | 0.48 s | 50.7% |
| 17 | 14.2 s | 10.8% | 113 MB | 1.4 s | 49.7% |
| 19 | 26.4 s | 10.5% | 146 MB | 3.2 s | 49.3% |
| 22 | 41.6 s | 10.3% | **748 MB** | 6.2 s | 49.3% |

Decompression 0.1 s for 50 MB at every level. **Level 22 buys 2.6% on text and nothing on PDFs for 58% more time and five times the memory; 19 is the knee.** The plan said 22 on desktop and 19 on Android, and the user's wish was
"maximum level"; **open decision for the user: recommend level 19 everywhere** (keep it one constant, `COMPRESSION_LEVEL`, so either answer is a one-line change). A phone is probably 3-5x slower than this laptop
(about 80-130 s for 50 MB of text at 19), so 7.6e must measure and show progress; a lower level on Android is possible.
The probe (level 3 on the first MiB) separates the cases as planned: text 23.6%, PDFs 66.8% (both compressed), already-packed archives and random data 100.0% (stored as `none`; compressing the 50 MB packed
sample at 19 would have cost 16 s for 2.3%). Keep the 3% threshold.

**2. Viewer isolation: Go on Linux (verified); Windows unknown; Android probably No-Go (by reading the code, not tested).**
- Linux/WebKitGTK, verified by running it: a `sandbox="allow-scripts"` frame has an opaque origin (`origin=null`), no `__TAURI_INTERNALS__`, `parent`/`top` access throws `SecurityError`, the backend received **zero**
  calls from it (also from the frame running pdf.js), and the same-origin unsandboxed control frame **did** reach the backend (so the probe can see a leak). Why: Tauri's invoke key and `__TAURI_INTERNALS__`
  are injected into the main frame only (`for_main_frame_only: true`, `tauri-2.12.3/src/manager/webview.rs`), and the key is the only barrier on this platform, because wry reports the *main frame's* URL for any frame's message
  (`wry-0.57.0/src/webkitgtk/mod.rs:735`). The raw script-message handler (`webkit.messageHandlers.ipc`) does exist in the sandboxed frame; a guessed key was rejected (and Tauri prints the expected key to stderr, which web content cannot read).
  `fetch("ipc://localhost/...")` from the frame returned HTTP 500 and ran nothing.
- **Windows (not run; the harness is ready):** wry docs say Tauri's init scripts, invoke key included, are *always* injected into subframes on WebView2 (`wry-0.57.0/src/lib.rs:1029`), and the IPC URL there is the frame's own
  (`args.Source()`). A sandboxed frame served from `http://tauri.localhost/...` would count as local and could call every command. An `about:srcdoc` frame should be treated as a *remote* origin (`is_local_url` is false, so Tauri rejects custom
  commands from it, `tauri-2.12.3/src/webview/mod.rs:2080`), but that depends on what WebView2 reports as the source of a `srcdoc` frame. **The PDF viewer must be a `srcdoc` frame (never a `src=` page of the app) and must stay off on Windows until the harness shows `sandboxed frames reached IPC = NO` there.**
- **Android (not run, no SDK on this laptop):** the bridge is an `addJavascriptInterface` object (`Ipc.postMessage`) available to every frame, it reports the main frame's URL (`webViewClient.currentUrl`), and the init scripts go in with
  `addDocumentStartJavaScript(..., setOf("*"))`, i.e. into subframes too. So a sandboxed frame most likely holds a valid key and is treated as local. **Treat in-app PDF on Android as No-Go (PDFs use "Open with" there, the plan's own fallback) unless an emulator run on `lp_api37` proves otherwise.**
  Images, audio and video do not run third-party script and are unaffected.
- So the plan's rule applies per platform: in-app PDF on Linux now, Windows after its check, Android system app only. One `pdfInApp` capability flag per platform, off unless verified.

**3. pdf.js: Go (Linux).** `pdfjs-dist` 6.4.299 (far above the CVE-2024-4367 floor 4.2.67). Findings that change the 7.6d design:
- Tauri answers asset requests with `Access-Control-Allow-Origin: <main window origin>` (not `*`), and an opaque-origin frame sends `Origin: null`, so **ES modules, module workers and `.wasm` fetches from the frame fail**. What works is **one classic IIFE script**
  (`bun build --format=iife --define 'import.meta.url="file:///viewer.js"'`: pdf.js has a Node-only `import.meta.url` that is a syntax error in a classic script) that contains pdf.js *and* its worker, which registers `globalThis.pdfjsWorker`, so pdf.js runs the "fake worker" on the
  frame's main thread. Bundle: 1.63 MB minified (460 KB main + 1.26 MB worker). So the viewer is not "a second Vite entry" but a separate IIFE build step (`vite build --config` in lib mode, or `bun build`) into `dist/viewer/`, loaded by `<script src>` from a `srcdoc` frame.
- A sandboxed `src=` page from the app (`tauri://localhost/...`) did **not** run its external scripts under `script-src 'self'` (no diagnostic arrived; the cause was not isolated, most likely 'self' not matching an opaque origin). `srcdoc` frames inherit the parent's policy and work. Another reason for `srcdoc`.
- Result: three real PDFs (a form, a font specimen with complex scripts, a 25-page spec) rendered page 1 correctly (dark-pixel check, 130-370 ms each), text content extracted (55/23/33 items), with and without `disableFontFace`. The default (FontFace) works, no `data:` font CSP was needed.
  No `unsafe-eval` and no `wasm-unsafe-eval` were needed. Not covered: JPX/JBIG2 images (they need wasm, which cannot load from the opaque origin; those pages would lack the image), PDFs with embedded script (`enableScripting` stays off), a 50 MB PDF.
  The frame's JavaScript shares the page's main thread in WebKitGTK, so a very heavy page render can freeze the app's UI for a moment.
- **CSP result: only `img-src ... blob:` and `media-src blob:` are needed. `frame-src 'self'` is NOT needed for `srcdoc` (the plan listed it); `script-src` stays `'self'`.** Everything else in the CSP is unchanged.

**4. Blob media in WebKitGTK: Go.** `<audio>`/`<video>` on `blob:` URLs (muted, with `media-src blob:`; without it every file is `NotSupportedError`, with `img-src` lacking `blob:` the image is blocked) played: wav, ogg (vorbis), opus, mp3, m4a (AAC), flac, webm VP8, webm VP9,
webm AV1, mp4 H.264 and mp4 MPEG-4 part 2, MKV (H.264 + Opus). **One unexplained miss:** in the first full run the two MP4 video clips showed `readyState=4` but no `timeupdate` past 0.5 s; in the next three runs (section alone, the deep dive with
a sampled clock, ended events) every file played to the end, so it was not reproduced. The viewer must still show "Open with..." next to the player and on `error`. H.265/HEVC reports no support, as planned.
This laptop has the full GStreamer set; **a stock Fedora without `gstreamer1-plugin-libav`/`openh264` will lack mp3/AAC/H.264**, hence the codec-error fallback is a real path, not a corner case.

**5. 50 MB from Rust to the webview as a raw `tauri::ipc::Response`:** 0.75-0.92 s (debug app), then 80-90 ms for `Blob` + `createObjectURL`; arrives as an `ArrayBuffer`. Fine for the viewers. Not measured on Windows/Android.

**Harness lessons** (for whoever reruns it): a window created with `visible: false` never ran the page (set `visible: true` for the spike); `fetch` of the app's own assets is blocked by `connect-src` (feed bytes through a raw IPC command, as the product will);
a cross-origin script error is reported as "Script error." with no detail, so test-parse bundles with `bun -e 'new Function(code)'` (it caught the `import.meta` problem).

## 7.6b results (2026-11-01, same chat as 7.6a, at the user's word "you can continue")

Nothing is committed (the user commits). Checks: `cargo fmt --all --check`, `cargo clippy --workspace --exclude lp-winauth --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast`:
**395 pass, 0 fail, 2 ignored** (347 before + 48 new). The Android compile of the shared Rust is not run here (no NDK): CI's Android job is the first check of `zstd-sys` for `aarch64-linux-android` and `x86_64-linux-android`.
New crates in `Cargo.lock`: `zstd` 0.13.3, `zstd-safe`, `zstd-sys`, `jobserver` (workspace dep `zstd` with `default-features = false`, used by `lp-engine` only).

What exists now:
- **`lp-core`**
  - `ids.rs`: `BlobId` (32 bytes, SHA-256 of the sealed blob).
  - `model.rs`: `kinds::FILE`; keys `file.name`, `file.mime`, `file.size`, `file.stored_size`, `file.blob` (hex), `file.key` (base64url, **secret**, in `is_secret`), `file.codec`, `file.added_ms`. Numbers are decimal text, like `passkey.created_ms`.
  - `fileblob.rs`: the blob container. Header 26 bytes (`LPF1`, version 1, codec, chunk log2 = 20, 19-byte random nonce prefix), then 1 MiB payload chunks each sealed with XChaCha20-Poly1305,
    nonce = prefix ‖ counter (u32 BE) ‖ last flag, AAD = the header. **`aead` 0.6 has no STREAM module, so the STREAM-BE32 construction is written by hand (about 60 lines).**
    `seal`, `open(key, blob, max_payload)` (the size bound is checked before anything is decrypted), `check_shape` (no key: header plausible, length can be a sealed blob, within `MAX_BLOB_LEN`), `blob_id`, `Codec`.
    Limits: `MAX_FILE_BYTES` = 50 MiB, `MAX_BLOB_LEN` = that + 64 KiB. `open` accepts chunk sizes 1 KiB..16 MiB (so tests can use small chunks); we write 1 MiB.
  - `filetype.rs`: `sniff_mime(name, head)` from magic bytes (png, jpeg, gif, bmp, webp, tiff, heic/avif by ISO brand, pdf, zip, gzip, 7z, rar, mp3/aac, wav, avi, ogg, flac, mp4/m4a/mov/3gp, webm/mkv, text); the name only refines *text* (json, xml/svg, markdown, html) and never turns binary into something else.
    HTML and SVG are text (`Viewer::Text`): nothing is rendered. `viewer_for(mime)` -> `text | image | imageConvert | imageSystem | audio | video | pdf | none`; `clean_name` (no directories, no control or bidi characters, no leading dots, 200 bytes).
    A UTF-16 LE byte-order mark (`FF FE`) is also an MPEG Layer I frame header, so the MP3 rule takes Layer II/III only.
- **`lp-store`**: `blobs.rs` `BlobStore` (folder `<data dir>/files/<hex id>`; `put` checks shape + SHA-256 + cap, writes `<id>.<pid>-<n>.partial`, fsync, rename, fsync of the folder; 0700 folder / 0600 files on Unix; `get` re-hashes and **removes a damaged file** and reports `Corrupt`; `has`, `size`, `ids`, `digest` (SHA-256 of the sorted ids, for the `Hello`), `delete`, `retain(keep, also_keep)`, `wipe`; stale `*.partial` files are removed at open),
  and on `Store`: `wanted_blobs()` / `set_wanted_blobs()` (meta key `files.wanted`, `META_WANTED`, 32-byte ids, sorted). **Sync (7.6c) reads the wanted list through the `Store`; it never needs the engine or a key.**
- **`lp-engine`**: `files.rs`, `COMPRESSION_LEVEL = 19`, `FileInfo { name, mime, viewer, size, storedSize, codec, addedMs, available }` (never the key or the blob id), `FileContent { info, bytes: Zeroizing }`.
  - `Engine::add_file(name, content, title, description)` (50 MB cap, sniff, probe (level 3 on the first MiB; under 3% saving => stored as `none`), zstd 19, seal under a fresh random key, store the blob, then one `ItemSet`), `add_file_path(path, ...)` (checks type and size from the metadata before reading; reads at most cap+1),
    `read_file(id)` (copies the key out of the session, then reads, re-hashes, opens, decompresses with a hard stop at the recorded size and a zstd window limit of 2^27; errors: `FileUnavailable` for a missing or damaged blob), `files_left_out_of_exports()`.
  - The **wanted list and cleanup** (`refresh_files`): the blob ids of all file items (live and trashed) are saved to the wanted list and every other blob is deleted. It runs on unlock, after every catch-up in `with_session`/`catch_up`, after a purge and after an add. **A blob stored for a file whose item is not in the log yet is protected by a `PendingBlob` guard** (`hold_blob`), otherwise the cleanup would take it for an orphan; a failed add removes its blob. A locked vault does no cleanup (it cannot tell what is wanted).
  - `ItemQuery.kind`; **the default list leaves files out, also from favourites; the trash shows them with everything else**; `ItemSummary.file` / `ItemView.file` (skipped when not a file); search also matches `file.name`; `ItemSummary.subtitle` is the file name; `file.*` conflicts are not shown (they would leak the key).
  - `save_item`: a `file` item cannot be created there and an item never changes into or out of a file; title, description (`notes`) and favourite of a file can be edited with it (pass `kind: "file"`).
  - `field_secret` refuses `file.key`; `export_items` (so the plain JSON export **and** the encrypted backup) skips files; `leave_vault` wipes the folder; `lp-cli export` says how many files were left out.
- **Not done in 7.6b (by design, other tasks):** the sync file phase (7.6c), any UI, the `file_*` Tauri commands, `temp_files.rs`, the app's export screen text ("N secure files are not in this export": the engine call exists, the app command `export_json` still returns only the passkey count), Android parts, README bullets (7.6f).

Tests (48 new):
- `lp-core` 16: round trips at chunk boundaries (1 KiB and 1 MiB chunks, 0 bytes included), non-deterministic blobs, wrong key, **every single flipped byte of a multi-chunk blob is caught**, truncation at every chunk boundary and inside chunks, extension, reordered / duplicated / spliced chunks, the payload bound, garbage and cap rejection, a proptest (random sizes and chunk sizes, any cut fails); sniffing of every type, brands, the BOM/MP3 case, name refinement, viewers, name cleaning.
- `lp-store` 8: put/get/has/size/delete, refusing a wrong id or non-blob or oversize, damaged file removed, partial files and strangers, `retain`, the digest, the wanted list, wipe, Unix modes.
- `lp-engine` lib 7 (compression decisions, probe window, zip bomb stops at the recorded size, huge zstd window refused, the pending guard) and `tests/files.rs` 17: round trips in 7 shapes, codec and size, type from bytes + name cleaning, the 50 MB limit (exactly 50 MiB of zeros round-trips; one byte more is refused and stores nothing), `add_file_path` (folder, missing, sparse over-limit file), list/favourites/search/trash/restore, purge removes content at once and empty-trash, orphan cleanup on unlock, wanted list for a locked vault, missing and damaged blob, another blob under this blob's name, nothing secret in the JSON or in `vault.db`, the save guards, exports (plain, backup round trip) leave files out, locked vault stores nothing, a failing add leaves nothing, leaving the vault wipes the folder.
- **Mutation-checked: 18 changes made one at a time (cleanup off, pending not recorded, files in the default list, files exported, key copyable, save guard off, purge not cleaning, leave not wiping, always-available, decompression bound, failed add keeping its blob, no last-chunk flag, header not authenticated, counter not in the nonce, store without hash check on put and on get, payload bound off, wanted list not saved): all 18 caught.**
  Not mutation-testable through the API: the `info.size > MAX_FILE_BYTES` guard in `read_file` and the early `with_session` lock check in `add_file` (the item cannot be forged through the engine; the lock check is redundant with the cleanup on error).
  The proptest wrote a regression seed during the mutated runs; it was deleted (the case passes on the real code).

Facts for 7.6c (sync):
- `Engine` owns the `BlobStore` (`Inner.blobs`) and passes nothing to `SyncService` yet; 7.6c must hand the service an `Arc<BlobStore>` (the service is built in `Engine::start_sync`, look for `SyncService::new`) and let the session read `Store::wanted_blobs()` / `BlobStore::{ids, digest, get, put}`.
- A received blob is checked by `BlobStore::put` itself (shape, SHA-256 against the *requested* id, cap): the sync code only has to ask for ids from the wanted list and respect the budgets.
- The cleanup runs inside `with_session` after a catch-up, with the pending guard; a blob that sync stored for a wanted id is in the wanted list, so it survives. A blob sync stores for an id that the session has not yet learned about cannot happen: ids come from the wanted list, which the unlocked engine wrote.
- A locked device's wanted list is from its last unlock, so a file added on another device after that is fetched only after the next unlock plus the next sync. This is the plan's behaviour; say so in the UI text ("Waiting for this file from ...").

## 7.6c results (2026-11-01, same chat as 7.6a/7.6b, at the user's word "start The next task is 7.6c")

Nothing is committed (the user commits). Checks: `cargo fmt --all --check`, `cargo clippy --workspace --exclude lp-winauth --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast`:
**418 pass, 0 fail, 3 ignored** (395 before + 23 new; the third ignored test is the 50 MiB timing below). The scenario tests were run 12 times in a row (no flake). The Android compile of the shared Rust is not run here (no NDK); the new code uses only std, tokio and `lp-store`.

What exists now (`crates/lp-sync`, plus the engine hooks):
- **Protocol** (`protocol.rs`): `Hello` gains `#[serde(default)] features: Vec<String>` (`"files1"`) and `files_digest: Option<ByteBuf>` (SHA-256 of the sorted held blob ids, `BlobStore::digest`); `PROTO_VERSION` stays 1.
  A test reads a new `Hello` with a copy of the 0.2.x struct and the reverse. New `SyncMsg`s: `FilesWant{ids}`, `FileBegin{id,len}`, `FileChunk{data}`, `FileEnd`, `FilesDone`. Constants: chunk 48 KiB, 1000 ids per `FilesWant`, at most 20 000 ids asked per session, budget 512 MiB per direction, 30 s idle, 20 min for the whole phase.
- **`files.rs`** (new): `FileSync { blobs: Arc<BlobStore>, budget, idle, total }` (`FileSync::new(blobs)` has the defaults; tests lower them), `FileStats`, and the phase. After both `Done`s, if both sides listed `files1` **and the digests differ** (both sides decide from the two `Hello`s, so they agree), each side sends `FilesWant` for the wanted-list ids it does not hold (+ `Done`), the initiator serves what the responder asked for (`FileBegin`, `FileChunk`*, `FileEnd`, ... `FilesDone`), then the responder serves.
  Sending: a blob nobody holds, or whose copy turns out damaged (`BlobStore::get` removes it), is skipped; a repeated id is served once; it stops before going past the budget (the next session carries on). Receiving: only ids that were asked for and only once; the announced size must be 1..=`MAX_BLOB_LEN`; chunks are non-empty, at most 48 KiB, never more than announced; `FileEnd` must match the size; the budget is checked per file; then `BlobStore::put` checks the shape and the SHA-256 against the *asked* id. File reads and writes run in `spawn_blocking`. A blob is held in memory while it arrives (at most 50 MiB + 64 KiB per session and direction).
- **`session.rs`**: the ops part is unchanged (60 s for the whole session) but `run_initiator`/`run_responder` now run the file phase after it with its own limits. **A file error never fails the session**: it goes into `SessionOutcome.files_error` (and `files_received`/`files_sent`), so ops exchanged before it are still applied and counted. `SessionParams.files: Option<&FileSync>`; `None` looks like a 0.2.x peer. If the blob folder cannot be read when the `Hello` is built, the device offers no files for that session (a test removes the folder).
- **`service.rs`**: `SyncService::start(replica, keys, config, files: Option<FileSync>)`; `SyncEvent::SyncCompleted` gained `files_received`, `files_sent`, `files_error` (the app and `lp` match with `..`; `lp` prints them); a session that received files counts as "new information" and re-syncs the other peers soon, like one that received ops.
- **Engine**: `start_service` passes `FileSync::new(blobs)`. `on_sync_event` also folds in and emits `VaultChanged` when files arrived (that is how a UI turns "waiting" into openable) and logs a `files_error`. **`refresh_files` asks for another round (`notify_local_change`) when the wanted list names a blob the device does not hold**: a session's file phase uses the wanted list as it was when the session began, and the list is rewritten only when the unlocked engine folds the new ops in, after the session. So a file item that is new to a device normally arrives in the second round (within one scheduler tick: 10 s in the app, 200 ms in tests), and one that arrived while the device was locked arrives after the next unlock.
- `docs/plans/2026-10-31-phase7-files-tools-builds.md` has a dated "Changes (2026-11-01, 7.6c)" section. **Deviations from the plan text:** no `FilesHave` message (each side sends only what it wants; the digest gate and the initiator-serves-first order are as planned), a 20 minute limit for the phase (the plan had only the idle limit), file errors recorded instead of failing the session, and the follow-up round above. CLAUDE.md has a "File phase (7.6c)" bullet.

Tests (23 new):
- `crates/lp-sync/tests/files_sync.rs`, 18 (+1 ignored): blobs move both ways and only wanted ones; the responder can be the receiver; ops and files in one session, a blob nobody holds is skipped; equal holdings add no messages (checked against a peer that hangs up after the ops); a peer without `files1` (both directions) and a digest without the feature; the `Hello` read by a 0.2.x struct and back; the sender stops at its budget and the next session carries on; a damaged blob is not sent and is removed; the asked ids exclude held blobs; a blob is served once and unknown ids are ignored; a revoked or foreign peer never reaches the phase; an unreadable store costs only the file phase.
  Against a scripted peer (a real member that breaks the rules; each case carries on politely afterwards so the reported reason comes from the check and the test names it): a blob nobody asked for, the wrong bytes for the asked id, size 0, size over the cap, more bytes than announced, `FileEnd` early, an empty chunk, a chunk over 48 KiB, a wrong message inside a file, the same blob twice, a peer past the budget, too many / empty / over-long want lists, a stall (idle limit) and a drip (total limit). In every case the ops before the phase still count and nothing bad is stored.
- `crates/lp-engine/tests/scenario.rs`, 5: a file reaches the other device and a purge removes it (and `VaultChanged`); a locked device fetches what it already wanted (and is told); a locked device learns of new files only after the next unlock and drops purged ones then; a file relays through a device that holds it while the origin is away; a device that joins later gets existing files, trashed ones included.
- **Mutation-checked: 23 changes made one at a time (digest gate, feature gate, both budgets, asked-id check, duplicate receipt, chunk and `FileEnd` checks, store under the computed hash, want-list caps and shape, duplicate serving, idle limit, total limit, error recording, no feature offered, size range, damaged-blob handling, asking for held blobs, the follow-up round, `VaultChanged` on arrival, no file phase in the engine): 22 caught.** Survivor: **M18, dropping `|| out.files_received > 0` from the "pass it on" bump in `finish_session`**. It only decides how soon a third device fetches a blob that a device has just received; a deterministic test needs a narrow race (the third device must have asked before the middle one held the blob). Without it that device waits for the next periodic resync (5 minutes in the app). The first mutation run found seven weak tests (the scripted-peer cases passed on a closed-stream error instead of the intended one, and one scenario test passed because ops-triggered rounds covered for the follow-up round); they were tightened and the whole list rerun. **Lesson:** a mutation script that restores a file must touch it, or cargo reuses the mutated build (the scratch script now does).
- Timing (`the_largest_blob_moves_and_is_timed`, ignored; `cargo test -p lp-sync --test files_sync -- --ignored --nocapture`): a 50 MiB blob over an in-memory stream takes about 0.8 s in the test profile (0.78 s release), i.e. the CPU cost of chunking, CBOR, Noise and hashing on this laptop; real transfers are limited by the network. Not measured on a phone.

Facts for 7.6d, 7.6e and 7.6f:
- **UI (7.6d):** `FileInfo.available` is false until the blob is here; the engine does not know which device holds it, so the text is "Waiting for this file from another device" with the existing "Sync now". `VaultChanged` is emitted when files arrive, also while locked. The app's `UiEvent::SyncCompleted` does not carry the file counts yet (add them if a toast is wanted).
- **Android (7.6e):** `SyncWorker` passes a 25 s limit to `runSyncOnce`. A big file may not finish in a background round; only whole blobs are stored, so a cut-off transfer restarts from zero next time. Decide whether to raise the limit when the wanted list has missing blobs (a locked phone only fetches ids from its last-unlock wanted list). Time a transfer on the phone.
- **Known limits to review in 7.6f:** (1) memory: each session holds one blob (up to ~50 MiB) in each direction, and 16 inbound sessions are allowed, so a hostile *member* could make a device hold about 800 MiB; limiting concurrent file phases (and then telling the peer, since both sides must agree) would fix it; (2) a purge folded in while a fetch of that blob is in flight can leave an orphan blob until the next refresh (catch-up or unlock); (3) a locked device keeps purged content until it is unlocked (by design, it cannot tell what is unreferenced).

## 7.6d results (2026-11-01, this laptop; the user said "start 7.6e", the handoff said 7.6d was next because 7.6e reuses all of it, and the user chose 7.6d first)

Nothing is committed (the user commits). Checks: `cargo fmt --all --check`, `cargo clippy --workspace --exclude lp-winauth --all-targets -- -D warnings` clean; `cargo test --workspace --exclude lp-winauth --no-fail-fast`:
**445 pass, 0 fail, 3 ignored** (418 before + 27 new in the app crate); `cd app && bunx tsc --noEmit`, `bun run build` clean, `bun test`: 24 pass, 9 skipped (the Windows-only uninstaller tests), 0 fail.
**New: `app/e2e/files.sh` drives the REAL app on Linux, 31 checks, all pass** (below). Not run: Windows, Android (CI compiles the shared Rust), and the assisted checks listed at the end.

What exists now:
- **Rust, `app/src-tauri/src`**
  - `temp_files.rs` (desktop): the copies "Open with…" leaves. `<data dir>/open/<32 hex>/<clean name>` (folder 0700, file 0600), `open/manifest.json` (tokens, clean names, times; never paths), `TempFiles::{open, create, list, delete, clear, sweep}`. `open` removes everything under `open/` at start; a copy that cannot be removed stays listed as `inUse`, overdue, and `sweep` (once a minute from the auto-lock loop in `lib.rs`) retries it. Names go through `safe_file_name` (no separators, control or bidi characters, nothing Windows rejects, reserved device names get a `_`, 200 bytes, extension kept) and `is_launchable` refuses programs and scripts by extension (exe, bat, cmd, ps1, js, vbs, lnk, desktop, sh, py, jar, ... see `LAUNCHABLE`) judged on the cleaned name. Tokens are validated before any path is built.
  - `file_view.rs`: `decode_text` (UTF-8, UTF-8 with mark, UTF-16 LE/BE with mark; lossy; first 5 MB, cut at a character boundary) and, desktop only, `tiff_to_png` (the `image` crate's `tiff` feature, at most 64 MP / 512 MiB, size checked from the header first).
  - `file_ops.rs`: the commands' logic as plain functions over `&Engine` so tests use a real engine: `require_viewer` (**re-sniffs the decrypted bytes and refuses a viewer that does not match**, so a `file.mime` written by another device cannot feed the text viewer or the PDF frame something else), `read_for_viewer`, `text_of`, `image_of`, `export_to` (0600, truncates), `open_copy` (refuses programs before decrypting), `parse_meta`.
  - `commands.rs`: `file_add_path`, `file_add_bytes` (raw body, details in an `x-lp-meta` header as base64url JSON), `file_read(id, expect: audio|video|pdf)` (a raw `tauri::ipc::Response`, the bytes moved not copied; `pdf` refused where `pdfInApp` is false), `file_text`, `file_image` (TIFF becomes PNG), `file_export`, `files_left_out`, desktop only `file_open_external`, `temp_files`, `temp_file_delete`, `temp_files_clear`. `export_json` and `export_backup` now return `{ passkeys, files }`. `AppInfo.files` = `{ pdfInApp: linux, openExternal: desktop, export: desktop }`, **the one place that switches viewers per platform**.
  - `settings.rs`: `openedFilesMinutes` (default 10, 0 = never) and `deleteOpenedOnLock` (default true). `lib.rs`: `AppState.temp_files` (desktop), copies cleared on every engine `Status` event that is not unlocked and on `RunEvent::Exit` when the option is on (`clear_opened_files`).
  - `tauri.conf.json` CSP: `img-src 'self' data: blob:; media-src blob:` (nothing else changed). `Cargo.toml`: `image` gets `tiff`; dev-dependency `lp-core` with `test-utils`.
- **Frontend, `app/src`**
  - `screens/Files.tsx` (nav "Secure files" under Favorites, "Files" in the phone's bottom nav): list, search, "Favorites only", Add (open dialog, or `<input type=file>` + `file_add_bytes` on a phone), drop files on the window (`onDragDropEvent`), one file asks for a title and description, several are added as they are, progress text while compressing; the opened-files panel.
  - `screens/FileDetail.tsx` (also used by the Trash for file rows): details, favourite, edit title and description, Open with…, Export… (warns that the copy is not protected), Move to trash / Restore / Delete forever, "Waiting for this file from another device" + Sync now.
  - `components/FileViewer.tsx`: text (wrap toggle, JSON pretty-print, 5 MB notice), image (fit/zoom; TIFF via Rust), audio, video (a codec failure shows a message and Open with…), PDF where `pdfInApp`; files over 8 MiB ask before they are decrypted for viewing; object URLs revoked on change and unmount (lock unmounts the screen). `blobType` only lets a plain `image|audio|video/...` type through (never svg), else a fixed safe type.
  - `components/PdfView.tsx`: a `srcdoc` frame with `sandbox="allow-scripts"` that loads `/viewer/pdfview.js`; bytes go in with `postMessage` (transferred); only `ready`, `loaded`, `shown`, `error` come back and `parseFrameMessage` believes only those shapes, only from this frame's window. Page and zoom controls are in the parent.
  - `components/OpenedFiles.tsx`: name, opened ago, time left (or "in use, will retry"), Delete now / Delete all; polls every 5 s and on `lp-opened-files`.
  - `lib/files.ts` (+ `test/files.test.ts`, 11 tests), `lib/api.ts` types and wrappers, Settings (Security: "Delete opened files after" 5/10/30/custom/never and "Also delete them when LocalPass locks or quits"; Import & backup: how many secure files a backup or export leaves out, and the export toasts say so).
- **The PDF viewer bundle:** `app/viewer/pdfview.entry.js` + `app/scripts/build-viewer.ts` (`bun run build:viewer`; `bun run dev` and `bun run build` run it first) -> `app/public/viewer/pdfview.js` (git-ignored, 1.6 MB, one classic IIFE with pdf.js 6.4.299 and its worker; checked to parse as a classic script). `pdfjs-dist` is a pinned dev dependency (its optional `@napi-rs/canvas` is not bundled). The frame draws one page to a canvas (no text layer, no search, no scripting), capped at 24 MP per canvas and 25 MP per image; CJK character maps and JPX/JBIG2 images are unavailable in the opaque-origin frame (7.6a).

**Real-app check (`app/e2e/files.sh`, Linux; the run takes about 3.5 minutes and shows a window).** It starts `target/debug/localpass` (a `--features custom-protocol` build with a fresh `app/dist`) through `WebKitWebDriver` (`TAURI_WEBVIEW_AUTOMATION=true` is what makes the webview accept it; no `tauri-driver` needed, the capabilities are just `webkitgtk:browserOptions.binary`), inside its own D-Bus session with its own keyring daemon and its own XDG folders (**the real keyring is never touched**: an empty `XDG_DATA_HOME` has no default keyring, so the script pre-creates an unlocked plain-text `login.keyring` + `default`), profile `e2efiles`. Fixtures are made with PIL and ffmpeg. The 31 checks, all passing: file_add_path of 9 files and file_add_bytes with a raw body; the list; files absent from All items; text with accents; JSON pretty-print; a 600x400 PNG; a TIFF converted (120x80); an ogg that loads (2 s); a VP8 webm (160x120); a 2-page PDF drawn in the frame ("1 / 2", then "2 / 2", screenshots looked at) and **the frame is `sandbox=allow-scripts`, `srcdoc`, no `src`, and the parent cannot touch its document**; a 10.6 MB text file asks first, then shows "first 5 MB"; "Open with…" refuses a `.sh` and leaves no copy; export is byte-identical and 0600; title edit; favourites filter; trash > Trash list > Restore; Settings texts; **purge removes the blob file at once (10 -> 9)**; lock leaves no `blob:` image in the page; a locked vault hands out nothing. Lessons: the first runs failed only in the harness (capabilities with a `browserName`, no `TAURI_WEBVIEW_AUTOMATION`, no default keyring, a fixture under the 5 MB limit, Trash rows are `role=button` divs not `<button>`).

**Mutation-checked: 8 changes made one at a time to the Rust guards, each run against the new tests, each caught, each reverted (the tree was re-checked: fmt, clippy, tests).** Viewer re-check off (3 tests fail), path separators not replaced in a file name (2), the program-extension refusal removed from `TempFiles::create` (2), expiry off by one in `sweep` (1), the UTF-8 character-boundary step back removed (1), the export file mode 0600 -> 0644 (1), the export no longer truncating an older longer file (1), the startup wipe of leftovers disabled (2). **Not run, on purpose: the token-validation guard (`valid_token`).** Without it `delete(token)` becomes `remove_dir_all(root.join(token))`, and my own first version of `a_tampered_token_cannot_reach_outside` listed `"/"` among the bad tokens, so a regression there would have asked the test to delete the filesystem root; the auto-mode classifier refused that run. The test now only uses tokens that resolve inside its own temp folder (an absolute path of a folder it made instead of `"/"`), and the guard is covered by that test passing on the real code. Not mutation-testable: the early program check in `file_ops::open_copy` (it only saves decrypting, `create` refuses the same names), the `pdf` backstop in `file_read` (needs Tauri state), and the TypeScript `blobType` fallback (covered by `test/files.test.ts` instead). A first attempt to script the mutations over a copy of the repo (own target dir in the scratchpad, which is tmpfs) was abandoned after an hour because the cold build of the whole Tauri tree was still running; the scratch folders were removed.

Deviations from the plan text are in the plan's "Changes (2026-11-01, 7.6d)": viewer re-check by bytes, `AppInfo.files`, no launching of programs, HEIC is Open-with only everywhere (Windows WIC left out: unrunnable here), the viewer as a separate bundle, exports report counts, phone upload already wired.

Not run (assisted, `LOCALPASS_PROFILE=dev bun run tauri dev`, a vault with a few files): (1) "Open with…" on a real file (a PDF or a picture): the system program opens it, the Opened files panel lists it with a countdown, "Delete now" removes it, it disappears at the set time, and with the vault locked or the app quit it is gone (and `<data dir>/open/` is empty); (2) drag a file, and several files, onto the window; (3) the file dialog's multi-select; (4) a stock Fedora without the extra GStreamer codecs: an mp3 or an H.264 clip gives the "cannot play" message with Open with…; (5) two dev profiles (`dev`, `dev2`) syncing a file, the "waiting" state, Sync now; (6) Windows: "Open with…", the "in use" retry, PDFs opening in the system program, `image` with `tiff` linking; (7) the 50 MB case in the UI (adding takes the time the spike measured, with the progress text).

Facts for 7.6e and 7.6f:
- **7.6e starting point.** Already wired and untested on a phone: the Files screen with `<input type=file>` -> `file_add_bytes`, the viewers (text, picture, audio, video; TIFF is not converted on a phone because the `image` crate is desktop-only in `Cargo.toml`, so a TIFF there is "cannot show"), the Trash rows. Still to do: `AppInfo.files.openExternal`/`export` are false on Android, so "Open with…" and "Export…" are hidden (the `file_open_external`, `temp_*` commands are `#[cfg(desktop)]`); 7.6e adds the Kotlin side (`LpMobilePlugin.openFile` with `cacheDir/open/` and `file_paths.xml`, `exportFile` with SAF, HEIC with `ImageDecoder`), the same expiry and lock rules for the cache copies, flips the two flags (and decides `pdfInApp` stays false), shows the Settings rows on a phone (they are hidden because `openExternal` is false), and `extension/e2e/android-files.ts`. The Files page uses `Modal`, `Button` etc. and should be looked at on a phone-width screen (never rendered at that width yet).
- **7.6f review points not yet covered:** (a) the PDF frame's `postMessage(..., "*")` from the parent carries the PDF bytes to an opaque-origin window (the frame is the parent's own, `srcdoc`, so only that window can receive them); (b) `file_add_bytes` copies the request body once (50 MB) into a `Zeroizing` buffer, the IPC layer's own copy is not zeroed; (c) a decrypted file lives in the web view's memory while it is shown (the README bullet is a 7.6f item); (d) `ExportReport` and the "left out" text assume no export carries file content; (e) the unlinked `open/` copy on btrfs and SSDs may stay recoverable.

## 7.1 results (2026-10-31, this laptop: 8 threads, 15 GB, rustc 1.99.1, nothing else running)

`origin/build/smaller-builds` (`e6668f5`: no `staticlib`, `debug = false` for dependencies in dev/test, only the aarch64 and x86_64 Android targets) was applied as
uncommitted edits of `Cargo.toml`, `app/src-tauri/Cargo.toml`, `rust-toolchain.toml` and the `CLAUDE.md` Android line (`git show e6668f5 | git apply`, because `CLAUDE.md` had other edits);
nothing is committed, so the branch itself is not merged: after the user commits these edits the remote branch can be deleted. The first build after the change rebuilt every dependency once (401 s, the profile change alters every fingerprint).

| Step | before (main) | with the branch |
|---|---|---|
| M1 incremental `cargo build -p localpass` after `touch crates/lp-engine/src/lib.rs`, 3 runs | 10.1, 4.1, 4.7 s | 3.4, 3.4, 3.4 s |
| M2 `cargo test --workspace --no-run` after the same touch, 3 runs | 14.3, 14.7, 13.1 s | 4.8, 4.7, 4.6 s |
| M3 cold `cargo build -p localpass` (own target dir): time, target dir, exe | 428.6 s, 3.5 GB, 168.0 MB | 416.9 s, 2.1 GB, 82.0 MB |
| M4 cold `cargo build --release -p localpass --features custom-protocol` (own target dir): time, target dir, exe | 491.1 s, 2.0 GB, 20,820,248 B | 490.6 s, 1.7 GB, 20,820,248 B |

Decisions:
- **Linux linker: rust-lld stays (nothing to add).** `readelf -p .comment target/debug/localpass` shows `Linker: LLD 23.1.1`; rustc already passes `-fuse-ld=lld` for `x86_64-unknown-linux-gnu`.
  mold 2.40.4 (already installed here) does take over with `-C link-arg=-fuse-ld=mold` (the `.comment` then names mold), but gave no gain, so it is not adopted
  (the bar was 20% on the link-bound steps): M1 5.6, 3.5, 3.2 s (median 3.5 vs 3.4) and M2 9.7, 11.8, 13.8 s (vs 4.7). The M2 slowdown was not isolated (the mold run used a fresh target dir and
  `RUSTFLAGS`, which are possible confounders); it cannot turn into a 20% gain, so mold stays out of `.cargo/config.toml` and of the CI installs.
- **Release profile unchanged.** Thin LTO (`CARGO_PROFILE_RELEASE_LTO=thin`, codegen-units 1 kept): 410.3 s (16% faster; the bar was a third) and a 22,038,040 B exe (+5.8%). Fat LTO stays.
  The branch's removal of `staticlib` did not shorten the release build at all (490.6 vs 491.1 s); its gain is disk (2.0 -> 1.7 GB release, 3.5 -> 2.1 GB debug) and the incremental/test builds above.
- **Android:** nothing to change for the linker (the NDK links with lld). Not run here: `cargo clippy -p localpass --target aarch64-linux-android` (no NDK). The branch changes the library's `crate-type` to `["cdylib", "rlib"]`, so CI's Android job is the check.
- **Windows (DONE 2026-11-01, applied in `.cargo/config.toml`; the user reported that it works):** `[target.x86_64-pc-windows-msvc] linker = "rust-lld.exe"`. Not reported in detail: whether the exe icon and manifest were looked at one by one, and no timing was taken. What was open before: Nothing in the repo passes link.exe-only flags (no `rustc-link-arg` in our build scripts), but `tauri-build` adds the manifest and icon
  resource arguments itself, so only a Windows build can say. Steps from the plan: put the line on a branch, push it (the user's step), look at Windows CI (clippy, tests), then on the work laptop build the NSIS installer and check the exe icon and manifest,
  that the app starts and that `lp-winauth` registers; drop the change if any of these fails. Its gain is unmeasured; expect it to matter only for link-bound steps like M1/M2 on Windows.

Checks after applying the branch: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace --exclude lp-winauth --no-fail-fast` (321 pass, 0 fail, as before) are clean. Windows and Android builds of the change are not verified here (CI).

Reproduce: `bench.sh` was a throwaway script (warm-up build, `touch`, `time` around each command, `CARGO_TARGET_DIR` outside the repo for the cold builds); the numbers above are the record.

## Next (fresh chat, updated 2026-11-09)

**7.6 is dropped.** The user requested removal of secure-file storage on 2026-11-09; its source, UI, viewers,
blob store and file-sync phase have been removed. The older 7.6 result sections above are historical, not current instructions.

**7.7: release 0.3.0 only when the user requests it.** Preserve the completed vault tools, QR features and Firefox toolbar picker.
The Firefox changes and removal compatibility guards are included in the local commit requested on 2026-11-09. Do not push or release without a request.
No migration or deletion of saved user vault/blob data was performed.

Facts the plan relies on were checked in the code on 2026-10-31; re-check the ones a task touches (the plan's "Facts" section lists them with file and line).

## Firefox toolbar password picker (2026-11-08, done)

The user requested a fresh project-state/Firefox capability check, selected the protected toolbar panel instead of an in-page
picker, and approved `docs/plans/2026-11-08-firefox-toolbar-picker.md`. Firefox 159 still lacks IOv2 `isVisible` (rechecked with
the existing spike). Mozilla documents programmatic `action.openPopup()` without user activation on Firefox Desktop 149+.

- `background/decide.ts` now presents `chooseLogin` in the tab-specific toolbar popup on Firefox 149+, using the existing
  `decide.html` page with an exact request URL and a panel presentation flag. Older Firefox keeps the separate chooser window.
  Save/passkey decisions keep their windows. No manifest permissions, broker protocol or vault format changed.
- Authorization requires the exact extension URL/request id, no sender tab/frame for panels, and the original tab active in
  its focused browser window with the original origin. It rechecks after fetching the credential. Secrets return only to the
  original content-script request, not to the picker. The content script fills only its original connected form and withdraws
  on `pagehide`. Navigation, tab/window changes, panel dismissal, tab closure and replacement cancel. A serialized popup queue
  and URL ownership check prevent stale cleanup from clearing a newer request. Normal toolbar popup is restored on completion
  or failed opening; no automatic separate-window fallback on modern Firefox.
- The picker preserves the initial action delay and Esc behavior, has a 360 px layout and bounded scrolling. Its port closes
  it when the request ends. If the app takes focus during Unlock, reopen the field picker afterward. README updated.
- Verification: **252 extension unit tests pass**, TypeScript clean, Chrome and both Firefox builds succeed, `web-ext lint`
  **0 errors / 0 notices / 0 warnings**. The lint tool's own update check cannot write its global config in the sandbox; the
  extension validation succeeds. Built `lp-broker --examples` for the native-host test harness. Did not rerun the full Rust
  workspace or app checks: their pre-existing secure-file changes were preserved.
- Real Firefox **159.0**, visible on GNOME through Xwayland: **89 browser checks pass**, including panel selection without a
  new window, exact-form fill, Esc, native dismissal, tab/navigation/window changes, locked vault, two browser windows,
  same-origin frame fill, blocked cross-origin and opaque frames, plus all existing save/passkey checks. Visually inspected
  the actual panel compositor snapshot at `/tmp/localpass-toolbar-preview.png`. All data is fake and profiles are throwaway;
  real `~/.mozilla/native-messaging-hosts` unchanged. No installed LocalPass instance or real vault was started.
- Harness detail: Firefox omits native toolbar popups from BiDi's content tree. The harness now accesses their documents via
  browser chrome scope and a temporary privileged window actor. Only that single temporary module is added to Firefox's
  read-path whitelist; the content sandbox remains enabled. The actor is never bundled in the extension. BiDi page clicks
  bypass Firefox's native outside-click handling, so the dismissal check invokes the native panel's `hidePopup()` path.
  A human's physical outside click and real-app Unlock focus transition were not independently assisted-tested in this task.
  The existing unit tests cover blur, and panel/window cancellation is covered in real Firefox.
- Commands: `cd extension && bun run e2e/firefox/run.ts` for headless; prefix `LP_FF_HEADED=1` for visible Xwayland testing,
  optionally `LP_FF_SCREENSHOT=/tmp/localpass-toolbar-preview.png` to capture the actual panel. Uses only the harness's fake
  vault/throwaway profile. The normal build for user preview is **`extension/dist-firefox/manifest.json`**, loaded from
  `about:debugging` → This Firefox → Load Temporary Add-on; the installed signed XPI does not contain these changes.
- Plan index updated. No commit, push, release or signed-XPI update. All unrelated uncommitted Phase 7 file work remains.

## Secure-file storage removed (2026-11-09)

At the user's request, task 7.6 is dropped and its uncommitted prototype removed. App navigation/settings and file screens,
Tauri file/temp-copy commands, viewers/PDF build assets, blob encryption/storage, compression/TIFF/PDF dependencies,
file transfer and sync messages, file-specific tests, and the webview spike implementation are gone. The app and CLI return
to their pre-file feature interfaces. Phase 7 vault tools, QR support, and all Firefox toolbar-picker changes are retained.
The extension source hash comparison confirms that the removal changed no extension files.

Saved vault data, metadata and blob directories on user devices were not opened, deleted or migrated. Legacy development
records with kind `file` are hidden from lists and exports, cannot be viewed or rewritten as another type, and their keys
remain non-copyable and excluded from conflict views. A regression test seeds such a record directly in the encrypted log
and checks that it survives lock/unlock with the old blob untouched while normal interfaces refuse it.

Removed source was archived before removal at `/tmp/localpass-remove-files-wuf4v4s2/source-backup.tar.gz`. This is a source-only
backup, not a vault backup. Historical 7.6 plan/result text above is retained as history; the approved plan has a dated
removal amendment and the index/current instructions were updated. The user requested a local commit of these changes on
2026-11-09; no push or release was requested.

Validation: the frontend production build passes and contains no secure-file route/viewer assets. App tests: 13 pass,
9 Windows-only skips, 0 fail. Rust workspace: 348 pass, 2 ignored, 0 fail (including the legacy-record regression).
The first workspace run hit sandbox-denied temporary Unix sockets; the rerun outside the sandbox passes. Targeted engine
unit tests also pass (25 tests). `cargo fmt --all -- --check` and `cargo clippy -p lp-core -p lp-engine --all-targets --locked -- -D warnings`
pass. `cargo build -p localpass --features custom-protocol --locked` succeeds; the debug app embeds the rebuilt frontend.
The rebuilt executable contains no removed secure-file screen labels or IPC command names. No app/vault was launched.
