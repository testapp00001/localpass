// The extension's side of the page's WebAuthn calls. `inject.ts` (running in the page) turns
// navigator.credentials.create/get into messages; this answers them: it asks the user (a dialog
// or the field's menu), has LocalPass create or sign, and sends the result back. When LocalPass
// can't or shouldn't handle a call, it says so and the page falls back to the browser's own flow.

import { displayText } from "../shared/limits";
import type { PasskeyAssertion, PasskeyCreateInput, PasskeyCreated } from "../shared/messages";
import { BrokerError, type PasskeyCandidate } from "../shared/protocol";
import { ask } from "./ask";
import { type Sheet, showSheet } from "./ui";
import { type GetInput, checkCreate, checkGet } from "./validate";

/** What the page posted: nothing in it is trusted until `validate.ts` has checked it. */
interface PageRequest {
  __lp: "req";
  id: string;
  op: "create" | "get" | "cancel";
  payload: unknown;
}

type Refusal = { ok: false; name: string; message: string };
type Outcome = { ok: true; result: PasskeyCreated | PasskeyAssertion } | Refusal | { fallback: true };

/** LocalPass can't be used right now (not connected, not paired, ...): let the browser handle it. */
const UNAVAILABLE = new Set(["unpaired", "appNotRunning", "disconnected", "denied", "untrusted", "error", "noVault", "offline", "timeout"]);

const open = new Map<string, { sheet?: Sheet }>();
let conditional: { id: string; payload: GetInput } | null = null;

function post(id: string, body: Outcome) {
  window.postMessage({ __lp: "res", id, ...body }, "*");
}

