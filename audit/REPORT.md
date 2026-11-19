# LocalPass independent security and logic review

| | |
|---|---|
| Commit | `742c89ace7fbaf31042f6e00a070c5d62dad76d6` (`742c89a`, branch `main`) |
| Tree state | clean except the untracked `audit/` folder (this review) |
| Date | 2026-11-15 |
| Model | Claude Fable 5.2 (`claude-fable-5-2`), with four read-only sub-reviewers of the same model |
| OS | Fedora Linux 46 (Workstation, prerelease), kernel 7.4.0-rc2, x86_64 |
| Scope | All Rust crates, the extension, the Tauri app and the hand-written Android Kotlin, read-only. Nothing was run against a real vault. |
| Time | About 5.5 hours of wall-clock time (11:10 to 16:30 UTC) including two forced pauses; roughly 2.5 hours of active work. |

## 1. Verdict

**Issues found: 1 High, 3 Medium, 18 Low, 6 Info, 9 logic bugs.** The High finding is new (no earlier review mentions it): removing a device does not reliably keep it out.

How to read the proofs: two findings have failing tests in `audit/poc/`. All security findings are proven by code traces that can be followed line by line. No runnable attack test was written for them; section 4 of the brief allows a trace as proof.

## 2. Coverage

| Area | Coverage | Notes |
|---|---|---|
| lp-core (crypto, ops, CRDT, roster, HLC, generator, TOTP) | Reviewed fully | `origin.rs` and `passkey.rs` were read by a sub-reviewer, not by me. |
| lp-store | Reviewed fully | |
| lp-sync (Noise, sync, pairing, mDNS, service) | Reviewed fully | |
| lp-engine | Partially | `lib.rs`, `session.rs`, `keys.rs`, `devices.rs`, `backup.rs`, `import_export.rs`, `secret.rs` in full; `credentials.rs` by a sub-reviewer; `items.rs`, `duplicates.rs`, `provider.rs`, `webauthn_json.rs` only where a call was followed. |
| lp-import | Reviewed fully | The `keepass` crate itself only for its key-derivation and decompression code. |
| lp-cli | Partially | Data dir, secrets, password handling and exports; the rest skimmed. |
| lp-broker | Reviewed fully (sub-reviewer, findings checked by me) | Windows pipe behaviour reasoned from code and Win32 documentation, not run. |
| lp-winauth | Partially (sub-reviewer) | Struct layouts not checked against SDK headers; tests and `register.rs` not read. |
| extension/ | Reviewed fully (sub-reviewer, findings checked by me) | Static only; no browser run. |
| app/ (Tauri, UI) | Partially (sub-reviewer, findings checked by me) | Commands, capabilities, CSP, broker prompts, clipboard, logs read; most UI screens only searched for sinks; `focus.rs`, `prompt_window.rs`, most of `windows_passkeys/` not read. |
| Android Kotlin | Partially (sub-reviewer) | Autofill, credential provider, device secret and biometric code read; no device or emulator. |

## 3. Attack surface

- **Network:** one TCP listener on `0.0.0.0:47821` (falls back to a random port). The first 4 bytes select sync (`LPS1`) or pairing (`LPP1`) (`crates/lp-sync/src/service.rs:909-932`). mDNS advertises and browses `_localpass._tcp` and, during an invite, `_localpass-pair._tcp` (`discovery.rs`).
- **Local IPC:** the broker, a named pipe on Windows and a Unix socket under `$XDG_RUNTIME_DIR/localpass/` on Linux (`crates/lp-broker/src/pipe.rs`, `pipe/unix.rs`). The native-messaging helper (`nmhost.rs`) relays stdio to it. The Windows passkey provider is a COM local server and a broker client.
- **Tauri:** one window, about 60 commands (`app/src-tauri/src/commands.rs`), capabilities `core:default`, three dialog permissions and `opener:allow-open-url` for http(s). No shell, fs or http plugin. CSP: `default-src 'self'; script-src 'self'; connect-src ipc: http://ipc.localhost`.
- **Extension:** background `onMessage`/`onConnect`, a content script and a MAIN-world script on https pages and localhost, no `web_accessible_resources`, no external messaging.
- **Android:** exported launcher activity, credential provider service and autofill service (both behind system bind permissions).
- **File parsers:** Bitwarden JSON, CSV, KeePass `.kdbx`, `.lpbackup`, QR images.
- **Where secrets are decrypted:** `keys::open_vault_keys` (unlock, quick unlock), `SignedOp::decode_data` (every data op while unlocked), `keys::open_net` (network keys, at start, with the device secret), `backup::open`.

## 4. Tool results

