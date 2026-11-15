// Messages between the extension's parts (content scripts, popup, service worker).

import type { BrokerState } from "../background/broker";
import type { ItemSummary, LoginFill, LoginMatch, PasskeyCandidate, SaveOutcome } from "./protocol";

export type ToBackground =
  /** Popup: connection, vault state and the logins for the active tab. */
  | { kind: "state" }
  | { kind: "connect" }
  /** Popup: ask the app to unlock. */
  | { kind: "unlock" }
  /**
   * Content script: what the field's badge needs for this frame's site (`SiteStatus`: whether LocalPass can
   * be used and has logins here), never titles or user names. `interactive` may connect (and start the app).
   */
  | { kind: "findLogins"; interactive: boolean }
  /**
   * Content script: a login form was submitted. `byUser`: a trusted click or Enter key sent it (see
   * `sentByUser` in content/index.ts); only such a capture may make the worker connect to the app.
   */
  | { kind: "capture"; username: string; password: string; byUser?: boolean }
  /** Content script: the first step of a two-step sign-in (an account name, no password box) was submitted. */
  | { kind: "seenUser"; username: string }
  /** Content script (top frame): whether the tab has a capture to offer; the answer is only its id (`PendingSaveRef`). */
  | { kind: "pendingSave" }
  /** Popup */
  | { kind: "loadLogins" }
  | { kind: "cachedLogins" }
  | { kind: "fillTab"; tabId: number; itemId: string; force: boolean }
  | { kind: "getPrivacy" }
  | { kind: "setPrivacy"; disableBrowserManager: boolean }
  /** Content script, for the page's WebAuthn calls: the popup's "use LocalPass for passkeys" switch. */
  | { kind: "passkeysEnabled" }
  /**
   * Content script. Asks the user to decide in LocalPass's own UI (the toolbar panel, or a window; see
   * background/decide.ts) and gets the outcome: `PasskeyCreated`, `PasskeyAssertion`, `ChooseOutcome` or
   * `SaveOutcome`. This is the only way a content script gets a password, a signature or a save. `ref` names
   * the request so the page can withdraw it. While the call is open the content script sends `decidePing`
   * every 10 s, which keeps the background awake when no panel shows the decision.
   */
  | { kind: "decide"; ref: string; request: DecisionRequest }
  | { kind: "decideCancel"; ref: string }
  | { kind: "decidePing"; ref: string }
  /** From the decision panel or window only (`id` names the decision). */
  | { kind: "decisionInfo"; id: string }
  /** The panel's Unlock: the background asks the app and keeps following it after the panel closed. */
  | { kind: "decisionUnlock"; id: string }
  | { kind: "decisionLogins"; id: string }
  | { kind: "decisionCandidates"; id: string }
  | { kind: "decisionAnswer"; id: string; answer: DecisionAnswer };

/** The parameters of a page's pending passkey sign-in (conditional mediation), as `content/validate.ts` checked them. */
export interface PasskeyGetParams {
  rpId: string;
  allow: string[];
  challenge: string;
  uv: UserVerification;
}

/**
 * What a content script asks the user to decide. Page text in it is shown only through `displayText`. Everything
 * in it is page input: the background checks it again (`background/decide.ts`, `checkRequest`).
 * `chooseLogin`: `newPassword` (the form asks for a new password) adds "Generate a strong password"; `passkeys`
 * (Chrome: the frame has a conditional passkey request pending) lists the site's passkeys for that request.
 */
export type DecisionRequest =
  | { decision: "passkeyCreate"; request: PasskeyCreateInput }
  | ({ decision: "passkeyGet" } & PasskeyGetParams)
  | { decision: "saveLogin"; id: string }
  | { decision: "chooseLogin"; newPassword?: boolean; passkeys?: PasskeyGetParams };

/** What a login pick hands the asking content script: a login to fill, a generated password, or a passkey sign-in. */
export type ChooseOutcome = { fill: LoginFill } | { generated: string } | { passkey: PasskeyAssertion };

