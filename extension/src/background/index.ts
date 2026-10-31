// The service worker: owns the connection to the app and answers the content scripts and the popup.
//
// Rules that keep this safe:
// * The site a request is for comes from the browser (`sender`), never from what a page's
//   script says. Secrets are only given to the frame they belong to, or to the top frame of a
//   tab the user picked in the popup.
// * Page-load queries never connect (so browsing never starts the app); user clicks do, and so do
//   passkey requests (the worker sleeps when idle, and a request must not slip to the browser).

import { sha256Hex } from "../shared/crypto";
import { MAX_CAPTURE_CHARS } from "../shared/limits";
import type {
  FindLoginsReply,
  PendingSave,
  PopupState,
  PrivacyState,
  Reply,
  ToBackground,
  ToContent,
} from "../shared/messages";
import { BrokerError, type ItemSummary, type LoginFill, type LoginMatch, type SaveOutcome, type VaultStatus } from "../shared/protocol";
import { BrokerClient } from "./broker";
import { Decisions } from "./decide";
import * as passkeys from "./passkeys";
import { local } from "./storage";

/** Set by `bun run build --firefox`: there the decisions are made in an extension window (see decide.ts). */
declare const __FIREFOX__: boolean;

// Content scripts can read storage.local unless it is restricted: do it the moment the worker
// starts (every later read and write waits for it, see storage.ts).
void local().catch((e) => console.warn("could not restrict storage.local", e));

const broker = new BrokerClient((state) => {
  chrome.runtime.sendMessage({ kind: "brokerState", state }).catch(() => {});
});

/** How long a captured login waits for the page after the sign-in to ask about it. */
const PENDING_TTL_MS = 2 * 60_000;
/** The popup closes when Windows Hello takes the focus: the list it asked for stays this long. */
const LOGIN_LIST_TTL_MS = 2 * 60_000;
const LOGIN_LIST_KEY = "logins:all";
/** The most logins one request for the full list returns (the app's own limit). */
const LOGIN_LIST_LIMIT = 200;

interface Pending extends PendingSave {
  password: string;
  tabId: number;
  at: number;
  /** Firefox: the save window was opened for this capture already (it is not reopened on every page load). */
  offered?: boolean;
}

// ---- who is asking ----

function originOf(url: string | undefined): string | null {
  if (!url) return null;
  try {
    const origin = new URL(url).origin;
    return origin === "null" ? null : origin;
  } catch {
    return null;
  }
}

/** The origin a content script's frame belongs to, if it may be served secrets. */
function frameOrigin(sender: chrome.runtime.MessageSender): string {
  if (sender.id !== chrome.runtime.id) throw new BrokerError("forbidden", "unknown sender");
  const origin = originOf(sender.origin ?? sender.url);
  if (!origin) throw new BrokerError("invalid", "this page has no usable address");
  // Frames from other sites are not filled (a login form in a third-party iframe).
  if (sender.frameId && sender.frameId !== 0) {
    const top = originOf(sender.tab?.url);
    if (top !== origin) throw new BrokerError("forbidden", "login forms in other sites' frames are not filled");
  }
  return origin;
}

function fromContentScript(sender: chrome.runtime.MessageSender): sender is chrome.runtime.MessageSender & { tab: chrome.tabs.Tab } {
  // The extension's own pages (popup, decision window) are chrome-extension:// or moz-extension://.
  return !!sender.tab && sender.id === chrome.runtime.id && !sender.url?.startsWith(chrome.runtime.getURL(""));
}

// ---- helpers ----

function describe(e: unknown): { code: string; message: string } {
  if (e instanceof BrokerError) return { code: e.code, message: e.message };
  return { code: "internal", message: e instanceof Error ? e.message : String(e) };
}

async function request<T>(op: string, params: Record<string, unknown> = {}): Promise<T> {
  return broker.request<T>(op, params);
}

async function sessionGet<T>(key: string): Promise<T | undefined> {
  const stored = await chrome.storage.session.get(key);
  return stored[key] as T | undefined;
}

async function neverSave(): Promise<string[]> {
  const stored = await (await local()).get("neverSave");
  return Array.isArray(stored.neverSave) ? (stored.neverSave as string[]) : [];
}

// ---- handlers ----

async function findLogins(sender: chrome.runtime.MessageSender, interactive: boolean): Promise<FindLoginsReply> {
  return findLoginsFor(frameOrigin(sender), interactive);
}

