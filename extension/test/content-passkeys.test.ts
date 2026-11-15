// What a page can make the content script do with a passkey request: sizes are capped (V9, over a cap the
// call goes back to the browser), only the checked fields go on, and the decision is made in LocalPass's
// own panel, never in the page (8.3b): nothing is drawn into the page, and the content script has no
// message of its own that creates or signs. What the panel shows (V13) is tested in decide-page.test.ts.

import { afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { MAX_CHALLENGE_BYTES } from "../src/shared/limits";
import { toBase64Url } from "../src/shared/crypto";
import { type Json, installChrome, sleep, until } from "./chrome";

const HOST = "login.example.com";
const b64 = (bytes: number) => toBase64Url(new Uint8Array(bytes).fill(7));
const results: Json[] = [];
let asked: Json[] = [];
let decideAnswer: (message: Json) => Promise<unknown>;
let passkeys: typeof import("../src/content/passkeys");

function createPayload(extra: Json = {}): Json {
  return {
    rpId: "example.com",
    rpName: "Example",
    userHandle: b64(16),
    userName: "alice",
    userDisplayName: "Alice A.",
    challenge: b64(32),
    algorithms: [-7, -257],
    exclude: [],
    uv: "preferred",
    ...extra,
  };
}
const getPayload = (extra: Json = {}) => ({ rpId: "example.com", allow: [], challenge: b64(32), uv: "preferred", conditional: false, ...extra });

let nextId = 0;
/** What a page (or its script) can do: post a request to the content script. */
function post(op: "create" | "get" | "cancel", payload: unknown): string {
  const id = `req-${++nextId}`;
  window.dispatchEvent(new MessageEvent("message", { data: { __lp: "req", id, op, payload }, source: window }));
  return id;
}

function reply(id: string): Json | undefined {
  return results.find((r) => r.id === id);
}

/** Whatever LocalPass drew into the page (nothing, since 8.3b). */
const inPage = () => [...document.documentElement.children].filter((e) => e.shadowRoot);
const decides = () => asked.filter((m) => m.kind === "decide");

beforeAll(async () => {
  (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM.setURL(`https://${HOST}/signin`);
  window.addEventListener("message", (e) => {
    if ((e.data as Json | undefined)?.__lp === "res") results.push(e.data as Json);
  });
  installChrome();
  // Another test file may have run first in this process and left things in the shared document, or installed the
  // bridge on a window that rp.test.ts has replaced since. The order of the files is not fixed (bun lists them in
  // the folder's order, which changes when a file is rewritten), so start clean with a copy of our own.
  for (const leftover of inPage()) leftover.remove();
  const copy = "content-passkeys";
  passkeys = (await import(`../src/content/passkeys.ts?${copy}`)) as typeof import("../src/content/passkeys");
  passkeys.installPasskeyBridge();
});

beforeEach(() => {
  results.length = 0;
  asked = [];
  decideAnswer = async () => ({ ok: true, data: { credentialId: "from-panel" } });
  const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime;
  runtime.sendMessage = async (message: Json) => {
    asked.push(message);
    if (message.kind === "passkeysEnabled") return { ok: true, data: true };
    if (message.kind === "findLogins") return { ok: true, data: { status: "ok", hasLogins: false } };
    if (message.kind === "decide") return decideAnswer(message);
    return { ok: true, data: null };
  };
});

afterEach(() => {
  // A conditional request left pending by a test is withdrawn, as the page would.
  const pending = passkeys.conditionalRequest();
  if (pending) post("cancel", { target: pending.id });
});

describe("V9: page-supplied sizes", () => {
  test("a request within the limits is asked in the panel", async () => {
    const id = post("create", createPayload({ exclude: Array.from({ length: 256 }, () => b64(1023)), userName: "x".repeat(256), challenge: b64(MAX_CHALLENGE_BYTES) }));
    await until(() => !!reply(id));
    expect(decides()).toHaveLength(1);
    expect(reply(id)).toMatchObject({ ok: true, result: { credentialId: "from-panel" } });
  });

  test.each([
    ["257 excluded credentials", { exclude: Array.from({ length: 257 }, () => b64(8)) }],
    ["a 1024-byte credential id", { exclude: [b64(1024)] }],
    ["a 257-character user name", { userName: "x".repeat(257) }],
    ["a 257-character display name", { userDisplayName: "x".repeat(257) }],
    ["a 257-character rp name", { rpName: "x".repeat(257) }],
    ["a 65-byte user handle", { userHandle: b64(65) }],
    ["a huge challenge", { challenge: b64(MAX_CHALLENGE_BYTES + 100) }],
    ["257 algorithms", { algorithms: Array.from({ length: 257 }, () => -7) }],
    ["a user name that is not text", { userName: { toString: "x" } }],
    ["a credential id that is not base64url", { exclude: ["not base64!"] }],
    ["an rp id for another site", { rpId: "bank.com" }],
    ["an rp id that is a suffix trick", { rpId: "ample.com" }],
  ])("create with %s goes back to the browser, and nobody is asked", async (_name, extra) => {
    const id = post("create", createPayload(extra as Json));
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(decides()).toEqual([]);
  });

  test("a payload that is not an object goes back to the browser", async () => {
    for (const bad of [null, "text", 42, [], { rpId: 1 }]) {
      const id = post("create", bad);
      await until(() => !!reply(id));
      expect(reply(id)).toMatchObject({ fallback: true });
    }
    const id = post("get", undefined);
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(decides()).toEqual([]);
  });

  test.each([
    ["257 allowed credentials", { allow: Array.from({ length: 257 }, () => b64(8)) }],
    ["a 1024-byte credential id", { allow: [b64(1024)] }],
    ["a huge challenge", { challenge: b64(MAX_CHALLENGE_BYTES + 100) }],
    ["an rp id for another site", { rpId: "bank.com" }],
  ])("get with %s goes back to the browser", async (_name, extra) => {
    const id = post("get", getPayload(extra as Json));
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(decides()).toEqual([]);
  });

  test("only the known fields are passed on to the service worker", async () => {
    const id = post("create", createPayload({ extraneous: "x".repeat(10_000), __proto__: { admin: true } }));
    await until(() => !!reply(id));
    const sent = (decides()[0]!.request as Json).request as Json;
    expect(Object.keys(sent).sort()).toEqual(["algorithms", "challenge", "exclude", "rpId", "rpName", "userDisplayName", "userHandle", "userName", "uv"]);
    const gid = post("get", getPayload({ extraneous: "x" }));
    await until(() => !!reply(gid));
    expect(decides()[1]!.request).toEqual({ decision: "passkeyGet", rpId: "example.com", allow: [], challenge: expect.any(String), uv: "preferred" });
  });

  test("a cancel for something that is not an id is ignored", () => {
    post("cancel", null);
    post("cancel", { target: { x: 1 } });
  });
});

describe("the decision is made in LocalPass's panel (8.3b)", () => {
  test("a create is asked by `decide`; nothing is drawn into the page and no other message creates", async () => {
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(inPage()).toEqual([]);
    expect(decides()[0]).toMatchObject({ ref: id, request: { decision: "passkeyCreate", request: { rpId: "example.com", userName: "alice" } } });
    expect(reply(id)).toMatchObject({ ok: true, result: { credentialId: "from-panel" } });
    expect(asked.map((m) => m.kind).filter((k) => !["passkeysEnabled", "findLogins", "decide", "decidePing"].includes(k as string))).toEqual([]);
  });

  test.each([
    ["the user chose another method", "fallback", { fallback: true }],
    ["the user closed the panel", "cancelled", { ok: false, name: "NotAllowedError" }],
    ["the user said no in the app", "denied", { ok: false, name: "NotAllowedError" }],
    ["the passkey already exists", "passkeyExists", { ok: false, name: "InvalidStateError" }],
  ])("when %s the page hears the matching answer", async (_why, code, expected) => {
    decideAnswer = async () => ({ ok: false, code, message: "x" });
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject(expected);
  });

  test("withdrawing a request withdraws the decision and the page hears nothing more", async () => {
    let finishPanel!: (v: unknown) => void;
    decideAnswer = () => new Promise((r) => (finishPanel = r));
    const id = post("create", createPayload());
    await until(() => decides().length > 0);
    post("cancel", { target: id });
    await until(() => asked.some((m) => m.kind === "decideCancel"));
    expect(asked.find((m) => m.kind === "decideCancel")).toMatchObject({ ref: id });
    finishPanel({ ok: false, code: "cancelled", message: "x" });
    await sleep(20);
    expect(reply(id)).toBeUndefined();
  });

  test("while its request is open the content script pings every 10 s (the panel may be closed), and stops after the answer", async () => {
    const { decideTiming } = await import("../src/content/ask");
    const saved = decideTiming.pingMs;
    decideTiming.pingMs = 15;
    try {
      let answer!: (v: unknown) => void;
      decideAnswer = () => new Promise((r) => (answer = r));
      const id = post("get", getPayload());
      await until(() => asked.filter((m) => m.kind === "decidePing").length >= 3, 1000, "pings");
      expect(asked.filter((m) => m.kind === "decidePing").every((m) => m.ref === id)).toBe(true);
      answer({ ok: true, data: { credentialId: "late" } });
      await until(() => !!reply(id));
      const count = asked.filter((m) => m.kind === "decidePing").length;
      await sleep(60);
      expect(asked.filter((m) => m.kind === "decidePing").length).toBe(count);
    } finally {
      decideTiming.pingMs = saved;
    }
  });

  test("an app that is not running still hands the call to the browser", async () => {
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: (m: Json) => Promise<unknown> } } }).chrome.runtime;
    const before = runtime.sendMessage;
    runtime.sendMessage = async (m: Json) => (m.kind === "findLogins" ? (asked.push(m), { ok: false, code: "appNotRunning", message: "x" }) : before(m));
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(decides()).toEqual([]);
  });

  test("the passkey switch: the content script asks the service worker, never storage", async () => {
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime;
    runtime.sendMessage = async (message: Json) => {
      asked.push(message);
      return message.kind === "passkeysEnabled" ? { ok: true, data: false } : { ok: true, data: { status: "ok", hasLogins: false } };
    };
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(asked.map((m) => m.kind)).toContain("passkeysEnabled");
  });
});

