> **Status: in progress (approved 2026-10-31).** State: `docs/handoff-phase7.md`. Index: `docs/plans/README.md`.

# Phase 7: secure files, vault tools, faster builds

## Context

Phases 0-6 shipped (v0.2.2 draft). The user set the next round of work on 2026-10-31:

- **Agent rule.** By default, no agents. Spawn one only for an independent look, where a fresh context that has not followed the main session's reasoning is the point. Examples: audits, reviews for insecure or slow code, a second opinion.
- **Faster Rust builds.** Fold in branch `origin/build/smaller-builds` and switch to a faster linker.
- **Duplicate logins.** Find logins with the same username and password on the same site. By default a subdomain is never grouped with a sibling or with the root domain; an option lets it be.
- **Firefox chooser window.** Close the account-chooser window with Esc or by clicking outside it.
- **Secure notes.** Hide the note body like a password: it shows only after clicking Show.
- **2FA QR on desktop.** Scan the 2FA QR code on desktop too. Today only the Android camera can do it (`app/src/lib/scan.ts`, `canScan` is Android-only).
- **Secure files tab.** A new tab next to All items and Favorites. Each file has a title, a description and the content, compressed at maximum level. Common types open in the app. Anything else opens in its system app from a tracked temp copy that is deleted automatically after a set time. Decrypted export is allowed. Files cannot be edited in this version.

**User decisions (2026-10-31):**
1. File content **syncs to every device** (phone included).
2. **50 MB** largest file, before compression.
3. Desktop QR sources. Main path (added on review): **paste an image from the clipboard**. The user grabs the QR with a screenshot (the browser's screenshot tool, PrtSc or Win+Shift+S) and presses Ctrl+V in the app, and nothing is saved to disk. Also: **"Scan QR on screen"** (Windows screen capture; on Fedora, GNOME's screenshot picker through the portal) and **open or drop an image file**.
5. **Plan history in the repo** (added on review): every approved plan is saved in `docs/plans/`, with an index that lists them all and their status, so past and open work is never lost.
4. Duplicates: **`www.` is ignored by default**, so www.example.com = example.com. Other subdomains stay separate unless the option is on.

## Facts the design relies on (checked in the code)

- `MAX_DATA_OP_LEN = 48 KiB` (`crates/lp-core/src/model.rs:17`), and the op log is append-only and relayed forever. File content therefore cannot go into ops, or a deleted file would live on in every device's log. Content goes to a separate blob store. Only small metadata goes into the item.
- Sync requires an exact `PROTO_VERSION` match (`crates/lp-sync/src/session.rs:129`). `Hello` and the other messages are plain serde structs with no `deny_unknown_fields`. A file phase gated by an optional `features` field in `Hello` therefore stays compatible with 0.2.x peers.
- 0.2.x UIs show an unknown kind with a fallback icon (`Vault.tsx:234`, `kindIcon[i.kind] ?? KeyRound`). A synced `file` item shows up there as a plain entry and does not crash the list.
- rustc 1.99 already links `x86_64-unknown-linux-gnu` with rust-lld (the default since 1.90). Windows MSVC still uses `link.exe`. The NDK already uses lld.
- The branch `origin/build/smaller-builds` is one commit, e6668f5, and merges cleanly into main. It drops `staticlib`, sets `debug = false` for dependencies in dev and keeps only the aarch64 and x86_64 Android targets. CI already uses `Swatinem/rust-cache`.
- The last release run took Windows 15 min, Android 12.5 min and Fedora 9.5 min.
- Chrome's in-page dialog already closes on Escape or a click outside (`extension/src/content/ui.ts:359-383`). Firefox's decision window (`extension/src/decide/index.ts`) has neither.
- `Engine::field_secret` copies any text field, notes included, so "copy note" needs no new Rust.
- The desktop clipboard is `arboard` 3 (`app/src-tauri/Cargo.toml:36`, `default-features = false`; `clipboard.rs`). Turning on its `image-data` feature gives `get_image()` (RGBA). On Linux it uses the X11 backend, which reaches GNOME Wayland's clipboard through XWayland.
- Plans so far:
  - Phase 6: `docs/phase6-linux-plan.md`, identical to `~/.claude/plans/this-application-was-build-crispy-sketch.md`.
  - Phases 0-5: no plan file; their plan and status live in `docs/handoff-phase4-5.md`.
  - `~/.claude/plans/i-want-to-know-jolly-rocket.md` (Fcitx5 input setup) is not about this project.