function finish(id: string, body: Outcome) {
  const entry = open.get(id);
  if (!entry && !(conditional?.id === id)) return;
  open.delete(id);
  entry?.sheet?.close();
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

/** Every passkey row names the site (its id, as the app verified it) before anything the page called it. */
function describe(c: PasskeyCandidate): { label: string; sub: string } {
  const site = displayText(c.rpId);
  const name = displayText(c.rpName);
  const named = name && name.toLowerCase() !== site.toLowerCase() ? ` (${name})` : "";
  return {
    label: displayText(c.userName || c.userDisplayName) || "Passkey",
    sub: `${site}${named}${c.createdMs ? ` · ${new Date(c.createdMs).toLocaleDateString()}` : ""}`,
  };
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

// ---- create ----

/**
 * What the create dialog says. The first line is the relying party id the call is for (it was
 * checked against this page's address), never the name the page gave itself; page text is
 * cleaned, shortened and only ever shown as text.
 */
export function createDialogLines(p: PasskeyCreateInput, host: string): string[] {
  const rpId = displayText(p.rpId || host, 253);
  const lines = [rpId];
  const account = displayText(p.userName);
  const display = displayText(p.userDisplayName);
  if (account) lines.push(`Account: ${account}${display && display !== account ? ` (${display})` : ""}`);
  const name = displayText(p.rpName);
  if (name && name.toLowerCase() !== rpId.toLowerCase()) lines.push(`The site calls itself "${name}"`);
  lines.push("The passkey is saved in your LocalPass vault and syncs to your other devices.");
  return lines;
}

async function startCreate(id: string, p: PasskeyCreateInput) {
  if (!(await usable())) return post(id, { fallback: true });
  const cancelled: Refusal = { ok: false, name: "NotAllowedError", message: "The user cancelled the request." };
  const sheet = showSheet("Create a passkey?", () => finish(id, cancelled));
  open.set(id, { sheet });
  sheet.setBody(createDialogLines(p, location.hostname));
  const create = async () => {
    sheet.setBody(["Waiting for LocalPass…", "Confirm with Windows Hello if it asks."]);
    sheet.setButtons([{ label: "Cancel", onClick: () => finish(id, cancelled) }]);
    try {
      const result = await ask<PasskeyCreated>({ kind: "passkeyCreate", request: p });
      finish(id, { ok: true, result });
    } catch (e) {
      const outcome = failure(e);
      if (e instanceof BrokerError && e.code === "denied") {
        // The user said no (to Windows Hello, or to replacing the passkey already saved): that is
        // the answer. Nothing was changed, and nothing here tries again or offers a quiet detour.
        sheet.setBody(["Nothing was changed.", displayText(outcome.message, 200)]);
        sheet.setButtons([{ label: "Close", onClick: () => finish(id, outcome) }]);
        return;
      }
      sheet.setBody(["Could not create the passkey.", displayText(outcome.message, 200)]);
      sheet.setButtons([
        { label: "Use another method", onClick: () => finish(id, { fallback: true }) },
        { label: "Close", onClick: () => finish(id, outcome) },
      ]);
    }
  };
  sheet.setButtons([
    { label: "Create passkey", primary: true, onClick: () => void create() },
    { label: "Use another method", onClick: () => finish(id, { fallback: true }) },
  ]);
}

// ---- sign in ----

async function sign(id: string, p: GetInput, candidate: PasskeyCandidate) {
  const result = await ask<PasskeyAssertion>({ kind: "passkeyAssert", itemId: candidate.itemId, rpId: p.rpId, challenge: p.challenge, uv: p.uv });
  finish(id, { ok: true, result });
}

async function startGet(id: string, p: GetInput) {
  if (!(await enabled())) return post(id, { fallback: true });
  if (p.conditional) return startConditional(id, p);
  if (!(await usable())) return post(id, { fallback: true });
  const cancelled: Refusal = { ok: false, name: "NotAllowedError", message: "The user cancelled the request." };
  const sheet = showSheet("Sign in with a passkey", () => finish(id, cancelled));
  open.set(id, { sheet });
  sheet.setBody(["Looking for your passkeys…"]);
  const other = { label: "Use another method", onClick: () => finish(id, { fallback: true }) };
  const load = async (): Promise<void> => {
    let found: PasskeyCandidate[];
    try {
      found = await ask<PasskeyCandidate[]>({ kind: "passkeyCandidates", rpId: p.rpId, allow: p.allow });
    } catch (e) {
      const code = e instanceof BrokerError ? e.code : "internal";
      if (code === "locked") {
        sheet.setBody(["LocalPass is locked.", "Unlock it to use your passkeys."]);
        sheet.setButtons([
          { label: "Unlock", primary: true, onClick: () => void ask({ kind: "unlock" }).then(load, () => load()) },
          other,
        ]);
        return;
      }
      if (UNAVAILABLE.has(code)) return finish(id, { fallback: true });
      return finish(id, failure(e));
    }
    if (!found.length) return finish(id, { fallback: true });
    sheet.setBody([displayText(p.rpId || location.hostname, 253), "Choose a passkey to sign in with:"]);
    sheet.setRows(
      found.map((c) => ({
        ...describe(c),
        onSelect: () => {
          sheet.setRows([]);
          sheet.setBody(["Waiting for LocalPass…", "Confirm with Windows Hello if it asks."]);
          sheet.setButtons([{ label: "Cancel", onClick: () => finish(id, cancelled) }]);
          sign(id, p, c).catch((e) => {
            const outcome = failure(e);
            sheet.setBody(["Could not sign in.", displayText(outcome.message, 200)]);
            sheet.setButtons([other, { label: "Close", onClick: () => finish(id, outcome) }]);
          });
        },
      })),
    );
    sheet.setButtons([other]);
  };
  await load();
}

// ---- sign in from a field's menu (conditional mediation) ----

function startConditional(id: string, p: GetInput) {
  // A newer request replaces an older one.
  if (conditional) post(conditional.id, { ok: false, name: "AbortError", message: "Replaced by a newer request." });
  conditional = { id, payload: p };
}

/** True while the page is waiting for a passkey to be picked from a field's menu. */
export function conditionalPending(): boolean {
  return conditional !== null;
}

/** The passkeys to list in the field menu (empty when locked or none match). */
export async function conditionalCandidates(): Promise<PasskeyCandidate[]> {
  if (!conditional) return [];
  try {
    return await ask<PasskeyCandidate[]>({ kind: "passkeyCandidates", rpId: conditional.payload.rpId, allow: conditional.payload.allow });
  } catch {
    return [];
  }
}

export function passkeyRow(candidate: PasskeyCandidate): { label: string; sub: string; onSelect: () => void } {
  const text = describe(candidate);
  return {
    label: `Passkey: ${text.label}`,
    sub: text.sub,
    onSelect: () => void completeConditional(candidate),
  };
}

async function completeConditional(candidate: PasskeyCandidate) {
  const current = conditional;
  if (!current) return;
  try {
    await sign(current.id, current.payload, candidate);
  } catch (e) {
    // Leave the request pending so the user can pick again; the page isn't told about a refusal.
    console.warn("passkey sign-in failed", e);
  }
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
      open.get(target)?.sheet?.close();
      open.delete(target);
      if (conditional?.id === target) conditional = null;
    }
  });
}
