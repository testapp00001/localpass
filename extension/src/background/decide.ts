// Every decision a page can lead to (choose a login, generate a password for a new-password form, save a
// login, create a passkey, sign in with one) is made in extension-owned UI, in every browser: the toolbar
// panel where the browser can open it without a click (`action.openPopup`: Chrome and Edge 127 and newer,
// Firefox 149 and newer), otherwise a separate small window. Nothing that acts is drawn into a page; the
// field's badge only asks for the panel (content/ui.ts).
//
// Why: a page decides what is drawn around, over and under its own content, so it could hide, cover or
// restyle a control drawn into it and trick a click into filling, saving or signing. The panel and the
// window are extension pages: page CSS and script cannot reach into them, and their clicks are real.
//
// Who is trusted with what:
// * The site comes from the browser's report for the asking frame (`Asker`, built by index.ts from
//   `sender`), never from the request. For a save it is the site of the capture, which the browser
//   reported when the sign-in was sent. The panel only shows it and answers by decision id. What the
//   request carries (passkey parameters, the new-password hint) is page input and checked again here.
// * Only the UI this module opened may answer: exact URL and request id, plus the created window
//   id, or for a panel no `sender.tab`/`frameId` and the decision's tab active in its focused
//   window (with the site unchanged, except for a save: see `checkPanelTab`). Chrome reports the window
//   as not focused while its own panel has the focus: see `inFront`.
// * Secrets go to the asking content script as the reply to its own `decide` message, not to the
//   panel: the panel learns whether it worked, nothing more.
// * One decision per tab (a newer request replaces the older one), eight in all.
//
// Lifetimes in the panel (a window keeps the old rules: closing it is "no"):
// * Choosing a login: closing the panel cancels.
// * Saving, and passkey requests, are "pendable": closing the panel leaves them open, with a badge
//   on the tab's toolbar icon, whose popup stays this decision's panel until it ends, so a click on
//   the icon shows it again. They end by an answer in the panel (Not now, Never, Esc, Cancel, Use
//   another method), the page withdrawing (passkeys), a navigation (passkeys), the tab closing, a
//   newer request from the tab, or the end of their life (`decisionTiming.lifeMs`; a save lives as
//   long as its capture).
// * While an answer is carried out or the app's Unlock is on screen (both may move the focus to the
//   app, which closes the panel), nothing is cancelled by a closing panel or a focus change. After
//   an Unlock asked from the panel, the panel opens again by itself once the tab is in front again.
//   Chrome on Wayland may keep the panel open behind the app until the Unlock is over and close it only
//   then (when its window is in front again, or at the user's next click into it): there, a panel that
//   closes after a successful Unlock, before the user answered in it, counts as closed during the Unlock
//   (`graceUntil`), within the same reopen time.
// * The panel opens by itself once per request (and once more after that Unlock); after that only
//   the user's click on the toolbar icon (or the shortcut) shows it. A panel the background opened
//   itself holds its acting controls for a moment; one the user opened does not (`ownOpens`), except
//   on the decision's first showing (`shown`): the page chose that moment. The normal
//   toolbar popup is told the same way, since an own `openPopup` can show it (`attachPopup`).

import type {
  DecisionAnswer,
  DecisionDone,
  DecisionInfo,
  DecisionRequest,
  FindLoginsReply,
  PanelNote,
  PendingSave,
} from "../shared/messages";
import { POPUP_PORT } from "../shared/messages";
import type { ChooseOutcome, PasskeyGetParams } from "../shared/messages";
import { BrokerError, type LoginFill, type PasskeyCandidate, type SaveOutcome } from "../shared/protocol";
import { saveMessage } from "../shared/shown";
import { checkCreate, checkGet } from "../content/validate";
import * as passkeys from "./passkeys";

/** A capture as the decisions see it: what the panel may show, plus its life and whether it was offered. */
export interface PendingSaveInfo extends PendingSave {
  /** When the capture expires (ms since the epoch). */
  expiresAt: number;
  /** The panel (or window) was opened by itself for this capture already. */
  offered?: boolean;
  /** The decision that offered it (kept so a panel survives a restart of the background). */
  decisionId?: string;
}

export interface Deps {
  request<T>(op: string, params?: Record<string, unknown>): Promise<T>;
  findLogins(origin: string): Promise<FindLoginsReply>;
  rememberFill(tabId: number, fill: LoginFill): Promise<void>;
  pendingInfo(tabId: number, id: string): Promise<PendingSaveInfo | null>;
  /** Records that the capture was offered by this decision (so it is not offered by itself again). */
  markOffered(tabId: number, id: string, decisionId: string): Promise<void>;
  savePending(tabId: number, id: string): Promise<SaveOutcome>;
  dismissPending(tabId: number, id: string, never: boolean): Promise<void>;
}

/** The frame that asked, as the browser reported it. */
export interface Asker {
  tabId: number;
  frameId: number;
  origin: string;
}

/** Timing rules; tests shorten them. */
export const decisionTiming = {
  /** How long a passkey request waits without a panel showing (the request's own life is the page's). */
  lifeMs: 2 * 60_000,
  /** After an Unlock asked from the panel: how long the panel waits for the tab to be in front again to reopen. */
  reopenMs: 60_000,
  /** After a login was fetched: how long it waits for its tab to be in front again (the app had the focus). */
  refocusMs: 60_000,
  /** How often that wait looks. */
  pollMs: 250,
  /**
   * Chrome: after the browser reports that the focus left its windows, how long to look again for this
   * decision's own panel before taking it as the user leaving (the panel may still be opening).
   */
  settleMs: 300,
};

/** The most decisions open at once (across tabs). */
const MAX_OPEN = 8;
/** How long a result stays on screen before the window or panel closes. */
const LINGER_MS = 1800;
const PAGE = "decide.html";
/** What the toolbar icon of a tab with a waiting save or passkey request shows. */
const BADGE = "1";
/** `windows.WINDOW_ID_NONE`: the focus went to something that is not a browser window (the app). */
const NO_WINDOW = -1;
/** Session-storage key that remembers which tab's popup a decision changed (cleaned up after a restart). */
const PANEL_KEY = "decidePanel:";

