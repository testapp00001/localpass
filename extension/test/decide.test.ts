// Firefox: the decision window (background/decide.ts). Who may open and answer it, what each answer
// does, and what happens when the user closes it. The window itself is a page (src/decide), driven
// here by calling the same methods its messages reach.

import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import { type Deps, type PendingSaveInfo, Decisions, checkRequest, decisionTiming } from "../src/background/decide";
import type { DecisionAnswer, DecisionRequest } from "../src/shared/messages";
import { BrokerError } from "../src/shared/protocol";
import { FakePort, installChrome, installWindows, sleep, until } from "./chrome";

const EXT = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const ORIGIN = "https://login.example.com";
const FILL = { id: "i1", username: "alice", password: "s3cret", totpCode: null };
const asker = (extra: Partial<{ tabId: number; frameId: number; origin: string }> = {}) => ({ tabId: 7, frameId: 0, origin: ORIGIN, ...extra });

const CREATE: DecisionRequest = {
  decision: "passkeyCreate",
  request: { rpId: "example.com", rpName: "Example", userHandle: "AA", userName: "alice", userDisplayName: "Alice", challenge: "AAAA", algorithms: [-7], exclude: [], uv: "preferred" },
};
const GET: DecisionRequest = { decision: "passkeyGet", rpId: "example.com", allow: [], challenge: "AAAA", uv: "preferred" };

let windows: ReturnType<typeof installWindows>;
let calls: { op: string; params: Record<string, unknown> }[];
let calledDeps: string[];
let pending: PendingSaveInfo | null;
let failWith: BrokerError | null;
/** When set, requests to the app wait for it. */
let gate: Promise<void> | null;
let decisions: Decisions;

beforeEach(() => {
  installChrome();
  windows = installWindows();
  calls = [];
  calledDeps = [];
  failWith = null;
  gate = null;
  pending = { id: "p1", origin: ORIGIN, username: "alice", expiresAt: Date.now() + 120_000 };
  const deps: Deps = {
    async request<T>(op: string, params: Record<string, unknown> = {}) {
      calls.push({ op, params });
      await gate;
      if (failWith) throw failWith;
      if (op === "fillLogin") return FILL as T;
      if (op === "passkeyCandidates") return [{ itemId: "k1", rpId: "example.com" }] as T;
      return { credentialId: "cred", authData: "a", signature: "s", userHandle: "u" } as T;
    },
    findLogins: async (origin) => (calledDeps.push(`findLogins:${origin}`), { status: "ok", logins: [] }),
    rememberFill: async (tabId) => void calledDeps.push(`rememberFill:${tabId}`),
    pendingInfo: async (_tab, id) => (pending && pending.id === id ? pending : null),
    markOffered: async (tab, id, decisionId) => {
      calledDeps.push(`offered:${tab}:${id}`);
      if (pending?.id === id) pending = { ...pending, offered: true, decisionId };
    },
    savePending: async (tab, id) => {
      calledDeps.push(`save:${tab}:${id}`);
      await gate;
      if (failWith) throw failWith;
      return { id: "item", status: "created" as const };
    },
    dismissPending: async (tab, id, never) => void calledDeps.push(`dismiss:${tab}:${id}:${never}`),
  };
  decisions = new Decisions(deps);
  // As background/index.ts wires it.
  chrome.windows.onRemoved.addListener((windowId) => decisions.onWindowRemoved(windowId));
});

// A test that fails midway can leave a decision open (and its waits running): end every one, so a failing
// run still ends instead of hanging.
afterEach(() => {
  const open = (decisions as unknown as { byId: Map<string, { tabId: number }> }).byId;
  for (const d of [...open.values()]) decisions.onTabRemoved(d.tabId);
});

/** Opens a decision and returns the window's id, the decision id and the outcome. */
async function open(request: DecisionRequest, who = asker(), ref = "r1") {
  const before = windows.created.length;
  const outcome = decisions.open(who, ref, request);
  outcome.catch(() => {});
  await until(() => windows.created.length > before, 500, "a window");
  const w = windows.created.at(-1)!;
  const id = new URL(w.url).searchParams.get("id")!;
  return { outcome, id, windowId: w.windowId };
}

const page = (id: string, windowId: number) =>
  ({ id: EXT, url: `chrome-extension://${EXT}/decide.html?id=${id}`, tab: { id: 1, windowId } }) as chrome.runtime.MessageSender;

