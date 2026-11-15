// V6, page side: a fill is applied only when it was fetched for this page's own site. And the field's
// badge (8.3b): its click asks for LocalPass's picker, whose outcome fills this form.

import { beforeAll, describe, expect, test } from "bun:test";
import { type FakeChrome, installChrome, until } from "./chrome";

let fake: FakeChrome;
const sent: { kind?: string; [key: string]: unknown }[] = [];
/** What the background answers, by message kind (`data` of the reply); anything else gets null. */
const answers: Record<string, unknown> = {};

beforeAll(async () => {
  (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM.setURL("https://a.example/login");
  // happy-dom has no layout: give every element a size so the form counts as visible.
  (Element.prototype as unknown as { getBoundingClientRect: () => unknown }).getBoundingClientRect = () => ({ x: 0, y: 0, top: 0, left: 0, right: 100, bottom: 20, width: 100, height: 20 });
  document.body.innerHTML = `<form><input name="username" autocomplete="username"><input name="password" type="password"></form>`;
  fake = installChrome();
  (globalThis as unknown as { chrome: { runtime: { sendMessage: unknown } } }).chrome.runtime.sendMessage = async (message: { kind?: string }) => {
    sent.push(message);
    return { ok: true, data: answers[message.kind ?? ""] ?? null };
  };
  await import("../src/content/index");
});

const value = (name: string) => (document.querySelector(`input[name=${name}]`) as HTMLInputElement).value;
const FILL = { id: "i", username: "alice", password: "pw", totpCode: null };
const deliver = (message: unknown) => fake.onMessage!(message, {}, () => {});

describe("doFill", () => {
  test("a fill fetched for another site is ignored", () => {
    deliver({ kind: "doFill", fill: FILL, origin: "https://evil.example" });
    expect(value("username")).toBe("");
    expect(value("password")).toBe("");
  });

  test("a fill without a site is ignored", () => {
    deliver({ kind: "doFill", fill: FILL });
    expect(value("password")).toBe("");
  });

  test("a fill for this site is applied", () => {
    deliver({ kind: "doFill", fill: FILL, origin: "https://a.example" });
    expect(value("username")).toBe("alice");
    expect(value("password")).toBe("pw");
  });
});

describe("capturing a sign-in to offer saving (RA-3)", () => {
  const submit = () => document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  const set = (name: string, text: string) => ((document.querySelector(`input[name=${name}]`) as HTMLInputElement).value = text);
  const captures = () => sent.filter((m) => m.kind === "capture");

  test("a typed sign-in is sent", () => {
    sent.length = 0;
    set("username", "bob");
    set("password", "bob-secret");
    submit();
    expect(captures()).toEqual([{ kind: "capture", username: "bob", password: "bob-secret", byUser: false }]);
  });

  test("a page that sets a huge value gets nothing sent", () => {
    sent.length = 0;
    set("username", "bob");
    set("password", "x".repeat(5_000_000));
    submit();
    set("password", "y");
    set("username", "z".repeat(5_000));
    submit();
    expect(captures()).toEqual([]);
  });
});

describe("two-step sign-ins and code boxes (8.2)", () => {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  /** A new page body; the content script finds its forms after its debounced rescan. */
  async function show(html: string) {
    document.body.innerHTML = html;
    await sleep(400);
    sent.length = 0;
  }
  const submit = () => document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
  const set = (name: string, text: string) => ((document.querySelector(`input[name=${name}]`) as HTMLInputElement).value = text);

  test("the account step reports the name it sends, and nothing is captured", async () => {
    await show(`<form><input name="identifier" type="email" autocomplete="username"><button>Next</button></form>`);
    set("identifier", "carol@a.example");
    submit();
    expect(sent).toEqual([{ kind: "seenUser", username: "carol@a.example" }]);
    // An empty box, or a huge value set by the page, reports nothing.
    sent.length = 0;
    set("identifier", "   ");
    submit();
    set("identifier", "c".repeat(5_000));
    submit();
    expect(sent).toEqual([]);
  });

  test("a one-time code typed into a password box is not captured", async () => {
    await show(`<form><input name="otp" type="password" autocomplete="one-time-code" maxlength="6"><button>Verify</button></form>`);
    set("otp", "123456");
    submit();
    expect(sent.filter((m) => m.kind === "capture" || m.kind === "seenUser")).toEqual([]);
  });

  test("the password step sends the password with no user name", async () => {
    await show(`<form><input name="password" type="password" autocomplete="current-password"><button>Sign in</button></form>`);
    set("password", "carol-pw");
    submit();
    expect(sent.filter((m) => m.kind === "capture")).toEqual([{ kind: "capture", username: "", password: "carol-pw", byUser: false }]);
  });
});

describe("a sign-in the user sent (8.2): only a trusted click or Enter counts", () => {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  /** An event as the browser reports a real one (happy-dom cannot make trusted events). */
  const trusted = <E extends Event>(e: E) => (Object.defineProperty(e, "isTrusted", { value: true }), e);
  const set = (name: string, text: string) => ((document.querySelector(`input[name=${name}]`) as HTMLInputElement).value = text);
  const captures = () => sent.filter((m) => m.kind === "capture");

  async function form(user: string) {
    document.body.innerHTML = `<form><input name="username" autocomplete="username"><input name="password" type="password"><button id="go">Sign in</button></form>`;
    await sleep(400);
    sent.length = 0;
    set("username", user);
    set("password", `${user}-pw`);
  }

  test("a submit event never counts, even a trusted one (requestSubmit from a page script is trusted)", async () => {
    await form("erin");
    document.querySelector("form")!.dispatchEvent(trusted(new Event("submit", { bubbles: true, cancelable: true })));
    document.querySelector("#go")!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(captures()).toEqual([{ kind: "capture", username: "erin", password: "erin-pw", byUser: false }]);
  });

  test("a trusted click on the button counts, and is sent although a page's own submit came first", async () => {
    await form("fay");
    document.querySelector("form")!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }));
    document.querySelector("#go")!.dispatchEvent(trusted(new MouseEvent("click", { bubbles: true })));
    document.querySelector("#go")!.dispatchEvent(trusted(new MouseEvent("click", { bubbles: true })));
    expect(captures().map((m) => m.byUser)).toEqual([false, true]);
  });

  test("a trusted Enter counts, an untrusted one does not", async () => {
    await form("gus");
    const box = document.querySelector('input[name="password"]')!;
    box.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    box.dispatchEvent(trusted(new KeyboardEvent("keydown", { key: "Enter", bubbles: true })));
    expect(captures().map((m) => m.byUser)).toEqual([false, true]);
  });
});