const ANSWERS: Record<DecisionRequest["decision"], DecisionAnswer["action"][]> = {
  passkeyCreate: ["create", "fallback", "cancel"],
  passkeyGet: ["assert", "fallback", "cancel"],
  saveLogin: ["save", "never", "cancel"],
  chooseLogin: ["fill", "generate", "assert", "cancel"],
};

/** Whether an answer fits this request: "generate" only for a new-password form, "assert" in a pick only with passkeys. */
function fits(spec: DecisionRequest, answer: DecisionAnswer | undefined): boolean {
  if (!answer || !ANSWERS[spec.decision].includes(answer.action)) return false;
  if (spec.decision !== "chooseLogin") return true;
  if (answer.action === "generate") return spec.newPassword === true;
  if (answer.action === "assert") return !!spec.passkeys;
  return true;
}

/**
 * A content script's request, checked again here: it runs next to the page, so its message is page input.
 * Only the known shapes and fields pass, with the limits `content/validate.ts` applies to the page's own call
 * (and the relying party id must fit the frame's site, which the browser reported).
 */
export function checkRequest(spec: unknown, origin: string): DecisionRequest {
  const bad = () => new BrokerError("invalid", "that request is not valid");
  if (!spec || typeof spec !== "object") throw bad();
  const s = spec as Record<string, unknown>;
  const host = new URL(origin).hostname;
  const get = (value: unknown): PasskeyGetParams => {
    const p = checkGet(value, host);
    if (!p) throw bad();
    return { rpId: p.rpId, allow: p.allow, challenge: p.challenge, uv: p.uv };
  };
  switch (s.decision) {
    case "passkeyCreate": {
      const request = checkCreate(s.request, host);
      if (!request) throw bad();
      return { decision: "passkeyCreate", request };
    }
    case "passkeyGet":
      return { decision: "passkeyGet", ...get(s) };
    case "saveLogin":
      if (typeof s.id !== "string" || !s.id || s.id.length > 64) throw bad();
      return { decision: "saveLogin", id: s.id };
    case "chooseLogin": {
      const out: Extract<DecisionRequest, { decision: "chooseLogin" }> = { decision: "chooseLogin" };
      if (s.newPassword === true) out.newPassword = true;
      if (s.passkeys !== undefined) out.passkeys = get(s.passkeys);
      return out;
    }
  }
  throw bad();
}

interface Decision extends Asker {
  id: string;
  ref: string;
  spec: DecisionRequest;
  username: string;
  /** A save that would change the password of a saved login (the app said so when it was offered). */
  update: boolean;
  windowId: number | undefined;
  windowReady: Promise<void>;
  panel: boolean;
  /** The panel or window page that is showing this decision now, if any. */
  port?: chrome.runtime.Port;
  /**
   * A panel page has attached to this decision once already: the user has seen it. Until then even a panel the user
   * opened is held (see `attach`).
   */
  shown: boolean;
  resolve(value: unknown): void;
  reject(error: unknown): void;
  settled: boolean;
  /** An answer is being carried out. */
  busy: boolean;
  /** The app's Unlock, asked from the panel, is on screen. */
  unlocking?: Promise<boolean>;
  /** The panel is being opened (`openPanel`); a focus change during that is the panel taking the focus. */
  opening?: Promise<void>;
  /** After that Unlock: reopen the panel by itself until this time (0: no). */
  reopenUntil: number;
  /**
   * That Unlock worked while the panel was still showing (Chrome on Wayland closes it only afterwards): until this
   * time, the panel closing by itself (no answer given in it, not hidden by this background) arms the same reopen as
   * a panel closed during the Unlock (0: no). Ends with the reopen time, at the first answer, when the background hides
   * the panel, and when the panel closes (then the reopen takes over, once). Only set while a panel is attached, and no
   * second panel attaches then.
   */
  graceUntil: number;
  /** A pendable decision ends when it is past this time and no panel shows it. */
  deadline: number;
  expired: boolean;
  timers: ReturnType<typeof setTimeout>[];
  /** The last thing that went wrong while carrying out an answer: what the page hears if the user gives up. */
  lastError: BrokerError | undefined;
}

const cancelled = () => new BrokerError("cancelled", "The user cancelled the request.");
const notActive = () => new BrokerError("cancelled", "The page is no longer active. Try again from the login field.");
const expiredError = () => new BrokerError("cancelled", "The request timed out.");
const openError = (e: unknown) => new BrokerError("error", `Could not open the LocalPass picker. Use the LocalPass toolbar icon to pick a login. (${String(e)})`);
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Saves and passkey requests wait (with a badge) when their panel closes; choosing a login does not. */
const pendable = (d: Decision) => d.spec.decision !== "chooseLogin";

function originOf(url: string | undefined): string | null {
  try {
    const origin = new URL(url ?? "").origin;
    return origin === "null" ? null : origin;
  } catch {
    return null;
  }
}

export class Decisions {
  private byId = new Map<string, Decision>();
  // Serialize popup changes so cleanup of a replaced request cannot erase its successor.
  private popupQueue: Promise<unknown> = Promise.resolve();
  /** Set while decisions are restored after the background restarted: everything else waits for it. */
  private ready: Promise<unknown> = Promise.resolve();
  /**
   * Per browser window: how many of this background's own `action.openPopup` calls have not been matched by a
   * panel attaching in that window yet. A panel that attaches while this is 0 was opened by the user (toolbar icon or
   * shortcut; the browser tells the background about neither while a popup is set, measured in Firefox 160), and
   * skips the hold. No time window: the panel attaches 20-100 ms after the call, after `openPopup` resolved in Firefox
   * (measured), and a busy background could stretch any window. A call that failed keeps its count too: nothing says
   * a failed call never showed anything, so the next opening in that window is held once (0.6 s, the safe side), as
   * after a panel that closed before attaching.
   *
   * The normal toolbar popup takes a count the same way (`attachPopup`): the browser may reset a tab's popup between
   * `setToolbar` and `openPopup` (Chrome does on every cross-document navigation, measured in 8.3b), and a panel whose
   * request is gone replaces itself with the popup (`toPopup` in decide/index.ts), so an `openPopup` of this
   * background can show the normal popup at a moment a page chose. Such a popup is held and focuses no Fill.
   */
  private ownOpens = new Map<number, number>();