| Check | Result |
|---|---|
| `cargo build -p lp-broker --examples` | ok |
| `cargo test --workspace` | **381 passed, 0 failed, 2 ignored** (both need a desktop session: Wayland clipboard, Secret Service) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo audit` (installed for this run) | **3 vulnerabilities**, 10 warnings. `libsqlite3-sys` 0.38.2 (RUSTSEC-2026-0459, SQLite CVE, medium 6.8; fix 0.39.0). `crypto-bigint` 0.7.5 (RUSTSEC-2026-0387) and `ctutils` 0.4.3 (RUSTSEC-2026-0385): timing variability, both under `p256`. Warnings: 4 unmaintained (`proc-macro-error`, `zbus_names`, `zvariant`, `zvariant_derive`), 3 unsound (`glib` 0.18.5, `rustix` 1.1.5, `tokio` 1.53.2), 3 yanked (`async-executor`, `clipboard-win`, `crypto-bigint`). |
| `cargo deny check advisories` (installed for this run, default config: the repo has no `deny.toml`) | **FAILED**, 8 errors: the same 3 vulnerabilities, 4 unmaintained, 1 unsound (`tokio`). |
| `app`: `bun install`, `bun run build`, `bunx tsc --noEmit` | ok, clean |
| `app`: `bun audit` | no vulnerabilities (97 packages) |
| `extension`: `bun install`, `bun run test`, `bunx tsc --noEmit` | **443 passed, 0 failed** (17 files); type-check clean |
| `extension`: `bun audit` | **22 advisories** (1 critical, 11 high, 8 moderate, 2 low), all in development tooling (`web-ext`, `happy-dom` and their dependencies). The extension has no runtime dependencies, so none of this ships. |

Not run: everything that needs Windows, Android, a real browser or a person (all `e2e/*` harnesses, ASSISTED checks, the installer smoke test, Kotlin unit tests).

## 5. Findings

Attackers: A disk thief, B same-LAN, C paired or revoked device, D website, E other local process or OS user, F import file, G shoulder surfer.

"Known" means an earlier review or the README already lists it; it is still open at this commit.

| Id | Title | Severity | Confidence | Attacker | Location |
|---|---|---|---|---|---|
| H-1 | A removed device can be admitted again and keep reading and writing | High | High (trace) | C | `crates/lp-sync/src/replica.rs:161-197` |
| M-1 | Revocations written by an already-revoked device still count; its later data is applied without review | Medium | High (trace) | C | `crates/lp-core/src/roster.rs:132-139`, `crates/lp-engine/src/session.rs:99-113` |
| M-2 | Toolbar-popup fill trusts the tab URL for a sandboxed top-level document | Medium | Medium (not run in a browser) | D | `extension/src/background/index.ts:415-428`, `extension/src/content/index.ts:218` |
| M-3 | Android autofill uses the first web domain in the screen for every field (known, RD-3) | Medium | Low (needs a device) | D | `AutofillParser.kt:70-89` |
| L-1 | TOTP parser panics on some non-ASCII text; during import this breaks the engine until restart | Low | High (test) | F, C | `crates/lp-core/src/totp.rs:78` |
| L-2 | KeePass import accepts unlimited key-derivation cost (known, documented) | Low | High | F | `crates/lp-import/src/keepass_import.rs:22` |
| L-3 | `lp` CLI and exports leave key material and plaintext readable by other OS users | Low | High | E | `crates/lp-engine/src/secret.rs:431-442`, `crates/lp-cli/src/main.rs:29-33,194,678,696`, `app/src-tauri/src/commands.rs:684,692` |
| L-4 | The extension's client secret gives silent passwords for any site the caller names (documented limit; a code comment says otherwise) | Low | High | E | `crates/lp-broker/src/server.rs:828-854` |
| L-5 | An unpaired local process can raise unlock and pairing prompts without limit and block real pairing | Low | High | E | `crates/lp-broker/src/server.rs:482-492,575-611` |
| L-6 | A browser-pairing or "Replace?" dialog stays answerable after the vault locks | Low | High | E | `app/src-tauri/src/broker.rs:458`, `app/src/App.tsx:65`, `app/src-tauri/src/commands.rs:931` |
| L-7 | Site names and item titles are written to the console log and the provider log | Low | Medium | A, E | `app/src-tauri/src/broker.rs:556`, `crates/lp-winauth/src/handler.rs:337` |
| L-8 | No lock on suspend or screen lock; the idle timer does not count suspended time on Linux | Low | Medium | thief of a resumed machine | `app/src-tauri/src/lib.rs:294-306` |
| L-9 | User checks are not tied to what is approved | Low | High | E | `crates/lp-broker/src/server.rs:732-747` |
| L-10 | The broker session is not bound to its handshake | Low | Medium | E | `crates/lp-broker/src/protocol.rs:47-55` |
| L-11 | Another Windows account can switch the broker off by creating the pipe name first (known, RB-5) | Low | Medium (not run) | E | `crates/lp-broker/src/pipe.rs:253-263` |
| L-12 | Sync listener, mDNS and pairing can be slowed or filled up from the LAN | Low | High | B | `crates/lp-sync/src/service.rs` |
| L-13 | "Already filled" check compares passwords across sites | Low | Medium | D | `extension/src/background/index.ts:163-165,213-214` |
| L-14 | A page can learn whether a passkey exists, and test one guessed login per real click | Low | Medium | D | `extension/src/decide/index.ts:304`, `extension/src/content/index.ts:125-133` |
| L-15 | Long host names lose their right end in the panel | Low | High | D | `extension/src/shared/limits.ts:61-65` |
| L-16 | Copied secrets stay on the clipboard after quit or lock | Low | High | E | `app/src-tauri/src/clipboard.rs:64-82` |
| L-17 | Hardening: `get_item` sends all secrets to the web view; `popupOnly` fails open | Low | High | D (after another bug) | `app/src-tauri/src/commands.rs:407`, `extension/src/background/index.ts:111-114,488-490` |
| L-18 | Dependencies with open advisories | Low | High | varies | `Cargo.lock` |
| I-1 | No key rotation on removal (documented) | Info | High | C | `crates/lp-engine/src/lib.rs:1027-1051` |
| I-2 | Android: `allowBackup="false"` without data-extraction rules | Info | Low | A | `AndroidManifest.xml:29-30` |
| I-3 | Windows Hello key signs a fixed challenge | Info | Medium | E | `app/src-tauri/src/hello.rs:133-149` |
| I-4 | The dialing side sends its Hello before it has checked the peer | Info | High | C | `crates/lp-sync/src/session.rs:70-79` |
| I-5 | Any import file can add passkey items | Info | High | F | `crates/lp-engine/src/import_export.rs:44-55,273` |
| I-6 | A saved address without a port matches every port (documented) | Info | High | D, E | `crates/lp-core/src/origin.rs:184-188` |

### H-1. A removed device can be admitted again and keep reading and writing

**What is wrong.** When a peer's Noise key is not in the roster, `verify_peer` builds a temporary roster from the stored roster records plus the peer's membership proof. A proof op is skipped only if a roster op with the same author and sequence number is already stored. Nothing compares a proof op with a *data* op stored at that position, so a proof op that contradicts the stored log is accepted.

**Trace.**
1. `crates/lp-sync/src/replica.rs:77-91`: `RosterCache.ops` is filled from `ops_by_class(OpClass::Roster)`. Data ops are not in it.
2. `replica.rs:174-187`: for each proof op the code checks vault id, class and signature, then `if !cache.ops.contains_key(&op.op_ref())` adds it to the temporary records. There is no lookup in the store and no fork check.
3. `crates/lp-core/src/roster.rs:146-147`: an "A adds Y" edge is valid when `e.seq <= cutoff` of A. A revoked device's edge is therefore valid if its sequence number is at or below the cutoff.
4. `roster.rs:163-167`: the added device has no cutoff of its own, so its status is `Active`; `replica.rs:190-191` returns it as the peer.
5. Every device log has a data op at a low sequence number: `write_device_meta` runs right after `create_vault` and `join` (`crates/lp-engine/src/lib.rs:310`, `1249`). `Engine::revoke` sets the cutoff to the highest sequence number it has seen (`lib.rs:1038`), so such a position is always at or below the cutoff.
6. The session then proceeds normally: `crates/lp-sync/src/session.rs:86-105` (responder) sends everything the peer lacks (`ops_missing_for`), and `receive_ops` ingests what the peer sends.
7. `replica.rs:254`: `ingest` stores ops from any *known* author, revoked ones included. `crates/lp-engine/src/session.rs:99-113` applies their data ops (see M-1).
8. `crates/lp-engine/src/lib.rs:1027-1051`: `revoke` writes one roster op. The vault key and the network key are unchanged (I-1), so the removed device still passes the Noise pre-shared-key step (`channel.rs:64-67`) and can decrypt whatever it receives.

**Limits.** It does not work when the revoker had never received an op from the removed device (cutoff 0), or for a first device revoked with cutoff 1.

**Aggravating.** After such a session the member stores the peer in its dial list (`service.rs:972-983`) and will dial it later. `Engine::devices()` lists only the stored roster (`lib.rs:990-1011`), so the user never sees this device.

**Precondition.** The attacker holds the removed device's keys, including its signing key. That key is wrapped with the master password (`keys.rs`), so this means a thief who knows that device's master password, a device taken while unlocked, or a device that was compromised before it was removed. The attacker must also reach one member on the local network.

**Impact.** Removal does not end access. The removed device's holder keeps receiving all new vault data, for example passwords changed after the theft, and can still send ops that every device stores and applies. This contradicts the README ("can act as that device until you remove it") and the comment on `Engine::revoke` ("It can no longer connect").

**Suggested fix.**
- In `verify_peer`, reject a proof op whose (author, seq) is at or below the stored maximum for that author unless the stored op has identical header bytes.
- Longer term, rotate the vault key and network key on removal, so a missed check cannot expose new data.
- Add a test: a proof that conflicts with a stored op must give `NotMember`.

### M-1. Revocations written by an already-revoked device still count; its later data is applied without review

**What is wrong.**
- `crates/lp-core/src/roster.rs:132-139` counts a `DeviceRevoked` op whenever its author is "known". It does not check that the op's own sequence number is at or below its author's cutoff. `DeviceAdded` ops are checked (`roster.rs:147`).
- The doc comment on `RosterOp::DeviceRevoked` says "Roster ops it authored after `cutoff_seq` are void" (`crates/lp-core/src/model.rs:241`). The module comment at `roster.rs:6-7` describes the current behaviour ("any revocation of it authored by a known device"), so the two disagree and the behaviour may be deliberate.
- Data ops from a revoked device after its cutoff get the policy `ApplyFlagged`, described as "applied, but surfaced for review" (`roster.rs:50-51`). `crates/lp-engine/src/session.rs:111-112` records them in `flagged`, and nothing in the workspace reads that field.

**Precondition.** The removed device's ops must still reach a member: through a device that has not yet learned of the removal, or through H-1.

**Impact.**
- A removed device can mark the remaining devices as revoked. Revocation is permanent in this model, and devices that see each other as revoked refuse to sync (`replica.rs:164-167`), so sync can stop for good.
- Its later edits and purges are applied on every device with no notice to the user.

**Suggested fix.** Ignore revocations whose sequence number is above their author's cutoff (resolve cutoffs in a fixed order to stay deterministic). Either skip post-cutoff data ops or show them in the UI as the comment promises.

### M-2. Toolbar-popup fill trusts the tab URL for a sandboxed top-level document

**What is wrong.** The popup path takes the site from `tab.url` (`extension/src/background/index.ts:389-390`, `415-417`) and the content script accepts the fill when `message.origin === location.origin` (`extension/src/content/index.ts:218`). In a document served with `Content-Security-Policy: sandbox`, the real origin is opaque, but `location.origin` is still derived from the URL. The badge and panel path is safe: it uses `sender.origin`, which is `"null"` there, and `frameOrigin` refuses it (`index.ts:99-102`).

**Precondition.** A site that serves user-supplied HTML as a top-level page on the same host as its login, isolated only by a sandbox CSP, and a user who fills from the toolbar popup on that page. Not verified in a browser: that the content script is injected into such a document.

**Impact.** The site's saved password, and its one-time code if stored, are typed into a page the site itself treats as untrusted. The address bar shows the real host.

**Suggested fix.** In the content script, compare with `self.origin` and refuse `"null"`; in `fillTab`, target the document (`documentId`) that was checked.

### M-3. Android autofill uses the first web domain in the screen for every field (known, RD-3)

**What is wrong.** `AutofillParser.scan` sets the domain from the first node that has one and collects fields from every node of every window (`AutofillParser.kt:70-89`). `parse`, `typed`, `LpAutofillService` and `AutofillActivity.checkedForm` (`AutofillActivity.kt:78-88`) all use that single domain. No code compares a field's own domain with the form's.

**Precondition.** A trusted browser whose structure reports a cross-origin frame's fields with a different domain. Not confirmed for any listed browser.

**Impact if the precondition holds.** The login for the top site is offered for, and after one tap filled into, a field in another site's frame. This would be High.

**Status.** Phase 5 recorded this as RD-3 (Low, suspected) and left it for a device run. It is unchanged.

**Suggested fix.** Use the nearest ancestor `webDomain` of the password node, require the same for the user-name node, and refuse structures with two domains.

### L-1. TOTP parser panics on some non-ASCII text

- **Proof:** `audit/poc/tests/totp_parse_panic.rs` fails at this commit: `end byte index 10 is not a char boundary` at `crates/lp-core/src/totp.rs:78`.
- **Reach:** import (`crates/lp-engine/src/import_export.rs:314`) calls the parser inside `with_session`. A panic there poisons the session mutex, and later calls that do `.lock().expect("session")` panic until the app restarts. The same parser runs on stored values in `lib.rs:843,865` and `credentials.rs:258`, which a paired device could write directly.
- **Impact:** a crafted Bitwarden JSON or CSV makes the app unusable until restart. `Engine::status()` and `Engine::lock()` use the same mutex, so the auto-lock task stops (`app/src-tauri/src/lib.rs:299`) and the vault cannot be locked: the decrypted data stays in memory until the process exits. Nothing is written or leaked.
- **Fix:** use `input.get(..10)`; add a property test over arbitrary strings.

### L-2. KeePass import accepts unlimited key-derivation cost (known, documented)

- **Trace:** `keepass_import.rs:22` hands the file to `keepass::Database::parse`. `keepass-0.15.2/src/crypt/kdf.rs:40` loops `for _ in 0..self.rounds` (u64); `kdf.rs:80-82` passes Argon2 memory and iterations as stated. Both run before the file is authenticated. The inner payload is decompressed with `read_to_end` and no size limit (`compression.rs:43`).
- **Status:** Phase 5 RB-3; the README warns about it.
- **Fix:** read the header first and apply the limits of `KdfParams::validate`; cap decompressed size.

### L-3. `lp` CLI and exports leave key material and plaintext readable by other OS users

- Off Windows, `FileSecretStore` writes the device secret unprotected with `std::fs::write` (`crates/lp-engine/src/secret.rs:393-415`; off Windows `platform::protect` returns the bytes unchanged, `secret.rs:431-442`).
- The CLI's data dir defaults to `.localpass-dev/cli` and is created with the default umask; no code restricts it (`crates/lp-cli/src/main.rs:29-33,194`). The app restricts its own dir to 0700 (`app/src-tauri/src/lib.rs:424-425`).
- With `device.key` and `vault.db`, another OS user gets the network key and the Noise key (enough to connect as this device and fetch ciphertext) and the wrapped vault keys for offline guessing.
- Plain exports and backups are written with `std::fs::write` in the CLI (`main.rs:678,696`) and the app (`commands.rs:684,692`): 0644 under a usual umask.
- **Fix:** create the CLI dir 0700 and these files 0600.

### L-4. The extension's client secret gives silent passwords for any site the caller names

- **Trace:** `server.rs:828-830` answers `findLogins{origin}` with no prompt; `server.rs:849-854` answers `fillLogin` silently when the login matches the origin string the client sent (`credentials.rs:252`), returning password and current TOTP code.
- The comment at `server.rs:832-834` says a program that merely holds the client secret "gets nothing silently". That holds only for sites the caller cannot name.
- **Status:** the README states this limit for malware running as the same user, and Phase 6 puts it out of scope. I keep it as Low because the code comment overstates the protection.
- **Fix:** correct the comment; consider a rate limit on distinct origins per session.

### L-5. An unpaired local process can raise unlock and pairing prompts without limit

- `server.rs:482-492`: a `Hello` with no known id leads straight to `pair`.
- `server.rs:575-585`: one pairing at a time; all others get `busy` for up to 180 s (`server.rs:608-611`).
- `app/src-tauri/src/broker.rs:458`: with a locked vault the unlock prompt starts for that unpaired client. With quick unlock on this is a Windows Hello prompt with a fixed text (`broker.rs:528-531`, `commands.rs:251-259`); otherwise the unlock screen shows the name the client chose (`broker.rs:537`). With a locked vault the wait ends after 110 s.
- There is no back-off; `unlock_failures` is not used on this path.
- **Impact:** prompt spam, blocked pairing for the real extension, and a chance that a reflexive approval pairs the wrong client.
- **Fix:** back off after a refused or timed-out pairing; do not start the unlock for an unpaired client until the user opens the dialog.

### L-6. A pairing or "Replace?" dialog stays answerable after the vault locks

- `broker.rs:458` checks the vault status once, before the dialog opens.
- `App.tsx:65` clears only the device-pairing dialog on lock; the browser-pairing and overwrite modals render outside the status switch (`App.tsx:217`).
- `commands.rs:931-935`: `broker_respond` has no status check; `server.rs:621-631` stores the client on approval.
- **Impact:** someone at the keyboard of a locked LocalPass can approve a pending browser pairing. The overwrite dialog over the lock screen shows a login's title, user name and site.
- **Fix:** close these dialogs on lock and refuse `broker_respond` approvals while locked.

### L-7. Site names and item titles reach the console log and the provider log

- `broker.rs:556`: `info!(reason, "asking for Windows Hello")` runs on every platform. `reason` is `Fill "<title>" on <host>` (`server.rs:863-867`) or a relying-party id.
- `app/src-tauri/src/lib.rs:371-378`: the console layer prints info-level events to stdout. On Linux, stdout of a desktop-started app normally goes to the user journal (assumption about the launcher).
- `crates/lp-winauth/src/handler.rs:337-343,399-403` log the relying-party id to `provider.log` (known, RC-8).
- **Fix:** log the reason at debug level or drop it.

### L-8. No lock on suspend or screen lock

- On desktop, nothing under `app/src-tauri/src` reacts to suspend or session lock (Android locks on a wall-clock background timer, `android.rs:126-135`).
- The auto-lock loop uses `Instant::elapsed()` (`lib.rs:302`), which on Linux does not advance during suspend; `android.rs:39-41` uses wall-clock time for this reason.
- **Impact:** a laptop suspended with the vault unlocked resumes unlocked.
- **Fix:** lock on suspend and screen lock; measure idle time with a clock that counts suspend.

### L-9. User checks are not tied to what is approved

- `server.rs:739-741`: if this request caused an unlock, that counts as user verification for a passkey signature. The unlock screen showed only the client's name (`server.rs:714`).
- `broker.rs:554-556`: on Windows the reason for a Hello check is only logged, so the user cannot tell "search", "fill X on Y" and "sign in to Z" apart.
- `server.rs:742-744`: with "verify every passkey" switched off, a client that asks for `uv=discouraged` gets a signature with no prompt in the app. The setting lives in unauthenticated `settings.json`.
- **Fix:** show the site in the unlock and Hello prompts where the platform allows; keep presence confirmation in the app for direct pipe clients.

### L-10. The broker session is not bound to its handshake

- The proofs cover a label, both nonces and the client id (`protocol.rs:47-55`). After `Ready` the stream is plain JSON with no key or MAC (`server.rs:364-442`, `client.rs:431-481`), and `Paired{secret}` is sent in clear (`server.rs:633-640`).
- A same-user process that can put itself on the path (rename the socket in the user's own 0700 dir on Linux, add a pipe instance on Windows) can relay the handshake and then read or change requests.
- Comments at `pipe.rs:3-4` and `unix.rs:11` claim more than this gives.
- **Fix:** derive a session key from the handshake and MAC or encrypt every frame.

### L-11. Another Windows account can switch the broker off (known, RB-5)

- `pipe.rs:253-263`: the first instance uses `first_pipe_instance(true)`; if the name exists, creation fails. `app/src-tauri/src/broker.rs:685-687` only logs it; there is no retry and no message to the user.
- Clients refuse a pipe owned by another account (`pipe.rs:162-176`), so nothing leaks. Browser features stay off for that run.
- Not run (Windows).

### L-12. Sync listener, mDNS and pairing can be slowed or filled up from the LAN

- The listener binds `0.0.0.0` and accepts any source address (`service.rs:496-505`, `555`); only outgoing dials are limited to local addresses. The Noise pre-shared key still stops non-members.
- Inbound slots: 4 per address, 16 in total (`service.rs:38-41`). A host with four addresses can hold all of them for 5 s at a time.
- `on_found` (`service.rs:674-732`) creates a peer entry for every advert with a current vault tag. A tag can be copied from a real advert for up to about three hours, entries and `mdns_names` are never pruned, and each entry is dialed.
- One pairing attempt at a time (`service.rs:1068`), each step may wait 15 s (documented in the README). Three wrong codes close the invite (by design).
- **What held up:** an outsider cannot get past the Noise handshake; a wrong pairing guess costs one of three tries and the joiner must confirm first, so there is no offline guessing (`pairing.rs:385-390`).

### L-13. "Already filled" check compares passwords across sites

- `rememberFill` stores a user name and an unsalted SHA-256 of the filled password per tab, with no origin, until the tab closes (`index.ts:163-165`, `567`).
- `storeCapture` drops a capture whose password hash equals it; an empty user name skips the name test (`index.ts:213-214`).
- A later page of another site in the same tab can therefore test a guess of the earlier password. A normal page gets about one guess per 1.5 s and each miss shows a save panel; a compromised renderer can ask `pendingSave` and see the difference silently.
- **Fix:** store the origin with the hash and compare only on that origin; clear it on navigation to another site.

### L-14. A page can learn whether a passkey exists, and test one guessed login per real click

- Passkeys: a non-conditional `get` opens the panel; with no match the panel answers `fallback` at once (`decide/index.ts:304`) and the page receives `{fallback:true}` (`content/passkeys.ts:88`). With a match nothing is sent. With `allowCredentials` this tells the page which credential ids exist. Same-site pages only.
- Logins: `sentByUser` proves a real click, not who filled the fields (`content/index.ts:125-133,172-180`). After one real click the background asks `checkLogin`, and `unchanged` is dropped silently (`background/index.ts:296-302`). Phase 8 marked F1 fixed; this is the part that remains, and a comment at `index.ts:220-221` concedes it.
- **Fix:** delay the passkey fallback as browsers do; treat `unchanged` like any other capture unless the fields were typed.

### L-15. Long host names lose their right end in the panel

- `displayText` keeps the first 79 characters (`limits.ts:61-65`); it is applied to the host in the decision header, pick view and save view (`decide/index.ts:404,361-363,315`).
- A host such as `accounts.google.com.signin…` shows only attacker-chosen labels. Secrets stay bound to the real origin; this weakens the user's own check. The Android code keeps the right end.
- **Fix:** show the registrable domain, or cut from the left.

### L-16. Copied secrets stay on the clipboard after quit or lock

- The clear runs in a detached thread after the delay (`clipboard.rs:64-82`). Quit is `app.exit(0)` with no clear (`lib.rs:344-347`); `Engine::lock` does not touch the clipboard.
- Android clears whatever is on the clipboard after the delay, also something the user copied later (`LpMobilePlugin.kt:80-84`).
- **What held up:** the desktop clear checks that the clipboard still holds the copied value; Windows history and cloud sync are excluded (`clipboard.rs:44-50`).

### L-17. Hardening

- `get_item` returns every secret field of an item to the web view when it is opened, not on reveal (`commands.rs:407`, `crates/lp-engine/src/items.rs:361-408`). CLAUDE.md says "Secrets stay in Rust"; that is true for lists only. App commands are not scoped per window. No script-injection sink was found in `app/src`.
- `popupOnly()` only refuses senders that have a `tab` (`background/index.ts:111-114,488-490`); it should require the exact popup URL, as `authorize` does.
- `import_file`, `export_json` and `export_backup` take a path string from the web view (`commands.rs:651-692`).

### L-18. Dependencies with open advisories

See section 4. `libsqlite3-sys` is used through `rusqlite` in `lp-store`; all SQL is parameterised and the database is local. The two timing advisories sit under `p256`, which signs with passkey private keys. I did not analyse whether either is reachable in a way that matters here; update them.

### Info

- **I-1.** `Engine::revoke` rotates no keys (`lib.rs:1027-1051`). The README says so. It is what makes H-1 serious.
- **I-2.** `allowBackup="false"` with target SDK 37 and no `dataExtractionRules`. My understanding is that device-to-device transfer can still copy app data on some phones; not checked on a device. The device secret is Keystore-bound and the vault needs the master password.
- **I-3.** The quick-unlock key is derived from a Hello signature over a fixed challenge (`hello.rs:133-149`), so any same-user process that gets one Hello approval can derive it. Same-user malware is outside the README's model.
- **I-4.** The dialing side sends its version vector, addresses and hints before it has verified the peer (`session.rs:70-79`). A removed device that is dialed learns those. The responder verifies first.
- **I-5.** Custom fields named `passkey.*` turn an imported item into a passkey for any import format, not only backups (`import_export.rs:44-55,273`). The key would be the attacker's, so no site accepts it; a duplicate credential id is skipped.
- **I-6.** Port matching as documented in the README; this includes `http://localhost` on any port.

## 6. Logic bugs

| Id | Bug | Proof |
|---|---|---|
| B-1 | A value saved with a far-future clock wins over every later edit. `HlcClock::observe` ignores clocks more than 24 h ahead (`hlc.rs:53-56`), but `VaultState::apply` still orders by that time (`materialize.rs:143-169`). The README mentions wrong clocks and the app shows a toast above one minute of skew (`App.tsx:73`); the later edit is still lost silently. | `audit/poc/tests/future_timestamp_sticks.rs` fails at this commit. |
| B-2 | Windows only: a vault unlocked for a browser with Windows Hello locks again within 5 seconds. `commands::quick_unlock` on the broker path never updates `last_activity` (`commands.rs:247-262`; the UI command does at `:320`), so the next tick of the auto-lock loop locks it (`lib.rs:302-305`). | Trace. Not run (Windows). |
| B-3 | "Forget this browser" can silently fail to stick. `ClientStore::remove` drops the client from memory and then saves (`app/src-tauri/src/broker.rs:99-103`); on a save error `forget_client` returns before `revoke` (`:703-704`), and the file still holds the client at the next start. | Trace. |
| B-4 | Replacing a passkey keeps the old user name in lists: `USERNAME` is written only for a new item (`credentials.rs:632-648`). | Trace. |
| B-5 | `plan_save_login` picks one candidate by `(kind, id)` (`credentials.rs:360`). With two logins of the same user name on a site, it can ask to overwrite one although the other already holds this password. | Trace. |
| B-6 | Imports drop data without a warning: Bitwarden `fido2Credentials` (passkeys) and `passwordHistory` (`bitwarden.rs:50-60`), KeePass attachments, tags and expiry (`keepass_import.rs:32-71`). | Trace. |
| B-7 | `rename_this_device` writes the local name before the session check (`lib.rs:1021-1022`): a call while locked changes the local name only and then fails. | Trace. |
| B-8 | A waiting save offer is destroyed by any newer request in the same tab, and the captured password is lost (`decide.ts:476-479`, `933-937`). Example: generate a password on sign-up, close the save panel, click the badge on the next form. | Trace. |
| B-9 | Captures fire on any button-like click inside a form, for example "show password" or Cancel (`content/index.ts:172-180`), so abandoned passwords are offered for saving. | Trace. |

Smaller items reported by sub-reviewers and not checked by me: a stale decision panel closes instead of turning into the popup; Save after the 2-minute capture life fails without a message; the pairing code appears about 4 s late in the popup; a broker request that times out after 130 s may still complete in the app; bytes a client sends while its pairing waits are lost; a client record is stored before `Paired` is sent; two app instances starting after a crash can remove each other's socket.

## 7. Disproof pass

A separate reviewer was given ten claims (H-1, M-1, L-4, M-3, L-11, L-1, L-2, L-5 with L-6, L-7, L-8 with B-2) and the files, without my reasoning, and asked to refute them.

**Result: 8 stand, 2 stand but narrower, none refuted.** The reviewer read the code and existing tests; nothing was executed.

| Claim | Verdict | What changed in this report |
|---|---|---|
| H-1 | Stands | Added its limits (it fails when the revoker never received an op from the removed device) and two aggravating points the reviewer found. |
| M-1 | Stands | Added that the module comment at `roster.rs:6-7` describes the current behaviour, so the two comments disagree. |
| M-3 | Stands as a conditional | No change; the browser behaviour cannot be decided from this repository. |
| L-1 | Stands | Added that auto-lock and Lock also fail after the panic. |
| L-2, L-4, L-7, L-11 | Stand | No change. |
| L-5, L-6 | Narrower | The Windows Hello prompt shows a fixed text, not the client's name; the 180 s wait applies to an unlocked vault (110 s when locked). Text corrected. |
| L-8, B-2 | Narrower | The re-lock (B-2) is Windows-only; Android has its own background lock. Text corrected. |

Nothing was dropped by the disproof pass.

**Not put through the separate disproof pass:** M-2 and the other extension findings (L-13 to L-15, L-17) and L-3, L-9, L-10, L-12, L-16, L-18. The extension report arrived after the pass had started. For these the only second look is my own check of the cited lines against the code.

Changes I made myself after reading the earlier reviews and the README (step 5):

| Item | Change | Why |
|---|---|---|
| Client secret gives silent fills (L-4) | Medium to Low | The README states it as a limit and same-user malware is declared out of scope. What remains is a wrong code comment. |
| Unpaired prompts, session binding, user-check binding (L-5, L-9, L-10) | kept Low | All need a process running as the same user, which the project puts out of scope. |
| Pipe-name squatting (L-11) | Medium to Low | Known (RB-5), no data exposure, shared PCs only. |
| KeePass cost (L-2) | kept Low, marked known | Documented in the README. |
| Far-future clock (B-1) | kept as a logic bug, marked documented | README mentions wrong clocks. |
| Pairing approval cannot tell an attacker who saw the code from the real device | dropped as a finding | The approval shows the joiner's key fingerprint (`lib.rs:1112-1117`); comparing it is the intended check. I did not verify that the joining device shows its fingerprint before joining. |
| Roster flood by a member (quadratic recompute) | dropped | A member can already delete everything; no extra power. |

## 8. Comparison with earlier reviews

| Earlier claim | My result |
|---|---|
| Phase 5: "There was no High finding" | **Contradicted** by H-1. The roster, proof and revocation code is not discussed in any of the three reviews. |
| README: a thief "can act as that device until you remove it" | **Contradicted** by H-1 and M-1. |
| README: "Removing a device doesn't rotate the vault keys yet" | Confirmed (I-1). |
| Phase 5 RB-4: inbound slots limited per address | Confirmed fixed (`service.rs:38-41,513-526`); residual in L-12. |
| Phase 5 RB-6: unplaceable ops bounded at 16 MiB | Confirmed (`replica.rs:20,282-287`). |
| Phase 5 RB-9: joining dials only local addresses | Confirmed (`pairing.rs:249-252`). |
| Phase 5 RB-5 / RC-1: clients refuse a pipe served by another account | Confirmed by sub-reviewer (`pipe.rs:162-176`); the outage part is still open (L-11). |
| Phase 5 RB-3: KeePass cost, left as a documented limit | Confirmed still open (L-2). |
| Phase 5 RD-3: autofill first `webDomain`, left for a device run | Confirmed still open (M-3). |
| Phase 5 RC-8: provider log names sites | Confirmed still open (L-7). |
| Phase 5 RB-1 / RB-2: passkeys only in the sealed backup; private key not copyable | Confirmed (`import_export.rs:419-453`, `lib.rs:825-829`). |
| Phase 5 RA-1, RA-2, RA-3, RA-5, RA-7, RB-7, RD-1, RD-2, RD-4, RD-5 | Not checked individually. |
| Phase 6 L4: data dir set to 0700 | Confirmed for the app (`lib.rs:424-425`); the CLI is not covered (L-3). |
| Phase 6 L1, L2, L3, L5 | Not checked. |
| Phase 6: "no High or Medium finding" for the Linux and Firefox work | Not contradicted within that scope. |
| Phase 8 F1: `checkLogin` guess oracle, fixed | **Partly**: the scripted-submit case is closed (`byUser`), one real click still allows one silent test (L-14). |
| Phase 8 "checked and sound": popup-only messages | Confirmed for tab content scripts; fails open for a sender without a tab (L-17). |
| README: backup header costs are capped | Confirmed (`crypto.rs:172-186`, `backup.rs:63`). |
| README: KDF, AEAD and signature design | Confirmed: Argon2id 64 MiB, 3 passes, 16-byte random salt; XChaCha20-Poly1305 with random 24-byte nonces and vault id, author and seq as associated data; Ed25519 `verify_strict` on every op before it is stored (`replica.rs:258`). Tampering with the stored KDF parameters cannot lower the cost: the wrapped keys then fail to open. |

Nothing an earlier review marked as fixed was found fully unfixed.

## 9. Not covered, and where a human should look first

**Not covered**
- Anything on Windows: named pipe behaviour, Hello, DPAPI, the passkey provider at run time, the installer.
- Anything on Android: no device or emulator; Kotlin read only.
- Real browsers: every statement about Chrome or Firefox behaviour is from reading.
- Dependencies were not read line by line (`snow`, `spake2`, `argon2`, `ed25519-dalek`, `keepass`, `psl`, Tauri and its plugins).
- Memory hygiene was only spot-checked: `Key32` and item values are wiped on lock; I did not confirm that `ed25519-dalek` wipes the signing key on drop in this version, and JS strings holding the master password cannot be wiped.
- Build and release pipeline, signing, the published artifacts.

**Look first**
1. H-1 and M-1: confirm with a scenario test in `crates/lp-engine/tests/scenario.rs` (remove a device, then check that a proof conflicting with the stored log is refused), then decide on key rotation.
2. M-2: load a page served with `Content-Security-Policy: sandbox allow-scripts` in Chrome and Firefox and try the popup fill.
3. M-3: on a phone, a page with a cross-origin iframe that contains a login form, in each trusted browser.

## Proof tests

```sh
cd audit/poc && CARGO_TARGET_DIR=/tmp/lp-audit-target cargo test --offline --no-fail-fast
```

Both tests fail at commit `742c89a`; each file's header says what a fix must make true. The crate is stand-alone (its own `[workspace]`), so the tracked tree is untouched.
