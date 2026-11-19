Status: in progress; see `../handoff-phase9.md`.

# Phase 9: Audit remediation and secure device recovery

## 1. Summary and agreed decisions

Address every audit finding through a code fix, platform verification, or an explicitly documented limitation. Implement in small, reviewable tasks, with security and data-preservation tests before integration.

Agreed behavior:

- Include encryption-key rotation through **vault replacement and re-pairing trusted devices**.
- Use the same lossless migration for the first security upgrade.
- Require coordinated app, extension and Windows provider upgrades. Older protocols receive an update-required response; no insecure fallback.
- Preserve convenient fills and the existing passkey verification setting.
- Allow consistent save offers for manually submitted logins, including an “already saved” result after confirmation.
- Reject imports exceeding documented resource limits.
- Use your Android phone during Android verification and your Windows 11 fingerprint-equipped PC for final Windows checks.

Keep LocalPass fully local. No cloud service, telemetry or updater is introduced.

## 2. Implementation phases

### 9.0 — Establish tracking and isolated test environments

- Save the approved plan as `docs/plans/2026-11-16-phase9-audit-remediation.md`, register it in the plan index, and maintain `docs/handoff-phase9.md`.
- Track each audit ID with its disposition, regression test, platform evidence and remaining limitations. Preserve the original report as historical evidence.
- Port the four demonstrated failures into maintained regression tests: conflicting membership proof, post-cutoff revocation, malformed Unicode TOTP and future-dated edits.
- Prepare a separate Android audit build using application ID `app.localpass.audit`, a distinct label, and isolated app data and Keystore storage. Keep Kotlin/JNI package names stable and qualify manifest component names correctly.
- Make Android helpers support Linux and Windows, require an explicit ADB serial and test package, check command failures, and restore changed provider settings and port forwards.
- Physical-device tests must never uninstall or clear the production app, remove the phone’s screen lock, or change enrolled fingerprints. Keep destructive/no-screen-lock cases on an emulator.

**Completion:** automated tests can identify their test profile/package and refuse production targets.

### 9.1 — Contain confirmed failures and update dependencies

**Findings: H-1, L-1, L-18; containment for M-1.**

- Validate membership proofs against all stored operation classes. Reject conflicting headers at the same author/sequence position, conflicting entries within a proof, and missing records inside an allegedly complete local prefix.
- Take consistent store/roster snapshots with the existing store-first lock order.
- Make TOTP parsing return errors for arbitrary Unicode input. Ensure a poisoned session cannot prevent locking and destroying decrypted state.
- Introduce a migration-required state for legacy vaults: local inspection and recovery remain available, but legacy network synchronization and writes are disabled.
- Update `crypto-bigint` to at least 0.7.6, `ctutils` to at least 0.4.4, and SQLite through `rusqlite` 0.41 or a later verified patched release. Raise the declared minimum Rust version to at least 1.89.
- Triage the remaining runtime unsoundness advisories individually. Update affected development tooling separately.
- Add CI advisory checks. Exceptions must name an advisory, explain reachability, and have a review date; do not ignore whole warning categories.

**Completion:** the proof-conflict and Unicode regressions pass, and legacy synchronization cannot expose data while migration is pending.

### 9.2 — Introduce the new vault format and lossless recovery

**Findings: M-1, I-1, I-4, B-1.**

Implement this as three sequential tasks: format and ordering, lossless snapshots, then recovery activation.

**Vault format and ordering**

- Introduce versioned vault and sync formats with explicit rejection of unsupported versions.
- Order new data operations by a persisted Lamport logical counter, then author and sequence as deterministic tie-breakers. Retain wall time for display and skew warnings, not conflict precedence.
- Advance the logical clock after accepted operations and before local writes. Use checked arithmetic and return an explicit error on exhaustion.
- Keep legacy timestamp ordering only inside the migration reader. Preserve existing selected values and all alternatives; flag suspicious dates for review instead of guessing which password the user intended.
- In the new format, removal happens by replacement. Legacy `DeviceRevoked` operations cannot change live membership; old roster records are migration provenance only.
- Exchange membership authentication before version vectors, address hints or vault operations.

