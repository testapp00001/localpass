// Firefox: the decision window (background/decide.ts). Who may open and answer it, what each answer
// does, and what happens when the user closes it. The window itself is a page (src/decide), driven
// here by calling the same methods its messages reach.

import { beforeEach, describe, expect, test } from "bun:test";
import { type Deps, Decisions } from "../src/background/decide";
import type { DecisionRequest } from "../src/shared/messages";
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
let pending: { id: string; origin: string; username: string } | null;
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
  pending = { id: "p1", origin: ORIGIN, username: "alice" };
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
    savePending: async (tab, id) => (calledDeps.push(`save:${tab}:${id}`), { id: "item", status: "created" as const }),
    dismissPending: async (tab, id, never) => void calledDeps.push(`dismiss:${tab}:${id}:${never}`),
  };
  decisions = new Decisions(deps);
  // As background/index.ts wires it.
  chrome.windows.onRemoved.addListener((windowId) => decisions.onWindowRemoved(windowId));
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
    expect(await outcome).toEqual(FILL);
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
  let focused: boolean;
  let popup: string;
  let opened: number[];
  let failOpen: boolean;
  let popupGate: Promise<void> | undefined;

  beforeEach(() => {
    tab = { id: 7, windowId: 20, active: true, url: `${ORIGIN}/login` };
    focused = true;
    popup = chrome.runtime.getURL("popup.html");
    opened = [];
    failOpen = false;
    popupGate = undefined;
    Object.assign(chrome.runtime, { getBrowserInfo: async () => ({ version: "159.0" }) });
    Object.assign(chrome.tabs, { get: async () => ({ ...tab }) });
    Object.assign(chrome.windows, { get: async () => ({ id: 20, focused }) });
    Object.assign(chrome, { action: {
      getPopup: async () => popup,
      setPopup: async (options: { popup: string | null }) => {
        await popupGate;
        popup = options.popup ?? chrome.runtime.getURL("popup.html");
      },
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
    expect(await p.outcome).toEqual(FILL);
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
});