async function findLoginsFor(origin: string, interactive: boolean): Promise<FindLoginsReply> {
  if (broker.state.phase !== "ready" && !interactive) return { status: "offline", logins: [] };
  try {
    const logins = await request<LoginMatch[]>("findLogins", { origin });
    return { status: "ok", logins };
  } catch (e) {
    const { code } = describe(e);
    if (["locked", "noVault", "unpaired", "appNotRunning", "denied", "untrusted", "disconnected", "error", "tooMany", "tooLarge"].includes(code)) {
      return { status: code, logins: [] };
    }
    throw e;
  }
}

/** Remembers what was filled so the sign-in that follows isn't offered for saving again. */
async function rememberFill(tabId: number, fill: LoginFill) {
  await chrome.storage.session.set({ [`filled:${tabId}`]: { username: fill.username, hash: await sha256Hex(fill.password) } });
}

async function fill(sender: chrome.runtime.MessageSender, itemId: string): Promise<LoginFill> {
  const origin = frameOrigin(sender);
  const fill = await request<LoginFill>("fillLogin", { itemId, origin, force: false });
  if (sender.tab?.id !== undefined) await rememberFill(sender.tab.id, fill);
  return fill;
}

/** Captures still being stored, so the page that follows a sign-in doesn't ask too early. */
const capturing = new Map<number, Promise<void>>();

function capture(sender: chrome.runtime.MessageSender, username: string, password: string): Promise<void> {
  if (!fromContentScript(sender) || sender.tab.id === undefined || !password) return Promise.resolve();
  // A page can set a field to any value; keep what it can make the worker store (storage.session has a small quota).
  if (typeof username !== "string" || typeof password !== "string") return Promise.resolve();
  if (username.length > MAX_CAPTURE_CHARS || password.length > MAX_CAPTURE_CHARS) return Promise.resolve();
  const tabId = sender.tab.id;
  const work = storeCapture(sender, tabId, username, password).finally(() => {
    if (capturing.get(tabId) === work) capturing.delete(tabId);
  });
  capturing.set(tabId, work);
  return work;
}

async function storeCapture(sender: chrome.runtime.MessageSender, tabId: number, username: string, password: string) {
  const origin = frameOrigin(sender);
  if ((await neverSave()).includes(origin)) return;
  const filled = await sessionGet<{ username: string; hash: string }>(`filled:${tabId}`);
  if (filled && filled.username === username && filled.hash === (await sha256Hex(password))) return;
  const pending: Pending = { id: crypto.randomUUID(), origin, username, password, tabId, at: Date.now() };
  await chrome.storage.session.set({ [`pending:${tabId}`]: pending });
}

async function pendingFor(tabId: number): Promise<Pending | null> {
  await capturing.get(tabId)?.catch(() => {});
  const pending = await sessionGet<Pending>(`pending:${tabId}`);
  if (!pending) return null;
  if (Date.now() - pending.at > PENDING_TTL_MS) {
    await chrome.storage.session.remove(`pending:${tabId}`);
    return null;
  }
  return pending;
}

async function pendingSave(sender: chrome.runtime.MessageSender): Promise<PendingSave | null> {
  if (!fromContentScript(sender) || sender.tab.id === undefined || sender.frameId !== 0) return null;
  const pending = await pendingFor(sender.tab.id);
  if (!pending) return null;
  if (__FIREFOX__) {
    // A window, not a bar: ask once per capture, or every page load would raise it again.
    if (pending.offered) return null;
    await chrome.storage.session.set({ [`pending:${sender.tab.id}`]: { ...pending, offered: true } });
  }
  return { id: pending.id, origin: pending.origin, username: pending.username };
}

async function pendingInfo(tabId: number, id: string): Promise<PendingSave | null> {
  const pending = await pendingFor(tabId);
  return pending && pending.id === id ? { id: pending.id, origin: pending.origin, username: pending.username } : null;
}

async function savePending(sender: chrome.runtime.MessageSender, id: string): Promise<SaveOutcome> {
  if (!fromContentScript(sender) || sender.tab.id === undefined) throw new BrokerError("forbidden", "not from a page");
  return savePendingFor(sender.tab.id, id);
}

async function savePendingFor(tabId: number, id: string): Promise<SaveOutcome> {
  const pending = await pendingFor(tabId);
  if (!pending || pending.id !== id) throw new BrokerError("notFound", "there is nothing to save any more");
  let outcome: SaveOutcome;
  try {
    outcome = await request<SaveOutcome>("saveLogin", { origin: pending.origin, username: pending.username, password: pending.password });
  } catch (e) {
    // The user said no to "Replace what is saved?" in the app: that is the answer, so the captured
    // password is dropped and nothing asks again or saves by another route.
    if (!(e instanceof BrokerError && e.code === "denied")) throw e;
    await chrome.storage.session.remove(`pending:${tabId}`);
    return { id: pending.id, status: "declined" };
  }
  await chrome.storage.session.remove(`pending:${tabId}`);
  return outcome;
}