/** What the decision panel (or window) sends back; only the answers that fit the decision are accepted. */
export type DecisionAnswer =
  | { action: "create" }
  | { action: "assert"; itemId: string }
  | { action: "save" }
  | { action: "never" }
  | { action: "fill"; itemId: string }
  | { action: "generate" }
  | { action: "fallback" }
  | { action: "cancel" };

/**
 * What the decision panel (or window) shows: the site is the one the browser reported for the asking frame (for a
 * save, the one it reported for the sign-in). `working`: an answer (or the app's Unlock) is being carried
 * out. `failed`: what went wrong the last time an answer was carried out while no panel showed it.
 */
export type DecisionInfo = { origin: string; host: string; working?: boolean; failed?: string } & (
  | { decision: "passkeyCreate"; request: PasskeyCreateInput }
  | { decision: "passkeyGet"; rpId: string }
  | { decision: "saveLogin"; username: string; update: boolean }
  | { decision: "chooseLogin"; newPassword: boolean; passkeys: boolean }
);

/**
 * What the background sends a decision panel (or the normal toolbar popup) over its port. `userOpened`: it was not
 * opened by the background's own `action.openPopup`, so the user opened it (toolbar icon or shortcut): it skips the
 * hold, and the popup focuses its first Fill.
 */
/** The name of the normal toolbar popup's port, followed by its browser window's id. */
export const POPUP_PORT = "popup:";

export interface PanelNote {
  userOpened?: true;
}

/** The reply to an answer: text to show before the window closes, if any. */
export interface DecisionDone {
  message?: string;
}

export type ToContent =
  /** `origin`: the site the password was fetched for; the page drops the fill if it is not that. */
  { kind: "doFill"; fill: LoginFill; origin: string };

export type Reply<T> = { ok: true; data: T } | { ok: false; code: string; message: string };

export interface PopupState {
  broker: BrokerState;
  tab: { id: number; origin: string | null; host: string | null; title: string } | null;
  logins: LoginMatch[];
  /** The vault status when the popup asked, if connected. */
  vault: "locked" | "unlocked" | "noVault" | null;
}

/** What a content script learns for its field's badge: "ok" or why LocalPass cannot help, and whether there are logins. */
export interface SiteStatus {
  status: string;
  hasLogins: boolean;
}

/** What a content script learns about a capture to offer: only enough to ask for the decision. */
export interface PendingSaveRef {
  id: string;
}

export interface FindLoginsReply {
  /** "ok", or why there are no logins: "locked", "offline", "unpaired", ... */
  status: string;
  logins: LoginMatch[];
}

export interface PendingSave {
  id: string;
  origin: string;
  username: string;
  /** The app said this would change the password of a saved login ("Update the saved password?"). */
  update?: boolean;
}

export interface PrivacyState {
  /** True when the browser's own password manager is turned off by this extension. */
  disabled: boolean;
  /** "controllable_by_this_extension" and friends: whether the setting can be changed. */
  control: string;
}

export type { ItemSummary, LoginFill, LoginMatch, SaveOutcome };

export type UserVerification = "required" | "preferred" | "discouraged";

/** What the page asked for when it called navigator.credentials.create (byte values base64url). */
export interface PasskeyCreateInput {
  rpId: string;
  rpName: string;
  userHandle: string;
  userName: string;
  userDisplayName: string;
  challenge: string;
  algorithms: number[];
  exclude: string[];
  uv: UserVerification;
}

export interface PasskeyCreated {
  credentialId: string;
  authData: string;
  attestationObject: string;
  publicKey: string;
  publicKeySpki: string;
  algorithm: number;
  /** The client data the browser would have produced (base64url of the JSON). */
  clientDataJSON: string;
}

export interface PasskeyAssertion {
  itemId: string;
  credentialId: string;
  authData: string;
  signature: string;
  userHandle: string;
  clientDataJSON: string;
}

export type { PasskeyCandidate };
