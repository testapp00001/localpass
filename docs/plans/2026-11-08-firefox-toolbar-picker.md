Status: done (2026-11-08); implementation and verification recorded in docs/handoff-phase7.md.

# Firefox password picker in the toolbar panel

## Findings and project state

- The project is on `phase7`, with substantial uncommitted secure-file work. The handoff marks desktop files complete; Android files and security review remain. Last documented release: **0.2.2**.
- Extension checks passed: **240 tests, zero failures**, plus TypeScript checking. The full app and Rust workspace were not retested.
- Firefox already displays an **L button inside login fields**. Clicking it deliberately opens a separate extension window.
- The existing capability probe reconfirmed that **Firefox 159 lacks `IntersectionObserver.isVisible`**, which protects Chrome’s in-page picker against hidden or covered controls. [MDN compatibility data](https://github.com/mdn/browser-compat-data/blob/main/api/IntersectionObserver.json)
- Firefox supports programmatically opening its toolbar panel without user activation from version 149. This supports the layout you selected; the complete LocalPass panel flow still needs browser validation. [Mozilla API announcement](https://blog.mozilla.org/addons/2026/04/23/webextensions-api-changes-firefox-149-152/)

## Intended behavior

Clicking the field’s **L button** opens a LocalPass password picker attached to Firefox’s toolbar, in the same browser window. It displays the verified site and matching accounts. Selecting an account fills the original form.

This change covers password picking. Save prompts and passkey dialogs keep their existing behavior.

## Implementation

- Reuse the existing Firefox decision page and login-selection logic, adding a toolbar-panel presentation for `chooseLogin`.
- In the background, bind each request to the browser-reported tab, frame and origin. Set that tab’s action popup to the decision page with its unique request ID, then call `action.openPopup()` in the originating browser window.
- Extend decision authorization for toolbar panels: require the extension’s exact decision-page URL and active request ID, reject content-script senders, and verify that the bound tab remains active in its browser window. Preserve the existing checks for separate windows.
- Keep passwords out of the picker page; deliver the selected credential only to the requesting content script. Cancel on navigation, tab closure, replacement, or panel dismissal; restore the normal toolbar popup afterward.
- Preserve the decision page’s initial action delay. Esc and clicking outside dismiss without filling.
- If unlocking moves focus to the desktop app, dismiss the picker without filling. The user can reopen it after unlocking.
- Keep the existing separate-window flow on Firefox below 149. On newer Firefox, an `openPopup()` failure ends the request without filling; the normal toolbar picker remains available.
- No native broker protocol or vault-format changes.

## Validation and completion

- Add tests for panel authorization, request replacement, popup restoration, dismissal, failed opening, and stale requests.
- Run Firefox browser checks for matching-account selection, exact-form filling, same-origin frames, blocked cross-origin frames, tab changes, navigation, locked vaults, and multiple browser windows.
- Confirm password picking creates no extra browser window on Firefox 149+.
- Run extension tests, type checking, both browser builds and Firefox manifest linting; manually check the panel on GNOME using the temporary development add-on.
- Preserve existing uncommitted work. Record the approved plan and results in the project’s plan index and handoff; do not commit, push or publish.
