// What a page can make the content script do with a passkey request: sizes are capped (V9, over a
// cap the call goes back to the browser), and the dialog names the verified site and shows page
// text only as text (V13).

import { afterEach, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { MAX_CHALLENGE_BYTES, displayText } from "../src/shared/limits";
import { toBase64Url } from "../src/shared/crypto";
import { type Json, installChrome, sleep, until } from "./chrome";

const HOST = "login.example.com";
const b64 = (bytes: number) => toBase64Url(new Uint8Array(bytes).fill(7));
const results: Json[] = [];
let asked: Json[] = [];
let createAnswer: unknown = { ok: true, data: { credentialId: "c" } };

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

function sheet(): ShadowRoot | undefined {
  const host = [...document.documentElement.children].find((e) => e.shadowRoot);
  return host?.shadowRoot ?? undefined;
}

function sheetText(): string {
  return sheet()?.querySelector(".sheet")?.textContent ?? "";
}

/** Removes the dialogs (the content script keeps using the same shadow root). */
function closeSheets() {
  for (const node of sheet()?.querySelectorAll(".backdrop, .panel, .bar") ?? []) node.remove();
}

beforeAll(async () => {
  (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM.setURL(`https://${HOST}/signin`);
  window.addEventListener("message", (e) => {
    if ((e.data as Json | undefined)?.__lp === "res") results.push(e.data as Json);
  });
  const fake = installChrome();
  void fake;
  const { installPasskeyBridge } = await import("../src/content/passkeys");
  installPasskeyBridge();
});

beforeEach(() => {
  results.length = 0;
  asked = [];
  createAnswer = { ok: true, data: { credentialId: "c" } };
  const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime;
  runtime.sendMessage = async (message: Json) => {
    asked.push(message);
    if (message.kind === "passkeysEnabled") return { ok: true, data: true };
    if (message.kind === "findLogins") return { ok: true, data: { status: "ok", logins: [] } };
    if (message.kind === "passkeyCreate") return createAnswer;
    if (message.kind === "passkeyCandidates") return { ok: true, data: [] };
    return { ok: true, data: null };
  };
});

afterEach(closeSheets);

/** Waits until the request was either handed back to the browser or got a dialog. */
async function settle(id: string): Promise<"fallback" | "dialog" | "other"> {
  await until(() => !!reply(id) || !!sheet()?.querySelector(".sheet"), 1500, `an answer for ${id}`);
  const r = reply(id);
  if (r?.fallback) return "fallback";
  return sheet()?.querySelector(".sheet") ? "dialog" : "other";
}

describe("V9: page-supplied sizes", () => {
  test("a request within the limits gets a dialog", async () => {
    const id = post("create", createPayload({ exclude: Array.from({ length: 256 }, () => b64(1023)), userName: "x".repeat(256), challenge: b64(MAX_CHALLENGE_BYTES) }));
    expect(await settle(id)).toBe("dialog");
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
    expect(await settle(id)).toBe("fallback");
    expect(sheet()?.querySelector(".sheet") ?? null).toBeNull();
    expect(asked.filter((m) => m.kind === "passkeyCreate" || m.kind === "passkeyCandidates")).toEqual([]);
  });

  test("a payload that is not an object goes back to the browser", async () => {
    for (const bad of [null, "text", 42, [], { rpId: 1 }]) {
      const id = post("create", bad);
      expect(await settle(id)).toBe("fallback");
    }
    const id = post("get", undefined);
    expect(await settle(id)).toBe("fallback");
  });

  test.each([
    ["257 allowed credentials", { allow: Array.from({ length: 257 }, () => b64(8)) }],
    ["a 1024-byte credential id", { allow: [b64(1024)] }],
    ["a huge challenge", { challenge: b64(MAX_CHALLENGE_BYTES + 100) }],
    ["an rp id for another site", { rpId: "bank.com" }],
  ])("get with %s goes back to the browser", async (_name, extra) => {
    const id = post("get", { rpId: "example.com", allow: [], challenge: b64(32), uv: "preferred", conditional: false, ...(extra as Json) });
    expect(await settle(id)).toBe("fallback");
    expect(asked.filter((m) => m.kind === "passkeyCandidates")).toEqual([]);
  });

  test("only the known fields are passed on to the service worker", async () => {
    const id = post("create", createPayload({ extraneous: "x".repeat(10_000), __proto__: { admin: true } }));
    expect(await settle(id)).toBe("dialog");
    [...sheet()!.querySelectorAll("button")].find((b) => b.textContent === "Create passkey")!.click();
    await until(() => asked.some((m) => m.kind === "passkeyCreate"));
    const sent = asked.find((m) => m.kind === "passkeyCreate")!.request as Json;
    expect(Object.keys(sent).sort()).toEqual(["algorithms", "challenge", "exclude", "rpId", "rpName", "userDisplayName", "userHandle", "userName", "uv"]);
    await until(() => !!reply(id));
    expect(reply(id)?.ok).toBe(true);
  });

  test("a cancel for something that is not an id is ignored", () => {
    post("cancel", null);
    post("cancel", { target: { x: 1 } });
  });
});

describe("V13: what the create dialog shows", () => {
  test("the first line is the verified relying party id, not the name the page chose", async () => {
    const id = post("create", createPayload({ rpName: "PayPal Secure Login", userName: "alice" }));
    expect(await settle(id)).toBe("dialog");
    const lead = sheet()!.querySelector(".sheet p.lead")!;
    expect(lead.textContent).toBe("example.com");
    expect(sheetText()).toContain("The site calls itself \"PayPal Secure Login\"");
    expect(sheetText()).toContain("Account: alice (Alice A.)");
    // The page's name is secondary text, never the headline.
    expect(sheet()!.querySelector(".sheet p.lead")!.textContent).not.toContain("PayPal");
  });

  test("with no rp id the page's own host is shown", async () => {
    const id = post("create", createPayload({ rpId: "", rpName: "" }));
    expect(await settle(id)).toBe("dialog");
    expect(sheet()!.querySelector(".sheet p.lead")!.textContent).toBe(HOST);
  });

  test("page text is shown as text: markup stays markup", async () => {
    const evil = '<img src=x onerror="alert(1)"><b>bold</b>';
    const id = post("create", createPayload({ userName: evil, rpName: evil, userDisplayName: evil }));
    expect(await settle(id)).toBe("dialog");
    expect(sheet()!.querySelector("img")).toBeNull();
    expect(sheet()!.querySelector(".sheet b")).toBeNull();
    expect(sheetText()).toContain("<img src=x");
  });

  test("long names are shortened for display and control or direction characters are dropped", async () => {
    const id = post("create", createPayload({ userName: "a".repeat(256), rpName: "‮evil​ name\u0007\n" }));
    expect(await settle(id)).toBe("dialog");
    const text = sheetText();
    expect(text).not.toContain("a".repeat(81));
    expect(text).toContain("…");
    expect(text).not.toMatch(/[‮​\u0007]/);
    expect(text).toContain('"evil name"');
  });

  test("displayText drops hidden characters, collapses space and cuts to the limit", () => {
    expect(displayText("  a‮​b \n\t c  ")).toBe("ab c");
    expect(displayText("x".repeat(100), 10)).toBe(`${"x".repeat(9)}…`);
    expect(displayText("😀".repeat(100), 5)).toBe(`${"😀".repeat(4)}…`);
  });

  test("declining the Replace? dialog says so, offers no detour and answers NotAllowed", async () => {
    createAnswer = { ok: false, code: "denied", message: "You chose not to replace what is saved in LocalPass" };
    const id = post("create", createPayload());
    expect(await settle(id)).toBe("dialog");
    [...sheet()!.querySelectorAll("button")].find((b) => b.textContent === "Create passkey")!.click();
    await until(() => sheetText().includes("Nothing was changed"));
    expect(sheetText()).toContain("You chose not to replace");
    const labels = [...sheet()!.querySelectorAll("button")].map((b) => b.textContent);
    expect(labels).toEqual(["Close"]);
    expect(asked.filter((m) => m.kind === "passkeyCreate")).toHaveLength(1);
    [...sheet()!.querySelectorAll("button")][0]!.click();
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ ok: false, name: "NotAllowedError" });
    await sleep(20);
    expect(asked.filter((m) => m.kind === "passkeyCreate")).toHaveLength(1);
  });

  test("another failure still offers the browser's own flow", async () => {
    createAnswer = { ok: false, code: "internal", message: "boom" };
    const id = post("create", createPayload());
    expect(await settle(id)).toBe("dialog");
    [...sheet()!.querySelectorAll("button")].find((b) => b.textContent === "Create passkey")!.click();
    await until(() => sheetText().includes("Could not create"));
    expect([...sheet()!.querySelectorAll("button")].map((b) => b.textContent)).toEqual(["Use another method", "Close"]);
  });
});

