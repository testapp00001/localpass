// Types for the broker protocol (see crates/lp-broker/src/protocol.rs). The extension talks to the
// LocalPass app through the native-messaging helper; every message is JSON.

export type VaultStatus = "noVault" | "locked" | "unlocked";

export interface LoginMatch {
  id: string;
  title: string;
  username: string;
  url: string | null;
  /** Same host as the page, as opposed to the same registrable domain. */
  exact: boolean;
  hasPassword: boolean;
  hasTotp: boolean;
  favorite: boolean;
}

export interface LoginFill {
  id: string;
  username: string;
  password: string;
  totpCode: string | null;
}

export interface SaveOutcome {
  id: string;
  /** "declined": the user said no to replacing what is saved. */
  status: "created" | "updated" | "unchanged" | "declined";
}

export interface ItemSummary {
  id: string;
  kind: string;
  title: string;
  subtitle: string;
  url: string | null;
  favorite: boolean;
  hasPassword: boolean;
  hasTotp: boolean;
  trashed: boolean;
}

export interface PasskeyCandidate {
  itemId: string;
  rpId: string;
  rpName: string;
  userName: string;
  userDisplayName: string;
  credentialId: string;
  userHandle: string;
  createdMs: number;
}

/** An error the app (or the helper) reports; `code` is stable, `message` is for people. */
export class BrokerError extends Error {
  constructor(
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

// Handshake messages

export type ServerMessage =
  | { type: "challenge"; nonce: string }
  | { type: "unpaired" }
  | { type: "pairing"; code: string }
  | { type: "paired"; clientId: string; secret: string; status: VaultStatus }
  | { type: "ready"; status: VaultStatus; serverProof: string; appVersion: string }
  | { type: "denied" }
  | { type: "error"; code: string; message: string }
  // From the native-messaging helper itself rather than the app.
  | { type: "hostError"; code: string; message: string };

export interface ResponseMessage {
  id: number;
  ok: boolean;
  result?: unknown;
  error?: { code: string; message: string };
}
