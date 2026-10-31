# Phase 6 security review (Linux + Firefox)

Date 2026-10-30. Read-only review of the uncommitted Phase 6 tree by a separate agent (code reading, no builds), then each
finding checked against the code by the main session. Threat model as in `docs/phase5-security-review.md`: a malicious page,
another local user, a LAN peer. Malware running as the same Linux user is out of scope. Outcome: **no High or Medium finding**.

## Findings

| # | Sev | What | Status |
|---|---|---|---|
| L1 | Low | `native_host::launch_app` (unix and Windows) let the app it starts inherit the helper's stdin/stdout/stderr, which are the browser's native-messaging pipes: the app's first log line went into the stream to Firefox (read as a ~900 MB frame length, port dropped) and the browser never saw the helper end. A page could not forge a frame (`displayText` strips control characters). | **Fixed**: all three streams are null. Not yet run for real (the path where the helper starts the app, Firefox closed and app not running, has not been tried on Linux). |
| L2 | Low | `AppHost::verify_user` opened the approval before marking it as a master-password check, so a caller of `broker_respond(id + 1, true)` could win the gap (ids are sequential). Needs code in the app's webview (CSP `default-src 'self'`, no remote content). | **Fixed**: open and mark happen under the `verifying` lock, which `respond` takes first. |
| L3 | Low | The passkey-creation reason read `Create a passkey for {page name} ({site id})`, so a page name like `Your Bank (bank.com)` could imitate the id. | **Fixed**: `Create a passkey for {site id} ("{name}")` (also the Windows Hello text; unit test updated). |
| L4 | Low | The data directory got the default umask (0755 on a 022 system). `vault.db` holds the Argon2 salt, KDF parameters and wrapped keys (offline guessing target); `device.key` and `broker-clients.bin` are sealed under the keyring key. Fedora homes are 0700, so it did not bite there. | **Fixed**: the data directory is set to 0700 at start (not on Android). |
| L5 | Low | `SecretServiceKeys::fetch_or_create`: with no entry in the default collection (alias changed, search failed) it creates a new key, so an existing `device.key` then fails to open ("keyring entry was replaced"). Fails closed, but the app is stuck and a spare entry is left. Two creators in the same second tie on `get_created()`. | **Fixed 2026-10-30**: `SecretServiceKeys::guarding([device.key, broker-clients.bin])` refuses to create a key while one of them exists (clear message); ties break by (created, object path). The refusal branch needs a real keyring and has no automated test. |
| I1 | Info | The `linux` release job uses `fedora:44` unpinned by digest (actions are pinned by SHA); the RPM is unsigned. | Open (pin the digest once the first release job has run). |
| I2 | Info | `exclude_from_history` is the KDE hint; GNOME clipboard-history extensions usually ignore it. Needs XWayland (arboard uses X11). | Documented in the README. |
| I3 | Info | The 5-try limit restarts per broker request; there is no global counter (`unlock` has the same property). `nmhost`'s caller check is argv-based, so it is a convenience; the gates are Firefox's `allowed_extensions` and the paired-secret handshake. A pre-existing 0755 runtime folder is refused, not repaired. `current_exe()` after an RPM upgrade may end in "(deleted)". | Accepted / noted. |
| I4 | Info | On Firefox a page can open decision windows without a user gesture (`navigator.credentials.get/create` goes straight to `viaWindow`). Bounded by one window per tab, 8 in all and the 600 ms hold; it can steal focus and reopen after Cancel, not act. A cheap mitigation (require `userActivation.isActive`) was not applied because sites call `get` on page load. | Accepted, documented. |

## What a hidden or covered badge gets a page (Firefox)

Measured by `extension/e2e/firefox/run.ts`: CSS hiding does not hide the badge (its host sets `all: initial` and the rest inline with
`!important`); a see-through decoy with `pointer-events: none` lets a real click reach the badge. Either way the click opens one window
per tab that shows the browser-reported site and the login titles, never a password, and nothing is filled until the user clicks a login
inside the window after the 600 ms hold. Fills go only to the asking frame for that frame's own origin with `force: false`.

## Verified safe

Socket folder/file permissions, symlink and owner refusal, `SO_PEERCRED`, stale-socket replacement (only our own socket), long profile
names; the `LPL1` keyring blob (aad, refusal of plain/foreign/tampered blobs, a wrong-size entry never replaced); the master-password check
(only `verify_password` can answer yes, tries counted before Argon2, ids never reused, reason shown as text with control/bidi characters
stripped); the Firefox native-messaging registration (one allowed extension, atomic write, no secrets); the decision window (`authorize`
checks sender id, URL with its own id and window id; answers must fit the decision; secrets go to the asking content script only; no
`web_accessible_resources`); sender checks including `moz-extension://` and the `"null"` origin; the IndexedDB area (a content script's
`indexedDB` is the page's); the Firefox manifest (no `host_permissions`, `nativeMessaging, storage, activeTab, privacy`); CI/release actions
pinned by SHA with `contents: read`, `web-ext` pinned exactly in the lockfile; clipboard clearing only when it still holds our value.