- `lp_core::origin::Origin::parse` / `registrable_domain()` and `TotpSpec::parse` exist. The `qrcode` crate is already a dependency (useful for tests).
- The CSP (`tauri.conf.json`) has no `blob:` and no `media-src`, so it must change for the viewers. Import and export are desktop-only today (`Settings.tsx:153`).

## Working rules (the new rule replaces the old agent lines)

- **Agents (new default):** the main session does reading, searching, planning, coding, test-debug loops and docs itself. Spawn an agent only for an **independent look**: a security audit, a hunt for insecure or slow code, a review of a finished task, or a second opinion on a design or bug the main session is stuck on. Brief it with the goal, the files or diff and the questions, **not the main session's conclusions**. Agents are read-only unless the task says otherwise, reply briefly, and run one at a time unless the user asks for more. The main session verifies each finding before acting on it.
- Unchanged: one task per fresh chat; builds and e2e in the background; never commit or push unless asked; experiments use `LOCALPASS_PROFILE=dev`; tests never touch the real keyring or `~/.mozilla`. Shell setup: `export PATH=$HOME/.cargo/bin:$HOME/.bun/bin:$PATH`.
- State lives in a new `docs/handoff-phase7.md` ("RESUME HERE"). The plan is saved as `docs/plans/2026-10-31-phase7-files-tools-builds.md` and listed in `docs/plans/README.md`.

## Tasks (in order; 7.0-7.3 are small and independent)

### 7.0 Agent rule, plan history and phase docs (docs only)
- **Plan history, `docs/plans/`:**
  - `docs/plans/README.md` is the plan log. A table with one row per plan: date, title, file, status (`approved` / `in progress` / `done` / `dropped`), a one-line summary, and where its state lives (the handoff). Newest first.
  - The table starts with three rows:
    1. Phase 7, this plan.
    2. Phase 6. `git mv docs/phase6-linux-plan.md docs/plans/2026-10-28-phase6-linux.md`, then update the two references (`CLAUDE.md:14`, `docs/handoff-phase6-linux.md:3`).
    3. Phases 0-5, pointing at `docs/handoff-phase4-5.md`, because no separate plan file exists.
  - Each plan is a file named `YYYY-MM-DD-<slug>.md`: an exact copy of the approved plan, with one status line added at the top.
  - Rules, written in the index and in CLAUDE.md:
    - Every plan the user approves is saved there in the same session and gets an index row.
    - An approved plan is never rewritten. A later change is appended as a dated "Changes" section.
    - The status in the index is updated when a task or phase finishes.
    - Plans that are not about this project stay out.
- `CLAUDE.md`:
  - a short "Working with agents" section holding the rule above;
  - a line under "Current work": "All plans, past and open: `docs/plans/README.md`";
  - "Current work" points to Phase 7.
- `docs/handoff-phase4-5.md`:
  - line 19: replace "at most one agent at a time and only for heavy work".
  - Working rule 2 (lines 68-71): replace with a pointer to CLAUDE.md.
  - "Prompt template for agents": keep it, retitled to independent reviews.
- `docs/handoff-phase6-linux.md:14` and `docs/phase6-linux-plan.md:50` ("no agents except the security review"): replace with a pointer to CLAUDE.md.
- New files: `docs/plans/2026-10-31-phase7-files-tools-builds.md` (this plan) and `docs/handoff-phase7.md`.
- Save the agent rule and the plan-history habit as feedback memories.

### 7.1 Faster Rust builds
1. **Baseline** on this laptop, run in the background, wall time from `cargo build --timings` / `time`:
   - M1: incremental `cargo build -p localpass` after `touch crates/lp-engine/src/lib.rs`, three runs.
   - M2: `cargo test --workspace --no-run` after the same touch.
   - M3: cold `cargo build -p localpass` and `du -sh` of its target dir, in a separate `CARGO_TARGET_DIR` so the 22 GB `target/` is left alone.
   - M4: cold `cargo build --release -p localpass` plus the binary size.
