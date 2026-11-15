// The extension's side of the page's WebAuthn calls. `inject.ts` (running in the page) turns
// navigator.credentials.create/get into messages; this answers them: the background shows the request in
// LocalPass's own panel (or a window), the user decides there, LocalPass creates or signs, and the result
// comes back as the reply to this script's `decide` message (background/decide.ts). Nothing is drawn into
// the page. When LocalPass can't or shouldn't handle a call, it says so and the page falls back to the
// browser's own flow.

import type { DecisionRequest, PasskeyAssertion, PasskeyCreateInput, PasskeyCreated, PasskeyGetParams } from "../shared/messages";
import { BrokerError } from "../shared/protocol";
import { ask, askDecision } from "./ask";
import { type GetInput, checkCreate, checkGet } from "./validate";

/** Set by `bun run build --firefox`. */
declare const __FIREFOX__: boolean;

/** What the page posted: nothing in it is trusted until `validate.ts` has checked it. */
interface PageRequest {
  __lp: "req";
  id: string;
  op: "create" | "get" | "cancel";
  payload: unknown;
}

type Refusal = { ok: false; name: string; message: string };
type Outcome = { ok: true; result: PasskeyCreated | PasskeyAssertion } | Refusal | { fallback: true };

/** The page requests a LocalPass decision is open for (a `decideCancel` withdraws them). */
const open = new Set<string>();
/** Chrome: the page's pending passkey sign-in from a field (conditional mediation), offered in the login picker. */
let conditional: { id: string; payload: GetInput } | null = null;

function post(id: string, body: Outcome) {
  window.postMessage({ __lp: "res", id, ...body }, "*");
}

function finish(id: string, body: Outcome) {
  if (!open.has(id) && conditional?.id !== id) return;
  open.delete(id);
  if (conditional?.id === id) conditional = null;
  post(id, body);
}

function failure(e: unknown): Refusal {
  const code = e instanceof BrokerError ? e.code : "internal";
  const message = e instanceof Error ? e.message : String(e);
  // WebAuthn error names a page's script knows how to handle.
  const name = code === "passkeyExists" ? "InvalidStateError" : code === "unsupported" ? "NotSupportedError" : code === "invalid" ? "SecurityError" : "NotAllowedError";
  return { ok: false, name, message };
}

/**
 * The popup's "use LocalPass for passkeys" switch (on unless the user turned it off). Content
 * scripts cannot read the extension's storage, so the service worker answers.
 */
async function enabled(): Promise<boolean> {
  try {
    return (await ask<boolean>({ kind: "passkeysEnabled" })) !== false;
  } catch {
    return true;
  }
}

/**
 * Whether LocalPass is reachable and unlocked-or-lockable (so asking the user makes sense). This
 * may connect: the service worker sleeps when idle, and a passkey request that finds it asleep
 * must not slip away to the browser's own flow.
 */
async function usable(): Promise<boolean> {
  if (!(await enabled())) return false;
  try {
    const reply = await ask<{ status: string }>({ kind: "findLogins", interactive: true });
    return reply.status === "ok" || reply.status === "locked";
  } catch {
    return false;
  }
}

/**
 * The background shows the request in the LocalPass toolbar panel (a window where the panel cannot be opened
 * without a click), the user decides there, and the outcome comes back as the reply. A request whose panel
 * was closed waits (badge on the toolbar icon) until it ends.
 */
async function viaPanel(id: string, request: DecisionRequest) {
  open.add(id);
  try {
    finish(id, { ok: true, result: await askDecision<PasskeyCreated | PasskeyAssertion>(id, request) });
  } catch (e) {
    finish(id, e instanceof BrokerError && e.code === "fallback" ? { fallback: true } : failure(e));
  }
}

async function startCreate(id: string, p: PasskeyCreateInput) {
  if (!(await usable())) return post(id, { fallback: true });
  return viaPanel(id, { decision: "passkeyCreate", request: p });
}

async function startGet(id: string, p: GetInput) {
  if (!(await enabled())) return post(id, { fallback: true });
  if (p.conditional) {
    // Firefox: a conditional request is left to the browser at once (its login picker offers no passkeys, and
    // parking it would leave the page waiting for a choice that never comes).
    if (__FIREFOX__) return post(id, { fallback: true });
    return startConditional(id, p);
  }
  if (!(await usable())) return post(id, { fallback: true });
  return viaPanel(id, { decision: "passkeyGet", rpId: p.rpId, allow: p.allow, challenge: p.challenge, uv: p.uv });
}

// ---- sign in from a field (conditional mediation, Chrome) ----

function startConditional(id: string, p: GetInput) {
  // A newer request replaces an older one.
  if (conditional) post(conditional.id, { ok: false, name: "AbortError", message: "Replaced by a newer request." });
  conditional = { id, payload: p };
}

/** True while the page is waiting for a passkey to be picked from a field. */
export function conditionalPending(): boolean {
  return conditional !== null;
}

/**
 * The pending conditional request, for the login picker (which lists the site's passkeys for it): its id and
 * the checked parameters, never anything else the page posted.
 */
export function conditionalRequest(): { id: string; params: PasskeyGetParams } | null {
  if (!conditional) return null;
  const { rpId, allow, challenge, uv } = conditional.payload;
  return { id: conditional.id, params: { rpId, allow, challenge, uv } };
}

/**
 * The user chose a passkey in the picker: the signature answers the page's pending request, if it is still
 * the one the picker was asked for (the page may have withdrawn or replaced it meanwhile: then nothing is sent).
 */
export function resolveConditional(id: string, assertion: PasskeyAssertion): boolean {
  if (conditional?.id !== id) return false;
  finish(id, { ok: true, result: assertion });
  return true;
}

// ---- the bridge to the page ----

let bridgeInstalled = false;

export function installPasskeyBridge() {
  if (bridgeInstalled) return;
  bridgeInstalled = true;
  window.addEventListener("message", (event) => {
    // Only the page in this very frame; other frames can't pose as it.
    if (event.source !== window) return;
    const data = event.data as PageRequest | undefined;
    if (!data || data.__lp !== "req" || typeof data.id !== "string" || data.id.length > 128) return;
    const id = data.id;
    // Anything over a limit or of the wrong shape goes back to the browser's own flow.
    if (data.op === "create") {
      const input = checkCreate(data.payload, location.hostname);
      if (!input) return post(id, { fallback: true });
      void startCreate(id, input).catch(() => post(id, { fallback: true }));
    } else if (data.op === "get") {
      const input = checkGet(data.payload, location.hostname);
      if (!input) return post(id, { fallback: true });
      void startGet(id, input).catch(() => post(id, { fallback: true }));
    } else if (data.op === "cancel") {
      const target = (data.payload as { target?: unknown } | null)?.target;
      if (typeof target !== "string") return;
      if (open.has(target)) void ask({ kind: "decideCancel", ref: target }).catch(() => {});
      open.delete(target);
      if (conditional?.id === target) conditional = null;
    }
  });
}
