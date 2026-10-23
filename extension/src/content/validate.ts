// Checks the passkey requests a page posts to the content script. The page's own script runs in
// the same window (anything there can post this message), so the shape and size of every field is
// checked here, and only the known fields are passed on. `null` means "not acceptable": the caller
// hands the call back to the browser.

import type { PasskeyCreateInput, UserVerification } from "../shared/messages";
import {
  MAX_CHALLENGE_BYTES,
  MAX_CREDENTIAL_ID_BYTES,
  MAX_LIST,
  MAX_RP_ID_CHARS,
  MAX_TEXT_CHARS,
  MAX_USER_HANDLE_BYTES,
  b64uBytes,
  isB64u,
  rpIdFits,
} from "../shared/limits";

export interface GetInput {
  rpId: string;
  allow: string[];
  challenge: string;
  uv: UserVerification;
  conditional: boolean;
}

const UV: readonly string[] = ["required", "preferred", "discouraged"];

function text(value: unknown, max: number): string | null {
  return typeof value === "string" && value.length <= max ? value : null;
}

function bytes(value: unknown, max: number): string | null {
  return isB64u(value) && b64uBytes(value) <= max ? value : null;
}

function ids(value: unknown): string[] | null {
  if (!Array.isArray(value) || value.length > MAX_LIST) return null;
  const out: string[] = [];
  for (const id of value) {
    const ok = bytes(id, MAX_CREDENTIAL_ID_BYTES);
    if (ok === null) return null;
    out.push(ok);
  }
  return out;
}

function uv(value: unknown): UserVerification {
  return typeof value === "string" && UV.includes(value) ? (value as UserVerification) : "preferred";
}

export function checkCreate(payload: unknown, host: string): PasskeyCreateInput | null {
  if (!payload || typeof payload !== "object") return null;
  const p = payload as Record<string, unknown>;
  const rpId = text(p.rpId, MAX_RP_ID_CHARS);
  const rpName = text(p.rpName, MAX_TEXT_CHARS);
  const userName = text(p.userName, MAX_TEXT_CHARS);
  const userDisplayName = text(p.userDisplayName, MAX_TEXT_CHARS);
  const userHandle = bytes(p.userHandle, MAX_USER_HANDLE_BYTES);
  const challenge = bytes(p.challenge, MAX_CHALLENGE_BYTES);
  const exclude = ids(p.exclude);
  if (rpId === null || rpName === null || userName === null || userDisplayName === null) return null;
  if (userHandle === null || challenge === null || exclude === null) return null;
  if (!rpIdFits(rpId, host)) return null;
  const algorithms = p.algorithms;
  if (!Array.isArray(algorithms) || algorithms.length > MAX_LIST || !algorithms.every((a) => Number.isInteger(a))) return null;
  return { rpId, rpName, userHandle, userName, userDisplayName, challenge, algorithms: algorithms as number[], exclude, uv: uv(p.uv) };
}

export function checkGet(payload: unknown, host: string): GetInput | null {
  if (!payload || typeof payload !== "object") return null;
  const p = payload as Record<string, unknown>;
  const rpId = text(p.rpId, MAX_RP_ID_CHARS);
  const challenge = bytes(p.challenge, MAX_CHALLENGE_BYTES);
  const allow = ids(p.allow);
  if (rpId === null || challenge === null || allow === null) return null;
  if (!rpIdFits(rpId, host)) return null;
  return { rpId, allow, challenge, uv: uv(p.uv), conditional: p.conditional === true };
}