2. **Merge `origin/build/smaller-builds`** (fixed: no staticlib, no dependency debug info, two Android targets). Re-measure M1-M3 and disk use.
3. **Linker**
   - **Linux:** confirm rust-lld with `readelf -p .comment target/debug/localpass`. The user runs `sudo dnf install mold`; then measure M1 and M2 with mold. Adopt mold only if link-bound steps get ≥20% faster. If adopted:
     - add a `[target.x86_64-unknown-linux-gnu]` entry in `.cargo/config.toml`, with flags that also switch off rustc's own `-fuse-ld=lld`; verify with the `.comment` check;
     - install mold in `ci.yml`'s Linux job (`apt-get install mold`) and in the Fedora container of `release.yml` (`dnf install mold`).

     Otherwise document "rust-lld is already the default".
   - **Windows:** add `[target.x86_64-pc-windows-msvc] linker = "rust-lld.exe"`.
     - Verify with Windows CI on a branch push (clippy and tests), then on the work laptop: the NSIS installer, the exe icon and manifest, the app starts, and `lp-winauth` registers.
     - Drop the change if any of these fail.
   - **Android:** nothing to do (the NDK already links with lld).
4. **Release profile:** measure M4 with fat LTO and `codegen-units=1` (current) against `lto="thin"`. Switch only if thin LTO cuts the time by at least a third and the binaries grow by less than 10%.
5. Docs: the CLAUDE.md Android line (from the branch), a linker note, and the numbers in the handoff.

### 7.2 Firefox decision window: Esc and click outside (`extension/src/decide/index.ts`)
- **Esc** (not a repeat, not during IME composition) runs the current view's safe dismiss button: Cancel, Close or "Not now". This applies to every decision and to the waiting view, and does the same thing as clicking Cancel.
- **Click outside / focus lost** closes the window only for the choose-login and save-login views. It is:
  - armed only after the window has had focus once (Wayland may open the window unfocused);
  - disarmed while an answer is carried out or the Unlock step runs, because the app's own prompt takes focus then;
  - triggered on `blur`: wait about 250 ms, then cancel if `!document.hasFocus()`.

  Passkey windows close with Esc only: a stray click on the page must not silently fail the site's WebAuthn call.
- Add a small "Esc to close" hint. Chrome is unchanged.
- Tests:
  - `extension/test/decide-page.test.ts`, one case each:
    - Esc sends cancel;
    - blur while unfocused cancels;
    - blur during carry-out does nothing;
    - blur before the first focus does nothing;
    - blur in a passkey window does nothing.
  - `e2e/firefox/run.ts`: Esc on the chooser fills nothing.
  - Assisted: a real click outside on GNOME.

### 7.3 Secure notes hidden until shown
- `app/src/screens/ItemDetail.tsx`: for `kind === "note"`, the Notes row is masked by default (`••••`) with an Eye toggle (`whitespace-pre-wrap` when shown) and a Copy button (`api.copyField(id, "notes")`, Rust clipboard with auto-clear). It hides again on item switch (the existing `setShown({})`). Notes on logins and cards stay as they are.
- `app/src/screens/ItemEditor.tsx` and `components/ui.tsx`: a secure note's textarea is masked with a show toggle (CSS `-webkit-text-security: disc`, supported by WebView2, WebKitGTK and Android WebView), like `PasswordInput`.
- Verify: `bun run build`, then check in `LOCALPASS_PROFILE=dev bun run tauri dev`.

### 7.4 2FA QR on desktop
- **Decoder:** new `app/src-tauri/src/qr.rs` uses `rqrr` (pure Rust) on a grayscale image. Image files are decoded with `image` (png, jpeg, bmp and webp only).
  - Input is capped at 20 MB and 50 MP.
  - The command keeps only codes that `TotpSpec::parse` accepts as `otpauth://totp`. Clear errors otherwise: no code found, not a 2FA code, `otpauth-migration://` and HOTP not supported.