**Lossless recovery snapshot**

- Add a native encrypted snapshot used by recovery and a versioned encrypted backup format.
- Preserve item and folder IDs, current values, full available field/password history, conflict alternatives, trash/tombstones, custom fields and provenance.
- Preserve passkey credential IDs, user handles and private keys exactly, so existing registrations continue working.
- Separate historical provenance from live membership authority. Archived device identities and roster records must never authorize a connection.
- Continue reading existing encrypted backups. Explain that histories absent from old backup files cannot be reconstructed.
- Do not use the current Bitwarden-format export/import round trip for migration.

**Recovery and activation**

- Require master-password reauthentication and a preview of the source vault, selected device and recovery effects.
- Freeze source writes, close sync sessions, and create fresh vault identity, genesis, signing identity, Noise keys, vault key, network key and wrapping salt.
- Re-encrypt retained contents and history under the replacement key.
- Build a staged vault generation, verify its contents, durably activate it, and retain the source as a private recovery archive. A restart must resume or complete the recorded transition without selecting a partial generation.
- Start with only the initiating device authorized. Remaining trusted devices join through explicit pairing.
- Before replacing a survivor’s old vault, show its unsynced changes relative to the recovery baseline. Import selected changes as new operations after review; never import old membership authority or silently overwrite conflicting values.
- Disable stale quick-unlock bundles and offer reenrollment. Cancel pending approvals and refresh provider caches after activation.
- Display outstanding device recovery steps. Independent recoveries produce separate vaults; they never merge trust automatically.

**Completion:** old device keys cannot authenticate to the replacement vault, decrypt its new data, or authorize another identity. Offline survivors can recover their unsynced work deliberately.

### 9.3 — Secure broker transport and locking lifecycle

**Findings: L-5, L-6, L-8, L-9, L-10, L-11, L-16, B-2, B-3.**

- Add broker protocol v2 with Noise transport using the existing `snow` implementation. Terminate encryption in the Rust native-messaging helper and Rust provider client.
- Bind paired-client authentication proofs to the actual transport handshake. Derive the pairing comparison code from that handshake; deliver new pairing secrets only through the encrypted channel.
- Preserve existing message-size limits through bounded authenticated chunks. Reject tampering, replay, truncation and downgrade.
- Preserve valid paired-client credentials when upgrading; require their updated protocol. Never silently turn failed authentication into pairing.
- Unknown clients must not launch Hello or raise unlock prompts. Pairing becomes actionable through a user-opened app pairing flow, with bounded requests and refusal/timeout backoff.
- Bind every approval and verification to its request, action, site, client, expiry and session generation. Locking, disconnection or client removal invalidates it in Rust and closes its UI.
- Show the requesting site/action in trusted app UI when an unlock or verification is needed. Preserve the chosen convenience policy.
- Refresh activity after successful broker quick unlock.
- Lock on Windows session lock/suspend and Linux session-lock/sleep signals. Add a resume safeguard and elapsed-time handling that counts suspension.
- Clear the app’s own copied secret on lock and orderly quit. Desktop and Android cleanup must leave unrelated newer clipboard content intact.
- Make client-store changes atomic. Failed removal persistence must still terminate existing sessions, remain visible as an error, and support retry.
- Surface broker startup failures with bounded retry and an actionable status instead of logging once and remaining unavailable.

### 9.4 — Fix browser origin binding, save privacy and capture loss

**Findings: M-2, L-13, L-14, L-15, extension portion of L-17, B-8, B-9.**

