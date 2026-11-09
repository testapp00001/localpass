// Firefox only: the decisions the in-page controls make in Chrome (choose a login, save a login,
// create a passkey, sign in with one) are made in extension-owned UI. Login picking uses the
// toolbar panel on Firefox 149+, and other decisions (and older Firefox) use separate windows.
//
// Why: Firefox cannot prove that a click on a control drawn into a page could be seen (no
// IntersectionObserver `isVisible`), so a page could hide or cover such a control. This window is
// an extension page: page CSS and script cannot reach into it, and its clicks are real.
//
// Who is trusted with what:
// * The site comes from the browser's report for the asking frame (`Asker`, built by index.ts from
//   `sender`), never from the request. The window only shows it and answers by decision id.
// * Only the UI this module opened may answer: exact URL and request id, plus the created window
//   id or the active tab in the focused panel window. It may only answer its own decision.
// * Secrets go to the asking content script as the reply to its own `decide` message, not to the
//   window: the window learns whether it worked, nothing more.
// * One window per tab: a newer request from the tab replaces the older one.

import type {
  DecisionAnswer,
  DecisionDone,
  DecisionInfo,
  DecisionRequest,
  FindLoginsReply,
  PendingSave,
} from "../shared/messages";
import { BrokerError, type LoginFill, type PasskeyCandidate, type SaveOutcome } from "../shared/protocol";
import { saveMessage } from "../shared/shown";
import * as passkeys from "./passkeys";

export interface Deps {
  request<T>(op: string, params?: Record<string, unknown>): Promise<T>;
  findLogins(origin: string): Promise<FindLoginsReply>;
  rememberFill(tabId: number, fill: LoginFill): Promise<void>;
  pendingInfo(tabId: number, id: string): Promise<PendingSave | null>;
  savePending(tabId: number, id: string): Promise<SaveOutcome>;
  dismissPending(tabId: number, id: string, never: boolean): Promise<void>;
}

/** The frame that asked, as the browser reported it. */
export interface Asker {
  tabId: number;
  frameId: number;
  origin: string;
}

/** The most decisions open at once (across tabs). */
const MAX_OPEN = 8;
/** How long a result stays on screen before the window closes. */
const LINGER_MS = 1800;
const PAGE = "decide.html";

const ANSWERS: Record<DecisionRequest["decision"], DecisionAnswer["action"][]> = {
  passkeyCreate: ["create", "fallback", "cancel"],
  passkeyGet: ["assert", "fallback", "cancel"],
  saveLogin: ["save", "never", "cancel"],
  chooseLogin: ["fill", "cancel"],
};

interface Decision extends Asker {
  id: string;
  ref: string;
  spec: DecisionRequest;
  username: string;
  windowId: number | undefined;
  windowReady: Promise<void>;
  panel: boolean;
  port?: chrome.runtime.Port;
  resolve(value: unknown): void;
  reject(error: unknown): void;
  settled: boolean;
  busy: boolean;
  /** The last thing that went wrong while carrying out an answer: what the page hears if the user gives up. */
  lastError: BrokerError | undefined;
}

const cancelled = () => new BrokerError("cancelled", "The user cancelled the request.");

export class Decisions {
  private byId = new Map<string, Decision>();
  // Serialize popup changes so cleanup of a replaced request cannot erase its successor.
  private popupQueue: Promise<unknown> = Promise.resolve();

  private popupChange<T>(action: () => Promise<T>): Promise<T> {
    const next = this.popupQueue.then(action);
    this.popupQueue = next.catch(() => {});
    return next;
  }

  private panelURL(d: Decision): string {
    return chrome.runtime.getURL(`${PAGE}?id=${d.id}&presentation=panel`);
  }

  private async checkPanelTab(d: Decision): Promise<void> {
    const tab = await chrome.tabs.get(d.tabId);
    const win = await chrome.windows.get(tab.windowId);
    if (d.settled || !tab.active || !win.focused || tab.windowId !== d.windowId || new URL(tab.url ?? "").origin !== d.origin) {
      throw new BrokerError("cancelled", "The page is no longer active. Try again from the login field.");
    }
  }

  private async present(d: Decision): Promise<void> {
    if (d.spec.decision === "chooseLogin") {
      const runtime = chrome.runtime as typeof chrome.runtime & { getBrowserInfo?: () => Promise<{ version: string }> };
      if (runtime.getBrowserInfo) {
        const info = await runtime.getBrowserInfo();
        d.panel = Number.parseInt(info.version, 10) >= 149;
      }
    }
    if (d.settled) return;
    if (d.panel) {
      const tab = await chrome.tabs.get(d.tabId);
      d.windowId = tab.windowId;
      await this.popupChange(async () => {
        await this.checkPanelTab(d);
        await chrome.action.setPopup({ tabId: d.tabId, popup: this.panelURL(d) });
        await this.checkPanelTab(d);
        await chrome.action.openPopup({ windowId: d.windowId });
      });
    } else {
      const w = await chrome.windows.create({ url: chrome.runtime.getURL(`${PAGE}?id=${d.id}`), type: "popup", width: 440, height: 600 });
      if (w?.id === undefined) throw new Error("the window has no id");
      d.windowId = w.id;
    }
  }