- **Commands, main path first:**
  - **`totp_qr_from_clipboard()` (the main desktop path).** Rust reads the image straight from the system clipboard: turn on arboard's `image-data` feature, call `get_image()`, and pass the RGBA pixels to rqrr. There is no file and the pixels never pass through the webview.
    - The user copies a screenshot of the QR: the browser's screenshot tool with "Copy", PrtSc on GNOME, or Win+Shift+S on Windows.
    - Triggers: Ctrl+V while focus is in the 2FA field or the 2FA block, and a "Paste QR image" button. Ctrl+V only goes to Rust when the paste carries no text, so pasting a text key or `otpauth://` link still works as today.
    - After a successful decode, the clipboard is cleared if it still holds that image, because the QR is the 2FA secret. A toast says so.
  - `totp_qr_from_file(path)`: from the open dialog or a dropped file (Tauri drag-drop gives paths).
  - `totp_qr_from_screen()`, per platform:
    - **Windows:** minimize LocalPass, capture all monitors with GDI (add the `Win32_Graphics_Gdi` feature), restore the window, decode. Nothing is written to disk.
    - **Linux:** the xdg-desktop-portal Screenshot through `ashpd` (`interactive: true`, so GNOME's picker lets the user select the QR). Read the file, decode it, then **delete the file**. Find out where GNOME writes it; it must not stay in `~/Pictures/Screenshots`.
- **UI (`ItemEditor.tsx` 2FA block):**
  - Android keeps the camera button.
  - Desktop shows "Paste QR image" (with the hint "copy a screenshot of the QR code, then press Ctrl+V"), "Scan QR on screen" and "From image file…", and accepts a dropped file.
  - On success the field is filled and "Found: issuer (account)" appears.
  - No new webview permissions.
- **Tests:**
  - Rust round trips: QR images built from the `qrcode` crate decode to the URI, both from RGBA buffers (the clipboard path) and from PNG files. A non-otpauth QR and noise give errors.
  - Assisted:
    - Paste on Fedora: Firefox screenshot "Copy", and GNOME PrtSc to the clipboard.
    - Paste on Windows: Win+Shift+S.
    - Windows screen capture and the GNOME portal.

### 7.5 Duplicate logins
- **Engine:** new `crates/lp-engine/src/duplicates.rs`, `Engine::find_duplicate_logins(include_subdomains) -> Vec<DuplicateGroup>`.
  - Scope: live logins only (not trashed, `kind=login`), with a non-empty password.
  - Matching: the username is trimmed and compared case-insensitively; the password must match exactly.
  - Site key per saved URL, parsed as `match_login` does (no scheme → https): the host in lower case, without the trailing dot and without a leading `www.`, with the port when one is given. With the option on, the key is `Origin::registrable_domain()` (IP, localhost and single labels keep their host).
  - Grouping: union-find, so an item with several URLs ends up in exactly one group.
  - Logins without a website are not checked, and the screen says so.
  - Output carries no secrets: `DuplicateGroup { sites, username, items: [ItemSummary + urls, has_totp, has_notes, custom_fields, updated_by] }`.
- **App:**
  - Command `find_duplicates(include_subdomains)`, its wrapper in `api.ts`, and a remembered setting `duplicatesIncludeSubdomains` (`settings.rs` and `api.ts`).
  - New `app/src/screens/Duplicates.tsx`, opened from a "Find duplicate logins" button in Settings (Shell page `duplicates`):
    - a toggle "Count subdomains as the same site";
    - per group, a Keep choice (the newest copy by default);
    - "Move the others to trash", which warns when a trashed copy has 2FA, notes or fields that the kept one lacks.
- **Tests:** unit tests for:
  - a.example.com and example.com: separate by default, merged with the option;
  - www.example.com equals example.com;
  - different passwords are not grouped;
  - usernames differing only in case are grouped;
  - trashed items are ignored;
  - IP addresses and ports;
  - `example.co.uk` (public suffix list);
  - transitive grouping across several URLs;
  - the serialized output contains no password.

### 7.6 Secure files

**Design**

- **Item:** a new kind `file` (`kinds::FILE`) with fields:
  - `title`, `notes` (shown as "Description"), `favorite` and `trashed`;
  - `file.name`, `file.mime` (sniffed from magic bytes, not trusted from the extension), `file.size`, `file.stored_size`;
  - `file.blob` (hex SHA-256 of the stored blob);
  - `file.key` (a random 32-byte key per file; added to `keys::is_secret`);
  - `file.codec` (`zstd` or `none`) and `file.added_ms`.

  Files appear only in the Files tab and in Trash. `ItemQuery` gets a `kind` filter, and the default list leaves `file` out. Files have no folders in this version.
- **Blob format** (new `lp-core/src/fileblob.rs`, pure logic):
  - header: `LPF1`, version, codec, chunk size and a random nonce prefix;
  - body: the compressed bytes sealed in 1 MiB chunks with XChaCha20-Poly1305 using the STREAM construction (nonce = prefix ‖ counter ‖ last flag, header as AAD);
  - blob id = SHA-256 of the stored bytes. The id sits in a signed op, so a peer cannot swap the blob.
- **Compression** (`lp-engine`, `zstd` crate):
  - level 22 (ultra, window ≤ file size) on desktop, 19 on Android for memory and time;
  - a level-3 probe of the first MiB: if it saves less than 3% (jpg, mp4, docx/zip…), the file is stored as `none`;
  - decompression stops at the recorded `file.size` (zip-bomb safe).

  The cap is 50 MB of plaintext (`MAX_FILE_BYTES`).
- **Blob store** (new `lp-store/src/blobs.rs`):
  - lives in `<data dir>/files/<id>`;
  - `put` checks the SHA-256, writes a `.partial`, fsyncs and renames;
  - files are 0600 on Unix;
  - the store is wiped with the vault and when leaving it.
- **Engine** (new `lp-engine/src/files.rs`):
  - `add_file`: sniff, compress, seal, put the blob, then one `ItemSet`.
  - `read_file`: verify, open, decompress, return `Zeroizing`.
  - `file_status`: available, or waiting for another device.
  - While unlocked (on unlock and after each materialize), the plain blob ids referenced by live and trashed file items go into store meta as the **wanted list**. Unreferenced blobs (purged items) are deleted. A purge deletes the local blob at once.
  - `ItemView` gains `file`.
  - The encrypted backup and the JSON export leave files out and say how many (the same pattern as `passkeys_left_out_of_plain_export`).
- **Sync** (`lp-sync/src/{protocol,session}.rs`):
  - `Hello` gains `#[serde(default)] features` (`"files1"`) and `files_digest` (SHA-256 of the sorted held ids). `PROTO_VERSION` stays 1, so 0.2.x peers keep syncing ops and never see file messages.
  - After both `Done`s, if both peers speak `files1` and their digests differ:
    1. `FilesHave` is sent both ways.
    2. Each side sends `FilesWant` for wanted ids it lacks.
    3. Blobs go as `FileChunk` (≤ 48 KiB) and `FileEnd`, then `FilesDone`. The initiator serves first.
  - The receiver checks the size cap and the SHA-256 before `put`.
  - The ops phase keeps its 60 s timeout. The file phase has a 30 s idle timeout per message and a 512 MiB budget per session.
  - Sync needs no vault key (hashes and bytes only). A locked device fetches only ids from its last wanted list, so the Android `SyncWorker` gets file sync for free.
  - The UI shows "Waiting for this file from <device>" with a "Sync now" button.

**Tasks**

- **7.6a Spike (risk first; go/no-go per item):**
  - `zstd` builds for `aarch64-linux-android` (CI clippy) and Windows; measure the time and memory of level 22 against 19 on 50 MB of text and PDF.
  - A sandboxed iframe in the Tauri webview: `<iframe sandbox="allow-scripts">` has an opaque origin. Check that it **cannot call Tauri commands** on WebKitGTK (here), WebView2 (the work laptop) and Android WebView (`lp_api37`), and that pdf.js renders in it under the CSP (with a worker or the fake worker).
  - WebKitGTK plays mp3, ogg, webm and mp4 from blob URLs with Fedora's GStreamer set.

  If the sandbox cannot keep Tauri IPC out, PDFs get "Open with system app" only.
- **7.6b Core, store and engine:** `fileblob.rs`, `blobs.rs`, `files.rs`, the kind and field keys, the query filter, the wanted list and GC, and the backup/export counts.
  - Tests: seal/open round trips plus tamper, truncation and reordered chunks; the size cap and the decompression bound; GC after purge; a property test for the chunk format.
- **7.6c Sync file phase.**
  - Scenario tests in `crates/lp-engine/tests/scenario.rs`:
    - a file added on A arrives on B, also while B is locked (from its wanted list);
    - a peer without `files1` still syncs ops;
    - a corrupt or oversize blob is refused;
    - the budget and the idle timeout hold;
    - a purge on A removes the blob on B after B unlocks.
- **7.6d Desktop UI, viewers and temp files.**
  - Shell nav: "Secure files" below Favorites, and "Files" in the mobile bottom nav.
  - `app/src/screens/Files.tsx`:
    - list, search and upload (dialog or drag-drop, then `file_add_path`; title defaults to the file name, plus a description);
    - detail with editable title and description, favorite, trash, restore and purge;
    - Export… (save dialog, with a "this copy is not protected" warning; Rust writes the file);
    - Open with system app.
  - `app/src/components/FileViewer.tsx`, which picks a viewer by the sniffed type:
    - **Text** (txt log ini conf json xml yaml toml md): decoded in Rust (UTF-8 or UTF-16 BOM, lossy) and shown with `textContent` in monospace, with wrap and JSON pretty-print toggles. Markdown is shown as text. Above 5 MB, the first 5 MB plus "Open with…".
    - **Images:** bytes come as a `tauri::ipc::Response`, become a Blob URL and go in an `<img>` with fit and zoom.
      - png, jpeg, bmp and webp display as they are.
      - TIFF is converted to PNG in Rust (`image` tiff feature).
      - HEIC is decoded by Windows WIC when the HEIF extension is installed, otherwise "Open with…". No HEVC decoder is bundled (patents; Fedora leaves it out too).
    - **Audio/video** (mp3 wav ogg mp4 webm): `<audio>`/`<video controls>` on a Blob URL. A codec error shows a message and "Open with…".
    - **PDF:** `pdfjs-dist` ≥ 4.2.67 (CVE-2024-4367) with `isEvalSupported:false` and no scripting, in the sandboxed viewer page (a second Vite entry). Bytes are handed over with `postMessage` (transfer).
    - Blob URLs are revoked and buffers dropped on close, item switch and lock.
    - CSP adds `img-src blob:`, `media-src blob:` and `frame-src 'self'`, plus whatever the spike shows pdf.js needs. Still no remote origins.
  - New `app/src-tauri/src/temp_files.rs`:
    - Location: `<data dir>/open/<random>/<sanitized name>` (0700/0600 on Linux), opened from Rust with the opener plugin's `open_path`, so the webview gets no new permission.
    - A manifest (`open/manifest.json`) tracks the copies, and everything under `open/` is deleted at startup.
    - New Settings > Security options:
      - "Delete opened files after" 5, 10 or 30 minutes, a custom number of minutes, or never;
      - "Also delete when LocalPass locks or quits", on by default.

      The auto-lock loop checks expiries every minute.
    - An "Opened files" panel in the Files tab lists name, opened at and time left, with "Delete now" and "Delete all".
    - On Windows, a file still open in another app is shown as "in use" and retried every minute.
    - Deletion is a plain unlink. Overwriting promises nothing on btrfs or SSDs; the README says so.
  - Commands: `file_add_path`, `file_add_bytes` (raw body), `file_read`, `file_text`, `file_image`, `file_export`, `file_open_external`, `temp_files`, `temp_file_delete` and `temp_files_clear`. Long-running ones go through `blocking`, like `export_backup`. Each command also gets a wrapper in `api.ts` and an entry in `generate_handler!`.
- **7.6e Android:**
  - Upload with `<input type=file>`, then `file_add_bytes`.
  - The viewers are the same components. HEIC is decoded by Kotlin `ImageDecoder` (API 28+).
  - "Open with": the copy goes to `cacheDir/open/`, `file_paths.xml` adds `<cache-path name="open" path="open/"/>` (nothing wider), and `LpMobilePlugin.openFile` sends `ACTION_VIEW` with a read grant. The same expiry and lock rules apply, and the grant is revoked on delete.
  - Export goes through SAF `ACTION_CREATE_DOCUMENT` (`LpMobilePlugin.exportFile`).
  - Checks: the Android clippy (CI) and a new `extension/e2e/android-files.ts` on `lp_api37`: add a file, view text and an image, open with another app, expiry.
- **7.6f Security review (one fresh read-only agent, per the new rule):**
  - Scope:
    - the blob format and AEAD;
    - compression and decompression limits;
    - the sync file phase (budget, hash check, behavior while locked);
    - viewer isolation and the CSP;
    - the temp-file lifecycle;
    - the FileProvider scope;
    - the duplicates and QR commands leak no secrets;
    - the screenshot file is deleted.
  - Output: `docs/phase7-security-review.md`, fixes each with a test, and README "Security notes and limits" bullets:
    - temp copies and what other apps keep;
    - the backup leaves files out;
    - files are decrypted in memory while viewed;
    - 0.2.x devices do not carry file content.

### 7.7 Release 0.3.0 (only when the user says so)
- `bun scripts/release.ts 0.3.0`, then the user pushes the tag.
- Release notes: update every device. 0.2.x devices list file items as plain entries and do not transfer file content.

## Critical files
- Docs:
  - existing: `CLAUDE.md`, `docs/handoff-phase4-5.md`, `docs/handoff-phase6-linux.md`, `README.md`;
  - new: `docs/plans/README.md` (the plan log), `docs/plans/2026-10-28-phase6-linux.md` (moved), `docs/plans/2026-10-31-phase7-files-tools-builds.md`, `docs/handoff-phase7.md`, `docs/phase7-security-review.md`.
- Build: `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain.toml`, `app/src-tauri/Cargo.toml`, `.github/workflows/{ci,release}.yml`.
- Extension: `extension/src/decide/index.ts`, `extension/test/decide-page.test.ts`, `extension/e2e/firefox/run.ts`.
- Core and engine:
  - `crates/lp-core/src/{model.rs,fileblob.rs}`;
  - `crates/lp-store/src/{lib.rs,blobs.rs}`;
  - `crates/lp-sync/src/{protocol,session,service}.rs`;
  - `crates/lp-engine/src/{lib,items,files,duplicates,import_export}.rs`;
  - `crates/lp-engine/tests/scenario.rs`.
- App:
  - `app/src-tauri/src/{commands,lib,settings,qr,temp_files}.rs`, `app/src-tauri/tauri.conf.json` (CSP);
  - `app/src/lib/api.ts`;
  - `app/src/screens/{Shell,ItemDetail,ItemEditor,Settings,Files,Duplicates}.tsx`;
  - `app/src/components/{ui,FileViewer}.tsx`;
  - the viewer page and `app/vite.config.ts`.
- Android: `gen/android/.../LpMobilePlugin.kt`, `res/xml/file_paths.xml`.

## Verification
- **7.0:** `docs/plans/README.md` lists Phases 0-5, Phase 6 and Phase 7 with the right links and statuses. `grep -rn phase6-linux-plan` finds no stale references.
- **Every task:**
  - update its status in the handoff and in the `docs/plans/README.md` row;
  - `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`;
  - `cd app && bun run build`;
  - for extension changes, `cd extension && bunx tsc --noEmit && bun test`;
  - Linux and Windows CI stay green; Android clippy for shared Rust.
- **7.1:** before/after table (M1-M4 and disk) in the handoff. The `.comment` linker check. Windows CI plus the release build, and the installer checked on the work laptop.
- **7.2:** unit tests, the Firefox BiDi e2e (`bun run build --firefox --e2e && bun run e2e/firefox/run.ts`), and an assisted click outside.
- **7.3-7.5:** unit tests where they exist, plus a manual pass in `LOCALPASS_PROFILE=dev bun run tauri dev`:
  - a masked note;
  - QR from a test QR shown in Firefox: a clipboard paste first (a screenshot copied, then Ctrl+V; the clipboard is cleared afterwards), then a file and the screen;
  - the duplicates screen on a dev vault seeded with a.example.com, example.com and www.example.com logins.
- **7.6:**
  - Rust unit, property and scenario tests.
  - Two dev profiles on this laptop syncing a file (`LOCALPASS_PROFILE=dev` and `dev2`).
  - Every listed type opened in the app; Open with, expiry and the "delete on lock" check.
  - Export and checksum compared to the original.
  - Android e2e on `lp_api37`.
  - The security review's fixes, each with a test.