  constructor(private deps: Deps) {}

  private popupChange<T>(action: () => Promise<T>): Promise<T> {
    const next = this.popupQueue.then(action);
    this.popupQueue = next.catch(() => {});
    return next;
  }

  private panelURL(d: Decision): string {
    return chrome.runtime.getURL(`${PAGE}?id=${d.id}&presentation=panel`);
  }

  /** The toolbar popup the manifest names (what a tab shows when no decision is open). */
  private defaultPopup(): string {
    const manifest = chrome.runtime.getManifest?.() as { action?: { default_popup?: string } } | undefined;
    return manifest?.action?.default_popup ?? "popup.html";
  }

  /**
   * Chrome, Edge and the other Chromium browsers: their toolbar panel takes the keyboard focus from the browser
   * window (Firefox's does not). Told apart by `runtime.getBrowserInfo`, which only Firefox has.
   */
  private chromium(): boolean {
    return typeof (chrome.runtime as { getBrowserInfo?: unknown }).getBrowserInfo !== "function";
  }

  /**
   * Whether the toolbar panel can be opened without a click: Chrome and Edge allow `action.openPopup` for every
   * extension from 127 on (the manifest's `minimum_chrome_version`), Firefox from 149 on.
   */
  private async panelSupported(): Promise<boolean> {
    const action = (chrome as { action?: { openPopup?: unknown } }).action;
    if (typeof action?.openPopup !== "function") return false;
    if (this.chromium()) return true;
    const runtime = chrome.runtime as typeof chrome.runtime & { getBrowserInfo: () => Promise<{ version: string }> };
    const info = await runtime.getBrowserInfo();
    return Number.parseInt(info.version, 10) >= 149;
  }

  /**
   * Chrome: this decision's own panel is open, in the browser window that had the focus last (its window).
   * Chrome opens a toolbar panel only in the focused window and closes it as soon as it loses the focus, so an
   * open panel means the user is in that window, in the panel. `runtime.getContexts` names the panel's exact
   * address (with this decision's id); it reports no window for a panel (-1, measured in Chrome 157), so the
   * window comes from `windows.getLastFocused`, which keeps naming the browser window while its panel has the focus.
   */
  private async panelShown(d: Decision): Promise<boolean> {
    if (!d.panel || d.windowId === undefined) return false;
    const runtime = chrome.runtime as { getContexts?: (filter: { contextTypes: string[] }) => Promise<{ documentUrl?: string }[]> };
    if (typeof runtime.getContexts !== "function") return false;
    const url = this.panelURL(d);
    const contexts = await runtime.getContexts({ contextTypes: ["POPUP"] }).catch(() => []);
    if (!contexts.some((c) => c.documentUrl === url)) return false;
    const last = await chrome.windows.getLastFocused().catch(() => null);
    return last?.id === d.windowId;
  }

  /**
   * The tab is the active tab of the decision's window, and that window has the focus. In Chrome the window
   * reports no focus while its toolbar panel has it (measured in Chrome 157: `windows.get().focused` is false,
   * and `onFocusChanged` reports `WINDOW_ID_NONE`): there, this decision's own panel being open in that
   * window counts as the window having the focus (`panelShown`). Firefox keeps the plain rule.
   */
  private async inFront(d: Decision, tab: chrome.tabs.Tab): Promise<boolean> {
    if (!tab.active || tab.windowId !== d.windowId) return false;
    if ((await chrome.windows.get(tab.windowId)).focused) return true;
    return this.chromium() && (await this.panelShown(d));
  }

  /**
   * The decision's tab is the active tab of its focused browser window (see `inFront`), and (except for a
   * save) still shows the site the decision is for.
   *
   * Saving is the one deliberate exception to the site check: a sign-in is followed by redirects, often
   * to another address of the site, and the offer must survive them. A save sends nothing to the page:
   * it stores the capture (whose site the browser reported when the sign-in was sent) in the app, and
   * the panel shows that site, not the tab's. So for a save only the tab and its focus are checked.
   */
  private async checkPanelTab(d: Decision): Promise<void> {
    const tab = await chrome.tabs.get(d.tabId);
    if (d.settled || !(await this.inFront(d, tab))) throw notActive();
    if (d.spec.decision !== "saveLogin" && originOf(tab.url) !== d.origin) throw notActive();
  }

  /** Points the tab's toolbar popup at this decision, with a badge when it may wait there. */
  private async setToolbar(d: Decision): Promise<void> {
    const url = this.panelURL(d);
    await chrome.storage.session.set({ [`${PANEL_KEY}${d.tabId}`]: url }).catch(() => {});
    await chrome.action.setPopup({ tabId: d.tabId, popup: url });
    if (pendable(d)) await chrome.action.setBadgeText({ tabId: d.tabId, text: BADGE });
  }

  /** Gives the tab its normal popup back, if the popup is still this decision's (a successor's is left alone). */
  private async clearToolbar(tabId: number, url: string): Promise<void> {
    const key = `${PANEL_KEY}${tabId}`;
    const stored = await chrome.storage.session.get(key).catch(() => ({}) as Record<string, unknown>);
    if ((stored as Record<string, unknown>)[key] === url) await chrome.storage.session.remove(key).catch(() => {});
    const mine = (await chrome.action.getPopup({ tabId })) === url;
    if (mine) await chrome.action.setPopup({ tabId, popup: this.defaultPopup() });
    // The badge belongs to whichever decision the tab has now: cleared when that is none.
    if (mine || ![...this.byId.values()].some((o) => o.tabId === tabId)) await chrome.action.setBadgeText({ tabId, text: "" });
  }

