# Handoff: Phase 7, secure files, vault tools, faster builds

The approved plan is `docs/plans/2026-10-31-phase7-files-tools-builds.md` (tasks 7.0-7.7, with the design of each). This file holds the
state; do not rely on conversation history. All plans: `docs/plans/README.md`.

## RESUME HERE (state saved 2026-11-01)

Phases 0-6 are done except 5.6 (the Windows final matrix, on the work laptop; `docs/handoff-phase4-5.md`). v0.2.2 is the last release; the user
tested and approved it on 2026-10-31. Phase 7 started from a clean tree at `d4eb731`.

**Branch:** tasks 7.0-7.4 are committed on branch `phase7` (five commits on top of `d4eb731`: build config, Firefox Esc/click-outside, hidden notes, desktop QR, docs) and pushed to
`origin/phase7` on 2026-11-01 at the user's request, so the Windows PC can fetch it for the open Windows checks. `main` is still at v0.2.2. Keep working on `phase7`; merging into `main` is the user's call.

Rules (the plan's "Working rules"; the agent rule itself is in `CLAUDE.md`, "Working with agents"):
- One task per fresh chat. 7.0-7.3 are small and independent; do the later tasks in the order of the plan.
- No agents by default. One only for an independent look (audit, review of a finished task, second opinion), briefed with the goal, the files or
  the diff and the questions, not your conclusions; read-only; check every finding against the code before acting.
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
- [x] 7.1 Faster Rust builds (2026-10-31): branch applied, linker and LTO decided, numbers below. The user confirmed mold is not wanted and **skipped the Windows `rust-lld.exe` change: the user will check it later** (not applied; steps below).
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
- [x] 7.4 2FA QR on desktop (2026-11-01, code and automated checks done; **the user ran their assisted passes on 2026-11-01 and reported "all work"** without naming items, so the Windows-only items (4) and the per-item results below are not individually recorded; the Windows `from_screen` is still only compile-checked unless the user says they ran it).
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
  **Windows is compile-checked only**: `from_screen` was built alone against `windows` 0.62 for `x86_64-pc-windows-gnu` (the app crate itself needs a MinGW C compiler for a dependency, so a
  full cross check was not possible); it has never run. Android is untouched (CI's Android job still has to build it).
  Not run (assisted, all in `LOCALPASS_PROFILE=dev bun run tauri dev`, new login, 2FA block): (1) Firefox screenshot "Copy" then Ctrl+V in the field and the Paste QR image button; GNOME PrtSc
  to clipboard (Ctrl+PrtSc / the screenshot UI's copy), also while the window is hidden or another window has focus; the clipboard is empty afterwards ("Nothing is copied" in `wl-paste`) and a text
  key pasted with Ctrl+V still works; (2) "Scan QR on screen" with a QR visible in Firefox: the window steps aside, GNOME's picker appears, a selection fills the field, the window returns with focus, and
  no file is left in `~/Pictures/Screenshots` (find where the portal wrote it; closing the picker leaves the form as it was); (3) a PNG from the file dialog and one dropped on the block (drop events need
  `core:event`, so check it really fires on WebKitGTK and WebView2); (4) Windows: Win+Shift+S then Ctrl+V, and "Scan QR on screen" on two monitors; (5) a non-2FA QR, a Google Authenticator export QR, a
  HOTP QR and a picture without a code give the right message.
- [ ] 7.5 Duplicate logins
- [ ] 7.6 Secure files (7.6a spike, b core/store/engine, c sync, d desktop UI, e Android, f security review)
- [ ] 7.7 Release 0.3.0 (only when the user says so)

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
- **Windows (OPEN):** `[target.x86_64-pc-windows-msvc] linker = "rust-lld.exe"` is NOT applied. Nothing in the repo passes link.exe-only flags (no `rustc-link-arg` in our build scripts), but `tauri-build` adds the manifest and icon
  resource arguments itself, so only a Windows build can say. Steps from the plan: put the line on a branch, push it (the user's step), look at Windows CI (clippy, tests), then on the work laptop build the NSIS installer and check the exe icon and manifest,
  that the app starts and that `lp-winauth` registers; drop the change if any of these fails. Its gain is unmeasured; expect it to matter only for link-bound steps like M1/M2 on Windows.

Checks after applying the branch: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace --exclude lp-winauth --no-fail-fast` (321 pass, 0 fail, as before) are clean. Windows and Android builds of the change are not verified here (CI).

Reproduce: `bench.sh` was a throwaway script (warm-up build, `touch`, `time` around each command, `CARGO_TARGET_DIR` outside the repo for the cold builds); the numbers above are the record.

## Next (fresh chat)

7.5 (duplicate logins), then 7.6 (the plan's order). On 2026-11-01 the user checked 7.4 (picture paste, file dialog; "all work") and 7.2 (Esc and click outside, after loading the temporary add-on). They also confirmed 7.3 (hide/show) and the 7.2 Unlock side check. **Still open, all Windows:** the `rust-lld.exe` step of 7.1 and the Windows items of 7.4 (Win+Shift+S then Ctrl+V; "Scan QR on screen" on two monitors). The user has a Windows machine but cannot get the code there until it is committed and pushed, so these wait for a commit.

Facts the plan relies on were checked in the code on 2026-10-31; re-check the ones a task touches (the plan's "Facts" section lists them with file and line).
