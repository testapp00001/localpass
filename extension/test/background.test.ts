// The service worker's message handling: who may ask what (V26 reads, popup-only), delivering a
// fill only to the page it was checked for (V6), the one-shot "all logins" list that replaces
// per-keystroke searching, and what happens when the app declines (Replace? / Hello).

import { beforeEach, describe, expect, test } from "bun:test";
import { type FakeChrome, type Json, installChrome, installWindows, until } from "./chrome";
import { AppError, honestApp, seedCredentials } from "./fakeapp";

const EXT = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const FILL = { id: "i1", username: "alice", password: "pw", totpCode: null };

let fake: FakeChrome;
let loads = 0;
let requests: Json[] = [];

async function boot(answer?: (request: Json) => unknown) {
  fake = installChrome();
  requests = [];
  seedCredentials(fake);
  fake.server = honestApp({
    answer: (request) => {
      requests.push(request);
      return answer ? answer(request) : { op: request.op };
    },
  });
  await import(`../src/background/index.ts?${++loads}`);
}

function call(message: unknown, sender: unknown): Promise<{ ok: boolean; data?: any; code?: string; message?: string }> {
  return new Promise((resolve) => fake.onMessage!(message, sender, resolve as (reply: unknown) => void));
}

const popup = { id: EXT, url: `chrome-extension://${EXT}/popup.html` };
const page = (url = "https://a.example/login", tabId = 7) => ({ id: EXT, url, origin: new URL(url).origin, frameId: 0, tab: { id: tabId, url } });

beforeEach(() => {
  requests = [];
});

describe("who may ask", () => {
  test("every popup-only message is refused for a page and answered for the popup", async () => {
    await boot();
    fake.tabs = [{ id: 7, url: "https://a.example/login" }];
    const popupOnly: unknown[] = [
      { kind: "state" },
      { kind: "connect" },
      { kind: "loadLogins" },
      { kind: "cachedLogins" },
      { kind: "fillTab", tabId: 7, itemId: "i1", force: false },
      { kind: "getPrivacy" },
      { kind: "setPrivacy", disableBrowserManager: false },
    ];
    for (const message of popupOnly) {
      const refused = await call(message, page());
      expect(refused, JSON.stringify(message)).toMatchObject({ ok: false, code: "forbidden" });
    }
    expect((await call({ kind: "cachedLogins" }, popup)).ok).toBe(true);
    // Nothing a page asked for reached the app.
    expect(requests).toEqual([]);
  });

  test("a message from another extension is refused", async () => {
    await boot();
    const reply = await call({ kind: "cachedLogins" }, { id: "someotherextension", url: "chrome-extension://someotherextension/x.html" });
    expect(reply).toMatchObject({ ok: false, code: "forbidden" });
  });

  test("storage.local is restricted as soon as the worker starts, before any message", async () => {
    await boot();
    await until(() => fake.local.accessLevel === "TRUSTED_CONTEXTS");
    expect(fake.local.log).toEqual(["setAccessLevel:TRUSTED_CONTEXTS"]);
  });

  test("a page learns the passkey switch from the service worker, after storage is restricted", async () => {
    await boot();
    expect((await call({ kind: "passkeysEnabled" }, page())).data).toBe(true);
    fake.local.data.passkeysEnabled = false;
    expect((await call({ kind: "passkeysEnabled" }, page())).data).toBe(false);
    expect(fake.local.log[0]).toBe("setAccessLevel:TRUSTED_CONTEXTS");
  });

  test("the never-save list is read and written through the restricted area", async () => {
    await boot();
    await call({ kind: "capture", username: "u", password: "p" }, page());
    expect(fake.local.log[0]).toBe("setAccessLevel:TRUSTED_CONTEXTS");
  });
});

describe("Firefox senders (F3, F7)", () => {
  test("a moz-extension:// page is an extension page, not a web page, so it can use the popup-only messages", async () => {
    await boot();
    // Firefox names its own pages moz-extension://; the worker compares with the extension's own base URL.
    (globalThis as unknown as { chrome: typeof chrome }).chrome.runtime.getURL = (path: string) => `moz-extension://abc/${path}`;
    const mozPopup = { id: EXT, url: "moz-extension://abc/popup.html", tab: { id: 1, url: "moz-extension://abc/popup.html" } };
    expect((await call({ kind: "cachedLogins" }, mozPopup)).ok).toBe(true);
    // A web page with the same shape of sender is still refused.
    expect(await call({ kind: "cachedLogins" }, page())).toMatchObject({ ok: false, code: "forbidden" });
  });

  test("a sandboxed frame (sender.origin is the string \"null\" but sender.url is real) is never served", async () => {
    await boot();
    const sandboxed = { id: EXT, url: "https://a.example/frame", origin: "null", frameId: 9, tab: { id: 7, url: "https://a.example/" } };
    for (const message of [{ kind: "findLogins", interactive: true }, { kind: "fill", itemId: "i1" }]) {
      expect(await call(message, sandboxed), JSON.stringify(message)).toMatchObject({ ok: false, code: "invalid" });
    }
    expect(requests).toEqual([]);
  });
});