  /** Opens the tab's panel showing this decision (the tab must be in front). */
  private openPanel(d: Decision): Promise<void> {
    const opening = this.popupChange(async () => {
      if (d.settled || d.port) return;
      await this.checkPanelTab(d);
      await this.setToolbar(d);
      await this.checkPanelTab(d);
      const windowId = d.windowId as number;
      // Counted before the call: the panel can attach before `openPopup` returns (measured in Chrome).
      this.ownOpens.set(windowId, (this.ownOpens.get(windowId) ?? 0) + 1);
      // A failed call keeps its count (see `ownOpens`).
      await chrome.action.openPopup({ windowId });
    });
    d.opening = opening.catch(() => {});
    return opening;
  }

  /** Takes back one of the background's own openings in this window; true if there was one (the panel is held). */
  private takeOwnOpen(windowId: number | undefined): boolean {
    const count = windowId === undefined ? 0 : (this.ownOpens.get(windowId) ?? 0);
    if (count <= 0) return false;
    if (count === 1) this.ownOpens.delete(windowId as number);
    else this.ownOpens.set(windowId as number, count - 1);
    return true;
  }

  /** `auto`: open the panel (or window) by itself; otherwise the decision only waits for the toolbar icon. */
  private async present(d: Decision, auto: boolean): Promise<void> {
    d.panel = await this.panelSupported();
    if (d.settled) return;
    if (d.spec.decision === "saveLogin" && auto) await this.deps.markOffered(d.tabId, d.spec.id, d.id).catch(() => {});
    if (!d.panel) {
      // A window cannot wait unseen: a capture that was offered once is not offered again.
      if (!auto) throw new BrokerError("notFound", "this login was offered already");
      const w = await chrome.windows.create({ url: chrome.runtime.getURL(`${PAGE}?id=${d.id}`), type: "popup", width: 440, height: 600 });
      if (w?.id === undefined) throw new Error("the window has no id");
      d.windowId = w.id;
      return;
    }
    const tab = await chrome.tabs.get(d.tabId);
    d.windowId = tab.windowId;
    if (!pendable(d)) {
      // A login pick needs its page in front from the start, and ends when the panel cannot open.
      await this.popupChange(async () => {
        await this.checkPanelTab(d);
        if (d.settled) return;
        await this.setToolbar(d);
      });
      await this.openPanel(d);
      return;
    }
    d.timers.push(setTimeout(() => this.expire(d), Math.max(0, d.deadline - Date.now())));
    await this.popupChange(async () => {
      if (!d.settled) await this.setToolbar(d);
    });
    // Not in front, or the panel could not open (another one is open, the window has no focus): the request
    // waits with the badge instead of failing.
    if (auto) await this.openPanel(d).catch(() => {});
  }

  private create(asker: Asker, ref: string, spec: DecisionRequest, extra: { id: string; username: string; update: boolean; deadline: number }) {
    let d!: Decision;
    const outcome = new Promise<unknown>((resolve, reject) => {
      d = {
        ...asker,
        ...extra,
        ref,
        spec,
        windowId: undefined,
        windowReady: Promise.resolve(),
        panel: false,
        shown: false,
        resolve,
        reject,
        settled: false,
        busy: false,
        reopenUntil: 0,
        graceUntil: 0,
        expired: false,
        timers: [],
        lastError: undefined,
      };
    });
    this.byId.set(d.id, d);
    return { d, outcome };
  }

  private start(d: Decision, auto: boolean) {
    d.windowReady = this.present(d, auto);
    // (A request that ends while its window is still opening closes the window in `close`.)
    d.windowReady.catch((e) => this.finish(d, { error: e instanceof BrokerError && e.code === "notFound" ? e : openError(e) }));
  }

  /** Opens protected UI and resolves with the outcome (or rejects with the reason it did not happen). */
  async open(asker: Asker, ref: string, request: unknown): Promise<unknown> {
    await this.ready;
    const spec = checkRequest(request, asker.origin);
    let username = "";
    let update = false;
    let origin = asker.origin;
    let deadline = Date.now() + decisionTiming.lifeMs;
    let offered = false;
    let knownId: string | undefined;
    if (spec.decision === "saveLogin") {
      if (asker.frameId !== 0) throw new BrokerError("forbidden", "only the top page may offer to save a login");
      const pending = await this.deps.pendingInfo(asker.tabId, spec.id);
      if (!pending) throw new BrokerError("notFound", "there is nothing to save any more");
      // The page after a sign-in (and every page after it) asks again for the same capture: it takes
      // the open decision over (and keeps the background awake for it), it does not open a new one.
      const same = [...this.byId.values()].find((o) => o.tabId === asker.tabId && o.spec.decision === "saveLogin" && o.spec.id === spec.id);
      if (same) return this.adopt(same, asker, ref);
      username = pending.username;
      update = !!pending.update;
      // The site the capture was made on, as the browser reported it then (the tab may show another page by now).
      origin = pending.origin;
      deadline = pending.expiresAt;
      offered = !!pending.offered;
      knownId = pending.decisionId;
    }
    // One decision per tab (and per page request): the newer request wins.
    for (const old of [...this.byId.values()]) {
      if (old.tabId === asker.tabId) this.cancel(old, new BrokerError("cancelled", "Replaced by a newer request."));
    }
    if (this.byId.size >= MAX_OPEN) throw new BrokerError("tooMany", "too many LocalPass windows are open");
    // A capture offered before keeps its decision id: a panel the toolbar still shows for it keeps working.
    const id = (offered && knownId && !this.byId.has(knownId) && knownId) || crypto.randomUUID();
    const { d, outcome } = this.create({ ...asker, origin }, ref, spec, { id, username, update, deadline });
    this.start(d, !offered);
    return outcome;
  }

