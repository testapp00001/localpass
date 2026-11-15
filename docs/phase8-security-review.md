# Phase 8 security review (one panel in every browser, Chromium on Linux, smoother unlock)

Date 2026-11-13, fixes checked 2026-11-14. Scope: the uncommitted Phase 8 working tree on branch `phase7` on top of `c682e84`
(`git diff HEAD` plus untracked files), i.e. 8.1-8.4: the Chromium browsers on Linux, `checkLogin` and the save-offer rules, the
decision panel as the only place to pick, save or confirm in Chrome and Firefox, the panel's hold and keys, and the Linux prompt window.
8.5 (the `.xpi` in the RPM) was coded after the review and is not part of it.

How: one independent read-only agent (Opus, medium effort) read the code, briefed with the goal, the files and the questions but not
the lead's conclusions; nothing was run. The lead checked each finding's path against the code on 2026-11-13 and decided what to fix.
Fixes were made by agents and checked by the lead (code read, the full check set re-run, mutations run) on 2026-11-14. Threat model as
in `docs/phase5-security-review.md` and `docs/phase6-security-review.md`: a malicious page or page script, another local user, a LAN
peer; programs running as the same user are out of scope. Outcome: **no High or Medium finding**; two Low, three Info.

## Findings

| # | Sev | What | Status |
|---|---|---|---|
| F1 | Low | `checkLogin` as a guess oracle for a page script. | **Fixed** |
| F2 | Low (hardening) | A request the page left waiting behind the toolbar icon opened without the hold on the user's own click. | **Fixed** |
| F3 | Info | A page can cancel a pending save offer of its own tab. | Accepted |
| F4 | Info | `is_invisible` missed some invisible characters. | **Fixed** (cheap hardening) |
| F5 | Info | Linux host manifest written through a fixed `.part` name. | Accepted |

### F1 (Low): `checkLogin` as a guess oracle

Where: `extension/src/background/index.ts` (`savedState`, `offerPending`), `crates/lp-broker/src/server.rs` (`checkLogin`).

Scenario: a page script puts a user name and a guessed password into a form and calls `form.requestSubmit()`. The content script
captures it (with `byUser` false) and the worker kept it as a pending save. At the next page load `pendingSave` asked the app
`checkLogin`: an exact match for the site was dropped silently (`unchanged`), anything else opened the save panel. In Chromium the
panel takes the focus, so the page could probably see the difference (blur, `document.hasFocus()`; not measured). In Firefox the panel
does not take the focus, so it is probably not observable there.

Limits: the vault must be unlocked and the connection up (a locked vault answers `locked`, not interactive); a Windows provider client
gets `forbidden`; one guess per capture (about 1.5 s); every wrong guess opens a panel the user sees; only logins of the page's own
registrable domain (realistic case: user content on a subdomain, such as `forum.example.com` against an `example.com` login). At the
broker level nothing is new (a paired extension already gets the password of a matching login from `fillLogin` with `force: false`, as
the comment in `protocol.rs` says); what 8.2 added is a difference a page might observe.

Fix: the stored capture (`Pending`) keeps `byUser`, true only for a trusted click or Enter key (never a submit event, which a page script
can fire as trusted). `offerPending` asks the app only when `pending.byUser === true`. Any other capture (a page script's submit, a
capture stored by an older worker without the field) is offered as before 8.2: never dropped as `unchanged`, never marked as an update.
The fill-hash drop in `storeCapture` is unchanged. Cost: a sign-in sent by the page's own script is offered as "Save this login?" even
when it is saved already (saving it then changes nothing).

Tests: `extension/test/background.test.ts` (a page-submitted capture with `byUser` false, missing, `"true"`, `1` or `{}` is offered
plainly and the app is never asked; user-sent captures are still checked). `extension/e2e/firefox/run.ts` step 4b sends the "already
saved" and "changed password" sign-ins with trusted WebDriver BiDi input (real click and Enter) and pins F1 in the real browser: the same
logins sent with `requestSubmit()` are offered as "Save this login" with `byUser: false` and no update mark. Measured in Firefox 160: a
real Enter or click gives `byUser: true`, `requestSubmit()` gives `false`. Mutations run by the lead on a scratch copy of `extension/`:
the guard removed, 4 tests fail; `byUser === true` changed to `!!byUser`, 2 fail.

