// The service worker: owns the connection to the app and answers the content scripts and the popup.
//
// Rules that keep this safe:
// * The site a request is for comes from the browser (`sender`), never from what a page's
//   script says. Secrets are only given to the frame they belong to, or to the top frame of a
//   tab the user picked in the popup.
// * A content script runs next to an untrusted page, so nothing it can send by itself yields a
//   password, a signature or a save. Those happen only as the outcome of a decision the user took in
//   LocalPass's own panel or window (`decide`, see decide.ts), and the outcome is the reply to that
//   content script's own `decide` message. What a content script may ask otherwise is listed in
//   `handle`: the badge's status (no titles or user names), captures, the id of a capture to offer,
//   and the passkey switch.
// * Page-load queries never connect (so browsing never starts the app); user clicks do, and so do
//   passkey requests (the worker sleeps when idle, and a request must not slip to the browser).
// * A sign-in the user really sent may connect; a page load never does. A `capture` the content
//   script marks `byUser` (a trusted click or Enter key, never a submit event, which a page script
//   can fire as trusted) and that is kept as a pending save starts a connection when there is none
//   (disconnected, or the app was not running): without pairing and without waiting for it, as at
//   browser start. Unpaired, denied, untrusted or errored connections are left as they are. Asking
//   whether a pending save is already in the vault (`pendingSave`, on page load) never starts a
//   connection; it only waits up to 2 s for one that is already being made. Only a capture the user
//   sent is checked with the app at all (`Pending.byUser`): the answer must not become a way for a page
//   script to test guessed logins.

import { sha256Hex } from "../shared/crypto";
import { MAX_CAPTURE_CHARS } from "../shared/limits";
import type {
  FindLoginsReply,
  PendingSave,
  PendingSaveRef,
  PopupState,
  PrivacyState,
  Reply,
  SiteStatus,
  ToBackground,
  ToContent,
} from "../shared/messages";
import { BrokerError, type ItemSummary, type LoginFill, type LoginMatch, type SaveOutcome, type VaultStatus } from "../shared/protocol";
import { BrokerClient } from "./broker";
import { Decisions, type PendingSaveInfo } from "./decide";
import { local } from "./storage";

// Content scripts can read storage.local unless it is restricted: do it the moment the worker
// starts (every later read and write waits for it, see storage.ts).
void local().catch((e) => console.warn("could not restrict storage.local", e));

const broker = new BrokerClient((state) => {
  chrome.runtime.sendMessage({ kind: "brokerState", state }).catch(() => {});
});

/** How long a captured login waits for the page after the sign-in to ask about it. */
const PENDING_TTL_MS = 2 * 60_000;
/** How long the account name from a two-step sign-in's first step is kept for the password step. */
const SEEN_USER_TTL_MS = 5 * 60_000;
/** How long the worker waits for the app to say whether a captured login is saved already. */
const CHECK_LOGIN_TIMEOUT_MS = 5_000;
/** How long a page load's question waits for a connection that is already being made (never one it starts). */
const CONNECTING_WAIT_MS = 2_000;
/** The connection states a sign-in the user sent may reconnect from: nothing is open, nothing was refused. */
const RECONNECT_FROM = new Set(["disconnected", "appNotRunning"]);
/** The popup closes when Windows Hello takes the focus: the list it asked for stays this long. */
const LOGIN_LIST_TTL_MS = 2 * 60_000;
const LOGIN_LIST_KEY = "logins:all";
/** The most logins one request for the full list returns (the app's own limit). */
const LOGIN_LIST_LIMIT = 200;

