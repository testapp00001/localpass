# Phase 9 handoff

Approved plan: [2026-11-16-phase9-audit-remediation.md](plans/2026-11-16-phase9-audit-remediation.md).
Historical evidence: [original audit report](../audit/REPORT.md), commit `742c89ace7fbaf31042f6e00a070c5d62dad76d6`. The report and original proofs have not been edited.

## Resume here

This is an initial implementation batch, **not a completed phase 9 or a release candidate**.
Implementation continues on the Windows laptop in the approved order. On 2026-11-19 the user authorized a local checkpoint commit and an offline Git bundle on `phase9/audit-remediation`. No push, tag, publication, personal-vault access or migration is part of this checkpoint.

- 9.0: approved plan registered; all audit IDs tracked below; four demonstrated failures ported into maintained tests. Proof-conflict and Unicode tests are enabled. Post-cutoff revocation and future timestamps remain explicitly ignored, tracked failing demonstrations awaiting replacement membership and versioned ordering. Audit Android build identity and guarded helpers are implemented; APK/manifest merge and device verification are still pending.
- 9.1: proof validation across all operation classes, proof contradiction checks, complete-prefix checks, consistent store/roster snapshots, Unicode-safe parsing and lock recovery after mutex poisoning are implemented. Rust security updates and an advisory CI gate are implemented. **Migration-required legacy write/sync containment is not implemented; 9.1 is incomplete.** JS development-tool updates also remain.
- 9.2–9.8: pending. No vault-format bump, logical ordering, lossless snapshot/recovery activation, broker v2, browser origin rewrite, Android form isolation, bounded KeePass patch, secret DTO/patch migration or release preparation is claimed.

Next bounded task: finish the 9.1 migration-required state and development dependency triage. Legacy local inspection and recovery must remain possible while all legacy writes and network synchronization are disabled. Do not label a vault as a new format until its operation format and ordering are implemented. Then carry out 9.2 as its three sequential tasks: format/ordering, lossless snapshots, recovery activation. Preserve the source generation, passkey keys/IDs and all available histories.

## Windows transfer checkpoint (2026-11-19)

**Resume on Windows with unfinished 9.1, then 9.2.** This checkpoint transports the existing initial implementation batch; it does not complete the remediation plan or enable production migration. The original audit report/proofs and the approved Phase 9 implementation plan are preserved.

The checkpoint includes the original 34 modified tracked files and 20 previously untracked audit/source/test/documentation files, plus the approved Windows checkpoint plan. Build artifacts, cached dependencies, temporary logs, credentials, device data and the transfer packet are excluded. The owner's `.cargo/config.toml` remains unchanged.

### Fresh verification for the transfer

| Check | Result on 2026-11-19 |
|---|---|
| `git diff --check` | Passed before logging updates; repeat on the final staged checkpoint |
| `PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p test_advisories.py` | **4 passed**, 0 failed |
| In `extension/`: `bun test --isolate test/android-adb.test.ts test/android-target.test.ts test/test-profile.test.ts` | **8 passed**, 0 failed, 40 expectations across 3 files; ADB calls mocked, no device accessed |
| Full Rust/Clippy/app/browser suites | Not rerun for this documentation/transfer task; earlier results below belong to 2026-11-16 |
| Windows and Android runtime checks | Pending; no platform success inferred from the Linux results |

The old `/tmp/localpass-phase9-*.log` and advisory JSON files are no longer present on this host. Their durable summaries below are historical test records, not newly inspected raw logs. Windows must rerun the relevant checks against its actual checkout.

### Transfer packet and clean checkout

Copy the files from `audit/transfer/` to the Windows laptop: the self-contained `localpass-phase9-windows.bundle`, `SHA256SUMS.txt` and `WINDOWS-START-HERE.txt` (or extract `localpass-phase9-windows.zip`). The packet is intentionally ignored by Git. The start-here file records the actual checkpoint commit and the bundle digest after commit creation.

From PowerShell in the transfer folder, make a fresh checkout beside any existing checkout:

```powershell
Get-FileHash .\localpass-phase9-windows.bundle -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
git clone --branch phase9/audit-remediation .\localpass-phase9-windows.bundle .\localpass-phase9
Set-Location .\localpass-phase9
git bundle verify ..\localpass-phase9-windows.bundle
git status --short
git rev-parse HEAD
```

Compare the digest and commit with the start-here file. The clone should have no local modifications. Its `origin` points to the bundle; do not push to that path or change an existing GitHub remote unnecessarily. A remote push or release is not authorized by this transfer.

### Windows prerequisites and first actions

1. Read this file and the approved Phase 9 plan. Use stable Rust with MSVC C++ build tools, Bun, Python 3, and WebView2. The declared Rust minimum is 1.89; Linux verification used 1.99.1, so minimum-version coverage is still pending. Retain the committed Windows LLD and Android 16 KB alignment settings.
2. From the repository root, install the web dependencies with `bun install --frozen-lockfile` in both `app` and `extension`. Prepare the web/extension resources before compiling the app; follow the existing build scripts. Build broker examples before tests that launch their helpers. Record the actual Windows results; do not turn the earlier Linux numbers into Windows results.
3. Finish migration-required legacy containment and JS development-dependency triage (9.1). Until containment is implemented, this checkout still allows legacy writes/sync. Use synthetic vaults only.
4. Continue 9.2 in order: versioned format/logical ordering, lossless snapshots, then recovery activation. No key rotation, old-key exclusion, secure migration or new broker transport is implemented by this checkpoint.
5. Keep M-1 and B-1 open. Their maintained tests are explicitly ignored demonstrations that still fail when run with `--ignored`; they are not passing security coverage. Preserve both until their replacements satisfy the agreed behavior. GLib runtime unsoundness and four dated maintenance exceptions also remain open (review due 2026-12-16).
6. Later Windows runtime checks cover Hello/fingerprint, Chrome/Edge, provider/cache behavior, cancellation, session lock/suspend and installer upgrade. Use audit profiles and the existing test provider identity, never the installed production app/profile.

### Android on the Windows laptop

Configure the Android SDK, its command-line tools/`apkanalyzer`, the JDK and NDK r27d when Android work starts. Before any physical-device run, build and inspect the audit APK's merged manifest: application ID must be `app.localpass.audit`, with a separate UID/data directory and correctly qualified components. Do not request the phone until the 9.5 fixtures and parser are ready.

```powershell
$env:LOCALPASS_PROFILE = 'audit'
$env:LP_ADB_SERIAL = '<explicit authorized device serial>'
$env:LP_ANDROID_PACKAGE = 'app.localpass.audit'
[Environment]::SetEnvironmentVariable('ORG_GRADLE_PROJECT_localpass.audit', 'true', 'Process')
```

Use the real SDK/JDK/NDK paths already configured on Windows. The helper selects the explicit serial and rejects production APKs. Physical fingerprint prompts require the owner; emulator-only destructive/no-screen-lock cases must not run on the phone. Restore provider settings and test-created port mappings after each run.

## Audit Android setup

The Gradle property `-Plocalpass.audit=true` changes the installed application ID to `app.localpass.audit` and its label to `LocalPass Audit`. Namespace, Kotlin package and JNI symbols stay `app.localpass`. Manifest component class names are fully qualified, including provider/authentication activities. Android app data and Keystore entries are isolated by the audit package's UID. This needs runtime/merged-manifest verification before use on a phone.

Build through Gradle with that property after producing the appropriate Rust Android library; alternatively pass `ORG_GRADLE_PROJECT_localpass.audit=true` in the environment when invoking the Tauri Android build. Do not use a production APK in these harnesses.

Chromium and Firefox harnesses now validate the reserved `audit`/`e2e` profile namespace and reject default profiles or path traversal. Assisted harnesses default to `audit` (update old `dev2` walkthrough commands). Generated automated profiles continue using `e2e<pid>`.

Every Android harness now requires:

```sh
export LOCALPASS_PROFILE=audit
export LP_ADB_SERIAL=<explicit-adb-serial>
export LP_ANDROID_PACKAGE=app.localpass.audit
export ANDROID_HOME=<android-sdk>
```

