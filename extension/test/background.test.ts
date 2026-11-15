// The service worker's message handling: who may ask what (V26 reads, popup-only; 8.3b: a content
// script gets no password, signature or save by a message of its own, and no login names), delivering
// a fill only to the page it was checked for (V6), the one-shot "all logins" list that replaces
// per-keystroke searching, and what happens when the app declines (Replace? / Hello).

import { beforeEach, describe, expect, test } from "bun:test";
import { IDBFactory } from "fake-indexeddb";
import { type FakeChrome, type Json, installChrome, installToolbar, installWindows, sleep, until } from "./chrome";
import { AppError, type SavedLogin, checkLoginAnswer, honestApp, seedCredentials } from "./fakeapp";

const EXT = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const FILL = { id: "i1", username: "alice", password: "pw", totpCode: null };

let fake: FakeChrome;
let loads = 0;
let requests: Json[] = [];

async function boot(answer?: (request: Json) => unknown, before?: () => void) {
  fake = installChrome();
  requests = [];
  seedCredentials(fake);
  before?.();
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

/**
 * What the top page learns from `pendingSave` (only the id, 8.3b), with what the worker keeps for the panel
 * (user name, update) read from session storage, so the 8.2 rules can still be checked.
 */
async function offer(sender: ReturnType<typeof page> = page()): Promise<{ id: string; username: unknown; update: true | undefined } | null> {
  const reply = await call({ kind: "pendingSave" }, sender);
  if (!reply.data) return null;
  expect(Object.keys(reply.data)).toEqual(["id"]);
  const stored = fake.session.data[`pending:${sender.tab.id}`] as Record<string, unknown> | undefined;
  return { id: reply.data.id, username: stored?.username, update: stored?.update ? true : undefined };
}

/** A login filled through the popup into tab `tabId` at `url` (what the worker remembers, so the sign-in after it is not offered). */
async function filledFromPopup(url = "https://a.example/login", tabId = 7) {
  fake.tabs = [{ id: tabId, url }];
  expect((await call({ kind: "fillTab", tabId, itemId: "i1", force: false }, popup)).ok).toBe(true);
}

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
    for (const message of [{ kind: "findLogins", interactive: true }, { kind: "decide", ref: "r", request: { decision: "chooseLogin" } }]) {
      expect(await call(message, sandboxed), JSON.stringify(message)).toMatchObject({ ok: false, code: "invalid" });
    }
    expect(requests).toEqual([]);
  });
});

describe("decisions are for pages, and only the decision's own UI may read or answer them (8.3b: both builds)", () => {
  const decide = { kind: "decide", ref: "r1", request: { decision: "chooseLogin" } };

  test("a window opens for a page (where the panel cannot), not for the popup, and the window's own messages need its own address", async () => {
    await boot();
    const windows = installWindows();
    // The popup (an extension page) may not ask for a decision on behalf of a site.
    expect(await call(decide, popup)).toMatchObject({ ok: false, code: "forbidden" });
    expect(windows.created).toEqual([]);
    void call(decide, page("https://a.example/login", 7));
    await until(() => windows.created.length === 1);
    const id = new URL(windows.created[0]!.url).searchParams.get("id")!;
    // A page's content script cannot read or answer it, and neither can the popup.
    for (const sender of [page("https://a.example/login", 7), popup]) {
      for (const message of [{ kind: "decisionInfo", id }, { kind: "decisionLogins", id }, { kind: "decisionCandidates", id }, { kind: "decisionUnlock", id }, { kind: "decisionAnswer", id, answer: { action: "fill", itemId: "i1" } }]) {
        expect(await call(message, sender), message.kind).toMatchObject({ ok: false, code: "forbidden" });
      }
    }
    expect(requests).toEqual([]);
    const window = { id: EXT, url: windows.created[0]!.url, tab: { id: 1, windowId: windows.created[0]!.windowId } };
    expect(await call({ kind: "decisionInfo", id }, window)).toMatchObject({ ok: true, data: { decision: "chooseLogin", origin: "https://a.example" } });
  });

  test("an extension page shown in a tab (sender.tab set, extension URL) may not ask for, withdraw or ping a decision", async () => {
    await boot();
    const windows = installWindows();
    const url = `chrome-extension://${EXT}/popup.html`;
    const extensionTab = { id: EXT, url, frameId: 0, tab: { id: 3, url } };
    for (const message of [decide, { kind: "decideCancel", ref: "r1" }, { kind: "decidePing", ref: "r1" }]) {
      expect(await call(message, extensionTab), message.kind).toMatchObject({ ok: false, code: "forbidden" });
    }
    expect(windows.created).toEqual([]);
    expect(requests).toEqual([]);
  });
});