describe("a passkey sign-in from a field (conditional mediation)", () => {
  const flag = globalThis as unknown as { __FIREFOX__: boolean };

  test("Chrome: it waits for the login picker, with only the checked parameters; nothing is asked yet", async () => {
    const id = post("get", getPayload({ conditional: true, extraneous: "x" }));
    await sleep(30);
    expect(reply(id)).toBeUndefined();
    expect(decides()).toEqual([]);
    expect(passkeys.conditionalPending()).toBe(true);
    expect(passkeys.conditionalRequest()).toEqual({ id, params: { rpId: "example.com", allow: [], challenge: expect.any(String), uv: "preferred" } });
  });

  test("Chrome: the passkey chosen in the picker answers the request that was pending, once", async () => {
    const id = post("get", getPayload({ conditional: true }));
    await until(() => passkeys.conditionalPending());
    const assertion = { itemId: "k1", credentialId: "c", authData: "a", signature: "s", userHandle: "u", clientDataJSON: "j" };
    expect(passkeys.resolveConditional("another-request", assertion)).toBe(false);
    expect(reply(id)).toBeUndefined();
    expect(passkeys.resolveConditional(id, assertion)).toBe(true);
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ ok: true, result: { signature: "s" } });
    expect(passkeys.conditionalPending()).toBe(false);
    expect(passkeys.resolveConditional(id, assertion)).toBe(false);
  });

  test("Chrome: a request the page withdrew, or replaced, is not answered", async () => {
    const first = post("get", getPayload({ conditional: true }));
    await until(() => passkeys.conditionalPending());
    const second = post("get", getPayload({ conditional: true }));
    await until(() => !!reply(first));
    expect(reply(first)).toMatchObject({ ok: false, name: "AbortError" });
    post("cancel", { target: second });
    await sleep(10);
    expect(passkeys.conditionalPending()).toBe(false);
    expect(passkeys.resolveConditional(second, { itemId: "k1" } as never)).toBe(false);
    expect(reply(second)).toBeUndefined();
  });

  test("Firefox: it goes back to the browser at once and is not parked", async () => {
    flag.__FIREFOX__ = true;
    try {
      const id = post("get", getPayload({ conditional: true }));
      await until(() => !!reply(id));
      expect(reply(id)).toMatchObject({ fallback: true });
      expect(decides()).toEqual([]);
      expect(passkeys.conditionalPending()).toBe(false);
    } finally {
      flag.__FIREFOX__ = false;
    }
  });
});