The helper selects that serial for every ADB call. It verifies the APK application ID with `apkanalyzer`, rejects non-audit packages, checks command failures, preserves provider settings and prior forward/reverse mappings, and reports cleanup failures for retry. Scan, release-first-start and no-lock suites require an emulator before any setup. Screen-lock/fingerprint modification commands are refused. Do not uninstall or clear the production app, remove a phone's screen lock or change enrolled fingerprints. Physical tests use dummy records and different fixture **hosts**, and must wait for the 9.5 fixtures and parser.

No Android SDK or ADB is installed/configured on this host. Do not request a physical phone until the audit APK and isolation fixtures are ready. Windows 11 fingerprint/Hello, browser, suspend and installer runtime checks remain a release requirement.

## Dependency dispositions

The lockfile now selects `rusqlite 0.41.1` / `libsqlite3-sys 0.39.1`, `crypto-bigint 0.7.11`, `ctutils 0.4.8`, `tokio 1.54.1`, `rustix 1.1.6`, `async-executor 1.14.1`, and `clipboard-win 5.4.3`. Workspace packages inherit the Rust minimum of 1.89.

The original three vulnerability advisories are absent from the local audit report. Tokio and rustix unsoundness advisories are resolved by upgrades. GLib `RUSTSEC-2024-0429` remains a runtime exception: application code does not call the affected iterator, but indirect GTK reachability has not been ruled out. Four individually identified maintenance exceptions concern `proc-macro-error`, `zbus_names`, `zvariant`, and `zvariant_derive`. See [advisory-exceptions.json](advisory-exceptions.json) for exact package versions, reasons and review dates. The CI checker does not ignore warning categories and fails on missing/expired exceptions, a new advisory or a different affected package version. Review is due 2026-12-16. A GLib/Tauri stack upgrade or validated backport is still needed.