  /** A newer page of the same tab asks for the save that is open: its request now gets the outcome. */
  private adopt(d: Decision, asker: Asker, ref: string): Promise<unknown> {
    // The earlier page is usually gone; if not, it hears that it was replaced (a save tells a page nothing).
    d.reject(new BrokerError("cancelled", "Taken over by a newer page."));
    const outcome = new Promise<unknown>((resolve, reject) => {
      d.resolve = resolve;
      d.reject = reject;
    });
    d.ref = ref;
    d.frameId = asker.frameId;
    if (d.panel) void this.popupChange(async () => void (d.settled || (await this.setToolbar(d)))).catch(() => {});
    return outcome;
  }

  /**
   * After the background restarted: give every tab whose popup a forgotten decision changed its normal
   * popup back, then let the saves that were offered and are still valid wait again (same id, no panel
   * by itself). Everything else waits for this.
   */
  restore(captures: () => Promise<{ tabId: number; pending: PendingSaveInfo }[]>): Promise<unknown> {
    this.ready = (async () => {
      const stored = await chrome.storage.session.get(null);
      for (const [key, url] of Object.entries(stored)) {
        if (!key.startsWith(PANEL_KEY) || typeof url !== "string") continue;
        const tabId = Number(key.slice(PANEL_KEY.length));
        await this.popupChange(() => this.clearToolbar(tabId, url)).catch(() => chrome.storage.session.remove(key).catch(() => {}));
      }
      for (const { tabId, pending } of await captures()) await this.resume(tabId, pending).catch(() => {});
    })().catch(() => {});
    return this.ready;
  }

  private async resume(tabId: number, pending: PendingSaveInfo): Promise<void> {
    if (!pending.offered || !pending.decisionId || pending.expiresAt <= Date.now()) return;
    if (!(await this.panelSupported())) return;
    if (!(await chrome.tabs.get(tabId).catch(() => null))) return;
    if ([...this.byId.values()].some((o) => o.tabId === tabId) || this.byId.size >= MAX_OPEN || this.byId.has(pending.decisionId)) return;
    const { d, outcome } = this.create({ tabId, frameId: 0, origin: pending.origin }, "", { decision: "saveLogin", id: pending.id }, {
      id: pending.decisionId,
      username: pending.username,
      update: !!pending.update,
      deadline: pending.expiresAt,
    });
    outcome.catch(() => {}); // nobody asked yet: the next page of the tab takes it over
    this.start(d, false);
  }

  /** The page withdrew its request (an AbortSignal, or a newer call). */
  async withdraw(asker: Asker, ref: string) {
    await this.ready;
    for (const d of [...this.byId.values()]) {
      if (d.tabId === asker.tabId && d.frameId === asker.frameId && d.ref === ref) this.cancel(d, new BrokerError("cancelled", "The request was withdrawn."));
    }
  }

  /** The asking content script's sign of life while its request is open (it keeps the background awake). */
  async ping(asker: Asker, ref: string): Promise<boolean> {
    await this.ready;
    return [...this.byId.values()].some((d) => d.tabId === asker.tabId && d.frameId === asker.frameId && d.ref === ref);
  }

  onWindowRemoved(windowId: number) {
    for (const d of [...this.byId.values()]) if (d.windowId === windowId) this.cancel(d);
  }

  onTabRemoved(tabId: number) {
    for (const d of [...this.byId.values()]) if (d.tabId === tabId) this.cancel(d);
  }

  onTabUpdated(tabId: number, change: { url?: string; status?: string }) {
    const navigated = !!change.url || change.status === "loading";
    if (!navigated && change.status !== "complete") return;
    for (const d of [...this.byId.values()]) {
      if (!d.panel || d.tabId !== tabId) continue;
      if (d.spec.decision === "saveLogin") {
        // A save outlives the redirects after a sign-in. The browser may reset a tab's popup and badge when
        // it loads a new page: set them again.
        void this.popupChange(async () => void (d.settled || (await this.setToolbar(d)))).catch(() => {});
      } else if (navigated) {
        this.cancel(d);
      }
    }
  }

  onTabActivated(info: { tabId: number; windowId: number }) {
    for (const d of [...this.byId.values()]) {
      if (!d.panel) continue;
      if (d.tabId === info.tabId) {
        void this.tryReopen(d);
        continue;
      }
      if (d.windowId !== info.windowId || d.unlocking) continue;
      // A login is never handed to a page the user left, even while it is being fetched.
      if (d.spec.decision === "chooseLogin") this.cancel(d);
      else if (!d.busy) this.hide(d);
    }
  }

  onFocusChanged(windowId: number) {
    for (const d of [...this.byId.values()]) {
      if (!d.panel) continue;
      if (windowId === d.windowId) {
        void this.tryReopen(d);
        continue;
      }
      // The app's own prompts (Unlock, Hello, Replace?) take the focus: that is not the user leaving.
      if (d.busy || d.unlocking) continue;
      if (d.reopenUntil && windowId === NO_WINDOW) continue;
      // Chrome reports "no window" when its toolbar panel takes the focus: only the user leaving counts.
      if (windowId === NO_WINDOW && this.chromium()) {
        void this.leftUnlessPanel(d);
        continue;
      }
      this.left(d);
    }
  }

  /** The user left the decision's tab or window: a login pick ends, a save or passkey request waits (hidden). */
  private left(d: Decision) {
    if (d.spec.decision === "chooseLogin") this.cancel(d);
    else this.hide(d);
  }