async function dismissPending(sender: chrome.runtime.MessageSender, id: string, never: boolean) {
  if (!fromContentScript(sender) || sender.tab.id === undefined) return;
  return dismissPendingFor(sender.tab.id, id, never);
}

async function dismissPendingFor(tabId: number, id: string, never: boolean) {
  const pending = await pendingFor(tabId);
  if (!pending || pending.id !== id) return;
  if (never) await (await local()).set({ neverSave: [...new Set([...(await neverSave()), pending.origin])] });
  await chrome.storage.session.remove(`pending:${tabId}`);
}

async function popupState(): Promise<PopupState> {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const origin = originOf(tab?.url);
  const info: PopupState = {
    broker: broker.state,
    tab: tab?.id !== undefined ? { id: tab.id, origin, host: origin ? new URL(origin).hostname : null, title: tab.title ?? "" } : null,
    logins: [],
    vault: null,
  };
  if (broker.state.phase !== "ready") {
    // Opening the popup is a user action, so it may connect (and start the app).
    await broker.connect().catch(() => {});
    await Promise.race([broker.whenReady().catch(() => {}), new Promise((r) => setTimeout(r, 4000))]);
    info.broker = broker.state;
  }
  if (broker.state.phase === "ready") {
    try {
      const status = await request<{ status: VaultStatus }>("status");
      info.vault = status.status;
      if (status.status === "unlocked" && origin) info.logins = await request<LoginMatch[]>("findLogins", { origin });
    } catch (e) {
      console.warn("popup state failed", e);
    }
  }
  return info;
}

async function fillTab(tabId: number, itemId: string, force: boolean) {
  const tab = await chrome.tabs.get(tabId);
  const origin = originOf(tab.url);
  if (!origin) throw new BrokerError("invalid", "this page can't be filled");
  const fill = await request<LoginFill>("fillLogin", { itemId, origin, force });
  // The request can wait minutes (unlock, Windows Hello) and the user may have gone elsewhere:
  // only hand the password to a tab that still shows the site it was fetched for. The page repeats
  // this check against its own address, which closes the last gap before delivery.
  const now = await chrome.tabs.get(tabId).catch(() => null);
  if (!now || originOf(now.url) !== origin) throw new BrokerError("changed", "The page changed while LocalPass was asking. Nothing was filled.");
  await rememberFill(tabId, fill);
  const message: ToContent = { kind: "doFill", fill, origin };
  await chrome.tabs.sendMessage(tabId, message, { frameId: 0 });
}

/** All live logins, for the popup's "search everything" (one Windows Hello prompt per call). */
async function loadLogins(): Promise<ItemSummary[]> {
  const items = await request<ItemSummary[]>("searchItems", { query: "", limit: LOGIN_LIST_LIMIT });
  await chrome.storage.session.set({ [LOGIN_LIST_KEY]: { at: Date.now(), items } });
  return items;
}

/** The list loaded a moment ago, if any (no prompt): the popup closes while Hello is asking. */
async function cachedLogins(): Promise<ItemSummary[] | null> {
  const cached = await sessionGet<{ at: number; items: ItemSummary[] }>(LOGIN_LIST_KEY);
  if (!cached) return null;
  if (Date.now() - cached.at > LOGIN_LIST_TTL_MS) {
    await chrome.storage.session.remove(LOGIN_LIST_KEY);
    return null;
  }
  return cached.items;
}

/** The popup's "use LocalPass for passkeys" switch (on unless the user turned it off). */
async function passkeysEnabled(): Promise<boolean> {
  try {
    return (await (await local()).get("passkeysEnabled")).passkeysEnabled !== false;
  } catch {
    return true;
  }
}

async function privacy(): Promise<PrivacyState> {
  const setting = await chrome.privacy.services.passwordSavingEnabled.get({});
  return { disabled: setting.value === false, control: setting.levelOfControl };
}

async function setPrivacy(disable: boolean): Promise<PrivacyState> {
  if (disable) await chrome.privacy.services.passwordSavingEnabled.set({ value: false });
  else await chrome.privacy.services.passwordSavingEnabled.clear({});
  return privacy();
}