Primary references: [SQLite advisory](https://rustsec.org/advisories/RUSTSEC-2026-0459.html), [crypto-bigint advisory](https://rustsec.org/advisories/RUSTSEC-2026-0387.html), [ctutils advisory](https://rustsec.org/advisories/RUSTSEC-2026-0385.html). Versions were resolved against the available registry; do not force the yanked 0.7.6 crypto-bigint release.

## Checks

Completed against this initial batch on 2026-11-16, Linux, Rust/Cargo 1.99.1, Bun 1.4.4. The declared Rust 1.89 minimum was not separately built.

| Check | Actual result |
|---|---|
| `cargo fmt --all --check` and `git diff --check` | Passed |
| `cargo clippy --offline --workspace --all-targets -- -D warnings` | Passed against the final Rust tree |
| `cargo build --offline -p lp-broker --examples` | Passed |
| `cargo test --offline --workspace --no-fail-fast` | **388 passed, 0 failed, 4 ignored** across 41 targets |
| Explicit execution of the two open audit demonstrations with `--ignored` | Both **fail as expected**: post-cutoff revocation and far-future timestamps remain unresolved; these are not counted as passing regression coverage |
| Extension `bun run test` | **451 passed, 0 failed** across 20 files |
| Extension regular typecheck and explicit typechecks of changed Android/Chromium/Firefox helpers | Passed |
| Chromium and Firefox production/e2e extension builds | Passed |
| Real headless Chrome suite | **63 successful checks**, exit 0; real browser configuration unchanged |
| Real headless Firefox suite | **174 successful checks**, exit 0; real native-messaging configuration unchanged |
| App `bun run build` | Passed |
| App `bun test` | **21 passed, 9 skipped, 0 failed**; Windows uninstaller tests unavailable on Linux |
| Advisory checker unit tests | **4 passed** |
| Local `cargo audit --no-fetch --json` plus `scripts/check-advisories.py --report <report>` | No vulnerability advisories; five individually documented informational/runtime exceptions; strict wrapper passed |
| Android Kotlin, merged APK manifest and instrumentation | Pending: Android SDK/ADB unavailable |
| Android phone, Windows 11 fingerprint/Hello/provider/installer | Pending |
| Coordinated migration, crash recovery, old-key isolation, new broker transport and release artifacts | Pending implementation; not tested |

The two other ignored Rust tests need Wayland clipboard access or a desktop Secret Service and were left untouched. Existing browser suites cover their existing frame and save/passkey cases; they **do not** establish the top-level CSP-sandbox reproduction required by 9.4 or Android origin isolation. All native/browser tests used synthetic vaults and isolated profiles; no personal vault was opened or migrated. The dependency CI job is prepared locally, not run on GitHub (nothing was pushed).

Builds and tests used `/tmp/localpass-phase9` and `/home/nimda/projects/localpass/audit/target-phase9`. Logs for this session are `/tmp/localpass-phase9-*.log` and local advisory reports `/tmp/localpass-phase9-audit*.json`. Sandbox-restricted listener suites were rerun with approved localhost access; initial sandbox listener denials were not treated as implementation failures. The initial proof property was updated to require validation of proofs even for cached peers. The expanded helper typecheck also exposed an existing Puppeteer button-handle typing issue, which is now typed to match the button-only DOM query.

## Finding tracker

“Pending” means no remediation or runtime verification is claimed. A proof-check fix alone does not establish key rotation or secure recovery.

| ID | Finding | Disposition | Regression/evidence | Platform evidence and limitations |
|---|---|---|---|---|
| H-1 | A removed device can be admitted again and keep reading and writing | Proof validation fixed; replacement-key protection pending 9.2 | `lp-sync` audit proof regressions; transitive proof property suite | Synthetic local replicas only; no removed-device/key-rotation release claim |
| M-1 | Revocations written by an already-revoked device still count; its later data is applied without review | Open: legacy containment and replacement membership pending | Ignored `audit_post_cutoff_revocation_must_not_remove_a_survivor` reproducer | Old revocation semantics remain; do not mark resolved |
| M-2 | Toolbar-popup fill trusts the tab URL for a sandboxed top-level document | Pending 9.4 | Pending | No new platform evidence |
| M-3 | Android autofill uses the first web domain in the screen for every field (known, RD-3) | Pending 9.5 | Pending | No new platform evidence |
| L-1 | TOTP parser panics on some non-ASCII text; during import this breaks the engine until restart | Fixed | Unicode property test and poisoned-session lock/unlock test | All inputs/vaults synthetic |
| L-2 | KeePass import accepts unlimited key-derivation cost (known, documented) | Pending 9.6 | Pending | No new platform evidence |
| L-3 | `lp` CLI and exports leave key material and plaintext readable by other OS users | Pending 9.6 | Pending | No new platform evidence |
| L-4 | The extension's client secret gives silent passwords for any site the caller names (documented limit; a code comment says otherwise) | Documented trust-model limit | Existing README; approved plan section 5 | Paired-client secret and same-user malware remain out of scope |
| L-5 | An unpaired local process can raise unlock and pairing prompts without limit and block real pairing | Pending 9.3 | Pending | No new platform evidence |
| L-6 | A browser-pairing or "Replace?" dialog stays answerable after the vault locks | Pending 9.3 | Pending | No new platform evidence |
| L-7 | Site names and item titles are written to the console log and the provider log | Pending 9.6 | Pending | No new platform evidence |
| L-8 | No lock on suspend or screen lock; the idle timer does not count suspended time on Linux | Pending 9.3 | Pending | No new platform evidence |
| L-9 | User checks are not tied to what is approved | Pending 9.3 | Pending | No new platform evidence |
| L-10 | The broker session is not bound to its handshake | Pending 9.3 | Pending | No new platform evidence |
| L-11 | Another Windows account can switch the broker off by creating the pipe name first (known, RB-5) | Pending 9.3 | Pending | No new platform evidence |
| L-12 | Sync listener, mDNS and pairing can be slowed or filled up from the LAN | Pending 9.6 | Pending | No new platform evidence |
| L-13 | "Already filled" check compares passwords across sites | Pending 9.4 | Pending | No new platform evidence |
| L-14 | A page can learn whether a passkey exists, and test one guessed login per real click | Pending 9.4 | Pending | No new platform evidence |
| L-15 | Long host names lose their right end in the panel | Pending 9.4 | Pending | No new platform evidence |
| L-16 | Copied secrets stay on the clipboard after quit or lock | Pending 9.3 | Pending | No new platform evidence |
| L-17 | Hardening: `get_item` sends all secrets to the web view; `popupOnly` fails open | Pending 9.4 / 9.7 | Pending | No new platform evidence |
| L-18 | Dependencies with open advisories | Partially fixed; individually dated exceptions and CI gate added | Patched lockfile; advisory exception tests; cargo-audit report | GLib runtime unsoundness and four maintenance exceptions; JS tooling updates pending |
| I-1 | No key rotation on removal (documented) | Pending 9.2 | Pending | No new platform evidence |
| I-2 | Android: `allowBackup="false"` without data-extraction rules | Pending 9.5 | Pending | No new platform evidence |
| I-3 | Windows Hello key signs a fixed challenge | Documented trust-model limit | Approved plan section 5 | Windows Hello fixed challenge is not claimed to resist same-user malware |
| I-4 | The dialing side sends its Hello before it has checked the peer | Pending 9.2 | Pending | No new platform evidence |
| I-5 | Any import file can add passkey items | Pending 9.6 | Pending | No new platform evidence |
| I-6 | A saved address without a port matches every port (documented) | Existing policy retained | Existing origin tests and README | No-port saved URLs continue matching any port |
| B-1 | A value saved with a far-future clock wins over every later edit. `HlcClock::observe` ignores clocks more than 24 h ahead (`hlc.rs:53-56`), but `VaultState::apply` still orders by that time (`materialize.rs:143-169`). The README mentions wrong clocks and the app shows a toast above one minute of skew (`App.tsx:73`); the later edit is still lost silently. | Open: versioned Lamport ordering pending 9.2 | Ignored maintained `legacy_future_timestamp` reproducer | Legacy wall-clock ordering remains until migration |
| B-2 | Windows only: a vault unlocked for a browser with Windows Hello locks again within 5 seconds. `commands::quick_unlock` on the broker path never updates `last_activity` (`commands.rs:247-262`; the UI command does at `:320`), so the next tick of the auto-lock loop locks it (`lib.rs:302-305`). | Pending 9.3 | Pending | No new platform evidence |
| B-3 | "Forget this browser" can silently fail to stick. `ClientStore::remove` drops the client from memory and then saves (`app/src-tauri/src/broker.rs:99-103`); on a save error `forget_client` returns before `revoke` (`:703-704`), and the file still holds the client at the next start. | Pending 9.3 | Pending | No new platform evidence |
| B-4 | Replacing a passkey keeps the old user name in lists: `USERNAME` is written only for a new item (`credentials.rs:632-648`). | Pending 9.7 | Pending | No new platform evidence |
| B-5 | `plan_save_login` picks one candidate by `(kind, id)` (`credentials.rs:360`). With two logins of the same user name on a site, it can ask to overwrite one although the other already holds this password. | Pending 9.7 | Pending | No new platform evidence |
| B-6 | Imports drop data without a warning: Bitwarden `fido2Credentials` (passkeys) and `passwordHistory` (`bitwarden.rs:50-60`), KeePass attachments, tags and expiry (`keepass_import.rs:32-71`). | Pending 9.6 | Pending | No new platform evidence |
| B-7 | `rename_this_device` writes the local name before the session check (`lib.rs:1021-1022`): a call while locked changes the local name only and then fails. | Pending 9.7 | Pending | No new platform evidence |
| B-8 | A waiting save offer is destroyed by any newer request in the same tab, and the captured password is lost (`decide.ts:476-479`, `933-937`). Example: generate a password on sign-up, close the save panel, click the badge on the next form. | Pending 9.4 | Pending | No new platform evidence |
| B-9 | Captures fire on any button-like click inside a form, for example "show password" or Cancel (`content/index.ts:172-180`), so abandoned passwords are offered for saving. | Pending 9.4 | Pending | No new platform evidence |
