// Messages between the extension's parts (content scripts, popup, service worker).

import type { BrokerState } from "../background/broker";
import type { ItemSummary, LoginFill, LoginMatch, PasskeyCandidate, SaveOutcome } from "./protocol";

export type ToBackground =
  /** Popup: connection, vault state and the logins for the active tab. */
  | { kind: "state" }
  | { kind: "connect" }
  | { kind: "unlock" }
  /** Content script: logins for this frame's site. `interactive` may connect (and start the app). */
  | { kind: "findLogins"; interactive: boolean }
  /** Content script: the secrets for one login, for this frame's site. */
  | { kind: "fill"; itemId: string }
  /** Content script: a login form was submitted. */
  | { kind: "capture"; username: string; password: string }
  | { kind: "pendingSave" }
  | { kind: "savePending"; id: string }
  | { kind: "dismissPending"; id: string; never: boolean }
  /** Popup */
  | { kind: "loadLogins" }
  | { kind: "cachedLogins" }
  | { kind: "fillTab"; tabId: number; itemId: string; force: boolean }
  | { kind: "generate" }
  | { kind: "getPrivacy" }
  | { kind: "setPrivacy"; disableBrowserManager: boolean }
  /** Content script, for the page's WebAuthn calls. */
  | { kind: "passkeysEnabled" }
  | { kind: "passkeyCandidates"; rpId: string; allow: string[] }
  | { kind: "passkeyCreate"; request: PasskeyCreateInput }
  | { kind: "passkeyAssert"; itemId: string; rpId: string; challenge: string; uv: UserVerification };

export type ToContent =
  /** `origin`: the site the password was fetched for; the page drops the fill if it is not that. */
  | { kind: "doFill"; fill: LoginFill; origin: string }
  | { kind: "showSave"; id: string; origin: string; username: string };

export type Reply<T> = { ok: true; data: T } | { ok: false; code: string; message: string };

export interface PopupState {
  broker: BrokerState;
  tab: { id: number; origin: string | null; host: string | null; title: string } | null;
  logins: LoginMatch[];
  /** The vault status when the popup asked, if connected. */
  vault: "locked" | "unlocked" | "noVault" | null;
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