### F2 (Low, hardening): a page-staged request skipped the hold on the user's click

Where: `extension/src/background/decide.ts` (`attach`, `setToolbar`).

Scenario: a page in a background tab or an unfocused window calls `navigator.credentials.create()` or `get()`. The panel cannot open
(`checkPanelTab` fails), so the request waits up to two minutes behind a badge on the toolbar icon, whose per-tab popup is now the
decision. The user's next click on the icon (or `Alt+Shift+L`) in that tab shows the passkey panel instead of the normal popup, and since
the user opened it, it acted at once without the 0.6 s hold: the page chose what the user's click would show.

Limits: the panel's focus starts on a safe button (Enter alone does not act); the relying party id must fit the page's host; replacing a
passkey asks "Replace?" in the app; a staged save only stores the page's own origin.

Fix: a decision has `shown` (a panel page attached to it once). `attach` takes the own-open count as before and posts
`{userOpened: true}` only when the opening was not the background's own **and** the decision was shown before. So a decision's first
showing is always held, also from the toolbar icon or the shortcut; a reopen after the user closed it acts at once; the normal popup is
unchanged. A login pick always opens by itself first, so it was held already. A panel that closes before it attaches does not count as
shown, and a decision recreated after a worker restart is held once again (both the safe side). This changes 8.3c's "one opened with the
icon or the shortcut acts at once" for that first showing; `CLAUDE.md` ("One decision panel") and the README say so.

Tests: `extension/test/decide.test.ts`. Mutations by the lead: `!own && !first` changed to `!own`, 3 tests fail; `first = false`, 3 fail;
the own-open count taken only when shown, 5 fail.

### F3 (Info): a page can cancel its own tab's pending save offer

Where: `extension/src/background/decide.ts` (a newer `decide` from the same tab cancels the older decision; for a save, the cancel drops
the captured password). A page can do this with, for example, `navigator.credentials.get()` in that tab.

Accepted, not fixed: it only denies the user a save offer, and a page could already replace the tab's pending capture with one of its own.

### F4 (Info): `is_invisible` missed some invisible characters

Where: `crates/lp-broker/src/server.rs` `is_invisible` (used by `display_text` for client and pairing names shown in the app). It missed
the tag characters U+E0000-E007F, U+2028/U+2029 (`is_control` covers only Cc), the Hangul fillers U+115F, U+1160, U+3164, U+FFA0, and
U+034F. The name comes from our own extension or a same-user process (outside the threat model), and React renders it as text, so this
allowed no injection.

