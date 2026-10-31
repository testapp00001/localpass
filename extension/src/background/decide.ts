// Firefox only: the decisions the in-page controls make in Chrome (choose a login, save a login,
// create a passkey, sign in with one) are made in a window that belongs to the extension.
//
// Why: Firefox cannot prove that a click on a control drawn into a page could be seen (no
// IntersectionObserver `isVisible`), so a page could hide or cover such a control. This window is
// an extension page: page CSS and script cannot reach into it, and its clicks are real.
//
// Who is trusted with what:
// * The site comes from the browser's report for the asking frame (`Asker`, built by index.ts from
//   `sender`), never from the request. The window only shows it and answers by decision id.
// * Only the window this module opened may answer (its URL, its id in the URL and its window id are
//   checked on every message), and only with an answer that fits the decision.
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

/** The most decision windows open at once (across tabs). */
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

  constructor(private deps: Deps) {}

  /** Opens the window and resolves with the outcome (or rejects with the reason it did not happen). */
  async open(asker: Asker, ref: string, spec: DecisionRequest): Promise<unknown> {
    let username = "";
    if (spec.decision === "saveLogin") {
      if (asker.frameId !== 0) throw new BrokerError("forbidden", "only the top page may offer to save a login");
      const pending = await this.deps.pendingInfo(asker.tabId, spec.id);
      if (!pending) throw new BrokerError("notFound", "there is nothing to save any more");
      username = pending.username;
    }
    // One window per tab (and per page request): the newer request wins.
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
        resolve,
        reject,
        settled: false,
        busy: false,
        lastError: undefined,
      };
      this.byId.set(id, d);
      d.windowReady = chrome.windows
        .create({ url: chrome.runtime.getURL(`${PAGE}?id=${id}`), type: "popup", width: 440, height: 600 })
        .then((w) => {
          if (w?.id === undefined) throw new Error("the window has no id");
          d.windowId = w.id;
        });
      // (A request that ends while its window is still opening closes the window in `close`.)
      d.windowReady.catch((e) => this.finish(d, { error: new BrokerError("error", `could not open the LocalPass window (${String(e)})`) }));
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

  /** The window's keep-alive port: closing the window (or the port) is "no". */
  async attach(port: chrome.runtime.Port): Promise<void> {
    const id = port.name.startsWith("decide:") ? port.name.slice("decide:".length) : "";
    try {
      const d = await this.authorize(port.sender, id);
      port.onDisconnect.addListener(() => this.cancel(d));
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
    if (d.windowId === undefined) return;
    const windowId = d.windowId;
    const remove = () => void chrome.windows.remove(windowId).catch(() => {});
    if (lingerMs > 0) setTimeout(remove, lingerMs);
    else remove();
  }

  /** Only the page this module opened, in the window it opened, may use a decision. */
  private async authorize(sender: chrome.runtime.MessageSender | undefined, id: string): Promise<Decision> {
    if (!sender || sender.id !== chrome.runtime.id) throw new BrokerError("forbidden", "unknown sender");
    const d = this.byId.get(id);
    if (!d) throw new BrokerError("notFound", "this request is no longer open");
    // The page's own address with its query: nothing else on the extension (the popup) or a lookalike path.
    const base = chrome.runtime.getURL(PAGE);
    const url = sender.url ?? "";
    if (!url.startsWith(`${base}?`)) throw new BrokerError("forbidden", "not the LocalPass window");
    let urlId: string | null = null;
    try {
      urlId = new URL(url).searchParams.get("id");
    } catch {
      // not an address
    }
    if (urlId !== id) throw new BrokerError("forbidden", "not the LocalPass window");
    await d.windowReady.catch(() => {});
    if (d.windowId === undefined || sender.tab?.windowId !== d.windowId) throw new BrokerError("forbidden", "not the LocalPass window");
    return d;
  }
}