  /**
   * Chrome: the focus left the browser windows. When it went into this decision's own panel (opening now, or
   * open in its window) the user is still deciding; otherwise they left. A panel that closes later (Chrome
   * closes it when it loses the focus) ends or pends the decision through its port anyway.
   */
  private async leftUnlessPanel(d: Decision): Promise<void> {
    await d.opening;
    if (await this.panelShown(d)) return;
    await sleep(decisionTiming.settleMs);
    if (await this.panelShown(d)) return;
    // A panel that outlived a successful Unlock may close now, which reopens it (`graceUntil`): not the user leaving.
    if (d.settled || d.busy || d.unlocking || d.reopenUntil || this.inGrace(d)) return;
    this.left(d);
  }

  /** The UI's port: closing a window, or a panel showing a login pick, is "no"; a closing panel leaves the others waiting. */
  async attach(port: chrome.runtime.Port): Promise<void> {
    if (port.name.startsWith(POPUP_PORT)) return this.attachPopup(port);
    const id = port.name.startsWith("decide:") ? port.name.slice("decide:".length) : "";
    let disconnected = false;
    let attached: Decision | undefined;
    port.onDisconnect.addListener(() => {
      disconnected = true;
      if (attached && attached.port === port) {
        attached.port = undefined;
        this.panelClosed(attached);
      }
    });
    try {
      const d = await this.authorize(port.sender, id);
      // One page at a time: a second panel or page never attaches to a decision that has one.
      if (d.port) throw new BrokerError("forbidden", "the picker is already attached");
      if (disconnected) return this.panelClosed(d);
      attached = d;
      d.port = port;
      d.reopenUntil = 0;
      // A toolbar panel the background did not open itself: the user opened it, and it acts without the hold, but not
      // on the decision's first showing. The page chose when to ask, and while a request waits for the toolbar icon (its
      // own opening failed: the tab was not in front, another panel was open) the user's click on the icon or the
      // shortcut shows the page's request instead of the normal popup: a click the page did not see coming but did
      // arrange. Once the user has seen the decision (a reopen after closing it, from the badge) the click is theirs.
      // (A separate window is always the background's, and always held.) The own-open count is taken either way, so a
      // user-opened first showing takes none (there is none) and an own opening's count is never left behind.
      if (d.panel) {
        const own = this.takeOwnOpen(d.windowId);
        const first = !d.shown;
        d.shown = true;
        if (!own && !first) {
          try {
            port.postMessage({ userOpened: true } satisfies PanelNote);
          } catch {} // closed meanwhile: its disconnect handles it
        }
      }
    } catch {
      port.disconnect();
    }
  }

  /**
   * The normal toolbar popup's port (`popup:<its browser window>`): it is told it was opened by the user (`PanelNote`)
   * when no own `openPopup` of this background is waiting in that window; otherwise it takes that count and gets
   * nothing, so it holds and focuses no Fill. Only the manifest's popup page, shown as a toolbar popup (no tab, no
   * frame), is answered; anything else (a content script, the popup page in a tab) gets nothing and takes nothing.
   */
  private async attachPopup(port: chrome.runtime.Port): Promise<void> {
    await this.ready;
    const sender = port.sender;
    const tail = port.name.slice(POPUP_PORT.length);
    const windowId = /^[0-9]{1,15}$/.test(tail) ? Number(tail) : Number.NaN;
    const popupPage = chrome.runtime.getURL(this.defaultPopup());
    if (!sender || sender.id !== chrome.runtime.id || sender.url !== popupPage || sender.tab || sender.frameId !== undefined || !Number.isSafeInteger(windowId)) {
      return port.disconnect();
    }
    if (this.takeOwnOpen(windowId)) return;
    try {
      port.postMessage({ userOpened: true } satisfies PanelNote);
    } catch {} // closed meanwhile
  }

  /** The page showing the decision went away (closed, clicked outside, hidden). */
  private panelClosed(d: Decision) {
    if (d.settled) return;
    // A separate window: closing it is "no", as it always was.
    if (!d.panel) return this.cancel(d);
    // The app's prompt took the focus while it works: carry on, the outcome still reaches the page.
    if (d.busy || d.unlocking) return;
    // The panel outlived a successful Unlock and closes now (Chrome on Wayland): as if it closed during the Unlock.
    if (this.inGrace(d)) {
      const until = d.graceUntil;
      d.graceUntil = 0;
      return this.armReopen(d, until);
    }
    d.graceUntil = 0;
    this.pend(d);
  }

  private inGrace(d: Decision): boolean {
    return d.graceUntil > Date.now();
  }

  /** Closes a showing panel (its page closes itself when the port ends) and leaves the decision waiting. */
  private hide(d: Decision) {
    // The background closes it because the user left: never a reason to reopen it.
    d.graceUntil = 0;
    const port = d.port;
    d.port = undefined;
    port?.disconnect();
    this.panelClosed(d);
  }

  /** No panel shows the decision: a login pick ends; a save or passkey request waits with the badge. */
  private pend(d: Decision) {
    if (d.settled) return;
    if (!pendable(d)) return this.cancel(d);
    if (d.expired) return this.cancel(d, expiredError());
    void this.popupChange(async () => void (d.settled || (await this.setToolbar(d)))).catch(() => {});
  }

  private expire(d: Decision) {
    if (d.settled) return;
    d.expired = true;
    if (!d.port && !d.busy && !d.unlocking) this.cancel(d, expiredError());
  }

  /** After an Unlock asked from the panel: opens it again once the tab is in front (and on its site). */
  private async tryReopen(d: Decision): Promise<void> {
    if (!d.reopenUntil || d.settled || d.port || d.busy || d.unlocking) return;
    if (d.expired) return this.panelClosed(d);
    const tab = await chrome.tabs.get(d.tabId).catch(() => null);
    if (!tab) return;
    if (d.spec.decision === "chooseLogin" && (!tab.active || originOf(tab.url) !== d.origin)) return this.cancel(d);
    try {
      await this.checkPanelTab(d);
    } catch {
      return; // Not in front yet: the next focus or tab event tries again.
    }
    if (!d.reopenUntil || d.port) return;
    d.reopenUntil = 0;
    await this.openPanel(d).catch((e) => (d.spec.decision === "chooseLogin" ? this.finish(d, { error: openError(e) }) : this.pend(d)));
  }