describe("what a content script may no longer ask by itself (8.3b)", () => {
  const LOGINS = [{ id: "i1", title: "Bank of Alice", username: "alice@a.example" }];
  const closed: { kind: string; [key: string]: unknown }[] = [
    { kind: "fill", itemId: "i1" },
    { kind: "savePending", id: "x" },
    { kind: "dismissPending", id: "x", never: true },
    { kind: "passkeyCandidates", rpId: "a.example", allow: [] },
    { kind: "passkeyCreate", request: { rpId: "a.example", rpName: "A", userHandle: "AA", userName: "u", userDisplayName: "U", challenge: "AAAA", algorithms: [-7], exclude: [], uv: "preferred" } },
    { kind: "passkeyAssert", itemId: "k1", rpId: "a.example", challenge: "AAAA", uv: "preferred" },
    { kind: "generate" },
    { kind: "unlock" },
  ];

  for (const message of closed) {
    test(`\`${message.kind}\` from a page is refused, and the app is not asked`, async () => {
      await boot((request) => (request.op === "fillLogin" ? FILL : request.op === "generatePassword" ? { password: "Zx9" } : { op: request.op }));
      await call({ kind: "capture", username: "alice", password: "typed-pw" }, page());
      // The real id, so a refusal cannot hide behind "nothing to save".
      if ("id" in message) message.id = (fake.session.data["pending:7"] as { id: string }).id;
      const before = JSON.stringify(fake.session.data);
      expect(await call(message, page()), message.kind).toMatchObject({ ok: false, code: "forbidden" });
      expect(requests).toEqual([]);
      expect(JSON.stringify(fake.session.data)).toBe(before);
      expect(fake.local.data.neverSave).toBeUndefined();
    });
  }

  test("only `unlock` is still there, for the popup; the others are gone for everyone", async () => {
    await boot();
    expect((await call({ kind: "unlock" }, popup)).ok).toBe(true);
    expect(requests.map((r) => r.op)).toEqual(["unlock"]);
    for (const message of closed.filter((m) => m.kind !== "unlock")) {
      expect(await call(message, popup), message.kind).toMatchObject({ ok: false, code: "forbidden" });
    }
    expect(await call({ kind: "nonsense" }, page())).toMatchObject({ ok: false, code: "forbidden" });
    expect(requests.map((r) => r.op)).toEqual(["unlock"]);
  });

  test("findLogins tells a page's content script whether there is something to offer, never titles or user names", async () => {
    await boot((request) => (request.op === "findLogins" ? LOGINS : {}));
    const reply = await call({ kind: "findLogins", interactive: true }, page());
    expect(reply).toEqual({ ok: true, data: { status: "ok", hasLogins: true } });
    expect(JSON.stringify(reply)).not.toContain("alice");
    await boot((request) => (request.op === "findLogins" ? [] : {}));
    expect((await call({ kind: "findLogins", interactive: true }, page())).data).toEqual({ status: "ok", hasLogins: false });
  });

  test("pendingSave hands the page only the capture's id: no user name, site or update flag", async () => {
    await boot();
    await call({ kind: "capture", username: "alice@a.example", password: "typed-pw" }, page());
    const reply = await call({ kind: "pendingSave" }, page());
    expect(Object.keys(reply.data)).toEqual(["id"]);
    expect(JSON.stringify(reply)).not.toMatch(/alice|a\.example|typed-pw/);
  });
});

