# Plan log

Every plan the user has approved for LocalPass, newest first, with its status and where its state lives. A plan file says what
was decided and why; the handoff says what is done and what comes next. Start a session from the handoff, not from the plan.

| Date | Plan | File | Status | Summary | State lives in |
|---|---|---|---|---|---|
| 2026-11-12 | Phase 8: one protected panel in every browser, Chromium on Linux, smoother unlock | [2026-11-12-phase8-browser-experience.md](2026-11-12-phase8-browser-experience.md) | in progress | Local walkthrough accepted; 8.6 closed; PR #1 merged with CI passing. v0.3.0 published on 2026-11-15: Windows/Android release builds and signed Firefox RPM packaging verified, all six release files attested. Assisted Windows checks, installed-app Open and five usability decisions remain open; runtime smoke checks were not rerun here. | [handoff-phase8.md](../handoff-phase8.md) |
| 2026-11-08 | Firefox password picker in the toolbar panel | [2026-11-08-firefox-toolbar-picker.md](2026-11-08-firefox-toolbar-picker.md) | done | Field badge opens the protected toolbar panel on Firefox 149+; save and passkey windows stay as they are | [handoff-phase7.md](../handoff-phase7.md) |
| 2026-10-31 | Phase 7: secure files, vault tools, faster builds | [2026-10-31-phase7-files-tools-builds.md](2026-10-31-phase7-files-tools-builds.md) | done | Tasks 7.0-7.5 complete, secure-file storage 7.6 dropped at the user’s request, release 7.7 published as [v0.3.0](https://github.com/testapp00001/localpass/releases/tag/v0.3.0) on 2026-11-15. All platform builds, download checksums, and six build attestations verified. | [handoff-phase7.md](../handoff-phase7.md) |
| 2026-10-28 | Phase 6: LocalPass on Fedora Linux + Firefox | [2026-10-28-phase6-linux.md](2026-10-28-phase6-linux.md) | done | Keyring protection, Unix-socket broker, master-password check, RPM, the Firefox extension port with its own decision window; v0.2.2 tested and approved by the user on 2026-10-31 | [handoff-phase6-linux.md](../handoff-phase6-linux.md) |
| 2026-10 (before) | Phases 0-5: core, browser extension, passkeys, Android provider, Windows passkey provider, hardening and release builds | none (the plan and its status are in the handoff) | 5.6 open (the final matrix on the Windows work laptop) | Everything up to the v0.2.0 release | [handoff-phase4-5.md](../handoff-phase4-5.md) |

## Rules

1. Every plan the user approves is saved here in the same session, as `YYYY-MM-DD-<slug>.md`, and gets a row in the table.
   The file is an exact copy of the approved plan with one status line added at the top.
2. An approved plan is never rewritten. A later change is appended at the end as a dated "Changes" section.
3. The status in the table (`approved`, `in progress`, `done`, `dropped`) is updated when a task or a phase finishes.
4. Plans that are not about this project stay out.