  /** The Unlock button of the panel: the background asks the app and follows the outcome even after the panel closed. */
  async unlock(sender: chrome.runtime.MessageSender, id: string): Promise<boolean> {
    const d = await this.authorize(sender, id);
    d.unlocking ??= this.deps.request("unlock").then(
      () => true,
      () => false,
    ).then((ok) => {
      d.unlocking = undefined;
      this.afterUnlock(d, ok);
      return ok;
    });
    return d.unlocking;
  }

  private afterUnlock(d: Decision, ok: boolean) {
    if (d.settled) return;
    const until = Date.now() + decisionTiming.reopenMs;
    if (d.port) {
      // A panel that is still open carries on by itself (it shows the list, or Unlock again). If it closes before the
      // user answers in it (Chrome on Wayland closes it only now, as its window comes back), it reopens: `graceUntil`.
      // Chromium only: Firefox closes its panel as the app takes the focus (measured in Firefox 160), so it keeps its rules.
      if (ok && d.panel && this.chromium()) d.graceUntil = until;
      return;
    }
    // Refused or timed out: a login pick ends, a save or passkey request waits with the badge.
    if (!ok) return this.panelClosed(d);
    this.armReopen(d, until);
  }

  /** The panel closed during (or, see `graceUntil`, right after) a successful Unlock: reopen it until `until`. */
  private armReopen(d: Decision, until: number) {
    d.reopenUntil = until;
    d.timers.push(
      setTimeout(() => {
        if (d.reopenUntil !== until || d.settled || d.port) return;
        d.reopenUntil = 0;
        this.panelClosed(d);
      }, Math.max(0, until - Date.now())),
    );
    void this.tryReopen(d);
    // Chrome may report its window as focused a moment after the panel's port closed (or the focus event came
    // first): look once more. `tryReopen` keeps every rule; a reopen in the meantime makes this a no-op.
    d.timers.push(setTimeout(() => void this.tryReopen(d), decisionTiming.settleMs));
  }

  async info(sender: chrome.runtime.MessageSender, id: string): Promise<DecisionInfo> {
    const d = await this.authorize(sender, id);
    const working = d.busy || !!d.unlocking;
    const base = {
      origin: d.origin,
      host: new URL(d.origin).hostname,
      ...(working ? { working: true } : {}),
      ...(!working && d.lastError ? { failed: d.lastError.message } : {}),
    };
    switch (d.spec.decision) {
      case "passkeyCreate":
        return { ...base, decision: "passkeyCreate", request: d.spec.request };
      case "passkeyGet":
        return { ...base, decision: "passkeyGet", rpId: d.spec.rpId };
      case "saveLogin":
        return { ...base, decision: "saveLogin", username: d.username, update: d.update };
      case "chooseLogin":
        return { ...base, decision: "chooseLogin", newPassword: d.spec.newPassword === true, passkeys: !!d.spec.passkeys };
    }
  }

  async logins(sender: chrome.runtime.MessageSender, id: string): Promise<FindLoginsReply> {
    const d = await this.authorize(sender, id);
    if (d.spec.decision !== "chooseLogin") throw new BrokerError("invalid", "this request does not choose a login");
    return this.deps.findLogins(d.origin);
  }

  /** The passkeys for a sign-in request, or for the passkey request a login pick's page has pending (Chrome). */
  async candidates(sender: chrome.runtime.MessageSender, id: string): Promise<PasskeyCandidate[]> {
    const d = await this.authorize(sender, id);
    const params = d.spec.decision === "passkeyGet" ? d.spec : d.spec.decision === "chooseLogin" ? d.spec.passkeys : undefined;
    if (!params) throw new BrokerError("invalid", "this request does not choose a passkey");
    return passkeys.candidates(this.deps.request, d.origin, params.rpId, params.allow);
  }

  async answer(sender: chrome.runtime.MessageSender, id: string, answer: DecisionAnswer): Promise<DecisionDone> {
    // The user answered in the panel (Esc, Cancel, Not now, a pick...): its closing afterwards never reopens it, even
    // when the answer itself is refused below (the panel then closes itself). Ending the grace can only mean fewer
    // reopens, so the panel page's exact address is enough here; everything else is checked by `authorize`.
    this.answered(sender, id);
    const d = await this.authorize(sender, id);
    if (!fits(d.spec, answer)) throw new BrokerError("invalid", "that answer does not fit this request");
    switch (answer.action) {
      case "cancel":
        this.cancel(d);
        return {};
      case "fallback":
        this.finish(d, { error: new BrokerError("fallback", "LocalPass handed the request back to the browser.") });
        return {};
      case "never":
        await this.deps.dismissPending(d.tabId, (d.spec as { id: string }).id, true);
        this.finish(d, { error: cancelled() });
        return {};
    }
    if (d.busy) throw new BrokerError("busy", "LocalPass is still working on the last answer");
    d.busy = true;
    let done: { value: unknown; message?: string };
    try {
      done = await this.run(d, answer);
    } catch (e) {
      d.busy = false;
      // The panel stays open to say what happened; "Close" then reports it to the page.
      d.lastError = e instanceof BrokerError ? e : new BrokerError("internal", e instanceof Error ? e.message : String(e));
      // The panel closed while the app worked: a login pick ends with the error, a save or passkey
      // request waits again (its panel shows the error when it is shown again).
      if (d.panel && !d.port && !d.settled) {
        if (pendable(d)) this.pend(d);
        else this.finish(d, { error: d.lastError });
      }
      throw e;
    }
    d.busy = false;
    d.lastError = undefined;
    this.finish(d, { value: done.value }, done.message ? LINGER_MS : 0);
    return done.message ? { message: done.message } : {};
  }