describe("the badge asks for LocalPass's picker (8.3b)", () => {
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
  async function show(html: string) {
    document.body.innerHTML = html;
    await sleep(400);
    sent.length = 0;
  }
  const badge = () => [...document.documentElement.children].find((e) => e.shadowRoot)?.shadowRoot?.querySelector(".badge") as HTMLButtonElement | null;
  const input = (name: string) => document.querySelector(`input[name=${name}]`) as HTMLInputElement;
  async function clickBadge(on: string) {
    input(on).focus();
    await until(() => !!badge(), 1500, "the badge");
    badge()!.click();
    await until(() => sent.some((m) => m.kind === "decide"), 1500, "a decide request");
    return sent.find((m) => m.kind === "decide")!;
  }

  test("the badge is shown from the site's status alone, and a click asks for the picker; a chosen login fills this form", async () => {
    answers.findLogins = { status: "ok", hasLogins: true };
    answers.decide = { fill: FILL };
    await show(`<form><input name="username" autocomplete="username"><input name="password" type="password"></form>`);
    const asked = await clickBadge("username");
    expect(asked.request).toEqual({ decision: "chooseLogin" });
    await until(() => value("password") === "pw");
    expect(value("username")).toBe("alice");
    // Nothing else was asked: the content script has no message that fills by itself.
    expect(sent.map((m) => m.kind).filter((k) => !["findLogins", "decide", "decidePing"].includes(k!))).toEqual([]);
  });

  test("a new-password form says so, and a generated password fills its new password boxes (not the user name)", async () => {
    answers.findLogins = { status: "ok", hasLogins: false };
    answers.decide = { generated: "Zx9-generated" };
    await show(`<form><input name="username" autocomplete="username"><input name="password" type="password" autocomplete="new-password"><input name="confirm" type="password" autocomplete="new-password"></form>`);
    const asked = await clickBadge("password");
    expect(asked.request).toEqual({ decision: "chooseLogin", newPassword: true });
    await until(() => value("password") === "Zx9-generated");
    expect(value("confirm")).toBe("Zx9-generated");
    expect(value("username")).toBe("");
  });

  test("a cancelled pick changes nothing", async () => {
    answers.findLogins = { status: "ok", hasLogins: true };
    answers.decide = undefined;
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: (m: { kind?: string }) => Promise<unknown> } } }).chrome.runtime;
    const before = runtime.sendMessage;
    runtime.sendMessage = async (m) => (m.kind === "decide" ? (sent.push(m), { ok: false, code: "cancelled", message: "x" }) : before(m));
    try {
      await show(`<form><input name="username" autocomplete="username"><input name="password" type="password"></form>`);
      await clickBadge("username");
      await sleep(20);
      expect(value("username")).toBe("");
      expect(value("password")).toBe("");
    } finally {
      runtime.sendMessage = before;
    }
  });

  test("Chrome: with a passkey sign-in pending in the frame, the picker gets its checked parameters and the chosen passkey answers it", async () => {
    const results: Record<string, unknown>[] = [];
    const listen = (e: MessageEvent) => (e.data as { __lp?: string } | undefined)?.__lp === "res" && results.push(e.data as Record<string, unknown>);
    window.addEventListener("message", listen);
    try {
      answers.passkeysEnabled = true;
      answers.findLogins = { status: "ok", hasLogins: false };
      answers.decide = { passkey: { itemId: "k1", credentialId: "c", authData: "a", signature: "sig", userHandle: "u", clientDataJSON: "j" } };
      await show(`<form><input name="username" autocomplete="username webauthn"><input name="password" type="password"></form>`);
      const payload = { rpId: "a.example", allow: [], challenge: "AAAA", uv: "preferred", conditional: true, extra: "dropped" };
      window.dispatchEvent(new MessageEvent("message", { data: { __lp: "req", id: "cond-1", op: "get", payload }, source: window }));
      await sleep(30);
      const asked = await clickBadge("username");
      expect(asked.request).toEqual({ decision: "chooseLogin", passkeys: { rpId: "a.example", allow: [], challenge: "AAAA", uv: "preferred" } });
      await until(() => results.some((r) => r.id === "cond-1"));
      expect(results.find((r) => r.id === "cond-1")).toMatchObject({ ok: true, result: { signature: "sig" } });
      expect(value("username")).toBe("");
    } finally {
      window.removeEventListener("message", listen);
    }
  });

  test("Chrome: a passkey chosen in the picker does not answer a request the page made after the picker opened", async () => {
    const results: Record<string, unknown>[] = [];
    const listen = (e: MessageEvent) => (e.data as { __lp?: string } | undefined)?.__lp === "res" && results.push(e.data as Record<string, unknown>);
    window.addEventListener("message", listen);
    const runtime = (globalThis as unknown as { chrome: { runtime: { sendMessage: (m: { kind?: string }) => Promise<unknown> } } }).chrome.runtime;
    const before = runtime.sendMessage;
    let choose = () => {};
    const chosen = new Promise<void>((resolve) => (choose = resolve));
    const PASSKEY = { itemId: "k1", credentialId: "c", authData: "a", signature: "sig", userHandle: "u", clientDataJSON: "j" };
    // The user is still choosing in the picker until `choose()`.
    runtime.sendMessage = async (m) => (m.kind === "decide" ? (sent.push(m), await chosen, { ok: true, data: { passkey: PASSKEY } }) : before(m));
    const request = (id: string) =>
      window.dispatchEvent(new MessageEvent("message", { data: { __lp: "req", id, op: "get", payload: { rpId: "a.example", allow: [], challenge: "AAAA", uv: "preferred", conditional: true } }, source: window }));
    try {
      answers.passkeysEnabled = true;
      answers.findLogins = { status: "ok", hasLogins: false };
      await show(`<form><input name="username" autocomplete="username webauthn"><input name="password" type="password"></form>`);
      request("cond-old");
      await sleep(30);
      await clickBadge("username");
      // While the picker is open, the page replaces its request.
      request("cond-new");
      await until(() => results.some((r) => r.id === "cond-old"));
      expect(results.find((r) => r.id === "cond-old")).toMatchObject({ ok: false, name: "AbortError" });
      choose();
      await sleep(50);
      expect(results.filter((r) => r.id === "cond-new")).toEqual([]);
      expect(results.filter((r) => r.ok === true)).toEqual([]);
    } finally {
      runtime.sendMessage = before;
      // The page withdraws its pending request, so nothing is left parked for other tests.
      window.dispatchEvent(new MessageEvent("message", { data: { __lp: "req", id: "cancel-1", op: "cancel", payload: { target: "cond-new" } }, source: window }));
      window.removeEventListener("message", listen);
    }
  });
});