describe("opening", () => {
  test("opens one popup window on the extension's own page, and shows the site the browser reported", async () => {
    const { id, windowId } = await open(CREATE);
    expect(windows.created).toHaveLength(1);
    expect(windows.created[0]!.url).toBe(`chrome-extension://${EXT}/decide.html?id=${id}`);
    const info = await decisions.info(page(id, windowId), id);
    expect(info).toMatchObject({ decision: "passkeyCreate", origin: ORIGIN, host: "login.example.com" });
  });

  test("a newer request from the same tab replaces the older; other tabs are left alone", async () => {
    const first = await open(CREATE, asker({ tabId: 7 }));
    const other = await open(GET, asker({ tabId: 8 }), "r2");
    const third = await open(GET, asker({ tabId: 7 }), "r3");
    await expect(first.outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(windows.removed).toContain(first.windowId);
    expect(windows.removed).not.toContain(other.windowId);
    expect(windows.removed).not.toContain(third.windowId);
  });

  test("at most 8 windows at once", async () => {
    for (let tab = 1; tab <= 8; tab++) await open(GET, asker({ tabId: tab }), `r${tab}`);
    await expect(decisions.open(asker({ tabId: 9 }), "r9", GET)).rejects.toMatchObject({ code: "tooMany" });
  });

  test("saving is only offered by the top page, and only for a capture that exists", async () => {
    await expect(decisions.open(asker({ frameId: 3 }), "r", { decision: "saveLogin", id: "p1" })).rejects.toMatchObject({ code: "forbidden" });
    await expect(decisions.open(asker(), "r", { decision: "saveLogin", id: "other" })).rejects.toMatchObject({ code: "notFound" });
    expect(windows.created).toHaveLength(0);
  });
});

describe("a content script's request is page input (8.3b)", () => {
  const big = "A".repeat(6000);
  test("a request outside the limits, for another site or of an unknown shape is refused before anything opens", async () => {
    const bad: [string, unknown][] = [
      ["nothing", null],
      ["an unknown decision", { decision: "fillEverything" }],
      ["a create for another site", { ...CREATE, request: { ...(CREATE as { request: object }).request, rpId: "bank.com" } }],
      ["a create with a huge challenge", { ...CREATE, request: { ...(CREATE as { request: object }).request, challenge: big } }],
      ["a create that is not an object", { decision: "passkeyCreate", request: "x" }],
      ["a sign-in for another site", { ...GET, rpId: "bank.com" }],
      ["a sign-in with too many credentials", { ...GET, allow: Array.from({ length: 257 }, () => "AA") }],
      ["a pick whose passkey request is for another site", { decision: "chooseLogin", passkeys: { rpId: "bank.com", allow: [], challenge: "AAAA", uv: "preferred" } }],
      ["a pick whose passkey request has no challenge", { decision: "chooseLogin", passkeys: { rpId: "example.com", allow: [] } }],
      ["a save without an id", { decision: "saveLogin", id: 5 }],
      ["a save with a huge id", { decision: "saveLogin", id: big }],
    ];
    for (const [name, request] of bad) {
      // A request that wrongly passes would open a window and wait for the user: give it a moment, then fail.
      const outcome = decisions.open(asker(), "r", request).then(
        () => "opened",
        (e: BrokerError) => e.code,
      );
      expect(await Promise.race([outcome, sleep(200).then(() => "opened")]), name).toBe("invalid");
    }
    expect(windows.created).toEqual([]);
    expect(calls).toEqual([]);
  });

  test("only the checked fields go on, and only a real `true` asks for Generate", async () => {
    const { id, windowId } = await open({ decision: "chooseLogin", newPassword: "yes", passkeys: { rpId: "", allow: [], challenge: "AAAA", uv: "bogus", extra: 1 }, extra: 2 } as never);
    expect(await decisions.info(page(id, windowId), id)).toMatchObject({ decision: "chooseLogin", newPassword: false, passkeys: true });
    await decisions.candidates(page(id, windowId), id);
    expect(calls[0]!.params).toEqual({ origin: ORIGIN, rpId: "login.example.com", allow: [] });
  });

  test("fields the page added are dropped: the checked request has only the known fields", () => {
    const extra = { extra: 1, origin: "https://bank.com", tabId: 99 };
    const passkeys = { rpId: "example.com", allow: [], challenge: "AAAA", uv: "preferred" };
    expect(checkRequest({ ...GET, ...extra }, ORIGIN)).toEqual(GET);
    expect(checkRequest({ decision: "saveLogin", id: "p1", ...extra }, ORIGIN)).toEqual({ decision: "saveLogin", id: "p1" });
    expect(checkRequest({ decision: "chooseLogin", ...extra }, ORIGIN)).toEqual({ decision: "chooseLogin" });
    expect(checkRequest({ decision: "chooseLogin", newPassword: true, passkeys: { ...passkeys, ...extra }, ...extra }, ORIGIN)).toEqual({
      decision: "chooseLogin",
      newPassword: true,
      passkeys,
    } as DecisionRequest);
  });
});

describe("who may use a decision", () => {
  test("only the window that was opened, with its own id in the URL", async () => {
    const { id, windowId } = await open(CREATE);
    const good = page(id, windowId);
    const bad: [string, unknown][] = [
      ["another window", { ...good, tab: { id: 1, windowId: windowId + 1 } }],
      ["no tab", { id: EXT, url: good.url }],
      ["the web page's content script", { id: EXT, url: ORIGIN + "/", tab: { id: 7, windowId } }],
      ["the popup", { id: EXT, url: `chrome-extension://${EXT}/popup.html`, tab: { id: 1, windowId } }],
      ["another extension", { ...good, id: "other" }],
      ["another decision's URL", { ...good, url: `chrome-extension://${EXT}/decide.html?id=nope` }],
      ["a lookalike page", { ...good, url: `chrome-extension://${EXT}/decide.html.evil/x?id=${id}` }],
    ];
    for (const [name, sender] of bad) {
      await expect(decisions.info(sender as chrome.runtime.MessageSender, id), name).rejects.toMatchObject({ code: "forbidden" });
      await expect(decisions.answer(sender as chrome.runtime.MessageSender, id, { action: "create" }), name).rejects.toMatchObject({ code: "forbidden" });
    }
    expect(calls).toEqual([]);
    await expect(decisions.info(good as chrome.runtime.MessageSender, "unknown")).rejects.toMatchObject({ code: "notFound" });
    expect((await decisions.info(good as chrome.runtime.MessageSender, id)).decision).toBe("passkeyCreate");
  });

  test("only the answers that fit the decision", async () => {
    const { id, windowId } = await open(GET);
    const sender = page(id, windowId);
    for (const answer of [{ action: "create" }, { action: "fill", itemId: "i" }, { action: "save" }, { action: "never" }, { action: "bogus" }] as never[]) {
      await expect(decisions.answer(sender, id, answer)).rejects.toMatchObject({ code: "invalid" });
    }
    expect(calls).toEqual([]);
    await expect(decisions.logins(sender, id)).rejects.toMatchObject({ code: "invalid" });
  });
});

describe("answers", () => {
  test("create: signs for the browser-reported site, tells the page the result and closes the window", async () => {
    const { outcome, id, windowId } = await open(CREATE);
    const done = await decisions.answer(page(id, windowId), id, { action: "create" });
    expect(done).toEqual({});
    expect(calls[0]).toMatchObject({ op: "passkeyCreate", params: { origin: ORIGIN, rpId: "example.com" } });
    expect(await outcome).toMatchObject({ credentialId: "cred", clientDataJSON: expect.any(String) });
    await until(() => windows.removed.includes(windowId));
  });

  test("sign in: lists passkeys for the site, then signs the chosen one", async () => {
    const { outcome, id, windowId } = await open(GET);
    const sender = page(id, windowId);
    expect(await decisions.candidates(sender, id)).toHaveLength(1);
    expect(calls[0]).toMatchObject({ op: "passkeyCandidates", params: { origin: ORIGIN, rpId: "example.com" } });
    await decisions.answer(sender, id, { action: "assert", itemId: "k1" });
    expect(calls[1]).toMatchObject({ op: "passkeyAssert", params: { origin: ORIGIN, itemId: "k1" } });
    expect(await outcome).toMatchObject({ signature: "s" });
  });

  test("fill: the password goes to the asking page, never to the window", async () => {
    const { outcome, id, windowId } = await open({ decision: "chooseLogin" });
    const sender = page(id, windowId);
    expect(await decisions.logins(sender, id)).toEqual({ status: "ok", logins: [] });
    expect(calledDeps).toContain(`findLogins:${ORIGIN}`);
    const done = await decisions.answer(sender, id, { action: "fill", itemId: "i1" });
    expect(JSON.stringify(done)).not.toContain("s3cret");
    expect(calls[0]).toMatchObject({ op: "fillLogin", params: { itemId: "i1", origin: ORIGIN, force: false } });
    expect(calledDeps).toContain("rememberFill:7");
    expect(await outcome).toEqual({ fill: FILL });
  });

  test("save: saves the capture of this tab, shows the result, closes the window after a moment", async () => {
    const { outcome, id, windowId } = await open({ decision: "saveLogin", id: "p1" });
    const sender = page(id, windowId);
    expect((await decisions.info(sender, id)) as unknown).toMatchObject({ decision: "saveLogin", username: "alice" });
    const done = await decisions.answer(sender, id, { action: "save" });
    expect(done).toEqual({ message: "Saved to LocalPass." });
    expect(calledDeps).toContain("save:7:p1");
    expect(await outcome).toMatchObject({ status: "created" });
    expect(windows.removed).not.toContain(windowId); // still showing the result
    await until(() => windows.removed.includes(windowId), 3000, "the window to close");
  });

  test("save: the window learns whether the capture is a new login or a changed password (8.2)", async () => {
    const first = await open({ decision: "saveLogin", id: "p1" });
    expect((await decisions.info(page(first.id, first.windowId), first.id)) as unknown).toMatchObject({ decision: "saveLogin", update: false });
    pending = { id: "p2", origin: ORIGIN, username: "alice", update: true, expiresAt: Date.now() + 120_000 };
    const second = await open({ decision: "saveLogin", id: "p2" });
    const info = (await decisions.info(page(second.id, second.windowId), second.id)) as unknown as Record<string, unknown>;
    expect(info).toMatchObject({ decision: "saveLogin", username: "alice", update: true });
    expect(JSON.stringify(info)).not.toContain("password\":");
  });

  test("fallback hands the request back to the browser", async () => {
    const { outcome, id, windowId } = await open(CREATE);
    await decisions.answer(page(id, windowId), id, { action: "fallback" });
    await expect(outcome).rejects.toMatchObject({ code: "fallback" });
    expect(windows.removed).toContain(windowId);
  });

  test("a failed answer leaves the window open, and giving up then reports that failure", async () => {
    const { outcome, id, windowId } = await open(CREATE);
    const sender = page(id, windowId);
    failWith = new BrokerError("denied", "no");
    await expect(decisions.answer(sender, id, { action: "create" })).rejects.toMatchObject({ code: "denied" });
    expect(windows.removed).toEqual([]);
    failWith = null;
    await decisions.answer(sender, id, { action: "cancel" });
    await expect(outcome).rejects.toMatchObject({ code: "denied" });
  });

  test("a second answer while one is being carried out is refused", async () => {
    const { outcome, id, windowId } = await open(CREATE);
    const sender = page(id, windowId);
    let release!: () => void;
    gate = new Promise((resolve) => (release = resolve));
    const first = decisions.answer(sender, id, { action: "create" });
    await until(() => calls.length === 1);
    await expect(decisions.answer(sender, id, { action: "create" })).rejects.toMatchObject({ code: "busy" });
    release();
    await first;
    await outcome;
    expect(calls).toHaveLength(1);
  });
});

describe("a separate window is always held (8.3c)", () => {
  test("its page never hears that the user opened it, also when it attaches again", async () => {
    const { id, windowId } = await open(CREATE);
    for (let i = 0; i < 2; i++) {
      const port = Object.assign(new FakePort(), { name: `decide:${id}`, sender: page(id, windowId) });
      await decisions.attach(port as unknown as chrome.runtime.Port);
      expect(port.disconnected).toBe(false);
      expect(port.sent).toEqual([]);
      port.disconnected = true; // its end goes away without closing the decision
      (decisions as unknown as { byId: Map<string, { port?: unknown }> }).byId.get(id)!.port = undefined;
    }
  });
});

describe("giving up", () => {
  test("closing the window is a no for the page", async () => {
    const { outcome, windowId } = await open(CREATE);
    windows.close(windowId);
    await expect(outcome).rejects.toMatchObject({ code: "cancelled" });
  });

  test("closing a save window is 'Not now': the captured password is dropped", async () => {
    const { outcome, windowId } = await open({ decision: "saveLogin", id: "p1" });
    windows.close(windowId);
    await expect(outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(calledDeps).toContain("dismiss:7:p1:false");
  });

  test("'never for this site' is passed on", async () => {
    const { outcome, id, windowId } = await open({ decision: "saveLogin", id: "p1" });
    await decisions.answer(page(id, windowId), id, { action: "never" });
    await expect(outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(calledDeps).toContain("dismiss:7:p1:true");
  });

  test("the page withdrawing its request, or the tab closing, closes the window", async () => {
    const a = await open(CREATE, asker(), "r1");
    decisions.withdraw(asker(), "other-ref");
    expect(windows.removed).toEqual([]);
    decisions.withdraw(asker(), "r1");
    await expect(a.outcome).rejects.toMatchObject({ code: "cancelled" });
    const b = await open(GET, asker({ tabId: 9 }), "r2");
    decisions.onTabRemoved(9);
    await expect(b.outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(windows.removed).toEqual([a.windowId, b.windowId]);
  });

  test("a withdraw from another frame or tab does not touch it", async () => {
    const a = await open(CREATE, asker(), "r1");
    decisions.withdraw(asker({ frameId: 2 }), "r1");
    decisions.withdraw(asker({ tabId: 99 }), "r1");
    await sleep(10);
    expect(windows.removed).toEqual([]);
    decisions.withdraw(asker(), "r1");
    await expect(a.outcome).rejects.toMatchObject({ code: "cancelled" });
  });
});

describe("Firefox toolbar picker", () => {
  let tab: { id: number; windowId: number; active: boolean; url: string };
  /** Other tabs of the browser (tab 7 is `tab`). */
  let others: Map<number, { id: number; windowId: number; active: boolean; url: string }>;
  let focused: boolean;
  let popup: string;
  /** Per-tab popups of the other tabs, and every tab's badge. */
  let popups: Map<number, string>;
  let badges: Map<number, string>;
  let opened: number[];
  let failOpen: boolean;
  let popupGate: Promise<void> | undefined;
  const DEFAULT = () => chrome.runtime.getURL("popup.html");
  const absolute = (p: string | null) => (!p ? DEFAULT() : p.includes("://") ? p : chrome.runtime.getURL(p));

  beforeEach(() => {
    tab = { id: 7, windowId: 20, active: true, url: `${ORIGIN}/login` };
    others = new Map();
    focused = true;
    popup = DEFAULT();
    popups = new Map();
    badges = new Map();
    opened = [];
    failOpen = false;
    popupGate = undefined;
    const tabOf = (id: number) => (id === 7 ? tab : others.get(id));
    Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "159.0" }) });
    Object.assign(chrome.tabs, {
      get: async (id: number) => {
        const t = tabOf(id);
        if (!t) throw new Error("No tab with id");
        return { ...t };
      },
    });
    Object.assign(chrome.windows, { get: async (id: number) => ({ id, focused: id === 20 && focused }) });
    Object.assign(chrome, { action: {
      getPopup: async ({ tabId }: { tabId: number }) => (tabId === 7 ? popup : popups.get(tabId) ?? DEFAULT()),
      setPopup: async (options: { tabId: number; popup: string | null }) => {
        await popupGate;
        if (!tabOf(options.tabId)) throw new Error("No tab with id");
        if (options.tabId === 7) popup = absolute(options.popup);
        else popups.set(options.tabId, absolute(options.popup));
      },
      setBadgeText: async ({ tabId, text }: { tabId: number; text: string }) => {
        if (!tabOf(tabId)) throw new Error("No tab with id");
        badges.set(tabId, text);
      },
      getBadgeText: async ({ tabId }: { tabId: number }) => badges.get(tabId) ?? "",
      openPopup: async (options: { windowId: number }) => {
        if (failOpen) throw new Error("another panel is open");
        opened.push(options.windowId);
      },
    } });
  });

  async function panel(ref = "panel") {
    const outcome = decisions.open(asker(), ref, { decision: "chooseLogin" });
    outcome.catch(() => {});
    await until(() => popup.includes("presentation=panel") && opened.length > 0);
    const id = new URL(popup).searchParams.get("id")!;
    const sender = { id: EXT, url: popup };
    return { id, sender, outcome };
  }

  async function restored() {
    await until(() => popup === chrome.runtime.getURL("popup.html"));
    expect(windows.removed).toEqual([]); // Never remove the user's browser window.
  }

  test("opens in the original browser window and sends the credential only to the original request", async () => {
    const p = await panel();
    expect(opened).toEqual([20]);
    expect(windows.created).toEqual([]);
    expect(await decisions.info(p.sender, p.id)).toMatchObject({ origin: ORIGIN, decision: "chooseLogin" });
    expect(await decisions.logins(p.sender, p.id)).toMatchObject({ status: "ok" });
    const reply = await decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" });
    expect(JSON.stringify(reply)).not.toContain(FILL.password);
    expect(await p.outcome).toEqual({ fill: FILL });
    await restored();
  });

  test("rejects content scripts, extension tabs, frames, unrelated popup pages and altered decision URLs", async () => {
    const p = await panel();
    for (const sender of [
      { ...p.sender, id: "other" },
      { ...p.sender, tab: { id: 7, windowId: 20 } },
      { ...p.sender, frameId: 3 },
      { ...p.sender, url: ORIGIN },
      { ...p.sender, url: chrome.runtime.getURL("popup.html") },
      { ...p.sender, url: `${p.sender.url}&extra=yes` },
    ]) {
      await expect(decisions.answer(sender as chrome.runtime.MessageSender, p.id, { action: "fill", itemId: "i1" })).rejects.toMatchObject({ code: "forbidden" });
    }
    expect(calls).toEqual([]);
    await decisions.answer(p.sender, p.id, { action: "cancel" });
    await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
    await restored();
  });

  test("panel disconnect cancels and resets its tab's popup", async () => {
    const p = await panel();
    const port = Object.assign(new FakePort(), { name: `decide:${p.id}`, sender: p.sender });
    await decisions.attach(port as unknown as chrome.runtime.Port);
    port.serverClose();
    await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
    await restored();
    expect(calls).toEqual([]);
  });

  test("navigation, a tab switch, a browser window switch and tab closure cancel the picker", async () => {
    for (const cancel of [
      () => decisions.onTabUpdated(7, { status: "loading" }),
      () => decisions.onTabUpdated(7, { url: `${ORIGIN}/other` }),
      () => decisions.onTabActivated({ tabId: 8, windowId: 20 }),
      () => decisions.onFocusChanged(21),
      () => decisions.onTabRemoved(7),
    ]) {
      const p = await panel();
      cancel();
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
      await restored();
      await expect(decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" })).rejects.toMatchObject({ code: "notFound" });
    }
    expect(calls).toEqual([]);
  });

  test("rechecks active tab, focused window and origin even without a browser event", async () => {
    const p = await panel();
    tab.active = false;
    await expect(decisions.logins(p.sender, p.id)).rejects.toMatchObject({ code: "cancelled" });
    tab.active = true;
    focused = false;
    await expect(decisions.logins(p.sender, p.id)).rejects.toMatchObject({ code: "cancelled" });
    focused = true;
    tab.url = "https://other.example/";
    await expect(decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" })).rejects.toMatchObject({ code: "cancelled" });
    expect(calls).toEqual([]);
    decisions.onTabRemoved(7);
    await restored();
  });

  test("a delayed credential response cannot fill after the panel is dismissed", async () => {
    const p = await panel();
    let release!: () => void;
    gate = new Promise<void>((resolve) => { release = resolve; });
    const answer = decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" });
    await until(() => calls.length === 1);
    decisions.onTabActivated({ tabId: 8, windowId: 20 });
    await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
    release();
    await expect(answer).rejects.toMatchObject({ code: "cancelled" });
    expect(calledDeps).not.toContain("rememberFill:7");
    await restored();
  });

  test("failed opening restores the toolbar without creating a separate window", async () => {
    failOpen = true;
    await expect(decisions.open(asker(), "r", { decision: "chooseLogin" })).rejects.toMatchObject({ code: "error" });
    await restored();
    expect(windows.created).toEqual([]);
  });

  test("cancelling while the popup URL is being set restores it and never opens a panel", async () => {
    let release!: () => void;
    popupGate = new Promise<void>(resolve => { release = resolve; });
    const outcome = decisions.open(asker(), "r", { decision: "chooseLogin" });
    outcome.catch(() => {});
    await sleep(10);
    decisions.onTabUpdated(7, { status: "loading" });
    release();
    await expect(outcome).rejects.toMatchObject({ code: "cancelled" });
    await restored();
    expect(opened).toEqual([]);
  });

  test("replacing a request cannot reset the successor's popup", async () => {
    const first = await panel();
    const secondOutcome = decisions.open(asker(), "new", { decision: "chooseLogin" });
    secondOutcome.catch(() => {});
    await expect(first.outcome).rejects.toMatchObject({ code: "cancelled" });
    await until(() => opened.length === 2);
    const id = new URL(popup).searchParams.get("id")!;
    expect(id).not.toBe(first.id);
    await sleep(10);
    expect(new URL(popup).searchParams.get("id")).toBe(id);
    await expect(decisions.info(first.sender, first.id)).rejects.toMatchObject({ code: "notFound" });
    await decisions.answer({ id: EXT, url: popup }, id, { action: "cancel" });
    await expect(secondOutcome).rejects.toMatchObject({ code: "cancelled" });
    await restored();
  });

  test("Firefox below 149 keeps the separate picker window", async () => {
    Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "140.0" }) });
    const p = await open({ decision: "chooseLogin" });
    expect(opened).toEqual([]);
    await decisions.answer(page(p.id, p.windowId), p.id, { action: "cancel" });
    await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
  });

  // ---- 8.3a: every decision in the panel ----

  const SAVE: DecisionRequest = { decision: "saveLogin", id: "p1" };
  const CHOOSE: DecisionRequest = { decision: "chooseLogin" };
  const ANSWER: Record<string, { action: "create" } | { action: "assert"; itemId: string } | { action: "save" } | { action: "fill"; itemId: string }> = {
    passkeyCreate: { action: "create" },
    passkeyGet: { action: "assert", itemId: "k1" },
    saveLogin: { action: "save" },
    chooseLogin: { action: "fill", itemId: "i1" },
  };
  const panelUrl = (id: string) => chrome.runtime.getURL(`decide.html?id=${id}&presentation=panel`);
  const dismissed = () => calledDeps.filter((c) => c.startsWith("dismiss"));
  /** Whether a promise has settled (read it after a tick). */
  function watch(p: Promise<unknown>) {
    const s = { done: false };
    p.then(
      () => (s.done = true),
      () => (s.done = true),
    );
    return s;
  }

  /** Opens a decision that shows in the panel by itself. */
  async function shown(request: DecisionRequest, ref = "r1", who = asker()) {
    const before = opened.length;
    const outcome = decisions.open(who, ref, request);
    outcome.catch(() => {});
    await until(() => opened.length > before, 1000, "the panel");
    const id = new URL(popup).searchParams.get("id")!;
    return { outcome, id, sender: { id: EXT, url: panelUrl(id) } as chrome.runtime.MessageSender, state: watch(outcome) };
  }

  /** The panel's page loads (the panel opened by itself, or the user clicked the toolbar icon) and connects. */
  async function attachPanel(id: string) {
    const port = Object.assign(new FakePort(), { name: `decide:${id}`, sender: { id: EXT, url: panelUrl(id) } });
    await decisions.attach(port as unknown as chrome.runtime.Port);
    return port;
  }

  async function normal() {
    await restored();
    await until(() => (badges.get(7) ?? "") === "", 1000, "the badge to go");
  }

  describe("every decision in the panel (8.3a)", () => {
    let release: () => void;
    beforeEach(() => {
      decisionTiming.lifeMs = 120_000;
      decisionTiming.reopenMs = 60_000;
      decisionTiming.refocusMs = 60_000;
      decisionTiming.pollMs = 5;
      others.set(8, { id: 8, windowId: 20, active: false, url: "https://other.example/" });
    });
    const hold = () => (gate = new Promise<void>((resolve) => (release = resolve)));

    test("save: opens by itself (no window), with a badge, and shows and saves the site of the capture, not the tab's", async () => {
      tab.url = "https://www.example.com/dashboard"; // the page the sign-in redirected to
      const p = await shown(SAVE, "r1", asker({ origin: "https://www.example.com" }));
      expect(windows.created).toEqual([]);
      expect(opened).toEqual([20]);
      expect(badges.get(7)).toBe("1");
      expect(calledDeps).toContain("offered:7:p1");
      expect(await decisions.info(p.sender, p.id)).toMatchObject({ decision: "saveLogin", origin: ORIGIN, host: "login.example.com", username: "alice" });
      expect(await decisions.answer(p.sender, p.id, { action: "save" })).toEqual({ message: "Saved to LocalPass." });
      expect(calledDeps).toContain("save:7:p1");
      expect(await p.outcome).toMatchObject({ status: "created" });
      await normal();
    });

    test("save: only the site check is relaxed; the tab must be the same one, active in the focused window", async () => {
      const p = await shown(SAVE);
      tab.url = "https://elsewhere.example/after-redirect";
      expect((await decisions.info(p.sender, p.id)).decision).toBe("saveLogin");
      tab.active = false;
      await expect(decisions.answer(p.sender, p.id, { action: "save" })).rejects.toMatchObject({ code: "cancelled" });
      tab.active = true;
      focused = false;
      await expect(decisions.answer(p.sender, p.id, { action: "save" })).rejects.toMatchObject({ code: "cancelled" });
      focused = true;
      tab.windowId = 21;
      await expect(decisions.answer(p.sender, p.id, { action: "save" })).rejects.toMatchObject({ code: "cancelled" });
      expect(calledDeps.filter((c) => c.startsWith("save:"))).toEqual([]);
      // Everything else still needs the same site.
      tab.windowId = 20;
      tab.url = `${ORIGIN}/login`;
      const k = await shown(CREATE, "r2");
      tab.url = "https://elsewhere.example/";
      await expect(decisions.info(k.sender, k.id)).rejects.toMatchObject({ code: "cancelled" });
      await expect(decisions.answer(k.sender, k.id, { action: "create" })).rejects.toMatchObject({ code: "cancelled" });
      expect(calls).toEqual([]);
    });

    test("closing the save panel is not Not now: the offer waits with the badge and the toolbar icon shows it again", async () => {
      const p = await shown(SAVE);
      const port = await attachPanel(p.id);
      port.serverClose();
      await sleep(20);
      expect(p.state.done).toBe(false);
      expect(dismissed()).toEqual([]);
      expect(popup).toBe(panelUrl(p.id));
      expect(badges.get(7)).toBe("1");
      expect(opened).toHaveLength(1); // never reopened by itself
      const again = await attachPanel(p.id);
      expect(again.disconnected).toBe(false);
      again.serverClose();
      await sleep(10);
      expect(opened).toHaveLength(1);
      await attachPanel(p.id);
      await decisions.answer(p.sender, p.id, { action: "save" });
      expect(await p.outcome).toMatchObject({ status: "created" });
      await normal();
    });

    test("Not now (Esc sends the same answer) drops the capture; Never too; both give the tab its normal popup back", async () => {
      let p = await shown(SAVE);
      await attachPanel(p.id);
      await decisions.answer(p.sender, p.id, { action: "cancel" });
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
      expect(dismissed()).toEqual(["dismiss:7:p1:false"]);
      await normal();
      pending = { id: "p1", origin: ORIGIN, username: "alice", expiresAt: Date.now() + 120_000 }; // a new capture
      p = await shown(SAVE, "r2");
      await decisions.answer(p.sender, p.id, { action: "never" });
      expect(dismissed()).toContain("dismiss:7:p1:true");
      await normal();
    });

    test("a second page never attaches to a decision that has one; a closed panel can attach again", async () => {
      for (const request of [SAVE, CREATE, CHOOSE]) {
        const p = await shown(request, `r-${request.decision}`);
        const first = await attachPanel(p.id);
        const second = await attachPanel(p.id);
        expect(second.disconnected, request.decision).toBe(true);
        expect(first.disconnected, request.decision).toBe(false);
        if (request.decision !== "chooseLogin") {
          first.serverClose();
          const third = await attachPanel(p.id);
          expect(third.disconnected, request.decision).toBe(false);
        }
        decisions.onTabRemoved(7);
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        await normal();
      }
    });

    test("a save survives navigations and a tab switch, and its popup and badge are set again after the browser reset them", async () => {
      const p = await shown(SAVE);
      const port = await attachPanel(p.id);
      for (const change of [{ status: "loading" }, { url: "https://www.example.com/home" }, { status: "complete" }]) {
        popup = DEFAULT(); // Firefox resets a tab's popup and badge when it loads a new page
        badges.delete(7);
        decisions.onTabUpdated(7, change);
        await until(() => popup === panelUrl(p.id) && badges.get(7) === "1", 1000, JSON.stringify(change));
      }
      decisions.onTabActivated({ tabId: 8, windowId: 20 });
      await sleep(10);
      expect(port.disconnected).toBe(true); // hidden, and waiting
      expect(p.state.done).toBe(false);
      expect(dismissed()).toEqual([]);
      decisions.onTabRemoved(7);
      await normal();
    });

    test("a later page of the tab takes the save over: no second panel, and the newer request gets the outcome", async () => {
      const p = await shown(SAVE, "r1");
      const later = decisions.open(asker({ origin: "https://www.example.com" }), "r2", SAVE);
      later.catch(() => {});
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
      await sleep(20);
      expect(opened).toHaveLength(1);
      expect(dismissed()).toEqual([]); // the capture is kept
      expect(await decisions.ping(asker(), "r2")).toBe(true);
      expect(await decisions.ping(asker(), "r1")).toBe(false);
      expect(await decisions.ping(asker({ tabId: 8 }), "r2")).toBe(false);
      expect(await decisions.ping(asker({ frameId: 2 }), "r2")).toBe(false);
      await decisions.answer(p.sender, p.id, { action: "save" });
      expect(await later).toMatchObject({ status: "created" });
      expect(await decisions.ping(asker(), "r2")).toBe(false);
    });

    test("a capture offered before whose decision was forgotten waits under its old id and does not open by itself", async () => {
      pending = { ...pending!, offered: true, decisionId: "0b5c0d6e-1111-4222-8333-444455556666" };
      const outcome = decisions.open(asker(), "r", SAVE);
      outcome.catch(() => {});
      await until(() => popup === panelUrl("0b5c0d6e-1111-4222-8333-444455556666"));
      await sleep(20);
      expect(opened).toEqual([]);
      expect(badges.get(7)).toBe("1");
      expect(calledDeps).not.toContain("offered:7:p1");
    });

    test("after a restart: tabs of forgotten decisions get their popup back, an offered save waits again under its id", async () => {
      const stale = panelUrl("gone");
      popups.set(8, stale);
      badges.set(8, "1");
      popup = panelUrl("keep");
      await chrome.storage.session.set({ "decidePanel:8": stale, "decidePanel:7": popup });
      await decisions.restore(async () => [{ tabId: 7, pending: { ...pending!, offered: true, decisionId: "keep" } }]);
      expect(popups.get(8)).toBe(DEFAULT());
      expect(badges.get(8)).toBe("");
      expect((await chrome.storage.session.get("decidePanel:8"))["decidePanel:8"]).toBeUndefined();
      await until(() => popup === panelUrl("keep") && badges.get(7) === "1");
      expect(opened).toEqual([]);
      const sender = { id: EXT, url: panelUrl("keep") } as chrome.runtime.MessageSender;
      expect(await decisions.info(sender, "keep")).toMatchObject({ decision: "saveLogin", username: "alice" });
      await decisions.answer(sender, "keep", { action: "save" });
      expect(calledDeps).toContain("save:7:p1");
      await normal();
    });

    test("closing a passkey panel leaves the request waiting; focus and tab changes too; shown again it completes", async () => {
      const p = await shown(GET);
      const port = await attachPanel(p.id);
      port.serverClose();
      decisions.onFocusChanged(-1);
      decisions.onFocusChanged(21);
      decisions.onTabActivated({ tabId: 8, windowId: 20 });
      await sleep(20);
      expect(p.state.done).toBe(false);
      expect(badges.get(7)).toBe("1");
      expect(popup).toBe(panelUrl(p.id));
      expect(opened).toHaveLength(1);
      await attachPanel(p.id);
      expect(await decisions.candidates(p.sender, p.id)).toHaveLength(1);
      await decisions.answer(p.sender, p.id, { action: "assert", itemId: "k1" });
      expect(await p.outcome).toMatchObject({ signature: "s" });
      await normal();
    });

    test("a waiting passkey request ends as a cancel does today: Cancel, another method, withdrawn, navigation, tab closed", async () => {
      const ends: [string, (p: { id: string; sender: chrome.runtime.MessageSender }) => unknown, string][] = [
        ["Cancel", (p) => decisions.answer(p.sender, p.id, { action: "cancel" }), "cancelled"],
        ["Use another method", (p) => decisions.answer(p.sender, p.id, { action: "fallback" }), "fallback"],
        ["the page withdrew it", () => decisions.withdraw(asker(), "r1"), "cancelled"],
        ["a new page", () => decisions.onTabUpdated(7, { status: "loading" }), "cancelled"],
        ["another address", () => decisions.onTabUpdated(7, { url: `${ORIGIN}/next` }), "cancelled"],
        ["the tab closed", () => decisions.onTabRemoved(7), "cancelled"],
      ];
      for (const [why, end, code] of ends) {
        const p = await shown(CREATE);
        (await attachPanel(p.id)).serverClose();
        await sleep(5);
        expect(p.state.done, why).toBe(false);
        await end(p);
        await expect(p.outcome, why).rejects.toMatchObject({ code });
        await normal();
      }
      expect(calls).toEqual([]);
    });

    test("a waiting request ends after its life (a save with its capture's), but not while a panel shows it", async () => {
      decisionTiming.lifeMs = 60;
      const p = await shown(GET);
      const port = await attachPanel(p.id);
      await sleep(120);
      expect(p.state.done).toBe(false);
      port.serverClose();
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled", message: "The request timed out." });
      await normal();
      // Not shown at all: it ends at its time.
      const q = await shown(CREATE, "r2");
      await expect(q.outcome).rejects.toMatchObject({ code: "cancelled" });
      await normal();
      pending = { ...pending!, expiresAt: Date.now() + 60 };
      const s = await shown(SAVE, "r3");
      await expect(s.outcome).rejects.toMatchObject({ code: "cancelled" });
      expect(dismissed()).toContain("dismiss:7:p1:false");
      await normal();
    });

    test("the panel cannot be opened: a save or passkey request waits with the badge; a login pick ends with the error", async () => {
      failOpen = true;
      for (const request of [SAVE, CREATE]) {
        const outcome = decisions.open(asker(), `r-${request.decision}`, request);
        const state = watch(outcome);
        outcome.catch(() => {});
        await until(() => popup.includes("presentation=panel") && badges.get(7) === "1");
        await sleep(20);
        expect(state.done, request.decision).toBe(false);
        const id = new URL(popup).searchParams.get("id")!;
        expect((await attachPanel(id)).disconnected).toBe(false); // the toolbar icon still shows it
        decisions.onTabRemoved(7);
        await normal();
      }
      await expect(decisions.open(asker(), "r", CHOOSE)).rejects.toMatchObject({ code: "error" });
      await normal();
    });

    test("a request from a tab that is not in front waits with the badge on that tab and does not open", async () => {
      focused = false;
      const outcome = decisions.open(asker(), "r", CREATE);
      const state = watch(outcome);
      outcome.catch(() => {});
      await until(() => badges.get(7) === "1");
      await sleep(20);
      expect(opened).toEqual([]);
      expect(state.done).toBe(false);
      focused = true;
      const id = new URL(popup).searchParams.get("id")!;
      await attachPanel(id);
      await decisions.answer({ id: EXT, url: panelUrl(id) }, id, { action: "create" });
      expect(await outcome).toMatchObject({ credentialId: "cred" });
    });

    test("work in progress is never cancelled by the panel closing or the focus moving to the app", async () => {
      for (const request of [CREATE, GET, SAVE, CHOOSE]) {
        const p = await shown(request, `r-${request.decision}`);
        const port = await attachPanel(p.id);
        hold();
        const answered = decisions.answer(p.sender, p.id, ANSWER[request.decision]!);
        answered.catch(() => {});
        await until(() => calls.length > 0 || calledDeps.some((c) => c.startsWith("save:")));
        port.serverClose(); // the app's prompt took the focus and the panel closed
        decisions.onFocusChanged(-1);
        decisions.onFocusChanged(21);
        await sleep(20);
        expect(p.state.done, request.decision).toBe(false);
        release();
        await p.outcome;
        await normal();
        gate = null;
        calls = [];
        calledDeps = [];
      }
    });

    test("an answer that fails while the panel is closed: a login pick ends with it; a passkey or save waits and shows it", async () => {
      for (const request of [CHOOSE, CREATE, SAVE]) {
        const p = await shown(request, `r-${request.decision}`);
        const port = await attachPanel(p.id);
        hold();
        failWith = new BrokerError("internal", "the app went away");
        const answered = decisions.answer(p.sender, p.id, ANSWER[request.decision]!);
        answered.catch(() => {});
        await until(() => calls.length > 0 || calledDeps.some((c) => c.startsWith("save:")));
        port.serverClose();
        release();
        await expect(answered).rejects.toMatchObject({ code: "internal" });
        failWith = null;
        gate = null;
        if (request.decision === "chooseLogin") {
          await expect(p.outcome).rejects.toMatchObject({ code: "internal" });
        } else {
          await sleep(10);
          expect(p.state.done, request.decision).toBe(false);
          expect(badges.get(7)).toBe("1");
          await attachPanel(p.id);
          expect(await decisions.info(p.sender, p.id)).toMatchObject({ failed: "the app went away" });
          await decisions.answer(p.sender, p.id, { action: "cancel" });
          // Giving up reports what went wrong, as before.
          await expect(p.outcome).rejects.toMatchObject({ code: "internal" });
        }
        await normal();
        calls = [];
        calledDeps = [];
      }
    });

    test("while shown again during the work, the panel learns that LocalPass is working", async () => {
      const p = await shown(CREATE);
      (await attachPanel(p.id)).serverClose();
      await sleep(5);
      await attachPanel(p.id);
      hold();
      const answered = decisions.answer(p.sender, p.id, { action: "create" });
      await until(() => calls.length > 0);
      expect(await decisions.info(p.sender, p.id)).toMatchObject({ working: true });
      release();
      await answered;
    });

    test("a fetched login waits until its tab is in front again, and is dropped when the site changed or time ran out", async () => {
      let p = await shown(CHOOSE);
      let port = await attachPanel(p.id);
      hold();
      void decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" }).catch(() => {});
      await until(() => calls.length === 1);
      focused = false; // the app's Hello or Unlock has the focus
      port.serverClose();
      release();
      await sleep(40);
      expect(p.state.done).toBe(false);
      expect(calledDeps).not.toContain("rememberFill:7");
      focused = true;
      expect(await p.outcome).toEqual({ fill: FILL });
      await normal();

      for (const [why, change] of [["the site changed", () => (tab.url = "https://other.example/")], ["time ran out", () => (decisionTiming.refocusMs = 30)]] as const) {
        calls = [];
        calledDeps = [];
        p = await shown(CHOOSE, why);
        port = await attachPanel(p.id);
        hold();
        void decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" }).catch(() => {});
        await until(() => calls.length === 1);
        focused = false;
        port.serverClose();
        change();
        release();
        await expect(p.outcome, why).rejects.toMatchObject({ code: "cancelled" });
        expect(calledDeps, why).not.toContain("rememberFill:7");
        focused = true;
        tab.url = `${ORIGIN}/login`;
        await normal();
      }
    });

    test("Unlock from the picker: nothing is cancelled while the app's Unlock is on screen; then the panel reopens by itself", async () => {
      const p = await shown(CHOOSE);
      const port = await attachPanel(p.id);
      hold();
      const unlocked = decisions.unlock(p.sender, p.id);
      const joined = decisions.unlock(p.sender, p.id); // a second Unlock click joins the first
      await until(() => calls.length > 0);
      port.serverClose(); // the app's Unlock took the focus
      focused = false;
      decisions.onFocusChanged(-1);
      decisions.onTabActivated({ tabId: 8, windowId: 20 });
      await sleep(20);
      expect(p.state.done).toBe(false);
      release();
      expect(await unlocked).toBe(true);
      expect(await joined).toBe(true);
      expect(calls.filter((c) => c.op === "unlock")).toHaveLength(1);
      await sleep(20);
      expect(opened).toHaveLength(1); // the browser has no focus yet
      decisions.onFocusChanged(-1); // the app is still in front
      await sleep(10);
      expect(p.state.done).toBe(false);
      focused = true;
      decisions.onFocusChanged(20);
      await until(() => opened.length === 2, 1000, "the panel to reopen");
      // Once only: more focus events do not open it again.
      decisions.onFocusChanged(20);
      await sleep(20);
      expect(opened).toHaveLength(2);
      await attachPanel(p.id);
      expect(await decisions.logins(p.sender, p.id)).toMatchObject({ status: "ok" });
      await decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" });
      expect(await p.outcome).toEqual({ fill: FILL });
      await normal();
    });

    test("the panel reopens after Unlock only for the same active tab in the focused window (and site, but for a save)", async () => {
      // A login pick whose tab is no longer the active one is cancelled.
      let p = await shown(CHOOSE);
      let port = await attachPanel(p.id);
      hold();
      let unlocked = decisions.unlock(p.sender, p.id);
      await until(() => calls.length > 0);
      port.serverClose();
      tab.active = false;
      release();
      await unlocked;
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
      expect(opened).toHaveLength(1);
      await normal();
      tab.active = true;

      // A passkey request on another site by now: not reopened (it waits).
      calls = [];
      p = await shown(CREATE, "r2");
      port = await attachPanel(p.id);
      hold();
      unlocked = decisions.unlock(p.sender, p.id);
      await until(() => calls.length > 0);
      port.serverClose();
      focused = false;
      release();
      await unlocked;
      tab.url = "https://other.example/";
      focused = true;
      decisions.onFocusChanged(20);
      await sleep(30);
      expect(opened).toHaveLength(2);
      expect(p.state.done).toBe(false);
      decisions.onTabRemoved(7);
      await normal();
      tab.url = `${ORIGIN}/login`;

      // A save: another browser window has the focus: not reopened there; back in the tab's window it is.
      calls = [];
      p = await shown(SAVE, "r3");
      port = await attachPanel(p.id);
      hold();
      unlocked = decisions.unlock(p.sender, p.id);
      await until(() => calls.length > 0);
      port.serverClose();
      focused = false;
      release();
      await unlocked;
      decisions.onFocusChanged(21);
      await sleep(30);
      expect(opened).toHaveLength(3);
      tab.url = "https://www.example.com/after-redirect"; // a save is not tied to the tab's site
      focused = true;
      decisions.onFocusChanged(20);
      await until(() => opened.length === 4, 1000, "the save panel to reopen");
    });

    test("the reopening has a time limit: then a login pick ends and a save only waits", async () => {
      decisionTiming.reopenMs = 30;
      for (const request of [CHOOSE, SAVE]) {
        const p = await shown(request, `r-${request.decision}`);
        const port = await attachPanel(p.id);
        hold();
        const unlocked = decisions.unlock(p.sender, p.id);
        await until(() => calls.length > 0);
        port.serverClose();
        focused = false;
        release();
        await unlocked;
        await sleep(60);
        focused = true;
        decisions.onFocusChanged(20);
        await sleep(20);
        expect(opened, request.decision).toHaveLength(1);
        if (request.decision === "chooseLogin") await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        else expect(p.state.done).toBe(false);
        decisions.onTabRemoved(7);
        await normal();
        opened = [];
        calls = [];
        gate = null;
      }
    });

    test("a refused Unlock: a login pick is cancelled, a save or passkey request waits with the badge", async () => {
      for (const request of [CHOOSE, SAVE, GET]) {
        const p = await shown(request, `r-${request.decision}`);
        const port = await attachPanel(p.id);
        hold();
        failWith = new BrokerError("locked", "The vault is locked");
        const unlocked = decisions.unlock(p.sender, p.id);
        await until(() => calls.length > 0);
        port.serverClose();
        release();
        expect(await unlocked).toBe(false);
        failWith = null;
        gate = null;
        await sleep(20);
        if (request.decision === "chooseLogin") await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        else {
          expect(p.state.done, request.decision).toBe(false);
          expect(badges.get(7)).toBe("1");
          expect(opened).toHaveLength(1);
          decisions.onTabRemoved(7);
        }
        await normal();
        opened = [];
        calls = [];
      }
    });

    test("an open panel follows its own Unlock: nothing reopens and nothing is cancelled", async () => {
      const p = await shown(CHOOSE);
      await attachPanel(p.id);
      expect(await decisions.unlock(p.sender, p.id)).toBe(true);
      await sleep(20);
      expect(opened).toHaveLength(1);
      expect(p.state.done).toBe(false);
    });

    test("only the panel page of the decision may use a save or passkey decision", async () => {
      for (const request of [SAVE, CREATE]) {
        const p = await shown(request, `r-${request.decision}`);
        for (const sender of [
          { ...p.sender, id: "other" },
          { ...p.sender, tab: { id: 7, windowId: 20 } },
          { ...p.sender, frameId: 0 },
          { ...p.sender, url: ORIGIN },
          { ...p.sender, url: DEFAULT() },
          { ...p.sender, url: `${p.sender.url}&x=1` },
          { ...p.sender, url: chrome.runtime.getURL(`decide.html?id=${p.id}`) },
        ]) {
          await expect(decisions.answer(sender as chrome.runtime.MessageSender, p.id, ANSWER[request.decision]!)).rejects.toMatchObject({ code: "forbidden" });
          await expect(decisions.unlock(sender as chrome.runtime.MessageSender, p.id)).rejects.toMatchObject({ code: "forbidden" });
          const port = Object.assign(new FakePort(), { name: `decide:${p.id}`, sender });
          await decisions.attach(port as unknown as chrome.runtime.Port);
          expect(port.disconnected).toBe(true);
        }
        expect(calls).toEqual([]);
        expect(calledDeps.filter((c) => c.startsWith("save:"))).toEqual([]);
        decisions.onTabRemoved(7);
        await normal();
      }
    });

    test("one decision per tab, eight in all, waiting ones included", async () => {
      for (let id = 1; id <= 8; id++) others.set(id + 100, { id: id + 100, windowId: 20, active: false, url: `${ORIGIN}/` });
      const first = decisions.open(asker({ tabId: 101 }), "a", CREATE);
      first.catch(() => {});
      for (let id = 2; id <= 8; id++) void decisions.open(asker({ tabId: id + 100 }), "a", GET).catch(() => {});
      await until(() => [...badges.values()].filter((b) => b === "1").length === 8);
      await expect(decisions.open(asker(), "b", GET)).rejects.toMatchObject({ code: "tooMany" });
      // A newer request from a tab replaces its waiting one (its badge stays, for the new one).
      void decisions.open(asker({ tabId: 101 }), "c", GET).catch(() => {});
      await expect(first).rejects.toMatchObject({ code: "cancelled", message: "Replaced by a newer request." });
      await sleep(20);
      expect(badges.get(101)).toBe("1");
      expect(new URL(popups.get(101)!).searchParams.get("id")).not.toBeNull();
    });
  });

  describe("a panel the user opened skips the hold (8.3c)", () => {
    beforeEach(() => {
      decisionTiming.lifeMs = 120_000;
      decisionTiming.reopenMs = 60_000;
      decisionTiming.refocusMs = 60_000;
    });
    const USER = { userOpened: true };
    const notes = (port: FakePort) => port.sent;

    test("the background's own opening is held; the same request shown again from the toolbar icon (or the shortcut) is not", async () => {
      for (const request of [SAVE, CREATE, GET]) {
        const p = await shown(request, `r-${request.decision}`);
        const own = await attachPanel(p.id);
        expect(notes(own), request.decision).toEqual([]);
        own.serverClose();
        await sleep(10);
        const user = await attachPanel(p.id); // nothing called openPopup: the user did
        expect(notes(user), request.decision).toEqual([USER]);
        user.serverClose();
        await sleep(10);
        expect(notes(await attachPanel(p.id)), request.decision).toEqual([USER]);
        decisions.onTabRemoved(7);
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        await normal();
      }
    });

    test("a panel that attaches before openPopup returns (measured in Chrome) is still the background's: held", async () => {
      let attaching: Promise<FakePort> | undefined;
      const real = chrome.action.openPopup;
      chrome.action.openPopup = (async (options: { windowId: number }) => {
        attaching = attachPanel(new URL(popup).searchParams.get("id")!); // the page connects while the call runs
        await sleep(5);
        opened.push(options.windowId);
      }) as typeof chrome.action.openPopup;
      try {
        await shown(CREATE);
        const early = await attaching!;
        expect(early.disconnected).toBe(false);
        expect(notes(early)).toEqual([]);
      } finally {
        chrome.action.openPopup = real;
      }
    });

    test("the reopen after Unlock is the background's too, also when its page attaches while openPopup still runs: held", async () => {
      const p = await shown(SAVE);
      const port = await attachPanel(p.id);
      expect(notes(port)).toEqual([]);
      let release!: () => void;
      gate = new Promise<void>((resolve) => (release = resolve));
      const unlocked = decisions.unlock(p.sender, p.id);
      await until(() => calls.length > 0);
      port.serverClose(); // the app's Unlock took the focus
      focused = false;
      decisions.onFocusChanged(-1);
      release();
      expect(await unlocked).toBe(true);
      let attaching: Promise<FakePort> | undefined;
      const real = chrome.action.openPopup;
      chrome.action.openPopup = (async (options: { windowId: number }) => {
        attaching = attachPanel(p.id); // the page connects while the call runs (measured in Chrome)
        await sleep(5);
        opened.push(options.windowId);
      }) as typeof chrome.action.openPopup;
      try {
        focused = true;
        decisions.onFocusChanged(20);
        await until(() => opened.length === 2, 1000, "the panel to reopen");
        const again = await attaching!;
        expect(again.disconnected).toBe(false);
        expect(notes(again)).toEqual([]);
      } finally {
        chrome.action.openPopup = real;
      }
    });

    test("a pick (the badge's panel) is always the background's: held", async () => {
      const p = await shown(CHOOSE);
      expect(notes(await attachPanel(p.id))).toEqual([]);
    });

    test("an opening that failed keeps its count (the next attach in that window is held once); so does one whose panel never attached", async () => {
      failOpen = true;
      const p = await shown(SAVE).catch(() => undefined);
      expect(p).toBeUndefined(); // `shown` waits for an opening that never came
      const id = new URL(popup).searchParams.get("id")!;
      failOpen = false;
      // Nothing says a failed `openPopup` never showed anything: the next attach is held once, then the user's are not.
      const first = await attachPanel(id);
      expect(notes(first)).toEqual([]);
      first.serverClose();
      await sleep(10);
      expect(notes(await attachPanel(id))).toEqual([USER]);
      decisions.onTabRemoved(7);
      await normal();
      pending = { id: "p1", origin: ORIGIN, username: "alice", expiresAt: Date.now() + 120_000 };
      const q = await shown(SAVE, "r2"); // opened, but its page never connected (closed at once)
      const late = await attachPanel(q.id); // so the next attach is held: it may be that panel, late
      expect(notes(late)).toEqual([]);
      late.serverClose();
      await sleep(10);
      expect(notes(await attachPanel(q.id))).toEqual([USER]);
    });

    test("an attach that is refused does not use up the background's opening", async () => {
      const p = await shown(CREATE);
      for (const sender of [
        { id: EXT, url: panelUrl(p.id), tab: { id: 7, windowId: 20 } }, // an extension page in a tab
        { id: EXT, url: panelUrl(p.id), frameId: 0 },
        { id: EXT, url: ORIGIN }, // a content script
        { id: "other", url: panelUrl(p.id) },
      ]) {
        const port = Object.assign(new FakePort(), { name: `decide:${p.id}`, sender });
        await decisions.attach(port as unknown as chrome.runtime.Port);
        expect(port.disconnected).toBe(true);
        expect(port.sent).toEqual([]);
      }
      expect(notes(await attachPanel(p.id))).toEqual([]); // still held
    });

    test("nothing a content script sends makes a panel skip the hold", async () => {
      // Extra fields are dropped from the request, and a page's own requests open the panel through the background.
      const p = await shown({ ...SAVE, userOpened: true, presentation: "user" } as unknown as DecisionRequest);
      expect(notes(await attachPanel(p.id))).toEqual([]);
      decisions.onTabRemoved(7);
      await normal();
      // A newer request from the page (it chooses when) opens a new panel: held again.
      const a = await shown(CREATE, "a");
      const port = await attachPanel(a.id);
      port.serverClose();
      await sleep(10);
      const b = await shown(GET, "b");
      expect(notes(await attachPanel(b.id))).toEqual([]);
      // Pings and a withdraw change nothing about it.
      expect(await decisions.ping(asker(), "b")).toBe(true);
    });

    test("8.6 F2: a request that could not open by itself is held on its first showing, also from the toolbar icon", async () => {
      const counts = (decisions as unknown as { ownOpens: Map<number, number> }).ownOpens;
      for (const request of [SAVE, CREATE, GET]) {
        // The tab was not in front: no own opening, the request waits with the badge for the user's click.
        focused = false;
        const outcome = decisions.open(asker(), `r-${request.decision}`, request);
        outcome.catch(() => {});
        await until(() => popup.includes("presentation=panel") && badges.get(7) === "1");
        await sleep(20);
        expect(opened, request.decision).toEqual([]);
        expect(counts.get(20), request.decision).toBeUndefined();
        focused = true;
        const id = new URL(popup).searchParams.get("id")!;
        // The user's click on the icon (or the shortcut) shows the page's request for the first time: held.
        const first = await attachPanel(id);
        expect(notes(first), request.decision).toEqual([]);
        expect(counts.get(20), request.decision).toBeUndefined(); // nothing taken, nothing left behind
        first.serverClose();
        await sleep(10);
        // The user has seen it: shown again from the badge, it acts at once.
        expect(notes(await attachPanel(id)), request.decision).toEqual([USER]);
        expect(counts.get(20), request.decision).toBeUndefined();
        decisions.onTabRemoved(7);
        await normal();
      }
    });

    test("8.6 F2: a first showing the user opened takes the background's waiting opening in that window, as before", async () => {
      const counts = (decisions as unknown as { ownOpens: Map<number, number> }).ownOpens;
      focused = false;
      const outcome = decisions.open(asker(), "r", CREATE);
      outcome.catch(() => {});
      await until(() => badges.get(7) === "1");
      focused = true;
      counts.set(20, 1); // an earlier own opening in this window whose panel never attached
      const id = new URL(popup).searchParams.get("id")!;
      expect(notes(await attachPanel(id))).toEqual([]);
      expect(counts.get(20)).toBeUndefined();
    });

    test("8.6 F2: a newer request from the page is a new decision: its first showing is held again", async () => {
      const a = await shown(CREATE, "a");
      const own = await attachPanel(a.id);
      own.serverClose();
      await sleep(10);
      expect(notes(await attachPanel(a.id))).toEqual([USER]);
      focused = false;
      const b = decisions.open(asker(), "b", GET);
      b.catch(() => {});
      await until(() => badges.get(7) === "1" && !popup.includes(a.id));
      focused = true;
      const id = new URL(popup).searchParams.get("id")!;
      expect(notes(await attachPanel(id))).toEqual([]);
    });

    test("the opening counts for its window: one in another window neither holds this one nor is used up by it", async () => {
      const p = await shown(CREATE);
      const own = await attachPanel(p.id);
      expect(notes(own)).toEqual([]);
      own.serverClose();
      await sleep(10);
      const counts = (decisions as unknown as { ownOpens: Map<number, number> }).ownOpens;
      counts.set(21, 1); // an opening of the background in another browser window
      expect(notes(await attachPanel(p.id))).toEqual([USER]);
      expect(counts.get(21)).toBe(1);
    });
  });

  describe("the normal toolbar popup is told who opened it (8.3c)", () => {
    beforeEach(() => {
      decisionTiming.lifeMs = 120_000;
      decisionTiming.reopenMs = 60_000;
      decisionTiming.refocusMs = 60_000;
    });
    const USER = { userOpened: true };
    const POPUP_SENDER = () => ({ id: EXT, url: chrome.runtime.getURL("popup.html") });
    async function attachPopup(windowId: number | string = 20, sender: Record<string, unknown> = POPUP_SENDER()) {
      const port = Object.assign(new FakePort(), { name: `popup:${windowId}`, sender });
      await decisions.attach(port as unknown as chrome.runtime.Port);
      return port;
    }
    const counts = () => (decisions as unknown as { ownOpens: Map<number, number> }).ownOpens;

    test("a popup with no own opening waiting in its window was opened by the user", async () => {
      const port = await attachPopup();
      expect(port.sent).toEqual([USER]);
      expect(port.disconnected).toBe(false);
    });

    test("an own openPopup that showed the normal popup (the tab's popup was reset in between) is held: the popup takes its count", async () => {
      // The page navigates while the panel is being opened: Chrome resets the tab's popup, so openPopup shows popup.html.
      const real = chrome.action.openPopup;
      let shownPopup: Promise<FakePort> | undefined;
      let id = "";
      chrome.action.openPopup = (async (options: { windowId: number }) => {
        id = new URL(popup).searchParams.get("id")!;
        popup = DEFAULT(); // reset by the navigation
        shownPopup = attachPopup(options.windowId);
        await sleep(5);
        opened.push(options.windowId);
      }) as typeof chrome.action.openPopup;
      try {
        await shown(CREATE);
        const port = await shownPopup!;
        expect(port.sent).toEqual([]);
        expect(counts().get(20)).toBeUndefined(); // taken by the popup
        // The panel the user opens next from the toolbar icon is the decision's first showing: held (8.6 F2), and it
        // takes no count (there is none). Shown again after that, it is the user's.
        const user = await attachPanel(id);
        expect(user.disconnected).toBe(false);
        expect(user.sent).toEqual([]);
        expect(counts().get(20)).toBeUndefined();
        user.serverClose();
        await sleep(10);
        expect((await attachPanel(id)).sent).toEqual([USER]);
      } finally {
        chrome.action.openPopup = real;
      }
    });

    test("a panel that replaced itself with the popup (its request was gone) leaves the count to the popup: held", async () => {
      const p = await shown(SAVE);
      decisions.onTabRemoved(7); // the request ends before the panel's page connected
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
      const refused = await attachPanel(p.id);
      expect(refused.disconnected).toBe(true); // `toPopup` follows in the page
      expect((await attachPopup()).sent).toEqual([]);
      expect((await attachPopup()).sent).toEqual([USER]);
    });

    test("a failed own openPopup keeps its count for the popup too", async () => {
      failOpen = true;
      await shown(CHOOSE).catch(() => undefined);
      failOpen = false;
      expect(counts().get(20)).toBe(1);
      expect((await attachPopup()).sent).toEqual([]);
      expect((await attachPopup()).sent).toEqual([USER]);
    });

    test("only the popup page shown as a toolbar popup is answered; refused ports take nothing", async () => {
      counts().set(20, 1);
      for (const [windowId, sender] of [
        [20, { ...POPUP_SENDER(), tab: { id: 7, windowId: 20 } }], // the popup page opened in a tab
        [20, { ...POPUP_SENDER(), frameId: 0 }],
        [20, { id: EXT, url: ORIGIN, tab: { id: 7, windowId: 20 } }], // a content script
        [20, { ...POPUP_SENDER(), id: "other" }],
        [20, { id: EXT, url: panelUrl("x") }], // a decision page
        [20, { id: EXT, url: `${chrome.runtime.getURL("popup.html")}?user=1` }],
        [20, { id: EXT, url: chrome.runtime.getURL("popup.html?from=panel") }],
        ["abc", POPUP_SENDER()],
        ["", POPUP_SENDER()],
      ] as [number | string, Record<string, unknown>][]) {
        const port = await attachPopup(windowId, sender);
        expect(port.disconnected, JSON.stringify(sender)).toBe(true);
        expect(port.sent).toEqual([]);
      }
      expect(counts().get(20)).toBe(1);
      expect((await attachPopup()).sent).toEqual([]);
    });

    test("the count is per window", async () => {
      counts().set(21, 1);
      expect((await attachPopup(20)).sent).toEqual([USER]);
      expect(counts().get(21)).toBe(1);
      expect((await attachPopup(21)).sent).toEqual([]);
      expect(counts().get(21)).toBeUndefined();
    });
  });

  describe("the picker's own rows (8.3b)", () => {
    const PASSKEYS = { rpId: "example.com", allow: [], challenge: "AAAA", uv: "preferred" as const };
    beforeEach(() => {
      decisionTiming.pollMs = 5;
    });

    test("a new-password form: the panel offers Generate, and the password goes to the asking page only", async () => {
      const p = await shown({ decision: "chooseLogin", newPassword: true });
      expect(await decisions.info(p.sender, p.id)).toMatchObject({ decision: "chooseLogin", newPassword: true, passkeys: false });
      const before = calls.length;
      const deps = (decisions as unknown as { deps: { request: (op: string) => Promise<unknown> } }).deps;
      const real = deps.request;
      deps.request = async (op: string) => (op === "generatePassword" ? (calls.push({ op, params: {} }), { password: "Zx9-generated" }) : real(op));
      const reply = await decisions.answer(p.sender, p.id, { action: "generate" });
      expect(JSON.stringify(reply)).not.toContain("Zx9");
      expect(calls.slice(before).map((c) => c.op)).toEqual(["generatePassword"]);
      expect(await p.outcome).toEqual({ generated: "Zx9-generated" });
      await restored();
    });

    test("Generate is refused for a form that did not ask for a new password, and a passkey for a pick without one", async () => {
      const p = await shown(CHOOSE);
      expect(await decisions.info(p.sender, p.id)).toMatchObject({ newPassword: false, passkeys: false });
      await expect(decisions.answer(p.sender, p.id, { action: "generate" })).rejects.toMatchObject({ code: "invalid" });
      await expect(decisions.answer(p.sender, p.id, { action: "assert", itemId: "k1" })).rejects.toMatchObject({ code: "invalid" });
      await expect(decisions.candidates(p.sender, p.id)).rejects.toMatchObject({ code: "invalid" });
      expect(calls).toEqual([]);
      await decisions.answer(p.sender, p.id, { action: "cancel" });
    });

    test("a page's pending passkey sign-in: its passkeys are listed and the chosen one signs for the frame's site", async () => {
      const p = await shown({ decision: "chooseLogin", passkeys: PASSKEYS });
      expect(await decisions.info(p.sender, p.id)).toMatchObject({ passkeys: true, newPassword: false });
      expect(await decisions.candidates(p.sender, p.id)).toEqual([{ itemId: "k1", rpId: "example.com" }] as never);
      expect(calls[0]).toMatchObject({ op: "passkeyCandidates", params: { origin: ORIGIN, rpId: "example.com" } });
      await decisions.answer(p.sender, p.id, { action: "assert", itemId: "k1" });
      expect(calls[1]).toMatchObject({ op: "passkeyAssert", params: { origin: ORIGIN, rpId: "example.com", itemId: "k1" } });
      expect(await p.outcome).toMatchObject({ passkey: { signature: "s", clientDataJSON: expect.any(String) } });
      await restored();
    });

    test("a generated password and a passkey picked in the pick also wait until the tab is in front again (like a fetched login)", async () => {
      let release = () => {};
      const deps = (decisions as unknown as { deps: { request: (op: string, params?: Record<string, unknown>) => Promise<unknown> } }).deps;
      const real = deps.request;
      deps.request = async (op: string, params?: Record<string, unknown>) =>
        op === "generatePassword" ? (calls.push({ op, params: {} }), await gate, { password: "Zx9-generated" }) : real(op, params);
      const cases: [string, DecisionRequest, DecisionAnswer, (outcome: unknown) => void][] = [
        ["Generate", { decision: "chooseLogin", newPassword: true }, { action: "generate" }, (o) => expect(o).toEqual({ generated: "Zx9-generated" })],
        ["a passkey", { decision: "chooseLogin", passkeys: PASSKEYS }, { action: "assert", itemId: "k1" }, (o) => expect(o).toMatchObject({ passkey: { signature: "s" } })],
      ];
      for (const [name, request, answer, check] of cases) {
        calls = [];
        const p = await shown(request, name);
        const port = await attachPanel(p.id);
        gate = new Promise<void>((resolve) => (release = resolve));
        void decisions.answer(p.sender, p.id, answer).catch(() => {});
        await until(() => calls.length === 1);
        focused = false; // the app's Hello or Unlock has the focus
        port.serverClose();
        release();
        await sleep(40);
        expect(p.state.done, name).toBe(false);
        focused = true;
        check(await p.outcome);
        gate = null;
        await restored();
      }
    });
  });

  describe("Chrome: the panel takes the focus from its window (8.3b)", () => {
    let contexts: { contextType: string; documentUrl: string; windowId: number }[];
    let lastFocused: number;
    let release: () => void;
    const hold = () => (gate = new Promise<void>((resolve) => (release = resolve)));
    beforeEach(() => {
      delete (chrome.runtime as { getBrowserInfo?: unknown }).getBrowserInfo;
      contexts = [];
      lastFocused = 20;
      decisionTiming.pollMs = 5;
      decisionTiming.settleMs = 20;
      Object.assign(chrome.runtime, { getContexts: async () => contexts });
      Object.assign(chrome.windows, { getLastFocused: async () => ({ id: lastFocused }) });
    });
    /** Chrome while the decision's own panel has the focus (measured in Chrome 157): the window reports no focus. */
    const panelHasFocus = (id: string) => {
      focused = false;
      contexts = [{ contextType: "POPUP", documentUrl: panelUrl(id), windowId: -1 }];
    };

    test("the panel opens without getBrowserInfo (Chrome 127+), never a window", async () => {
      const p = await shown(CHOOSE);
      expect(opened).toEqual([20]);
      expect(windows.created).toEqual([]);
      await decisions.answer(p.sender, p.id, { action: "cancel" });
    });

    test("its own open panel counts as the window having the focus; anything less does not", async () => {
      const p = await shown(CHOOSE);
      panelHasFocus(p.id);
      expect((await decisions.info(p.sender, p.id)).decision).toBe("chooseLogin");
      const refusals: [string, () => void][] = [
        ["no panel open", () => (contexts = [])],
        ["another decision's panel", () => (contexts = [{ contextType: "POPUP", documentUrl: panelUrl("other"), windowId: -1 }])],
        ["the normal popup", () => (contexts = [{ contextType: "POPUP", documentUrl: chrome.runtime.getURL("popup.html"), windowId: -1 }])],
        ["another window had the focus last", () => (lastFocused = 21)],
        ["the tab is not the active one", () => (tab.active = false)],
        ["the tab is on another site", () => (tab.url = "https://other.example/")],
      ];
      for (const [name, change] of refusals) {
        panelHasFocus(p.id);
        lastFocused = 20;
        tab.active = true;
        tab.url = `${ORIGIN}/login`;
        change();
        await expect(decisions.logins(p.sender, p.id), name).rejects.toMatchObject({ code: "cancelled" });
      }
      expect(calls).toEqual([]);
      decisions.onTabRemoved(7);
    });

    test("Firefox is not loosened: an unfocused window is refused even with the panel open", async () => {
      Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "160.0" }) });
      const p = await shown(CHOOSE);
      panelHasFocus(p.id);
      await expect(decisions.logins(p.sender, p.id)).rejects.toMatchObject({ code: "cancelled" });
      decisions.onFocusChanged(-1);
      await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
    });

    test("focus leaving to the panel keeps a pick; leaving to another program ends it", async () => {
      const p = await shown(CHOOSE);
      decisions.onFocusChanged(20);
      panelHasFocus(p.id);
      decisions.onFocusChanged(-1);
      await sleep(60);
      expect(p.state.done).toBe(false);
      // The fetched login is delivered while the panel still has the focus.
      await decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" });
      expect(await p.outcome).toEqual({ fill: FILL });
      await restored();
      focused = true;
      contexts = [];
      const q = await shown(CHOOSE, "r2");
      focused = false;
      contexts = [];
      decisions.onFocusChanged(-1);
      await expect(q.outcome).rejects.toMatchObject({ code: "cancelled" });
    });

    test("a fetched login waits while neither the window nor its panel has the focus", async () => {
      const p = await shown(CHOOSE);
      panelHasFocus(p.id);
      hold();
      const answered = decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" });
      answered.catch(() => {});
      await until(() => calls.length === 1);
      contexts = []; // the app's prompt took the focus and Chrome closed the panel
      release();
      await sleep(40);
      expect(p.state.done).toBe(false);
      focused = true; // back in the browser
      expect(await p.outcome).toEqual({ fill: FILL });
    });

    test("focus to -1 keeps a pick when its panel shows up only on the second look (after settleMs)", async () => {
      const p = await shown(CHOOSE);
      let looks = 0;
      Object.assign(chrome.runtime, { getContexts: async () => (++looks === 1 ? [] : contexts) });
      panelHasFocus(p.id);
      decisions.onFocusChanged(-1);
      await sleep(60);
      expect(looks).toBeGreaterThanOrEqual(2);
      expect(p.state.done).toBe(false);
      await decisions.answer(p.sender, p.id, { action: "cancel" });
    });

    test("focus to -1 while the panel is still opening (openPanel running) keeps the pick", async () => {
      let started = false;
      let letOpen = () => {};
      const opening = new Promise<void>((resolve) => (letOpen = resolve));
      Object.assign(chrome.action, {
        openPopup: async (options: { windowId: number }) => {
          started = true;
          await opening; // Chrome is still opening the panel, longer than settleMs
          opened.push(options.windowId);
          contexts = [{ contextType: "POPUP", documentUrl: popup, windowId: -1 }];
        },
      });
      const outcome = decisions.open(asker(), "r1", CHOOSE);
      outcome.catch(() => {});
      const state = watch(outcome);
      await until(() => started);
      focused = false; // the panel is taking the focus
      decisions.onFocusChanged(-1);
      await sleep(4 * decisionTiming.settleMs);
      letOpen();
      await until(() => opened.length === 1);
      await sleep(4 * decisionTiming.settleMs);
      expect(state.done).toBe(false);
      const id = new URL(popup).searchParams.get("id")!;
      await decisions.answer({ id: EXT, url: panelUrl(id) } as chrome.runtime.MessageSender, id, { action: "cancel" });
      await expect(outcome).rejects.toMatchObject({ code: "cancelled" });
    });

    test("an answer or an Unlock that starts during the settleMs wait is not ended by the focus change", async () => {
      decisionTiming.settleMs = 100;
      for (const what of ["answer", "unlock"] as const) {
        focused = true;
        contexts = [];
        calls = [];
        const p = await shown(CHOOSE, what);
        let looks = 0;
        Object.assign(chrome.runtime, { getContexts: async () => (looks++, contexts) });
        focused = false; // the focus left, no panel shows
        decisions.onFocusChanged(-1);
        await until(() => looks >= 1);
        focused = true; // back in the window before the second look: the user acts
        hold();
        const acted = what === "answer" ? decisions.answer(p.sender, p.id, { action: "fill", itemId: "i1" }) : decisions.unlock(p.sender, p.id);
        acted.catch(() => {});
        await until(() => calls.length === 1);
        await sleep(3 * decisionTiming.settleMs); // past the second look
        expect(looks, what).toBeGreaterThanOrEqual(2);
        expect(p.state.done, what).toBe(false);
        if (what === "answer") {
          release();
          expect(await p.outcome).toEqual({ fill: FILL });
        } else {
          decisions.onTabRemoved(7);
          release();
          await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        }
        gate = null;
        await restored();
      }
    });

    describe("a panel still open when the Unlock worked, closed afterwards (Chrome on Wayland, 8.4)", () => {
      const realNow = Date.now;
      let skew = 0;
      beforeEach(() => {
        decisionTiming.lifeMs = 120_000;
        decisionTiming.reopenMs = 60_000;
        decisionTiming.refocusMs = 60_000;
        skew = 0;
        Date.now = () => realNow() + skew;
      });
      afterEach(() => {
        Date.now = realNow;
      });

      /** Shown, attached, Unlock pressed and answered while the panel is still attached (it has the focus, as in Chrome). */
      async function unlockedWithPanel(request: DecisionRequest, ref = "r1") {
        pending = { id: "p1", origin: ORIGIN, username: "alice", expiresAt: realNow() + 120_000 }; // a fresh capture each time
        focused = true;
        contexts = [];
        const p = await shown(request, ref);
        const port = await attachPanel(p.id);
        panelHasFocus(p.id);
        hold();
        const unlocked = decisions.unlock(p.sender, p.id);
        await until(() => calls.length > 0);
        decisions.onFocusChanged(-1); // the app's Unlock in front, the panel left open behind it
        release();
        expect(await unlocked).toBe(true);
        gate = null;
        await sleep(10);
        expect(opened, "nothing reopens while the panel is open").toHaveLength(1);
        expect(p.state.done).toBe(false);
        return { ...p, port };
      }
      /** The browser window is in front again and Chrome closed the panel. */
      const windowBack = () => {
        focused = true;
        contexts = [];
      };

      test("closing 0 s, 1 s or 10 s after the Unlock reopens it (a pick, a save, a passkey), held, in both event orders", async () => {
        for (const request of [CHOOSE, SAVE, CREATE, GET]) {
          for (const later of [0, 1000, 10_000]) {
            for (const focusFirst of [false, true]) {
              const why = `${request.decision} after ${later} ms, focus event ${focusFirst ? "before" : "after"} the close`;
              calls = [];
              opened = [];
              const p = await unlockedWithPanel(request, `r-${request.decision}-${later}-${focusFirst}`);
              skew = later;
              windowBack();
              if (focusFirst) decisions.onFocusChanged(20); // the panel is still attached then: nothing yet
              p.port.serverClose();
              if (!focusFirst) decisions.onFocusChanged(20);
              await until(() => opened.length === 2, 1000, why);
              expect(p.state.done, why).toBe(false);
              const again = await attachPanel(p.id);
              expect(again.sent, `${why}: the reopened panel is the background's own (held)`).toEqual([]);
              // Once only.
              again.serverClose();
              decisions.onFocusChanged(20);
              await sleep(20);
              expect(opened, why).toHaveLength(2);
              if (request.decision === "chooseLogin") await expect(p.outcome, why).rejects.toMatchObject({ code: "cancelled" });
              else decisions.onTabRemoved(7);
              skew = 0;
              await normal();
            }
          }
        }
      });

      test("the user clicks back into the browser only later (E): it reopens then, and a focus leaving to -1 meanwhile ends nothing", async () => {
        for (const request of [CHOOSE, SAVE]) {
          calls = [];
          opened = [];
          const p = await unlockedWithPanel(request, `r-${request.decision}`);
          focused = false;
          contexts = [];
          p.port.serverClose(); // closed while the app is still in front
          decisions.onFocusChanged(-1);
          await sleep(3 * decisionTiming.settleMs);
          expect(opened, request.decision).toHaveLength(1);
          expect(p.state.done, request.decision).toBe(false);
          skew = 10_000;
          focused = true;
          decisions.onFocusChanged(20);
          await until(() => opened.length === 2, 1000, request.decision);
          decisions.onTabRemoved(7);
          skew = 0;
          await normal();
        }
      });

      test("a focus change to -1 while the panel is still open but not seen (or seen only later) ends nothing", async () => {
        const p = await unlockedWithPanel(CHOOSE);
        focused = false;
        contexts = []; // Chrome does not list the panel (yet)
        decisions.onFocusChanged(-1);
        await sleep(3 * decisionTiming.settleMs);
        expect(p.state.done).toBe(false);
        windowBack();
        p.port.serverClose();
        await until(() => opened.length === 2, 1000, "the panel to reopen");
      });

      test("an Unlock that works during the settleMs wait of a focus change to -1 is not ended by it", async () => {
        decisionTiming.settleMs = 100;
        const p = await shown(CHOOSE);
        const port = await attachPanel(p.id);
        focused = false;
        contexts = [];
        decisions.onFocusChanged(-1); // starts the wait: no panel seen
        await sleep(10);
        panelHasFocus(p.id);
        expect(await decisions.unlock(p.sender, p.id)).toBe(true); // the panel is seen again for the Unlock
        contexts = []; // and not at the second look
        await sleep(3 * decisionTiming.settleMs);
        expect(p.state.done).toBe(false);
        windowBack();
        port.serverClose();
        await until(() => opened.length === 2, 1000, "the panel to reopen");
      });

      test("the window reports the focus only a moment after the panel closed, with no further event: it still reopens", async () => {
        const p = await unlockedWithPanel(SAVE);
        focused = false;
        contexts = [];
        decisions.onFocusChanged(20); // the event came first; the window does not say it is focused yet
        p.port.serverClose();
        await sleep(2);
        focused = true;
        await until(() => opened.length === 2, 1000, "the panel to reopen");
      });

      test("the Unlock answer after the port closed (the order Firefox shows) still reopens", async () => {
        const p = await shown(CHOOSE);
        const port = await attachPanel(p.id);
        panelHasFocus(p.id);
        hold();
        const unlocked = decisions.unlock(p.sender, p.id);
        await until(() => calls.length > 0);
        focused = false;
        contexts = [];
        port.serverClose();
        decisions.onFocusChanged(-1);
        release();
        expect(await unlocked).toBe(true);
        windowBack();
        decisions.onFocusChanged(20);
        await until(() => opened.length === 2, 1000, "the panel to reopen");
        expect((await attachPanel(p.id)).sent).toEqual([]);
      });

      test("an answer after the Unlock ends it: Esc/Cancel, Not now, Never, another method, a pick; none reopens", async () => {
        const cases: [DecisionRequest, DecisionAnswer][] = [
          [CHOOSE, { action: "cancel" }],
          [CHOOSE, { action: "fill", itemId: "i1" }],
          [SAVE, { action: "cancel" }],
          [SAVE, { action: "never" }],
          [CREATE, { action: "fallback" }],
          [GET, { action: "cancel" }],
        ];
        for (const [request, a] of cases) {
          const why = `${request.decision} ${a.action}`;
          calls = [];
          opened = [];
          const p = await unlockedWithPanel(request, `r-${why}`);
          await decisions.answer(p.sender, p.id, a);
          await until(() => p.state.done, 1000, why);
          windowBack();
          p.port.serverClose(); // the panel closes itself after the answer
          decisions.onFocusChanged(20);
          await sleep(30);
          expect(opened, why).toHaveLength(1);
          await normal();
        }
      });

      test("an answer that is refused (the tab not in front) still ends the grace: Esc then closes it for good", async () => {
        for (const request of [CHOOSE, SAVE]) {
          calls = [];
          opened = [];
          const p = await unlockedWithPanel(request, `r-${request.decision}`);
          focused = false;
          contexts = []; // neither the window nor the panel counts as in front: the answer is refused
          await expect(decisions.answer(p.sender, p.id, { action: "cancel" })).rejects.toMatchObject({ code: "cancelled" });
          p.port.serverClose(); // the page closes itself on that refusal
          windowBack();
          decisions.onFocusChanged(20);
          await sleep(30);
          expect(opened, request.decision).toHaveLength(1);
          if (request.decision === "chooseLogin") await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
          else {
            expect(p.state.done).toBe(false);
            expect(badges.get(7)).toBe("1"); // the save waits with the badge, as a closed panel always does
            decisions.onTabRemoved(7);
          }
          await normal();
        }
      });

      test("an answer from anything but the panel page does not end the grace", async () => {
        const p = await unlockedWithPanel(CHOOSE);
        const strangers = [
          { id: EXT, url: `${ORIGIN}/login`, tab: { id: 7, windowId: 20 }, frameId: 0 },
          { id: EXT, url: panelUrl(p.id), tab: { id: 9, windowId: 20 } },
          { id: EXT, url: panelUrl(p.id), frameId: 0 },
          { id: "other", url: panelUrl(p.id) },
          { id: EXT, url: chrome.runtime.getURL("popup.html") },
        ] as chrome.runtime.MessageSender[];
        for (const s of strangers) await expect(decisions.answer(s, p.id, { action: "cancel" })).rejects.toBeDefined();
        expect(p.state.done).toBe(false);
        windowBack();
        p.port.serverClose();
        await until(() => opened.length === 2, 1000, "the panel to reopen");
      });

      test("a tab switch or another site cancels the pick; another window ends it as before; a hidden save does not reopen", async () => {
        // The tab went to another site meanwhile: the pick is cancelled when its panel closes.
        let p = await unlockedWithPanel(CHOOSE, "site");
        tab.url = "https://other.example/";
        windowBack();
        p.port.serverClose();
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        tab.url = `${ORIGIN}/login`;
        await normal();
        // The tab is no longer the active one.
        calls = [];
        opened = [];
        p = await unlockedWithPanel(CHOOSE, "inactive");
        tab.active = false;
        windowBack();
        p.port.serverClose();
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        tab.active = true;
        await normal();
        // Another tab activated while the panel is still open: the pick ends at once.
        calls = [];
        opened = [];
        p = await unlockedWithPanel(CHOOSE, "tab");
        decisions.onTabActivated({ tabId: 8, windowId: 20 });
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        await normal();
        // Another browser window took the focus: as before (a pick ends).
        calls = [];
        opened = [];
        p = await unlockedWithPanel(CHOOSE, "window");
        decisions.onFocusChanged(21);
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        await normal();
        // A save hidden by the background (another tab): it waits, and coming back does not reopen it.
        calls = [];
        opened = [];
        p = await unlockedWithPanel(SAVE, "hidden");
        decisions.onTabActivated({ tabId: 8, windowId: 20 });
        expect(p.port.disconnected).toBe(true);
        windowBack();
        decisions.onTabActivated({ tabId: 7, windowId: 20 });
        decisions.onFocusChanged(20);
        await sleep(30);
        expect(opened).toHaveLength(1);
        expect(p.state.done).toBe(false);
        expect(badges.get(7)).toBe("1");
        decisions.onTabRemoved(7);
        await normal();
      });

      test("the grace ends with the reopen time: then a pick ends and a save only waits", async () => {
        for (const request of [CHOOSE, SAVE]) {
          calls = [];
          opened = [];
          const p = await unlockedWithPanel(request, `r-${request.decision}`);
          skew = decisionTiming.reopenMs + 1;
          windowBack();
          p.port.serverClose();
          decisions.onFocusChanged(20);
          await sleep(30);
          expect(opened, request.decision).toHaveLength(1);
          if (request.decision === "chooseLogin") await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
          else {
            expect(p.state.done).toBe(false);
            decisions.onTabRemoved(7);
          }
          skew = 0;
          await normal();
        }
      });

      test("a panel closed late in the grace gets only the rest of the reopen time, not a fresh one", async () => {
        const p = await unlockedWithPanel(CHOOSE);
        skew = decisionTiming.reopenMs - 20; // still in the grace
        focused = false;
        contexts = [];
        p.port.serverClose();
        await sleep(60); // the rest of the reopen time runs out
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        windowBack();
        decisions.onFocusChanged(20);
        await sleep(20);
        expect(opened).toHaveLength(1);
      });

      test("a reopened panel ends the grace: closing that one again does not reopen a second time", async () => {
        const p = await unlockedWithPanel(SAVE);
        windowBack();
        p.port.serverClose();
        await until(() => opened.length === 2, 1000, "the panel to reopen");
        const again = await attachPanel(p.id);
        again.serverClose();
        decisions.onFocusChanged(20);
        await sleep(30);
        expect(opened).toHaveLength(2);
        expect(badges.get(7)).toBe("1");
      });

      test("a refused Unlock gives no grace: the pick ends when its panel closes", async () => {
        const p = await shown(CHOOSE);
        const port = await attachPanel(p.id);
        panelHasFocus(p.id);
        failWith = new BrokerError("locked", "The vault is locked");
        expect(await decisions.unlock(p.sender, p.id)).toBe(false);
        failWith = null;
        windowBack();
        port.serverClose();
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        expect(opened).toHaveLength(1);
      });

      test("Firefox is unchanged: a panel that outlives the Unlock and closes then ends a pick", async () => {
        Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "160.0" }) });
        const p = await shown(CHOOSE);
        const port = await attachPanel(p.id);
        expect(await decisions.unlock(p.sender, p.id)).toBe(true);
        port.serverClose();
        decisions.onFocusChanged(20);
        await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
        expect(opened).toHaveLength(1);
      });
    });
  });

  test("Firefox below 149 keeps separate windows for saving and passkeys, and closing a save window is still Not now", async () => {
    Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "140.0" }) });
    const p = await open({ decision: "saveLogin", id: "p1" });
    expect(opened).toEqual([]);
    windows.close(p.windowId);
    await expect(p.outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(calledDeps).toContain("dismiss:7:p1:false");
    const k = await open(CREATE, asker(), "r2");
    expect(opened).toEqual([]);
    windows.close(k.windowId);
    await expect(k.outcome).rejects.toMatchObject({ code: "cancelled" });
    expect(popup).toBe(DEFAULT());
  });
});