describe("the passkey switch", () => {
  test("the content script asks the service worker, never storage", async () => {
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime;
    runtime.sendMessage = async (message: Json) => {
      asked.push(message);
      return message.kind === "passkeysEnabled" ? { ok: true, data: false } : { ok: true, data: { status: "ok", logins: [] } };
    };
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(asked.map((m) => m.kind)).toContain("passkeysEnabled");
  });
});

// ---- Firefox: the decision is made in a LocalPass window, not in the page ----

describe("Firefox build", () => {
  const flag = globalThis as unknown as { __FIREFOX__: boolean };
  let decideAnswer: () => Promise<unknown>;
  beforeEach(() => {
    flag.__FIREFOX__ = true;
    decideAnswer = async () => ({ ok: true, data: { credentialId: "from-window" } });
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: (m: Json) => Promise<unknown> } } }).chrome.runtime;
    const before = runtime.sendMessage;
    runtime.sendMessage = async (message: Json) => (message.kind === "decide" ? (asked.push(message), decideAnswer()) : before(message));
  });
  afterEach(() => {
    flag.__FIREFOX__ = false;
  });

  const getPayload = (extra: Json = {}) => ({ rpId: "example.com", allow: [], challenge: b64(32), uv: "preferred", conditional: false, ...extra });

  test("a create is decided by the background's window: the page gets no dialog and the result is passed on", async () => {
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(sheet()?.querySelector(".sheet") ?? null).toBeNull();
    const decide = asked.find((m) => m.kind === "decide")!;
    expect(decide).toMatchObject({ ref: id, request: { decision: "passkeyCreate", request: { rpId: "example.com", userName: "alice" } } });
    expect(reply(id)).toMatchObject({ ok: true, result: { credentialId: "from-window" } });
    expect(asked.some((m) => m.kind === "passkeyCreate")).toBe(false);
  });

  test("a get goes to the window with only the validated fields", async () => {
    const id = post("get", getPayload({ extraneous: "x" }));
    await until(() => !!reply(id));
    const decide = asked.find((m) => m.kind === "decide")!;
    expect(decide.request).toEqual({ decision: "passkeyGet", rpId: "example.com", allow: [], challenge: expect.any(String), uv: "preferred" });
    expect(sheet()?.querySelector(".sheet") ?? null).toBeNull();
  });

  test.each([
    ["the user chose another method", "fallback", { fallback: true }],
    ["the user closed the window", "cancelled", { ok: false, name: "NotAllowedError" }],
    ["the user said no in the app", "denied", { ok: false, name: "NotAllowedError" }],
    ["the passkey already exists", "passkeyExists", { ok: false, name: "InvalidStateError" }],
  ])("when %s the page hears the matching answer", async (_why, code, expected) => {
    decideAnswer = async () => ({ ok: false, code, message: "x" });
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject(expected);
  });

  test("a conditional get goes back to the browser at once and is not parked", async () => {
    const id = post("get", getPayload({ conditional: true }));
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(asked.some((m) => m.kind === "decide")).toBe(false);
    expect(asked.some((m) => m.kind === "passkeyCandidates")).toBe(false);
  });

  test("withdrawing a request closes its window and the page hears nothing more", async () => {
    let finishWindow!: (v: unknown) => void;
    decideAnswer = () => new Promise((r) => (finishWindow = r));
    const id = post("create", createPayload());
    await until(() => asked.some((m) => m.kind === "decide"));
    post("cancel", { target: id });
    await until(() => asked.some((m) => m.kind === "decideCancel"));
    expect(asked.find((m) => m.kind === "decideCancel")).toMatchObject({ ref: id });
    finishWindow({ ok: false, code: "cancelled", message: "x" });
    await sleep(20);
    expect(reply(id)).toBeUndefined();
  });

  test("an app that is not running still hands the call to the browser", async () => {
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: (m: Json) => Promise<unknown> } } }).chrome.runtime;
    const before = runtime.sendMessage;
    runtime.sendMessage = async (m: Json) => (m.kind === "findLogins" ? { ok: false, code: "appNotRunning", message: "x" } : before(m));
    const id = post("create", createPayload());
    await until(() => !!reply(id));
    expect(reply(id)).toMatchObject({ fallback: true });
    expect(asked.some((m) => m.kind === "decide")).toBe(false);
  });
});