- Reproduce the sandbox claim in actual Chromium and Firefox before changing it; record whether the original exploit is reachable.
- Register top-level documents through browser-authenticated messages and document-bound ports. Validate sender identity and non-opaque origin before fetching or delivering a secret.
- Bind popup fills to the same live document/port across unlock and verification waits. Navigation or disconnection cancels delivery; tab URL alone never authorizes it.
- Require exact privileged extension-page URLs and expected sender context for popup-only operations.
- Bind remembered fills to origin, document and lifetime. Handle capture/navigation races without restoring cross-site comparisons.
- Stop checking guessed manual submissions against saved credentials before protected user consent. Use a consistent save offer; show “already saved” only after confirmation.
- Continue suppressing captures attributable to a fill LocalPass performed in that same document.
- For nonconditional passkey requests, do not expose existence through automatic no-match fallback. Fallback follows a protected user action or the common request deadline.
- Preserve pending saves when a different decision opens. Use a bounded queue with visible expiry/capacity errors; only explicit dismissal, successful completion or expiry removes a capture.
- Capture intended submissions, including supported custom login flows, rather than arbitrary show-password or Cancel buttons.
- Render hostnames with their meaningful right-hand end visible and isolated from surrounding text.

### 9.5 — Fix Android form isolation and verify on your phone

**Findings: M-3, I-2; Android portions of L-8 and L-16.**

- Associate fields with their nearest domain-bearing ancestry and their containing browser document.
- Select username/password fields only from an unambiguous matching group. Refuse cross-origin frame groups and unresolved origin mappings.
- Apply the same parser and checks to fill, save and the authentication activity. Re-derive field IDs from the system-provided structure rather than accepting extras as authority.
- Add explicit backup/data-extraction exclusions and test the resulting manifest and supported backup behavior.
- Verify browser trust checks, lock/background behavior, clipboard ownership, biometric cancellation and cold service startup.

**Phone checkpoint:** request your connected, USB-authorized phone after the isolated audit build and fixtures are ready. Use dummy credentials and genuinely different fixture hosts, not merely different ports.

Test each supported browser available on the device. A browser that cannot provide sufficient isolation evidence remains unsupported for the affected autofill path; do not mark it verified.

### 9.6 — Bound imports, protect files and reduce metadata exposure

**Findings: L-2, L-3, L-7, L-12, I-5, B-6.**

- Add bounded KeePass parsing through a narrowly maintained parser patch that enforces limits before KDF execution and during decompression.
- Initial external-import limits: 64 MiB input, 256 MiB decompressed data, 256 MiB Argon2 memory, 10 iterations, 8 lanes and 10 million AES-KDF rounds. Reject excess with a clear explanation; no override.
- Bound parsing depth and record counts and ensure rejected imports make no partial vault changes.
- Report unsupported passkeys, password history, attachments, tags and expiry explicitly. Reserved passkey fields activate credentials only through the supported encrypted-backup path.
- Use a shared secure file-writing facility for CLI secrets, exports, backups and temporary files: private permissions from creation, safe replacement, and protection against following unexpected links.
- Remove site names, item titles and other unnecessary sensitive metadata from routine app/provider logs.
- Bound unauthenticated discovery records, names, addresses and concurrent dialing; expire stale advertisements without evicting verified peers in favor of spam. Keep existing connection and handshake limits.

### 9.7 — Minimize secret exposure and finish consistency fixes

**Findings: app portion of L-17, B-4, B-5, B-7.**

- Make normal item-detail responses carry metadata and secret-presence indicators. Reveal a requested secret or history value explicitly.
- Change editing to patch semantics: unchanged, set or clear. Saving a metadata edit must not require fetching or resending existing passwords.
- Replace arbitrary webview file paths with single-use selections issued by Rust file dialogs. Keep normal path arguments for the CLI.
- Update displayed usernames when replacing passkeys.
- Prefer an appropriately matching login already containing the submitted password before selecting an overwrite candidate.
- Validate an unlocked session before changing device-name metadata, and keep local/synced changes consistent.

### 9.8 — Integrate, independently review and prepare release

