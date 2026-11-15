# Plan log

Every plan the user has approved for LocalPass, newest first, with its status and where its state lives. A plan file says what
was decided and why; the handoff says what is done and what comes next. Start a session from the handoff, not from the plan.

| Date | Plan | File | Status | Summary | State lives in |
|---|---|---|---|---|---|
| 2026-11-12 | Phase 8: one protected panel in every browser, Chromium on Linux, smoother unlock | [2026-11-12-phase8-browser-experience.md](2026-11-12-phase8-browser-experience.md) | in progress | Implementation and Linux walkthrough accepted by the user on 2026-11-15; 8.6 closed; local Phase 8 commit requested. Automated checks: Rust 381 passed, extension 443 passed, app 21 passed; TypeScript and fmt passed. Windows compilation/workspace and Linux CI passed during the authorized PR #1 merge. Android builds, assisted Windows checks and release verification remain open; the five usability follow-ups remain undecided. | [handoff-phase8.md](../handoff-phase8.md) |
| 2026-11-08 | Firefox password picker in the toolbar panel | [2026-11-08-firefox-toolbar-picker.md](2026-11-08-firefox-toolbar-picker.md) | done | Field badge opens the protected toolbar panel on Firefox 149+; save and passkey windows stay as they are | [handoff-phase7.md](../handoff-phase7.md) |
| 2026-10-31 | Phase 7: secure files, vault tools, faster builds | [2026-10-31-phase7-files-tools-builds.md](2026-10-31-phase7-files-tools-builds.md) | in progress | New agent rule and this plan log (7.0), faster Rust builds (7.1), Firefox chooser Esc/click-outside (7.2), hidden secure notes (7.3), 2FA QR on desktop (7.4), duplicate logins (7.5), secure-file storage removed at user request on 2026-11-09 (7.6 dropped), release 0.3.0 only when requested (7.7) | [handoff-phase7.md](../handoff-phase7.md) |
| 2026-10-28 | Phase 6: LocalPass on Fedora Linux + Firefox | [2026-10-28-phase6-linux.md](2026-10-28-phase6-linux.md) | done | Keyring protection, Unix-socket broker, master-password check, RPM, the Firefox extension port with its own decision window; v0.2.2 tested and approved by the user on 2026-10-31 | [handoff-phase6-linux.md](../handoff-phase6-linux.md) |
| 2026-10 (before) | Phases 0-5: core, browser extension, passkeys, Android provider, Windows passkey provider, hardening and release builds | none (the plan and its status are in the handoff) | 5.6 open (the final matrix on the Windows work laptop) | Everything up to the v0.2.0 release | [handoff-phase4-5.md](../handoff-phase4-5.md) |

## Rules

1. Every plan the user approves is saved here in the same session, as `YYYY-MM-DD-<slug>.md`, and gets a row in the table.
   The file is an exact copy of the approved plan with one status line added at the top.
2. An approved plan is never rewritten. A later change is appended at the end as a dated "Changes" section.
3. The status in the table (`approved`, `in progress`, `done`, `dropped`) is updated when a task or a phase finishes.
4. Plans that are not about this project stay out.