interface Pending extends PendingSave {
  password: string;
  tabId: number;
  at: number;
  /**
   * The save panel (or window) was opened by itself for this capture already, by decision
   * `decisionId`: later pages of the tab take that decision over instead of opening it again (decide.ts).
   */
  offered?: boolean;
  decisionId?: string;
  /**
   * The user sent this sign-in (a trusted click or Enter key, `capture`'s `byUser`). Only such a capture is checked
   * with the app (`checkLogin`): a page script can fill a guess and submit it itself, and whether an offer follows
   * would tell it whether the guess is the saved login. Any other capture is offered as it is, never dropped as
   * unchanged and never marked as an update. Missing (a capture stored by an older worker) counts as not sent by the user.
   */
  byUser?: boolean;
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
  // The extension's own pages (popup, decision panel or window) are chrome-extension:// or moz-extension://.
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

/**
 * What a content script learns for its field's badge: whether LocalPass can be used here and has logins for
 * the frame's site. Never their titles or user names: those are shown only in LocalPass's own panel.
 */
async function siteStatus(sender: chrome.runtime.MessageSender, interactive: boolean): Promise<SiteStatus> {
  const reply = await findLoginsFor(frameOrigin(sender), interactive);
  return { status: reply.status, hasLogins: reply.logins.length > 0 };
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

/** The account name typed into the first step of a two-step sign-in (no password box on that page). */
interface SeenUser {
  origin: string;
  username: string;
  at: number;
}

/**
 * Whether two user names name the same account, compared the way the app compares them when it
 * saves (`Engine::plan_save_login`): surrounding white space ignored, ASCII letters in any case.
 */
function sameUsername(a: string, b: string): boolean {
  const norm = (s: string) => s.trim().replace(/[A-Z]/g, (c) => c.toLowerCase());
  return norm(a) === norm(b);
}

/** Captures still being stored, so the page that follows a sign-in doesn't ask too early. */
const capturing = new Map<number, Promise<void>>();

function capture(sender: chrome.runtime.MessageSender, username: string, password: string, byUser: unknown): Promise<void> {
  if (!fromContentScript(sender) || sender.tab.id === undefined || !password) return Promise.resolve();
  // A page can set a field to any value; keep what it can make the worker store (storage.session has a small quota).
  if (typeof username !== "string" || typeof password !== "string") return Promise.resolve();
  if (username.length > MAX_CAPTURE_CHARS || password.length > MAX_CAPTURE_CHARS) return Promise.resolve();
  const tabId = sender.tab.id;
  // Page input like the other fields: only a real `true` counts.
  const sentByUser = byUser === true;
  const work = storeCapture(sender, tabId, username, password, sentByUser)
    .then((stored) => {
      // The user sent a sign-in that will be offered: connect now, so the page that follows can ask
      // the app whether it is saved already. Never pairs, never waits.
      if (stored && sentByUser && RECONNECT_FROM.has(broker.state.phase)) void broker.connect().catch(() => {});
    })
    .finally(() => {
      if (capturing.get(tabId) === work) capturing.delete(tabId);
    });
  capturing.set(tabId, work);
  return work;
}

/** Keeps a captured sign-in for an offer; false when it was dropped (never-save site, or what LocalPass filled). */
async function storeCapture(sender: chrome.runtime.MessageSender, tabId: number, username: string, password: string, byUser: boolean): Promise<boolean> {
  const origin = frameOrigin(sender);
  if ((await neverSave()).includes(origin)) return false;
  // What LocalPass filled in this tab is saved already. A password-only step (the account name was
  // asked on the page before) captures no user name, so the same password alone is enough then.
  const filled = await sessionGet<{ username: string; hash: string }>(`filled:${tabId}`);
  if (filled && filled.hash === (await sha256Hex(password)) && (!username.trim() || sameUsername(filled.username, username))) return false;
  if (!username.trim()) {
    // A password-only step: the account is the one typed into this site's first step, if that was recent.
    const seen = await sessionGet<SeenUser>(`seenUser:${tabId}`);
    if (seen && seen.origin === origin && Date.now() - seen.at <= SEEN_USER_TTL_MS) username = seen.username;
  }
  // (A user name taken from the step before keeps this capture's `byUser`: what that step's page put in its box
  // could be changed by the page during the user's own Enter or click on this step too.)
  const pending: Pending = { id: crypto.randomUUID(), origin, username, password, tabId, at: Date.now(), byUser };
  await chrome.storage.session.set({ [`pending:${tabId}`]: pending });
  return true;
}

/**
 * The first step of a two-step sign-in was sent: remember the account name for the password step
 * that follows. It is page input, so it gets the checks a capture gets, and it is kept only in
 * session storage, for this tab and the frame's own site.
 */
async function seenUser(sender: chrome.runtime.MessageSender, username: unknown): Promise<void> {
  if (!fromContentScript(sender) || sender.tab.id === undefined) return;
  if (typeof username !== "string" || username.length > MAX_CAPTURE_CHARS || !username.trim()) return;
  const record: SeenUser = { origin: frameOrigin(sender), username, at: Date.now() };
  await chrome.storage.session.set({ [`seenUser:${sender.tab.id}`]: record });
}

/**
 * Asks the app what saving the captured login would do. Only on a connection that is already up:
 * a page load never connects or starts the app. `null` when it cannot be known (locked, not
 * connected, no answer, or an app older than `checkLogin`): the offer is made as before. Asked only for a capture
 * the user sent (`Pending.byUser`).
 */
async function savedState(pending: Pending): Promise<"new" | "update" | "unchanged" | null> {
  if (broker.state.phase === "connecting") {
    // A connection being made (a sign-in the user sent started it): wait for it a little. This
    // never starts one: `whenReady` only joins the connection that is already being opened.
    await Promise.race([broker.whenReady().catch(() => {}), new Promise((r) => setTimeout(r, CONNECTING_WAIT_MS))]);
  }
  if (broker.state.phase !== "ready") return null;
  try {
    const answer = await Promise.race([
      request<{ status?: unknown }>("checkLogin", { origin: pending.origin, username: pending.username, password: pending.password }),
      new Promise<null>((resolve) => setTimeout(() => resolve(null), CHECK_LOGIN_TIMEOUT_MS)),
    ]);
    const status = answer?.status;
    return status === "new" || status === "update" || status === "unchanged" ? status : null;
  } catch {
    return null;
  }
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

/** One offer at a time per tab: the app is asked between reading and marking a capture. */
const offering = new Map<number, Promise<unknown>>();

/** The top page of a tab asks whether there is a capture to offer: it learns only its id, which it passes to `decide`. */
async function pendingSave(sender: chrome.runtime.MessageSender): Promise<PendingSaveRef | null> {
  if (!fromContentScript(sender) || sender.tab.id === undefined || sender.frameId !== 0) return null;
  const tabId = sender.tab.id;
  const turn = (offering.get(tabId) ?? Promise.resolve()).catch(() => {}).then(() => offerPending(tabId));
  offering.set(tabId, turn);
  void turn.finally(() => offering.get(tabId) === turn && offering.delete(tabId)).catch(() => {});
  const pending = await turn;
  return pending ? { id: pending.id } : null;
}

async function offerPending(tabId: number): Promise<PendingSave | null> {
  let pending = await pendingFor(tabId);
  if (!pending) return null;
  // Every page of the tab gets the capture and asks for the decision; the background opens the panel by
  // itself only for the first (`offered`), later pages take the open decision over (decide.ts).
  let learned = false;
  // Only a sign-in the user sent is checked with the app (see `Pending.byUser`): anything else is offered as it is.
  if (pending.update === undefined && pending.byUser === true) {
    // Only a new login or a changed password is offered: one the app has already is dropped.
    const state = await savedState(pending);
    if (state === "unchanged") {
      await dropPending(tabId, pending.id);
      return null;
    }
    // The capture may have been answered or replaced while the app was asked.
    const now = await pendingFor(tabId);
    if (!now || now.id !== pending.id) return null;
    // Asked once per capture; a later page load of the same capture reuses the answer.
    if (state) {
      pending = { ...now, update: state === "update" };
      learned = true;
    }
  }
  if (learned) await chrome.storage.session.set({ [`pending:${tabId}`]: pending });
  return shownPending(pending);
}

/** What the decision panel learns about a capture (never the password; the page learns only the id). */
function shownPending(pending: Pending): PendingSave {
  const shown: PendingSave = { id: pending.id, origin: pending.origin, username: pending.username };
  if (pending.update) shown.update = true;
  return shown;
}

/** Removes the tab's capture if it is still this one. */
async function dropPending(tabId: number, id: string) {
  const now = await sessionGet<Pending>(`pending:${tabId}`);
  if (now?.id === id) await chrome.storage.session.remove(`pending:${tabId}`);
}

async function pendingInfo(tabId: number, id: string): Promise<PendingSaveInfo | null> {
  const pending = await pendingFor(tabId);
  return pending && pending.id === id ? decisionView(pending) : null;
}

/** What the decisions learn about a capture: what the panel shows, its life, and whether it was offered. */
function decisionView(pending: Pending): PendingSaveInfo {
  return { ...shownPending(pending), expiresAt: pending.at + PENDING_TTL_MS, offered: pending.offered, decisionId: pending.decisionId };
}

/** The capture was offered by this decision (in the per-tab queue, so it never races `offerPending`'s write). */
function markOffered(tabId: number, id: string, decisionId: string): Promise<void> {
  const turn = (offering.get(tabId) ?? Promise.resolve())
    .catch(() => {})
    .then(async () => {
      const now = await sessionGet<Pending>(`pending:${tabId}`);
      if (now?.id === id) await chrome.storage.session.set({ [`pending:${tabId}`]: { ...now, offered: true, decisionId } });
    });
  offering.set(tabId, turn);
  void turn.finally(() => offering.get(tabId) === turn && offering.delete(tabId)).catch(() => {});
  return turn;
}

/** The captures whose save offer is open (after a restart of the background, decide.ts lets them wait again). */
async function offeredCaptures(): Promise<{ tabId: number; pending: PendingSaveInfo }[]> {
  const stored = await chrome.storage.session.get(null);
  const out: { tabId: number; pending: PendingSaveInfo }[] = [];
  for (const [key, value] of Object.entries(stored)) {
    if (!key.startsWith("pending:")) continue;
    const pending = value as Pending;
    if (pending?.offered && Date.now() - pending.at <= PENDING_TTL_MS) out.push({ tabId: pending.tabId, pending: decisionView(pending) });
  }
  return out;
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
  markOffered,
  savePending: savePendingFor,
  dismissPending: dismissPendingFor,
});

/** The frame a decision is for, as the browser reports it: only a page's content script asks for one. */
function asker(sender: chrome.runtime.MessageSender, fromPage: boolean) {
  if (!fromPage || sender.tab?.id === undefined) throw new BrokerError("forbidden", "not available here");
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
    // ---- a page's content script ----
    case "findLogins":
      return siteStatus(sender, message.interactive);
    case "capture":
      return capture(sender, message.username, message.password, message.byUser);
    case "seenUser":
      return seenUser(sender, message.username);
    case "pendingSave":
      return pendingSave(sender);
    case "passkeysEnabled":
      return passkeysEnabled();
    case "decide":
      return decisions.open(asker(sender, fromPage), message.ref, message.request);
    case "decideCancel":
      return decisions.withdraw(asker(sender, fromPage), message.ref);
    case "decidePing":
      return decisions.ping(asker(sender, fromPage), message.ref);
    case "decisionInfo":
      return decisions.info(sender, message.id);
    case "decisionLogins":
      return decisions.logins(sender, message.id);
    case "decisionCandidates":
      return decisions.candidates(sender, message.id);
    case "decisionAnswer":
      return decisions.answer(sender, message.id, message.answer);
    case "decisionUnlock":
      return decisions.unlock(sender, message.id);
    // ---- the popup ----
    case "unlock":
      popupOnly();
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
    case "fillTab":
      popupOnly();
      return fillTab(message.tabId, message.itemId, message.force);
    case "getPrivacy":
      popupOnly();
      return privacy();
    case "setPrivacy":
      popupOnly();
      return setPrivacy(message.disableBrowserManager);
  }
  // Anything else (also the messages the in-page controls of earlier versions sent: fill, savePending,
  // dismissPending, passkeyCandidates, passkeyCreate, passkeyAssert, generate) is refused.
  throw new BrokerError("forbidden", "unknown request");
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
  void chrome.storage.session.remove([`pending:${tabId}`, `filled:${tabId}`, `seenUser:${tabId}`, `decidePanel:${tabId}`]);
});
// The decisions' listeners, registered at once at the top level so a restarted worker hears the events.
chrome.windows.onRemoved.addListener((windowId) => decisions.onWindowRemoved(windowId));
chrome.windows.onFocusChanged.addListener((windowId) => decisions.onFocusChanged(windowId));
chrome.tabs.onUpdated.addListener((tabId, change) => decisions.onTabUpdated(tabId, change));
chrome.tabs.onActivated.addListener((info) => decisions.onTabActivated(info));
// The decision UI's port reports dismissal; its periodic messages keep the worker (or event page) alive.
chrome.runtime.onConnect.addListener((port) => void decisions.attach(port));
// The worker may have been stopped while a decision was open (Chrome stops an idle service worker, Firefox
// suspends an idle event page): no tab keeps a panel of a forgotten decision, and a save that was offered and is
// still valid waits again.
void decisions.restore(offeredCaptures);