const decisions = new Decisions({
  request,
  findLogins: (origin) => findLoginsFor(origin, true),
  rememberFill,
  pendingInfo,
  savePending: savePendingFor,
  dismissPending: dismissPendingFor,
});

/** The frame a Firefox decision is for. Chrome never uses decisions: its in-page controls decide. */
function asker(sender: chrome.runtime.MessageSender, fromPage: boolean) {
  if (!__FIREFOX__ || !fromPage || sender.tab?.id === undefined) throw new BrokerError("forbidden", "not available here");
  return { tabId: sender.tab.id, frameId: sender.frameId ?? 0, origin: frameOrigin(sender) };
}

async function handle(message: ToBackground, sender: chrome.runtime.MessageSender): Promise<unknown> {
  if (sender.id !== chrome.runtime.id) throw new BrokerError("forbidden", "unknown sender");
  const fromPage = fromContentScript(sender);
  // Only extension pages (the popup) may use these.
  const popupOnly = () => {
    if (fromPage) throw new BrokerError("forbidden", "not available to pages");
  };
  switch (message.kind) {
    case "findLogins":
      return findLogins(sender, message.interactive);
    case "fill":
      return fill(sender, message.itemId);
    case "capture":
      return capture(sender, message.username, message.password);
    case "pendingSave":
      return pendingSave(sender);
    case "savePending":
      return savePending(sender, message.id);
    case "dismissPending":
      return dismissPending(sender, message.id, message.never);
    case "passkeyCandidates":
      return passkeys.candidates(request, frameOrigin(sender), message.rpId, message.allow);
    case "passkeyCreate":
      if (!fromPage) throw new BrokerError("forbidden", "passkeys are only created for pages");
      return passkeys.create(request, frameOrigin(sender), message.request);
    case "passkeyAssert":
      if (!fromPage) throw new BrokerError("forbidden", "passkeys are only used for pages");
      return passkeys.assert(request, frameOrigin(sender), message.itemId, message.rpId, message.challenge, message.uv);
    case "decide":
      return decisions.open(asker(sender, fromPage), message.ref, message.request);
    case "decideCancel":
      return decisions.withdraw(asker(sender, fromPage), message.ref);
    case "decisionInfo":
      return decisions.info(sender, message.id);
    case "decisionLogins":
      return decisions.logins(sender, message.id);
    case "decisionCandidates":
      return decisions.candidates(sender, message.id);
    case "decisionAnswer":
      return decisions.answer(sender, message.id, message.answer);
    case "unlock":
      return request("unlock");
    case "state":
      popupOnly();
      return popupState();
    case "connect":
      popupOnly();
      await broker.connect(true);
      return broker.state;
    case "loadLogins":
      popupOnly();
      return loadLogins();
    case "cachedLogins":
      popupOnly();
      return cachedLogins();
    case "passkeysEnabled":
      return passkeysEnabled();
    case "fillTab":
      popupOnly();
      return fillTab(message.tabId, message.itemId, message.force);
    case "generate":
      // Open to pages: the in-page "Generate a strong password" row asks, and a random password reveals nothing.
      return request("generatePassword");
    case "getPrivacy":
      popupOnly();
      return privacy();
    case "setPrivacy":
      popupOnly();
      return setPrivacy(message.disableBrowserManager);
  }
}

chrome.runtime.onMessage.addListener((message: ToBackground | { kind: "brokerState" }, sender, sendResponse) => {
  if (message.kind === "brokerState") return false;
  handle(message, sender).then(
    (data) => sendResponse({ ok: true, data } satisfies Reply<unknown>),
    (e) => sendResponse({ ok: false, ...describe(e) } satisfies Reply<unknown>),
  );
  return true;
});

// Connect when the browser starts so icons can appear without a click. This starts the app if
// it isn't running (it normally starts with Windows).
chrome.runtime.onStartup.addListener(() => void broker.connect().catch(() => {}));
chrome.runtime.onInstalled.addListener(() => void broker.connect().catch(() => {}));
// A pending sign-in capture belongs to its tab.
chrome.tabs.onRemoved.addListener((tabId) => {
  decisions.onTabRemoved(tabId);
  void chrome.storage.session.remove([`pending:${tabId}`, `filled:${tabId}`]);
});
if (__FIREFOX__) {
  chrome.windows.onRemoved.addListener((windowId) => decisions.onWindowRemoved(windowId));
  // The decision window keeps a port open: it keeps the event page alive while the user decides.
  chrome.runtime.onConnect.addListener((port) => void decisions.attach(port));
}
