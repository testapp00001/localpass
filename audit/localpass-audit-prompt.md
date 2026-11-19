# LocalPass independent security and logic review

You are doing an independent, read-only security and correctness review of LocalPass, the
password manager in this repository. You are a skeptical reviewer, not the author. Your goal is
to find real problems, prove each one, and publish an honest result with a badge that matches it.

## 0. Session rules (these override CLAUDE.md for this session only)

- This is a review session, not a coding session. Ignore the "Current work", handoff and
  "Working with agents" instructions in CLAUDE.md. Do not resume any phase or plan.
- Do NOT edit, refactor or "fix" any tracked source file. Do not commit, push, tag, or open PRs.
- The only files you may create are under `audit/` (gitignored by you if needed): the report,
  the badge snippet, and proof tests or scripts in `audit/poc/`.
- Never open, unlock, copy or sync my real vault. Every run of the app, `lp` CLI or e2e harness
  uses a throwaway profile and data dir (`LOCALPASS_PROFILE=audit`, `--data-dir <temp dir>`,
  `--password-env` with a dummy password). If a step would touch real data, skip it and note it.
- Do not read the earlier review conclusions (`docs/phase5-security-review.md`,
  `docs/phase6-security-review.md`, README "Security notes and limits", handoff notes) until
  step 5. Form your own findings first, so you are not anchored by them.
- Subagents are allowed (read-only, one area each, at most 3 at once). Brief them with the
  area, the files and the questions below, never with your own conclusions. You personally check
  every finding they report against the code before it goes in the report.
- If you are unsure whether something is a bug or intended, record it as a question, not a finding.

## 1. Pin and baseline

1. Record `git rev-parse HEAD`, `git status --short` (must be clean, otherwise note it), the date,
   your model name, and the OS you are running on.
2. Run and record results (pass/fail counts, warnings), in the background where slow:
   - `cargo build -p lp-broker --examples` then `cargo test --workspace`
   - `cargo clippy --workspace --all-targets -- -D warnings`
   - `cargo audit` and `cargo deny check advisories` (install with `cargo install` if missing;
     if install fails, say so)
   - `cd app && bun install && bun run build && bunx tsc --noEmit`
   - `cd extension && bun install && bun run test && bunx tsc --noEmit`, and an npm advisory
     check of both lockfiles (`bun audit` or `npx audit-ci`/`osv-scanner` if available)
   Tests that need Windows, Android or a person ("ASSISTED") are not run; list them as not covered.
3. Map the attack surface before reading deeply: every network listener, named pipe / socket,
   Tauri command (`#[tauri::command]` and `capabilities/*.json`), native-messaging entry,
   extension message handler, file parser, and every place a secret is decrypted.

## 2. Threat model to review against

Treat each of these as an attacker and ask "what can they get?":

| Attacker | Can do |
|---|---|
| A. Thief with the disk / a backup | Reads all files at rest, offline brute force |
| B. Same-LAN attacker | Sees mDNS, connects to the sync port, MITMs, replays, floods |
| C. Malicious or compromised paired device, or a revoked device | Sends validly signed but hostile ops, keeps syncing after revocation |
| D. Malicious website | Runs in a tab with the extension, uses iframes, look-alike domains, http pages |
| E. Other local process / other OS user | Talks to the pipe, native-messaging host, clipboard, logs, temp files |
| F. Malicious import file | Crafted Bitwarden JSON/CSV, KeePass .kdbx, browser CSV |
| G. Shoulder surfer during pairing | Sees or guesses the pairing code |

## 3. Areas and the specific questions to answer

For each area: read the code, answer every question with file:line evidence, and list findings.

**lp-core (crypto, signed ops, CRDT, roster, generator, TOTP)**
- KDF: algorithm, parameters, salt source; are parameters stored and enforced so they cannot be
  downgraded by a tampered file? Offline cost per guess for attacker A.
- AEAD: algorithm, nonce generation and uniqueness across devices and restarts, associated data
  (is item/op identity bound so ciphertexts cannot be swapped between items or vaults?).
- Key hierarchy and rotation after device revocation: does a revoked device lose access to future data?
- Signatures: is every op signature verified before it is stored or applied? Is the signer checked
  against the roster at that op's position (not the current roster only)? Replay, reordering,
  duplicate ops, forged timestamps / clocks far in the future.
- CRDT: do all devices converge for any delivery order? Can one peer delete or overwrite data it
  should not (e.g. resurrect deleted items, roll back a password change, change the roster)?
  Write a property test in `audit/poc/` if convergence is not already property-tested.
- Secrets in memory: zeroize on lock, `Debug`/`Display` impls or logs that print secrets.
- Password generator: CSPRNG source, modulo bias, character-set handling. TOTP: RFC 6238
  correctness (SHA1/256/512, digits, period, base32 padding/lowercase).

**lp-store**
- SQL built by string formatting, file permissions of the DB and its WAL/SHM, what is stored
  unencrypted (titles? URLs? usernames? peer addresses?) and whether README claims match.