  constructor(private deps: Deps) {}

  /** Opens protected UI and resolves with the outcome (or rejects with the reason it did not happen). */
  async open(asker: Asker, ref: string, spec: DecisionRequest): Promise<unknown> {
    let username = "";
    if (spec.decision === "saveLogin") {
      if (asker.frameId !== 0) throw new BrokerError("forbidden", "only the top page may offer to save a login");
      const pending = await this.deps.pendingInfo(asker.tabId, spec.id);
      if (!pending) throw new BrokerError("notFound", "there is nothing to save any more");
      username = pending.username;
    }
    // One decision per tab (and per page request): the newer request wins.
    for (const old of [...this.byId.values()]) {
      if (old.tabId === asker.tabId) this.cancel(old, new BrokerError("cancelled", "Replaced by a newer request."));
    }
    if (this.byId.size >= MAX_OPEN) throw new BrokerError("tooMany", "too many LocalPass windows are open");
    const id = crypto.randomUUID();
    const outcome = new Promise<unknown>((resolve, reject) => {
      const d: Decision = {
        ...asker,
        id,
        ref,
        spec,
        username,
        windowId: undefined,
        windowReady: Promise.resolve(),
        panel: false,
        resolve,
        reject,
        settled: false,
        busy: false,
        lastError: undefined,
      };
      this.byId.set(id, d);
      d.windowReady = this.present(d);
      // (A request that ends while its window is still opening closes the window in `close`.)
      d.windowReady.catch((e) => this.finish(d, { error: new BrokerError("error", `Could not open the LocalPass picker. Use the LocalPass toolbar icon to pick a login. (${String(e)})`) }));
    });
    return outcome;
  }

  /** The page withdrew its request (an AbortSignal, or a newer call). */
  withdraw(asker: Asker, ref: string) {
    for (const d of [...this.byId.values()]) {
      if (d.tabId === asker.tabId && d.frameId === asker.frameId && d.ref === ref) this.cancel(d, new BrokerError("cancelled", "The request was withdrawn."));
    }
  }

  onWindowRemoved(windowId: number) {
    for (const d of [...this.byId.values()]) if (d.windowId === windowId) this.cancel(d);
  }

  onTabRemoved(tabId: number) {
    for (const d of [...this.byId.values()]) if (d.tabId === tabId) this.cancel(d);
  }

  onTabUpdated(tabId: number, change: { url?: string; status?: string }) {
    if (!change.url && change.status !== "loading") return;
    for (const d of [...this.byId.values()]) if (d.panel && d.tabId === tabId) this.cancel(d);
  }

  onTabActivated(info: { tabId: number; windowId: number }) {
    for (const d of [...this.byId.values()]) if (d.panel && d.windowId === info.windowId && d.tabId !== info.tabId) this.cancel(d);
  }

  onFocusChanged(windowId: number) {
    for (const d of [...this.byId.values()]) if (d.panel && d.windowId !== windowId) this.cancel(d);
  }

  /** The UI's port: closing the window, panel or port is "no". */
  async attach(port: chrome.runtime.Port): Promise<void> {
    const id = port.name.startsWith("decide:") ? port.name.slice("decide:".length) : "";
    let disconnected = false;
    let attached: Decision | undefined;
    port.onDisconnect.addListener(() => {
      disconnected = true;
      if (attached) this.cancel(attached);
    });
    try {
      const d = await this.authorize(port.sender, id);
      if (d.port) throw new BrokerError("forbidden", "the picker is already attached");
      attached = d;
      d.port = port;
      if (disconnected) this.cancel(d);
    } catch {
      port.disconnect();
    }
  }

  async info(sender: chrome.runtime.MessageSender, id: string): Promise<DecisionInfo> {
    const d = await this.authorize(sender, id);
    const base = { origin: d.origin, host: new URL(d.origin).hostname };
    switch (d.spec.decision) {
      case "passkeyCreate":
        return { ...base, decision: "passkeyCreate", request: d.spec.request };
      case "passkeyGet":
        return { ...base, decision: "passkeyGet", rpId: d.spec.rpId };
      case "saveLogin":
        return { ...base, decision: "saveLogin", username: d.username };
      case "chooseLogin":
        return { ...base, decision: "chooseLogin" };
    }
  }