Fix: `is_invisible` now covers every Cf character of Unicode 17 (checked against Python's `unicodedata` 17.0.0) plus those above; the
extension's `displayText` (`extension/src/shared/limits.ts`) the same set. Tests: Rust
`display_text_drops_tags_separators_fillers_and_other_format_characters` (asserts each added range), `extension/test/display-text.test.ts`.
Not mutation-checked.

### F5 (Info): Linux manifest written through a fixed `.part` name

Where: `app/src-tauri/src/native_messaging/linux.rs` `write_manifest` writes `<name>.json.part` with `fs::write` (which follows a symlink
planted there) and then renames it. Only the same user could plant one, which is out of scope; `O_EXCL` or a random temporary name would
harden it. Also by design: Firefox's file (and `~/.mozilla/native-messaging-hosts`) is written even when Firefox is absent; Chromium
files only when the browser's config folder exists, which is never created; `$CHROME_CONFIG_HOME` is not followed (documented);
`allowed_origins`/`allowed_extensions` hold only our id; `XDG_CONFIG_HOME` is used only when absolute. Accepted, not fixed.

## Related fix found while checking: a badge click lost in Firefox

Not a review finding; found by the agent that moved the Firefox e2e to trusted input after F1, as the cause of a known e2e flake ("timed
out waiting for a decision window"). When a tab or window gets the focus back, Firefox fires `focus` on the field again; `onFocus` then
called `showBadge`, which removed the badge and drew a new button. When that happened between the mousedown and the mouseup of a click on
the badge, the click had no common target and was never dispatched, so nothing opened. Evidence from temporary instrumentation (a
MutationObserver and pointer listeners, since removed): in the failing run pointerdown and mousedown on the badge at 28 ms, the badge
replaced at 29 ms, mouseup at 30 ms, no click. A person can hit it too: a click on the badge in a window that did not have the focus.

Fix: `extension/src/content/ui.ts` `showBadge` updates the existing button in place (class and click handler, read through the current
badge record) for the same field while the button is still in the page; another field or a removed host gets a new button. The lead read
it against `armOpener`, which has no timing guard of its own (only "ignore an untrusted click" where required), so keeping the button
weakens nothing; the click still only asks for the panel, which holds. Tests: 2 new in `extension/test/ui-guard.test.ts`; the agent's 5
mutations were caught. 12 of 12 short runs passed with the fix (1 of 12 failed before). Not verified with a real mouse in a visible
window.

## Checked and sound

- Content-script messages (`background/index.ts` `handle`): `findLogins` answers only `{status, hasLogins}`; `capture` and `seenUser` are
  type- and length-checked, with the origin from `sender`; `pendingSave` (top frame) gives only the capture's id; `decide`,
  `decideCancel` and `decidePing` take the asker from `sender` with the frame-origin rule; `decisionInfo`/`Logins`/`Candidates`/`Answer`/
  `Unlock` pass `authorize()` (exact panel URL with the decision id, no `sender.tab`/`frameId`, `checkPanelTab`); `unlock`, `state`,
  `connect`, `loadLogins`, `cachedLogins`, `fillTab` and the privacy calls are popup-only; anything else is `forbidden`. No
  `onMessageExternal`, `externally_connectable` or `web_accessible_resources`.
- Secrets reach only the asking content script, as the reply to its own `decide`: a fill uses `fillLogin` with `force: false` for the
  decision's origin and is delivered after `whenInFront` re-checks the tab's site; the content script re-checks `location.origin` and that
  the form is still in the page; navigation or a tab switch cancels a login pick, also while it is busy.
- `checkRequest` re-validates page input (relying party id against the frame's host, length limits, known fields only); `fits()` limits
  "Generate" to new-password forms and passkey sign-in to requests that carry passkeys.
- Ports: content scripts cannot attach to `decide:` or `popup:` ports (sender URL, tab and frame are checked before any `ownOpens` use);
  only `openPanel` raises the count; a failed or unmatched opening keeps its count (the error is on the held side);
  `popup.html?from=panel` is always held; the panel focuses only a safe button.
- Saves: the origin is the capture's own as `sender` reported it; the panel shows that host; skipping the site check sends nothing to the
  page; `adopt`/`resume` need a matching capture id from session storage, which content scripts cannot read.
- 8.4 app side: the waiting browser's name passes `display_text` and is rendered as React text; the `Waiting` and `Prompt` guards remove
  the name and restore the window on a dropped connection; the dialogs' `OpenApproval` guards close them; a page cannot raise the app
  window without a click in the panel or popup (every interactive broker request comes from a panel answer or the popup); release builds
  always use `remap`; `LOCALPASS_RAISE` is read in debug builds only.
- Broker: the Linux `nmhost` caller check matches the exact extension origin with its trailing slash; pairing names are cleaned with
  `display_text`.
- The Firefox manifest drops `tabs`; without host access `tab.url` is unreadable and the panel checks fail closed (`notActive`).

## Not covered

- Whether a page can really observe the panel opening (blur, focus) in Chrome or Firefox (F1's premise); F1 was fixed without measuring it.
- The Windows side of 8.4 (`focus.rs`, Hello), changed only through `PromptWindow` calling `show_main`.
- `inject.ts` and `validate.ts` beyond confirming that `checkRequest` re-applies them.
- The e2e harness files, `shared/platform.ts`, the panel's CSS and HTML.
- `runtime.getContexts` and `windows.getLastFocused` in a real browser on Wayland (the `panelShown` rule): logic only.
- Concurrency of the hold timer in the decision page beyond reading.
- 8.5 (the `.xpi` in the RPM, `release.yml`, `open_firefox_addon`) and the 8.6 app fixes: checked by the lead, not by the reviewer.
- Windows and Android builds of the changed code were not compiled; the Windows panel was not run.