describe("Firefox decision windows are only for Firefox pages", () => {
  const flag = globalThis as unknown as { __FIREFOX__: boolean };
  const decide = { kind: "decide", ref: "r1", request: { decision: "chooseLogin" } };

  test("the Chrome build refuses every decision message, from anyone", async () => {
    await boot();
    const windows = installWindows();
    const kinds = [decide, { kind: "decisionInfo", id: "x" }, { kind: "decisionLogins", id: "x" }, { kind: "decisionCandidates", id: "x" }, { kind: "decisionAnswer", id: "x", answer: { action: "cancel" } }, { kind: "decideCancel", ref: "r1" }];
    for (const message of kinds.filter((m) => m.kind === "decide" || m.kind === "decideCancel")) {
      expect(await call(message, page()), JSON.stringify(message)).toMatchObject({ ok: false, code: "forbidden" });
    }
    expect(windows.created).toEqual([]);
  });

  test("the Firefox build opens a window for a page, not for the popup, and the window's own messages need its own address", async () => {
    await boot();
    const windows = installWindows();
    flag.__FIREFOX__ = true;
    try {
      // The popup (an extension page) may not ask for a decision on behalf of a site.
      expect(await call(decide, popup)).toMatchObject({ ok: false, code: "forbidden" });
      expect(windows.created).toEqual([]);
      void call(decide, page("https://a.example/login", 7));
      await until(() => windows.created.length === 1);
      const id = new URL(windows.created[0]!.url).searchParams.get("id")!;
      // A page's content script cannot read or answer it, and neither can the popup.
      for (const sender of [page("https://a.example/login", 7), popup]) {
        expect(await call({ kind: "decisionInfo", id }, sender)).toMatchObject({ ok: false, code: "forbidden" });
      }
      const window = { id: EXT, url: windows.created[0]!.url, tab: { id: 1, windowId: windows.created[0]!.windowId } };
      expect(await call({ kind: "decisionInfo", id }, window)).toMatchObject({ ok: true, data: { decision: "chooseLogin", origin: "https://a.example" } });
    } finally {
      flag.__FIREFOX__ = false;
    }
  });
});

describe("generating a password for a page (RA-4)", () => {
  test("a page may ask for one, and the app is asked to generate it", async () => {
    await boot((request) => (request.op === "generatePassword" ? { password: "Zx9-generated" } : {}));
    const reply = await call({ kind: "generate" }, page());
    expect(reply).toMatchObject({ ok: true, data: { password: "Zx9-generated" } });
    expect(requests.map((r) => r.op)).toContain("generatePassword");
  });
});

describe("captured sign-ins (RA-3)", () => {
  test("a page cannot make the worker store a huge or non-text value", async () => {
    await boot();
    const sender = page("https://a.example/login");
    await call({ kind: "capture", username: "alice", password: "x".repeat(5_000_000) }, sender);
    await call({ kind: "capture", username: "u".repeat(5_000), password: "pw" }, sender);
    await call({ kind: "capture", username: 5, password: { toString: "x" } }, sender);
    expect((await call({ kind: "pendingSave" }, sender)).data).toBeNull();
    expect(Object.keys(fake.session.data).filter((k) => k.startsWith("pending:"))).toEqual([]);
    await call({ kind: "capture", username: "alice", password: "ok-pw" }, sender);
    expect(((await call({ kind: "pendingSave" }, sender)).data as { username: string }).username).toBe("alice");
  });
});

describe("fillTab (V6)", () => {
  test("a fill is dropped when the tab went to another site while LocalPass was asking", async () => {
    await boot((request) => {
      if (request.op !== "fillLogin") return {};
      fake.tabs[0]!.url = "https://evil.example/phish"; // the user navigated during the unlock
      return FILL;
    });
    fake.tabs = [{ id: 5, url: "https://a.example/login" }];
    const reply = await call({ kind: "fillTab", tabId: 5, itemId: "i1", force: false }, popup);
    expect(reply).toMatchObject({ ok: false, code: "changed" });
    expect(fake.sentToTabs).toEqual([]);
    expect(fake.session.data["filled:5"]).toBeUndefined();
  });

  test("a fill is dropped when the tab is gone", async () => {
    await boot((request) => {
      if (request.op !== "fillLogin") return {};
      fake.tabs.length = 0;
      return FILL;
    });
    fake.tabs = [{ id: 5, url: "https://a.example/login" }];
    const reply = await call({ kind: "fillTab", tabId: 5, itemId: "i1", force: false }, popup);
    expect(reply.ok).toBe(false);
    expect(fake.sentToTabs).toEqual([]);
  });

  test("the fill goes to the top frame of the same site, with the site it was checked for", async () => {
    await boot((request) => {
      if (request.op !== "fillLogin") return {};
      fake.tabs[0]!.url = "https://a.example/next"; // same site, another page: still fine
      return FILL;
    });
    fake.tabs = [{ id: 5, url: "https://a.example/login" }];
    const reply = await call({ kind: "fillTab", tabId: 5, itemId: "i1", force: true }, popup);
    expect(reply.ok).toBe(true);
    expect(fake.sentToTabs).toHaveLength(1);
    expect(fake.sentToTabs[0]).toMatchObject({ tabId: 5, options: { frameId: 0 }, message: { kind: "doFill", origin: "https://a.example", fill: { password: "pw" } } });
    expect(requests.find((r) => r.op === "fillLogin")).toMatchObject({ origin: "https://a.example", force: true });
  });

  test("a page the extension can't fill is refused before anybody is asked", async () => {
    await boot();
    fake.tabs = [{ id: 5, url: "about:blank" }];
    const reply = await call({ kind: "fillTab", tabId: 5, itemId: "i1", force: false }, popup);
    expect(reply).toMatchObject({ ok: false, code: "invalid" });
    expect(requests).toEqual([]);
  });
});