  /**
   * After a login was fetched (the app may have asked for Unlock or Windows Hello, which takes the
   * focus): waits until its tab is the active tab of the focused window again, on the same site, before
   * it is delivered. Gives up when the decision ends, the site changes or the time runs out.
   */
  private async whenInFront(d: Decision): Promise<void> {
    const until = Date.now() + decisionTiming.refocusMs;
    for (;;) {
      if (d.settled) throw cancelled();
      const tab = await chrome.tabs.get(d.tabId).catch(() => null);
      if (!tab || originOf(tab.url) !== d.origin) throw notActive();
      if (await this.inFront(d, tab)) {
        if (d.settled) throw cancelled();
        return;
      }
      if (Date.now() >= until) throw notActive();
      await sleep(decisionTiming.pollMs);
    }
  }

  private async run(d: Decision, answer: DecisionAnswer): Promise<{ value: unknown; message?: string }> {
    const spec = d.spec;
    if (answer.action === "create" && spec.decision === "passkeyCreate") {
      return { value: await passkeys.create(this.deps.request, d.origin, spec.request) };
    }
    if (answer.action === "assert" && spec.decision === "passkeyGet") {
      return { value: await passkeys.assert(this.deps.request, d.origin, answer.itemId, spec.rpId, spec.challenge, spec.uv) };
    }
    if (answer.action === "save" && spec.decision === "saveLogin") {
      const outcome = await this.deps.savePending(d.tabId, spec.id);
      return { value: outcome, message: saveMessage(outcome) };
    }
    if (spec.decision === "chooseLogin") {
      // What the page gets goes to the asking content script only, once the tab is in front again on the same site.
      const deliver = async (value: ChooseOutcome) => {
        if (d.panel) await this.whenInFront(d);
        if (d.settled) throw cancelled();
        return { value };
      };
      if (answer.action === "fill") {
        const fill = await this.deps.request<LoginFill>("fillLogin", { itemId: answer.itemId, origin: d.origin, force: false });
        const done = await deliver({ fill });
        await this.deps.rememberFill(d.tabId, fill);
        return done;
      }
      if (answer.action === "generate" && spec.newPassword) {
        const { password } = await this.deps.request<{ password: string }>("generatePassword");
        if (typeof password !== "string" || !password) throw new BrokerError("internal", "LocalPass did not make a password");
        return deliver({ generated: password });
      }
      if (answer.action === "assert" && spec.passkeys) {
        const p = spec.passkeys;
        return deliver({ passkey: await passkeys.assert(this.deps.request, d.origin, answer.itemId, p.rpId, p.challenge, p.uv) });
      }
    }
    throw new BrokerError("invalid", "that answer does not fit this request");
  }

  /** The user gave up (Cancel, "Not now", Esc, a window closed), or the page or the tab went away. */
  private cancel(d: Decision, error?: BrokerError) {
    if (d.settled) return;
    // "Not now" to a save: the captured password is dropped.
    if (d.spec.decision === "saveLogin") void this.deps.dismissPending(d.tabId, d.spec.id, false).catch(() => {});
    this.finish(d, { error: error ?? d.lastError ?? cancelled() });
  }

  private finish(d: Decision, outcome: { value: unknown } | { error: BrokerError }, lingerMs = 0) {
    if (d.settled) return;
    d.settled = true;
    for (const timer of d.timers) clearTimeout(timer);
    this.byId.delete(d.id);
    if ("error" in outcome) d.reject(outcome.error);
    else d.resolve(outcome.value);
    void this.close(d, lingerMs);
  }

  private async close(d: Decision, lingerMs: number) {
    await d.windowReady.catch(() => {});
    if (d.panel) {
      // Disconnect closes the panel through its own script, never its browser window; a result stays readable first.
      const port = d.port;
      d.port = undefined;
      if (port && lingerMs > 0) setTimeout(() => port.disconnect(), lingerMs);
      else port?.disconnect();
      const url = this.panelURL(d);
      await this.popupChange(() => this.clearToolbar(d.tabId, url)).catch(() => {}); // The tab may already have closed.
      return;
    }
    if (d.windowId === undefined) return;
    const windowId = d.windowId;
    const remove = () => void chrome.windows.remove(windowId).catch(() => {});
    if (lingerMs > 0) setTimeout(remove, lingerMs);
    else remove();
  }

  /** An answer came from the decision's own panel page (exact address, a toolbar popup: no tab, no frame). */
  private answered(sender: chrome.runtime.MessageSender | undefined, id: string) {
    const d = this.byId.get(id);
    if (!d?.panel || !sender || sender.id !== chrome.runtime.id || sender.tab || sender.frameId !== undefined) return;
    if (sender.url === this.panelURL(d)) d.graceUntil = 0;
  }

  /** Only the exact extension page in the authorized window or panel may use a decision. */
  private async authorize(sender: chrome.runtime.MessageSender | undefined, id: string): Promise<Decision> {
    await this.ready;
    if (!sender || sender.id !== chrome.runtime.id) throw new BrokerError("forbidden", "unknown sender");
    const d = this.byId.get(id);
    if (!d) throw new BrokerError("notFound", "this request is no longer open");
    // The page's own address with its query: nothing else on the extension (the popup) or a lookalike path.
    const url = sender.url ?? "";
    await d.windowReady.catch(() => {});
    const expected = d.panel ? this.panelURL(d) : chrome.runtime.getURL(`${PAGE}?id=${id}`);
    if (url !== expected) throw new BrokerError("forbidden", "not the LocalPass picker");
    if (d.settled) throw cancelled();
    if (d.panel) {
      // Toolbar popups have no sender.tab. An extension tab or embedded frame is not a panel.
      if (sender.tab || sender.frameId !== undefined) throw new BrokerError("forbidden", "not the LocalPass panel");
      await this.checkPanelTab(d);
    } else if (d.windowId === undefined || sender.tab?.windowId !== d.windowId) throw new BrokerError("forbidden", "not the LocalPass window");
    return d;
  }
}