**lp-sync (Noise, sync protocol, SPAKE2 pairing, mDNS)**
- Noise pattern and how remote static keys are authenticated against the roster.
- Pairing: code length and entropy, attempt limits / lockout, what happens on a wrong guess,
  can an online attacker (G or B) brute force during the pairing window, identity binding of the
  SPAKE2 transcript.
- Message size limits, allocation from attacker-controlled lengths, unbounded loops, panics on
  malformed input (DoS by attacker B or C). What mDNS broadcasts (device names, vault ids?).

**lp-broker, native messaging, lp-winauth**
- Who can connect to the pipe/socket (ACLs, abstract vs filesystem socket on Linux, permissions)?
- Is the caller's extension id / origin verified, or just trusted from the message?
- Broker pairing and request authentication; can attacker E request secrets without the user?
- Framing: length limits, partial reads, panics.

**extension/ (autofill, saving logins, passkeys)**
- URL matching: subdomains, public suffix handling, ports, http vs https, punycode / IDN
  look-alikes, `about:blank` and sandboxed iframes, cross-origin iframes filling the top page.
- Does any page script (attacker D) reach privileged messages? Validate `sender` checks in every
  `onMessage`/`onConnect` handler. Can a page trigger fill or read filled values silently?
- Passkeys: rpId vs origin validation, user presence/verification, sign counter, locked-vault paths.
- Manifest permissions, CSP, `web_accessible_resources`, remote code.

**app/ (Tauri desktop and Android)**
- Tauri capabilities: which commands are exposed to which windows; any command that returns
  secrets or writes files from webview-controlled paths; CSP in `tauri.conf.json`; dangerous
  plugins (shell, fs, http) and their scopes.
- Clipboard: auto-clear timing, clipboard history/cloud sync exclusion.
- Logs (`applog.rs` and others): ever written with secrets, URLs or usernames?
- Auto-lock, lock on suspend, unlock attempt limits, Windows Hello / biometric fallback logic.

**lp-import / export**
- Parser robustness against attacker F (huge files, deep nesting, kdbx KDF params set to
  absurd values, zip/xml bombs). Export: is plaintext written with safe permissions and is the
  user warned? Data loss or silent field drops on import (logic bugs).

**lp-engine (the API every front end uses)**
- Lifecycle logic: lock/unlock state machine, revocation flow end to end, item history, what a
  front end can do while locked.

## 4. Proof and verification rules

- A finding needs: location (file:line), the attacker from section 2, preconditions, impact, and
  proof. Proof is either a failing test in `audit/poc/` (preferred; a normal unit/integration
  test that demonstrates the wrong behaviour, run against a throwaway vault) or a precise code
  trace that another reviewer can follow line by line.
- After drafting all findings, do a separate disproof pass: for each finding, actively try to show
  it is wrong (a check elsewhere, unreachable path, mitigated by the platform). Use a fresh
  subagent that gets only the finding text and the files, not your reasoning. Drop or downgrade
  anything that does not survive, and say so.
- Severity:
  - Critical: attacker gets vault secrets or keys, or can silently alter vault data, with no user mistake.
  - High: same, but needs a realistic precondition (same LAN, a click, a paired device).
  - Medium: weakens a defence (e.g. lower KDF cost, metadata leak, DoS of sync).
  - Low: hardening, defence in depth.
  - Info: notes, questions, things you could not verify.
- Logic bugs (wrong behaviour that is not a security issue) are reported in their own section
  with the same proof rules.

## 5. Compare with earlier reviews

Only now read `docs/phase5-security-review.md`, `docs/phase6-security-review.md` and the README
security notes. For each of their claims, mark: confirmed by you / contradicted (with evidence) /
not checked. Note anything they marked fixed that you found still present.

## 6. Deliverables

Write `audit/REPORT.md`:

1. Header: commit hash, date, model, OS, scope, and time spent.
2. Verdict in one line (see badge rules).
3. Coverage table: each area from section 3 with Reviewed fully / Partially / Not reviewed, and why.
4. Tool results from step 1 (tests, clippy, cargo audit/deny, JS advisories).
5. Findings table sorted by severity (id, title, severity, confidence, attacker, file:line), then
   one section per finding: description, proof (link to `audit/poc/...`), impact, suggested fix.
6. Logic bugs.
7. Findings that were dropped in the disproof pass, and why.
8. Comparison with earlier reviews (step 5).
9. Not covered / limits: what you could not run or read (Windows-only, Android, assisted e2e,
   dependencies not reviewed line by line) and what a human auditor should look at first.

Then write `audit/BADGE.md` with exactly one badge, chosen by these rules:

- Any open Critical or High: `![AI security review](https://img.shields.io/badge/AI_security_review-issues_found-red)`
- Open Medium only: `![AI security review](https://img.shields.io/badge/AI_security_review-medium_issues-orange)`
- Only Low / Info: `![AI security review](https://img.shields.io/badge/AI_security_review-passed_(<short commit>)-brightgreen)`

Each badge links to `audit/REPORT.md`, and the line under it must read:
"Automated review by <model> on <date> at commit <short hash>. Not a professional third-party audit."
Never use the words "audited", "certified" or "secure" in the badge. Do not add the badge to the
README yourself; I will do that after reading the report.

Finish by telling me the verdict, the count per severity, and the three findings I should look at first.