describe("the list of all logins", () => {
  const ITEMS = [{ id: "i1", kind: "login", title: "Shop", subtitle: "alice", url: null, favorite: false, hasPassword: true, hasTotp: false, trashed: false }];

  test("one explicit action asks for up to 200 logins once; the popup reuses the answer for a short time", async () => {
    await boot((request) => (request.op === "searchItems" ? ITEMS : {}));
    expect((await call({ kind: "cachedLogins" }, popup)).data).toBeNull();
    const loaded = await call({ kind: "loadLogins" }, popup);
    expect(loaded.data).toEqual(ITEMS);
    const searches = requests.filter((r) => r.op === "searchItems");
    expect(searches).toHaveLength(1);
    expect(searches[0]).toMatchObject({ query: "", limit: 200 });
    // Reopening the popup (it closes when Windows Hello takes the focus) shows the list again
    // without another prompt.
    expect((await call({ kind: "cachedLogins" }, popup)).data).toEqual(ITEMS);
    expect(requests.filter((r) => r.op === "searchItems")).toHaveLength(1);
  });

  test("the list is not kept for long", async () => {
    await boot((request) => (request.op === "searchItems" ? ITEMS : {}));
    await call({ kind: "loadLogins" }, popup);
    const cached = fake.session.data["logins:all"] as { at: number };
    cached.at -= 10 * 60_000;
    expect((await call({ kind: "cachedLogins" }, popup)).data).toBeNull();
  });

  test("a declined Hello is an ordinary `denied` answer and keeps nothing", async () => {
    await boot((request) => (request.op === "searchItems" ? new AppError("denied", "The request was not approved") : {}));
    const reply = await call({ kind: "loadLogins" }, popup);
    expect(reply).toMatchObject({ ok: false, code: "denied" });
    expect(fake.session.data["logins:all"]).toBeUndefined();
  });
});

describe("saving a login", () => {
  async function capturedLogin() {
    const sender = page("https://a.example/login");
    await call({ kind: "capture", username: "alice", password: "new-pw" }, sender);
    const pending = (await call({ kind: "pendingSave" }, sender)).data as { id: string };
    return { sender, id: pending.id };
  }

  test("declining the Replace? dialog says so, forgets the password and is never retried", async () => {
    await boot((request) => (request.op === "saveLogin" ? new AppError("denied", "You chose not to replace what is saved in LocalPass") : {}));
    const { sender, id } = await capturedLogin();
    const reply = await call({ kind: "savePending", id }, sender);
    expect(reply).toMatchObject({ ok: true, data: { status: "declined" } });
    expect(requests.filter((r) => r.op === "saveLogin")).toHaveLength(1);
    expect(Object.keys(fake.session.data).filter((k) => k.startsWith("pending:"))).toEqual([]);
    // A second click finds nothing to save.
    expect(await call({ kind: "savePending", id }, sender)).toMatchObject({ ok: false, code: "notFound" });
    expect(requests.filter((r) => r.op === "saveLogin")).toHaveLength(1);
  });

  test("other failures keep the capture so the user can try again", async () => {
    await boot((request) => (request.op === "saveLogin" ? new AppError("tooMany", "busy") : {}));
    const { sender, id } = await capturedLogin();
    expect(await call({ kind: "savePending", id }, sender)).toMatchObject({ ok: false, code: "tooMany" });
    expect(Object.keys(fake.session.data).some((k) => k.startsWith("pending:"))).toBe(true);
  });
});

describe("answers for pages", () => {
  test("busy and too-large answers reach the menu as a status, not an exception", async () => {
    await boot((request) => (request.op === "findLogins" ? new AppError("tooMany", "busy") : {}));
    const reply = await call({ kind: "findLogins", interactive: true }, page());
    expect(reply).toMatchObject({ ok: true, data: { status: "tooMany" } });
  });
});