- Run the complete checks below against the final tree.
- Re-review membership admission, recovery activation, encrypted broker pairing and document-bound secret delivery.
- Update user guidance for coordinated upgrades, recovery, import limits and remaining threat-model boundaries.
- Prepare the next minor release, provisionally v0.4.0. Commit, push, tag and publish only when separately requested.

## 3. Public interfaces and compatibility

- Add engine recovery preview/commit/progress interfaces. Commit consumes a single-use preview tied to the source generation and revision; changed source state requires a new preview.
- Add an explicit migration-required status and structured errors for unsupported versions, stale approvals, changed documents and resource limits.
- Version native snapshots/backups, operation payloads, sync/pairing protocols and broker proofs. Never reinterpret legacy bytes as new-format operations.
- Add secret-presence DTOs, explicit reveal operations and typed item patches across Rust, Tauri, TypeScript and test fixtures.
- Keep the vault key inaccessible to background sync. Device-secret protection continues to unlock network functionality only.
- Browser/provider credentials can survive the coordinated protocol upgrade, but open sessions cannot survive locking or vault replacement.
- Legacy vaults are migration inputs; legacy backups remain importable. Downgrading cannot open or synchronize the new-format vault.

## 4. Test and acceptance criteria

**Security and recovery**

- Reject data-versus-roster proof conflicts, contradictory proof entries, forged signatures, wrong vaults and oversized proofs.
- Preserve admission of legitimately paired devices and valid transitive proofs.
- Reject legacy revocation injection into a new vault, including relayed and reordered delivery.
- Recover with an offline survivor, concurrent survivor edits, a removed genesis device and interrupted pairing.
- Inject crashes and storage failures throughout migration and activation. Verify no lost values, half-active generations or plaintext temporary files.
- Verify old keys cannot decrypt future changes, including after restart and subsequent pairing.
- Independently verify passkey assertions before and after recovery using the existing relying-party harness.

**Correctness and privacy**

- Test arbitrary Unicode TOTP inputs and locking after a simulated poisoned session.
- Test future/backward wall clocks, equal logical times, restart persistence, reordered delivery, duplicate operations and overflow errors.
- Verify unchanged manual submissions and guessed submissions do not expose different pre-consent outcomes.
- Test sandboxed top-level pages, frame isolation and navigation during every delayed fill stage.
- Test generated-password capture retention, intervening decisions, expiry, repeated submissions and two-step forms.
- Test import limits before expensive computation, decompression bombs, permission handling and failed file replacement.

**Platforms and final checks**

- Run Rust formatting, Clippy, workspace tests, app/extension tests and TypeScript builds.
- Run Chromium and Firefox end-to-end suites and Android Kotlin/instrumentation tests.
- On your Android device: same-site forms, cross-site frames, save/fill, passkeys, biometrics, cold start and real LAN recovery/sync.
- When your Windows PC is available: Hello, Chrome/Edge, provider integration/cache, cancellation, suspend/session locking and installer upgrade.
- Run release builds for shipped platforms and verify checksums, signatures and packaged extensions.
- Report unavailable tests as pending. Do not infer runtime success from compilation.

## 5. Rollout and explicit limits

- Implement in the listed order, one bounded task at a time. Record actual checks and remaining work after each task.
- Before personal-vault migration, complete the full recovery matrix on throwaway vaults and produce an encrypted recovery backup.
- During coordinated rollout, migrate the most complete trusted copy first, then reconcile and re-pair survivors individually.
- Retain private recovery archives until the owner confirms successful reconciliation; never silently delete the only remaining copy.
- Windows runtime verification remains a release requirement even though that machine is not currently present.
- **L-4 and I-3 remain documented trust-model limits:** this plan does not claim protection against malware already running as the user or holding a paired-client secret.
- **I-6 remains the existing documented port-matching policy.**
- Fresh vault keys protect future vault data. Passwords and passkey private keys already copied by a compromised device require separate rotation or revocation at their websites; vault replacement cannot invalidate those copies.
