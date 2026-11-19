Status: approved on 2026-11-19; execution state and portable transfer instructions are in `../handoff-phase9.md`.

# Checkpoint the work and continue on Windows

## Current state

The existing [Phase 9 handoff](/home/nimda/projects/localpass/docs/handoff-phase9.md) already records implemented changes, previous Linux checks and outstanding work.

There are **34 modified files and 20 untracked files**, with nothing staged or committed. Fresh checks passed:

- `git diff --check`
- Four advisory-checker tests
- Eight Android/device-isolation helper tests

**Plan mode currently prevents updating logs, staging files or creating a commit.**

## Prepare the checkpoint

1. Add a dated Windows-transfer entry to the handoff, recording these fresh results separately from the earlier full test runs.
2. Preserve the approved implementation plan. Update the plan index to identify Windows as the continuation environment.
3. Record the remaining work accurately:
   - Phase 9.1 migration containment and development dependency triage remain unfinished.
   - Post-cutoff revocation and future-timestamp demonstrations remain unresolved and explicitly ignored.
   - Phases 9.2–9.8 remain pending.
   - Android APK isolation and Windows runtime behavior remain unverified.
4. Stage the reviewed Phase 9 source, tests, documentation and original audit evidence. Exclude build output, temporary logs, caches, credentials and machine-specific configuration.
5. Prepare a checkpoint commit titled:

   `security: checkpoint initial audit fixes and Windows handoff`

   Its description must identify this as incomplete remediation, not a release candidate.

## Transfer to Windows

- Place the checkpoint on `phase9/audit-remediation`.
- Create a Git bundle containing that branch for transfer without requiring a push.
- On Windows, import the branch and verify its commit hash and clean working tree.
- Install dependencies from the committed lockfiles and follow the existing Windows toolchain configuration.
- Read the Phase 9 handoff first, then resume with **unfinished Phase 9.1 containment**, followed by Phase 9.2.
- Use isolated profiles and the audit Android package. Leave the installed production app and personal vault untouched.

## Work on the Windows laptop

Continue implementation there, then perform Hello/fingerprint, Chrome/Edge, passkey-provider, suspend/session-lock and installer checks.

Request the Android phone once the isolated APK and origin-isolation fixtures are ready. No phone is needed for this checkpoint.

After leaving Plan mode, apply this logging and commit preparation before transferring the code.
