// Passkey requests from pages. The site and the client data come from here, never from the page:
// the page only says what it wants (challenge, relying party, accounts), and the service worker
// builds the client data from the origin the browser reports for the frame.

import type { PasskeyAssertion, PasskeyCandidate, PasskeyCreateInput, PasskeyCreated, UserVerification } from "../shared/messages";
import { fromBase64Url, toBase64Url } from "../shared/crypto";

type Ask = <T>(op: string, params?: Record<string, unknown>) => Promise<T>;

const encoder = new TextEncoder();

/** The relying party id a call defaults to: the page's host. */
function defaultRpId(origin: string): string {
  return new URL(origin).hostname;
}

/** What a browser puts in clientDataJSON, in the same order. */
function clientData(type: "webauthn.create" | "webauthn.get", challenge: string, origin: string): Uint8Array {
  return encoder.encode(JSON.stringify({ type, challenge, origin, crossOrigin: false }));
}

async function sha256(bytes: Uint8Array): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes as BufferSource));
}

export async function candidates(ask: Ask, origin: string, rpId: string, allow: string[]): Promise<PasskeyCandidate[]> {
  return ask<PasskeyCandidate[]>("passkeyCandidates", { origin, rpId: rpId || defaultRpId(origin), allow });
}

export async function create(ask: Ask, origin: string, input: PasskeyCreateInput): Promise<PasskeyCreated> {
  const rpId = input.rpId || defaultRpId(origin);
  const json = clientData("webauthn.create", input.challenge, origin);
  const created = await ask<Omit<PasskeyCreated, "clientDataJSON">>("passkeyCreate", {
    origin,
    rpId,
    rpName: input.rpName,
    userHandle: input.userHandle,
    userName: input.userName,
    userDisplayName: input.userDisplayName,
    algorithms: input.algorithms,
    exclude: input.exclude,
    uv: input.uv,
  });
  return { ...created, clientDataJSON: toBase64Url(json) };
}

export async function assert(ask: Ask, origin: string, itemId: string, rpIdOrEmpty: string, challenge: string, uv: UserVerification): Promise<PasskeyAssertion> {
  const rpId = rpIdOrEmpty || defaultRpId(origin);
  const json = clientData("webauthn.get", challenge, origin);
  const hash = await sha256(json);
  const signed = await ask<Omit<PasskeyAssertion, "clientDataJSON">>("passkeyAssert", {
    origin,
    rpId,
    itemId,
    clientDataHash: toBase64Url(hash),
    uv,
  });
  return { ...signed, clientDataJSON: toBase64Url(json) };
}

export { fromBase64Url };