describe("captured sign-ins (RA-3)", () => {
  test("a page cannot make the worker store a huge or non-text value", async () => {
    await boot();
    const sender = page("https://a.example/login");
    await call({ kind: "capture", username: "alice", password: "x".repeat(5_000_000) }, sender);
    await call({ kind: "capture", username: "u".repeat(5_000), password: "pw" }, sender);
    await call({ kind: "capture", username: 5, password: { toString: "x" } }, sender);
    expect((await offer(sender))).toBeNull();
    expect(Object.keys(fake.session.data).filter((k) => k.startsWith("pending:"))).toEqual([]);
    await call({ kind: "capture", username: "alice", password: "ok-pw" }, sender);
    expect((await offer(sender))!.username).toBe("alice");
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

describe("saving a login (through the decision, the only way since 8.3b)", () => {
  /** A capture offered in the decision window (the fake browser has no toolbar panel); `save()` is the window's Save. */
  async function offeredInWindow() {
    const windows = installWindows();
    const sender = page("https://a.example/login");
    await call({ kind: "capture", username: "alice", password: "new-pw" }, sender);
    const { id } = (await offer(sender))!;
    const outcome = call({ kind: "decide", ref: "r", request: { decision: "saveLogin", id } }, sender);
    await until(() => windows.created.length === 1);
    const w = windows.created[0]!;
    const decision = new URL(w.url).searchParams.get("id")!;
    const save = () => call({ kind: "decisionAnswer", id: decision, answer: { action: "save" } }, { id: EXT, url: w.url, tab: { id: 1, windowId: w.windowId } });
    return { outcome, save };
  }

  test("declining the Replace? dialog says so, forgets the password and is never retried", async () => {
    await boot((request) => (request.op === "saveLogin" ? new AppError("denied", "You chose not to replace what is saved in LocalPass") : {}));
    const { outcome, save } = await offeredInWindow();
    expect(await save()).toMatchObject({ ok: true, data: { message: "You chose not to replace the saved password. Nothing was changed." } });
    expect(await outcome).toMatchObject({ ok: true, data: { status: "declined" } });
    expect(requests.filter((r) => r.op === "saveLogin")).toHaveLength(1);
    expect(Object.keys(fake.session.data).filter((k) => k.startsWith("pending:"))).toEqual([]);
    // A second click finds nothing to save.
    expect(await save()).toMatchObject({ ok: false, code: "notFound" });
    expect(requests.filter((r) => r.op === "saveLogin")).toHaveLength(1);
  });

  test("other failures keep the capture so the user can try again", async () => {
    await boot((request) => (request.op === "saveLogin" ? new AppError("tooMany", "busy") : {}));
    const { save } = await offeredInWindow();
    expect(await save()).toMatchObject({ ok: false, code: "tooMany" });
    expect(Object.keys(fake.session.data).some((k) => k.startsWith("pending:"))).toBe(true);
  });
});

describe("answers for pages", () => {
  test("busy and too-large answers reach the badge as a status, not an exception", async () => {
    await boot((request) => (request.op === "findLogins" ? new AppError("tooMany", "busy") : {}));
    const reply = await call({ kind: "findLogins", interactive: true }, page());
    expect(reply).toMatchObject({ ok: true, data: { status: "tooMany", hasLogins: false } });
  });
});

// ---- 8.2: a save is offered only for a new login or a changed password ----

/** The app with these logins saved: `checkLogin` answers from them, fills give FILL. */
function savedLogins(saved: SavedLogin[]) {
  const check = checkLoginAnswer(saved);
  return (request: Json) => (request.op === "fillLogin" ? FILL : request.op === "checkLogin" ? check(request) : {});
}

const pendingKeys = () => Object.keys(fake.session.data).filter((k) => k.startsWith("pending:"));
const checks = () => requests.filter((r) => r.op === "checkLogin");

describe("a sign-in LocalPass filled is not offered (8.2)", () => {
  async function filled(sender = page("https://a.example/login")) {
    await filledFromPopup(sender.tab.url, sender.tab.id);
    return sender;
  }

  test("the password step of a two-step sign-in (no user name box) is not offered", async () => {
    await boot((request) => (request.op === "fillLogin" ? FILL : new AppError("unsupported", "unknown request")));
    const sender = await filled();
    await call({ kind: "capture", username: "", password: "pw" }, sender);
    await call({ kind: "capture", username: "   ", password: "pw" }, page("https://a.example/next"));
    expect(pendingKeys()).toEqual([]);
    expect((await offer(sender))).toBeNull();
  });

  test("the user name may differ in case and surrounding spaces, as the app compares them", async () => {
    await boot((request) => (request.op === "fillLogin" ? FILL : new AppError("unsupported", "unknown request")));
    const sender = await filled();
    for (const username of ["Alice", " alice ", "ALICE\t"]) await call({ kind: "capture", username, password: "pw" }, sender);
    expect(pendingKeys()).toEqual([]);
  });

  test("another account, or another password, is still captured", async () => {
    await boot((request) => (request.op === "fillLogin" ? FILL : new AppError("unsupported", "unknown request")));
    const sender = await filled();
    await call({ kind: "capture", username: "bob", password: "pw" }, sender);
    expect((await offer(sender))!.username).toBe("bob");
    await call({ kind: "capture", username: "", password: "typed-new" }, sender);
    expect((await offer(sender))!.username).toBe("");
  });

  test("a fill from the popup counts too", async () => {
    await boot((request) => (request.op === "fillLogin" ? FILL : new AppError("unsupported", "unknown request")));
    fake.tabs = [{ id: 7, url: "https://a.example/login" }];
    expect((await call({ kind: "fillTab", tabId: 7, itemId: "i1", force: false }, popup)).ok).toBe(true);
    await call({ kind: "capture", username: "", password: "pw" }, page("https://a.example/login", 7));
    await call({ kind: "capture", username: "Alice", password: "pw" }, page("https://a.example/login", 7));
    expect(pendingKeys()).toEqual([]);
  });
});

describe("the account name of a two-step sign-in (8.2)", () => {
  test("a password-only capture takes the name sent on the step before, on the same site", async () => {
    await boot();
    await call({ kind: "seenUser", username: "carol" }, page("https://a.example/identifier"));
    await call({ kind: "capture", username: "", password: "typed" }, page("https://a.example/challenge"));
    expect((await offer(page("https://a.example/challenge")))).toMatchObject({ username: "carol" });
    // A box that has a user name keeps its own.
    await call({ kind: "capture", username: "dave", password: "typed2" }, page("https://a.example/login"));
    expect((await offer(page()))).toMatchObject({ username: "dave" });
  });

  test("another site, another tab, or an old name is not used", async () => {
    await boot();
    await call({ kind: "seenUser", username: "carol" }, page("https://a.example/identifier", 7));
    await call({ kind: "capture", username: "", password: "typed" }, page("https://b.example/login", 7));
    expect((await offer(page("https://b.example/login", 7)))).toMatchObject({ username: "" });
    await call({ kind: "capture", username: "", password: "typed" }, page("https://a.example/login", 8));
    expect((await offer(page("https://a.example/login", 8)))).toMatchObject({ username: "" });
    (fake.session.data["seenUser:7"] as { at: number }).at -= 5 * 60_000 + 1000;
    await call({ kind: "capture", username: "", password: "typed-again" }, page("https://a.example/login", 7));
    expect((await offer(page("https://a.example/login", 7)))).toMatchObject({ username: "" });
  });

  test("what a page reports is checked like a capture, and never comes from the popup", async () => {
    await boot();
    for (const username of ["", "   ", "u".repeat(5_000), 5, { toString: "x" }, null]) {
      await call({ kind: "seenUser", username }, page());
    }
    await call({ kind: "seenUser", username: "eve" }, popup);
    expect(Object.keys(fake.session.data).filter((k) => k.startsWith("seenUser:"))).toEqual([]);
    expect(requests).toEqual([]);
  });

  test("the password step after a filled first step is not offered", async () => {
    await boot((request) => (request.op === "fillLogin" ? FILL : new AppError("unsupported", "unknown request")));
    await filledFromPopup("https://a.example/identifier");
    await call({ kind: "seenUser", username: "alice" }, page("https://a.example/identifier"));
    await call({ kind: "capture", username: "", password: "pw" }, page("https://a.example/challenge"));
    expect(pendingKeys()).toEqual([]);
  });
});

describe("asking the app before offering (8.2)", () => {
  const SAVED = [{ origin: "https://a.example", username: "Alice", password: "old-pw" }];

  async function connected(answer: (request: Json) => unknown) {
    await boot(answer);
    expect((await call({ kind: "connect" }, popup)).ok).toBe(true);
    await until(() => fake.ports.length === 1 && fake.ports[0]!.sent.some((m) => m.type === "auth"));
    await sleep(10);
  }

  test("a login typed by hand that is saved already is dropped without an offer", async () => {
    await connected(savedLogins(SAVED));
    await call({ kind: "capture", username: " alice", password: "old-pw", byUser: true }, page());
    expect((await offer(page()))).toBeNull();
    expect(checks()).toEqual([expect.objectContaining({ origin: "https://a.example", username: " alice", password: "old-pw" })]);
    expect(pendingKeys()).toEqual([]);
  });

  test("a changed password is offered as an update, a new login as a save, and the app is asked once per capture", async () => {
    await connected(savedLogins(SAVED));
    await call({ kind: "capture", username: "alice", password: "new-pw", byUser: true }, page());
    expect((await offer(page()))).toMatchObject({ username: "alice", update: true });
    expect((await offer(page()))).toMatchObject({ update: true });
    expect(checks()).toHaveLength(1);
    await call({ kind: "capture", username: "bob", password: "pw", byUser: true }, page());
    const offered = (await offer(page()))!;
    expect(offered).toMatchObject({ username: "bob" });
    expect(offered.update).toBeUndefined();
  });

  test("locked, an older app, or no answer: offered as before", async () => {
    for (const answer of [new AppError("locked", "The vault is locked"), new AppError("unsupported", "unknown request: checkLogin"), { op: "checkLogin" }]) {
      await connected((request) => (request.op === "checkLogin" ? answer : {}));
      await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
      const offered = (await offer(page()))!;
      expect(offered, JSON.stringify(answer)).toMatchObject({ username: "alice" });
      expect(offered.update).toBeUndefined();
      expect(checks()).toHaveLength(1);
    }
  });

  test("a page load never connects to ask: without a connection the capture is offered as before", async () => {
    await boot(savedLogins(SAVED));
    await call({ kind: "capture", username: "alice", password: "old-pw" }, page());
    expect((await offer(page()))).toMatchObject({ username: "alice" });
    expect(fake.ports).toHaveLength(0);
    expect(requests).toEqual([]);
  });

  test("8.6 F1: a capture the user did not send is never checked with the app, and is offered as it is", async () => {
    // A page script can fill a guess and submit it (byUser false): whether an offer follows must not say whether it is saved.
    for (const byUser of [false, undefined, "true", 1, {}]) {
      await connected(savedLogins(SAVED));
      await call({ kind: "capture", username: "alice", password: "old-pw", byUser }, page());
      const offered = (await offer(page()))!;
      expect(offered, String(byUser)).toMatchObject({ username: "alice" });
      expect(offered.update, String(byUser)).toBeUndefined();
      expect(checks(), String(byUser)).toEqual([]);
      // A changed password the page made up is not marked as an update either.
      await call({ kind: "capture", username: "alice", password: "guess-2", byUser }, page());
      expect((await offer(page()))!.update, String(byUser)).toBeUndefined();
      expect(checks(), String(byUser)).toEqual([]);
    }
  });

  test("8.6 F1: a page's capture after the user's replaces it and is not checked; the user's sending again is", async () => {
    await connected(savedLogins(SAVED));
    await call({ kind: "capture", username: "alice", password: "new-pw", byUser: true }, page());
    expect((await offer(page()))).toMatchObject({ update: true });
    expect(checks()).toHaveLength(1);
    // The page submits a guess of its own: it replaces the capture, which is not checked and not an update.
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: false }, page());
    const guessed = (await offer(page()))!;
    expect(guessed.update).toBeUndefined();
    expect(checks()).toHaveLength(1);
    // The user sends the same sign-in: now it is checked (and dropped, it is saved already).
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    expect((await offer(page()))).toBeNull();
    expect(checks()).toHaveLength(2);
  });

  test("8.6 F1: the password step takes the name from the step before and its own byUser", async () => {
    await connected(savedLogins(SAVED));
    await call({ kind: "seenUser", username: "alice" }, page("https://a.example/identifier"));
    await call({ kind: "capture", username: "", password: "old-pw" }, page("https://a.example/challenge"));
    expect((await offer(page()))).toMatchObject({ username: "alice" });
    expect(checks()).toEqual([]);
    await call({ kind: "capture", username: "", password: "old-pw", byUser: true }, page("https://a.example/challenge"));
    expect((await offer(page()))).toBeNull();
    expect(checks()).toEqual([expect.objectContaining({ username: "alice", password: "old-pw" })]);
  });

  test("8.6 F1: a capture stored without byUser (an older worker) is offered unchecked", async () => {
    await connected(savedLogins(SAVED));
    fake.session.data["pending:7"] = { id: "old-1", origin: "https://a.example", username: "alice", password: "old-pw", tabId: 7, at: Date.now() };
    expect((await offer(page()))).toMatchObject({ id: "old-1", username: "alice" });
    expect(checks()).toEqual([]);
  });

  test("Firefox: an unchanged login is never offered; an update is, and the app is asked once per capture", async () => {
    const flag = globalThis as unknown as { __FIREFOX__: boolean; indexedDB: unknown };
    const savedIdb = flag.indexedDB;
    try {
      // Connected as the Chrome build (the test's credentials are in storage.local), then the
      // worker behaves as the Firefox build, whose never-save list is in IndexedDB.
      await connected(savedLogins(SAVED));
      flag.__FIREFOX__ = true;
      flag.indexedDB = new IDBFactory();
      await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
      expect((await offer(page()))).toBeNull();
      await call({ kind: "capture", username: "alice", password: "new-pw", byUser: true }, page());
      expect((await offer(page()))).toMatchObject({ update: true });
      // A later page of the tab gets the capture again, to take the open offer over: the background opens
      // the panel by itself only once per capture (see the 8.3a tests below and decide.test.ts).
      expect((await offer(page()))).toMatchObject({ update: true });
      expect(checks()).toHaveLength(2);
    } finally {
      flag.__FIREFOX__ = false;
      flag.indexedDB = savedIdb;
    }
  });
});

describe("a sign-in the user sent may connect; a page load never does (8.2)", () => {
  const SAVED = [{ origin: "https://a.example", username: "alice", password: "old-pw" }];
  const pairs = () => fake.ports.flatMap((p) => p.sent).filter((m) => m.type === "pair");

  test("(a) a user-sent capture with no connection connects, and the page load after it drops a saved login", async () => {
    await boot(savedLogins(SAVED));
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    expect(fake.ports).toHaveLength(1);
    expect((await offer(page()))).toBeNull();
    expect(checks()).toHaveLength(1);
    expect(pendingKeys()).toEqual([]);
    expect(pairs()).toEqual([]);
  });

  test("(b) a capture that is not marked as sent by the user never connects", async () => {
    for (const byUser of [false, undefined, "true", 1]) {
      await boot(savedLogins(SAVED));
      await call({ kind: "capture", username: "alice", password: "old-pw", byUser }, page());
      expect((await offer(page())), String(byUser)).toMatchObject({ username: "alice" });
      expect(fake.ports, String(byUser)).toHaveLength(0);
      expect(requests).toEqual([]);
    }
  });

  test("(c) asking for a pending save never connects", async () => {
    await boot(savedLogins(SAVED));
    expect((await offer(page()))).toBeNull();
    await call({ kind: "capture", username: "alice", password: "old-pw" }, page());
    for (let i = 0; i < 3; i++) await call({ kind: "pendingSave" }, page());
    expect(fake.ports).toHaveLength(0);
  });

  test("(d) a capture that is dropped (what LocalPass filled, or a never-save site) does not connect", async () => {
    await boot(savedLogins(SAVED));
    await filledFromPopup();
    fake.ports[0]!.serverClose(); // the app went away (or the worker lost its connection)
    await until(() => fake.ports.length === 1);
    await call({ kind: "capture", username: "", password: "pw", byUser: true }, page());
    await call({ kind: "capture", username: "Alice", password: "pw", byUser: true }, page());
    fake.local.data.neverSave = ["https://b.example"];
    await call({ kind: "capture", username: "zoe", password: "zoe-pw", byUser: true }, page("https://b.example/login"));
    await sleep(20);
    expect(fake.ports).toHaveLength(1);
    expect(pendingKeys()).toEqual([]);
    // The control: a sign-in that is kept for an offer does reconnect from the same state.
    await call({ kind: "capture", username: "zoe", password: "zoe-pw", byUser: true }, page());
    expect(fake.ports).toHaveLength(2);
  });

  test("an unpaired browser is left unpaired: no new connection, no pairing request", async () => {
    await boot(savedLogins(SAVED));
    delete fake.local.data.clientId;
    delete fake.local.data.secret;
    await call({ kind: "findLogins", interactive: true }, page());
    await until(() => fake.ports[0]?.sent.some((m) => m.type === "hello") ?? false);
    await sleep(20);
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    await sleep(20);
    expect(fake.ports).toHaveLength(1);
    expect(pairs()).toEqual([]);
    expect((await offer(page()))).toMatchObject({ username: "alice" });
  });

  test("an app that could not prove who it is stays refused: a sign-in does not reconnect to it", async () => {
    await boot(savedLogins(SAVED));
    fake.server = (port) => {
      port.onSend = (message) => {
        if (message.type === "hello") port.emit({ type: "challenge", nonce: "A".repeat(43) });
        else if (message.type === "auth") port.emit({ type: "ready", status: "unlocked", serverProof: "A".repeat(43), appVersion: "x" });
      };
    };
    await call({ kind: "findLogins", interactive: true }, page());
    await until(() => fake.ports[0]?.disconnected ?? false);
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    await sleep(20);
    expect(fake.ports).toHaveLength(1);
    expect((await offer(page()))).toMatchObject({ username: "alice" });
  });

  test("(e) a page load waits a little for a connection being made, and no longer than about 2 s", async () => {
    // The app answers the handshake after 300 ms: the page load waits and gets the app's answer.
    await boot(savedLogins(SAVED));
    const honest = fake.server!;
    fake.server = (port) => {
      honest(port);
      const answer = port.onSend!;
      port.onSend = (message, p) => void setTimeout(() => answer(message, p), "op" in message ? 0 : 300);
    };
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    let start = Date.now();
    expect((await offer(page()))).toBeNull();
    expect(Date.now() - start).toBeLessThan(1500);
    expect(checks()).toHaveLength(1);

    // An app that never answers: the offer is made as before after the bounded wait, nothing is asked.
    await boot(savedLogins(SAVED));
    fake.server = (port) => void (port.onSend = () => {});
    await call({ kind: "capture", username: "alice", password: "old-pw", byUser: true }, page());
    start = Date.now();
    expect((await offer(page()))).toMatchObject({ username: "alice" });
    const waited = Date.now() - start;
    expect(waited).toBeGreaterThanOrEqual(1900);
    expect(waited).toBeLessThan(3000);
    expect(checks()).toEqual([]);
  });
});

describe("Firefox: the save offer in the toolbar panel (8.3a)", () => {
  const flag = globalThis as unknown as { __FIREFOX__: boolean; indexedDB: unknown };
  let savedIdb: unknown;
  let toolbar: ReturnType<typeof installToolbar>;
  const panelOf = (id: string) => `chrome-extension://${EXT}/decide.html?id=${id}&presentation=panel`;
  const pendingOf = (tab = 7) => fake.session.data[`pending:${tab}`] as Record<string, unknown> | undefined;

  async function firefox(answer?: (request: Json) => unknown, seed?: () => void) {
    savedIdb = flag.indexedDB;
    flag.__FIREFOX__ = true;
    flag.indexedDB = new IDBFactory();
    await boot(answer, () => {
      toolbar = installToolbar();
      fake.tabs = [{ id: 7, url: "https://a.example/login", windowId: 20, active: true } as never];
      seed?.();
    });
  }
  const done = () => {
    flag.__FIREFOX__ = false;
    flag.indexedDB = savedIdb;
  };

  test("the panel opens by itself once per capture; the pages after the sign-in take it over and keep it alive", async () => {
    try {
      await firefox();
      await call({ kind: "capture", username: "zoe", password: "zoe-pw" }, page());
      const capture = (await offer(page("https://a.example/dashboard")))!;
      void call({ kind: "decide", ref: "first", request: { decision: "saveLogin", id: capture.id } }, page("https://a.example/dashboard"));
      await until(() => toolbar.opened.length === 1);
      const decisionId = new URL(toolbar.popups.get(7)!).searchParams.get("id")!;
      await until(() => pendingOf()?.offered === true);
      expect(pendingOf()).toMatchObject({ offered: true, decisionId });
      expect(toolbar.badges.get(7)).toBe("1");
      // A redirect to another address of the site: its page asks again and takes the offer over.
      fake.tabs[0]!.url = "https://www.a.example/home";
      const again = (await offer(page("https://www.a.example/home")))!;
      expect(again.id).toBe(capture.id);
      void call({ kind: "decide", ref: "second", request: { decision: "saveLogin", id: capture.id } }, page("https://www.a.example/home"));
      await sleep(30);
      expect(toolbar.opened).toHaveLength(1);
      expect((await call({ kind: "decidePing", ref: "second" }, page("https://www.a.example/home"))).data).toBe(true);
      expect((await call({ kind: "decidePing", ref: "first" }, page("https://www.a.example/home"))).data).toBe(false);
      // The panel still shows the capture's site, never the password.
      const info = await call({ kind: "decisionInfo", id: decisionId }, { id: EXT, url: panelOf(decisionId) });
      expect(info).toMatchObject({ ok: true, data: { decision: "saveLogin", host: "a.example", username: "zoe" } });
      expect(JSON.stringify(info)).not.toContain("zoe-pw");
    } finally {
      done();
    }
  });

  test("a ping is answered only for the page's own open request, in both builds", async () => {
    await boot();
    expect((await call({ kind: "decidePing", ref: "r" }, page())).data).toBe(false);
    try {
      await firefox();
      expect((await call({ kind: "decidePing", ref: "r" }, page())).data).toBe(false);
      expect(await call({ kind: "decidePing", ref: "r" }, popup)).toMatchObject({ ok: false, code: "forbidden" });
    } finally {
      done();
    }
  });

  test("after a restart the forgotten panels are gone from the toolbar and an offered save waits again under its id", async () => {
    try {
      const stale = panelOf("gone");
      const kept = panelOf("keep-id");
      await firefox(undefined, () => {
        fake.tabs.push({ id: 8, url: "https://b.example/", windowId: 20, active: false } as never);
        fake.session.data = {
          "decidePanel:8": stale,
          "decidePanel:7": kept,
          "pending:7": { id: "cap", origin: "https://a.example", username: "zoe", password: "pw", tabId: 7, at: Date.now(), offered: true, decisionId: "keep-id" },
        };
        toolbar.popups.set(8, stale);
        toolbar.badges.set(8, "1");
        toolbar.popups.set(7, kept);
      });
      // A panel the toolbar showed for the save before the restart still works.
      const info = await call({ kind: "decisionInfo", id: "keep-id" }, { id: EXT, url: kept });
      expect(info).toMatchObject({ ok: true, data: { decision: "saveLogin", username: "zoe" } });
      expect(toolbar.popups.get(8)).toBe(`chrome-extension://${EXT}/popup.html`);
      expect(toolbar.badges.get(8)).toBe("");
      expect(toolbar.popups.get(7)).toBe(kept);
      expect(toolbar.badges.get(7)).toBe("1");
      expect(toolbar.opened).toEqual([]);
      expect(fake.session.data["decidePanel:8"]).toBeUndefined();
    } finally {
      done();
    }
  });

  test("after a restart an expired capture is not offered again, and its tab gets the normal popup", async () => {
    try {
      const kept = panelOf("old-id");
      await firefox(undefined, () => {
        fake.session.data = {
          "decidePanel:7": kept,
          "pending:7": { id: "cap", origin: "https://a.example", username: "zoe", password: "pw", tabId: 7, at: Date.now() - 3 * 60_000, offered: true, decisionId: "old-id" },
        };
        toolbar.popups.set(7, kept);
        toolbar.badges.set(7, "1");
      });
      expect(await call({ kind: "decisionInfo", id: "old-id" }, { id: EXT, url: kept })).toMatchObject({ ok: false, code: "notFound" });
      expect(toolbar.popups.get(7)).toBe(`chrome-extension://${EXT}/popup.html`);
      expect(toolbar.badges.get(7)).toBe("");
    } finally {
      done();
    }
  });
});