  async logins(sender: chrome.runtime.MessageSender, id: string): Promise<FindLoginsReply> {
    const d = await this.authorize(sender, id);
    if (d.spec.decision !== "chooseLogin") throw new BrokerError("invalid", "this request does not choose a login");
    return this.deps.findLogins(d.origin);
  }

  async candidates(sender: chrome.runtime.MessageSender, id: string): Promise<PasskeyCandidate[]> {
    const d = await this.authorize(sender, id);
    if (d.spec.decision !== "passkeyGet") throw new BrokerError("invalid", "this request does not choose a passkey");
    return passkeys.candidates(this.deps.request, d.origin, d.spec.rpId, d.spec.allow);
  }

  async answer(sender: chrome.runtime.MessageSender, id: string, answer: DecisionAnswer): Promise<DecisionDone> {
    const d = await this.authorize(sender, id);
    if (!ANSWERS[d.spec.decision].includes(answer?.action)) throw new BrokerError("invalid", "that answer does not fit this request");
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
    try {
      const done = await this.run(d, answer);
      d.lastError = undefined;
      this.finish(d, { value: done.value }, done.message ? LINGER_MS : 0);
      return done.message ? { message: done.message } : {};
    } catch (e) {
      // The window stays open to say what happened; "Close" then reports it to the page.
      d.lastError = e instanceof BrokerError ? e : new BrokerError("internal", e instanceof Error ? e.message : String(e));
      throw e;
    } finally {
      d.busy = false;
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
    if (answer.action === "fill" && spec.decision === "chooseLogin") {
      const fill = await this.deps.request<LoginFill>("fillLogin", { itemId: answer.itemId, origin: d.origin, force: false });
      if (d.panel) await this.checkPanelTab(d);
      if (d.settled) throw cancelled();
      await this.deps.rememberFill(d.tabId, fill);
      return { value: fill };
    }
    throw new BrokerError("invalid", "that answer does not fit this request");
  }

  /** The user gave up (window closed, Cancel, "Not now"), or the page or the tab went away. */
  private cancel(d: Decision, error?: BrokerError) {
    if (d.settled) return;
    // "Not now" to a save: the captured password is dropped, as the save bar's dismissal does.
    if (d.spec.decision === "saveLogin") void this.deps.dismissPending(d.tabId, d.spec.id, false).catch(() => {});
    this.finish(d, { error: error ?? d.lastError ?? cancelled() });
  }

  private finish(d: Decision, outcome: { value: unknown } | { error: BrokerError }, lingerMs = 0) {
    if (d.settled) return;
    d.settled = true;
    this.byId.delete(d.id);
    if ("error" in outcome) d.reject(outcome.error);
    else d.resolve(outcome.value);
    void this.close(d, lingerMs);
  }

  private async close(d: Decision, lingerMs: number) {
    await d.windowReady.catch(() => {});
    if (d.panel) {
      // Disconnect closes the panel through its own script, never its browser window.
      d.port?.disconnect();
      await this.popupChange(async () => {
        if (await chrome.action.getPopup({ tabId: d.tabId }) === this.panelURL(d)) {
          // Firefox accepts null to remove the per-tab override (Chrome's typings omit null).
          await chrome.action.setPopup({ tabId: d.tabId, popup: null as unknown as string });
        }
      }).catch(() => {}); // The tab may already have closed.
      return;
    }
    if (d.windowId === undefined) return;
    const windowId = d.windowId;
    const remove = () => void chrome.windows.remove(windowId).catch(() => {});
    if (lingerMs > 0) setTimeout(remove, lingerMs);
    else remove();
  }

  /** Only the exact extension page in the authorized window or panel may use a decision. */
  private async authorize(sender: chrome.runtime.MessageSender | undefined, id: string): Promise<Decision> {
    if (!sender || sender.id !== chrome.runtime.id) throw new BrokerError("forbidden", "unknown sender");
    const d = this.byId.get(id);
    if (!d) throw new BrokerError("notFound", "this request is no longer open");
    // The page's own address with its query: nothing else on the extension (the popup) or a lookalike path.
    const url = sender.url ?? "";
    const expected = d.panel ? this.panelURL(d) : chrome.runtime.getURL(`${PAGE}?id=${id}`);
    if (url !== expected) throw new BrokerError("forbidden", "not the LocalPass picker");
    await d.windowReady.catch(() => {});
    if (d.settled) throw cancelled();
    if (d.panel) {
      // Toolbar popups have no sender.tab. An extension tab or embedded frame is not a panel.
      if (sender.tab || sender.frameId !== undefined) throw new BrokerError("forbidden", "not the LocalPass panel");
      await this.checkPanelTab(d);
    } else if (d.windowId === undefined || sender.tab?.windowId !== d.windowId) throw new BrokerError("forbidden", "not the LocalPass window");
    return d;
  }
}
