// A small fake of the `chrome` extension API for the unit tests: native-messaging ports, storage
// areas (with a log of the order of calls), tabs and runtime messaging. Only what the code under
// test touches.

export type Json = Record<string, unknown>;

export class FakePort {
  sent: Json[] = [];
  disconnected = false;
  /** Called with every message the extension posts (the fake server's brain). */
  onSend: ((message: Json, port: FakePort) => void) | null = null;
  private messageListeners: Array<(message: unknown) => void> = [];
  private disconnectListeners: Array<() => void> = [];
  onMessage = { addListener: (f: (message: unknown) => void) => void this.messageListeners.push(f) };
  onDisconnect = { addListener: (f: () => void) => void this.disconnectListeners.push(f) };

  postMessage(message: Json) {
    if (this.disconnected) throw new Error("Attempting to use a disconnected port object");
    const copy = JSON.parse(JSON.stringify(message)) as Json;
    this.sent.push(copy);
    this.onSend?.(copy, this);
  }

  /** The extension closes its end (Chrome does not fire onDisconnect for that side). */
  disconnect() {
    this.disconnected = true;
  }

  /** The other end sends a message. */
  emit(message: unknown) {
    for (const f of this.messageListeners) f(message);
  }

  /** The other end goes away. */
  serverClose() {
    this.disconnected = true;
    for (const f of this.disconnectListeners) f();
  }

  /** Requests (messages with an `op`) the extension sent. */
  get requests(): Json[] {
    return this.sent.filter((m) => "op" in m);
  }
}

export class FakeStorage {
  data: Record<string, unknown> = {};
  /** "setAccessLevel:TRUSTED_CONTEXTS", "get", "set", "remove", in call order. */
  log: string[] = [];
  accessLevel: string | null = null;
  failAccessLevel = false;

  async setAccessLevel(options: { accessLevel: string }) {
    this.log.push(`setAccessLevel:${options.accessLevel}`);
    // Real Chrome answers asynchronously; a read that does not wait for this is a bug.
    await new Promise((r) => setTimeout(r, 5));
    if (this.failAccessLevel) throw new Error("setAccessLevel is not available");
    this.accessLevel = options.accessLevel;
  }

  async get(keys?: string | string[] | null) {
    this.log.push("get");
    if (keys == null) return { ...this.data };
    const list = Array.isArray(keys) ? keys : [keys];
    const out: Record<string, unknown> = {};
    for (const k of list) if (k in this.data) out[k] = this.data[k];
    return out;
  }

  async set(items: Record<string, unknown>) {
    this.log.push("set");
    Object.assign(this.data, JSON.parse(JSON.stringify(items)));
  }

  async remove(keys: string | string[]) {
    this.log.push("remove");
    for (const k of Array.isArray(keys) ? keys : [keys]) delete this.data[k];
  }
}

export interface FakeChrome {
  ports: FakePort[];
  local: FakeStorage;
  session: FakeStorage;
  /** What `connectNative` does with each new port (the fake server). */
  server: ((port: FakePort) => void) | null;
  tabs: { url: string; id: number }[];
  /** The listener the service worker registered with chrome.runtime.onMessage. */
  onMessage: ((message: unknown, sender: unknown, sendResponse: (reply: unknown) => void) => boolean | void) | null;
  sentToTabs: { tabId: number; message: unknown; options: unknown }[];
  /** Runs before the next tabs.get answers (to change the tab mid-flight). */
  beforeTabGet: (() => void) | null;
  /** What `runtime.getPlatformInfo` reports as the OS ("win" unless a test changes it). */
  os: string;
}

export function installChrome(): FakeChrome {
  const fake: FakeChrome = {
    ports: [],
    local: new FakeStorage(),
    session: new FakeStorage(),
    server: null,
    tabs: [],
    onMessage: null,
    sentToTabs: [],
    beforeTabGet: null,
    os: "win",
  };
  const noEvent = { addListener() {} };
  (globalThis as unknown as { chrome: unknown }).chrome = {
    runtime: {
      id: "khlgdnhadbjfghhonaohnhdmedgnenlc",
      lastError: undefined,
      getURL: (path: string) => `chrome-extension://khlgdnhadbjfghhonaohnhdmedgnenlc/${path}`,
      connectNative() {
        const port = new FakePort();
        fake.ports.push(port);
        fake.server?.(port);
        return port;
      },
      sendMessage: async () => undefined,
      getPlatformInfo: async () => ({ os: fake.os, arch: "x86-64", nacl_arch: "x86-64" }),
      onMessage: {
        addListener(f: FakeChrome["onMessage"]) {
          fake.onMessage = f;
        },
      },
      onStartup: noEvent,
      onInstalled: noEvent,
      onConnect: noEvent,
    },
    storage: { local: fake.local, session: fake.session },
    tabs: {
      async get(tabId: number) {
        fake.beforeTabGet?.();
        fake.beforeTabGet = null;
        const tab = fake.tabs.find((t) => t.id === tabId);
        if (!tab) throw new Error("No tab with id");
        return { ...tab };
      },
      async query() {
        return fake.tabs.slice(0, 1).map((t) => ({ ...t, title: "t" }));
      },
      async sendMessage(tabId: number, message: unknown, options: unknown) {
        fake.sentToTabs.push({ tabId, message, options });
      },
      onRemoved: noEvent,
      onUpdated: noEvent,
      onActivated: noEvent,
    },
    windows: { get: async (id: number) => ({ id, focused: true }), onRemoved: noEvent, onFocusChanged: noEvent },
    privacy: { services: { passwordSavingEnabled: { get: async () => ({ value: true, levelOfControl: "controllable_by_this_extension" }) } } },
  };
  return fake;
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Waits (polling) until `ok()` is true, or fails after `ms`. */
export async function until(ok: () => boolean, ms = 1500, what = "condition") {
  const start = Date.now();
  while (!ok()) {
    if (Date.now() - start > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(5);
  }
}

/** The window API the Firefox decision window uses. `created` lists the options of every window opened. */
export function installWindows() {
  const created: { url: string; windowId: number }[] = [];
  const removed: number[] = [];
  const listeners: Array<(windowId: number) => void> = [];
  let next = 100;
  const c = (globalThis as unknown as { chrome: { windows?: Record<string, unknown> } }).chrome;
  c.windows = {
    ...(c.windows ?? {}),
    async create(options: { url: string }) {
      const windowId = ++next;
      created.push({ url: options.url, windowId });
      return { id: windowId };
    },
    async remove(windowId: number) {
      removed.push(windowId);
    },
    onRemoved: { addListener: (f: (windowId: number) => void) => void listeners.push(f) },
  };
  return {
    created,
    removed,
    /** The user closes the window. */
    close(windowId: number) {
      for (const f of listeners) f(windowId);
    },
  };
}

/**
 * The toolbar (`action`) of a browser that can open the panel by itself (Firefox 160), per tab: popup,
 * badge and the panels opened. Also the events the Firefox wiring of background/index.ts listens to.
 * Install it after `installChrome` and before the background is imported.
 */
export function installToolbar() {
  const c = (globalThis as unknown as { chrome: Record<string, Record<string, unknown>> }).chrome;
  const url = c.runtime!.getURL as (path: string) => string;
  const state = { popups: new Map<number, string>(), badges: new Map<number, string>(), opened: [] as number[] };
  const noEvent = { addListener() {} };
  c.runtime!.getBrowserInfo = async () => ({ version: "160.0" });
  c.runtime!.onConnect = noEvent;
  c.tabs!.onUpdated = noEvent;
  c.tabs!.onActivated = noEvent;
  c.windows = { ...(c.windows ?? {}), get: async (id: number) => ({ id, focused: true }), onRemoved: noEvent, onFocusChanged: noEvent };
  c.action = {
    getPopup: async ({ tabId }: { tabId: number }) => state.popups.get(tabId) ?? url("popup.html"),
    setPopup: async ({ tabId, popup }: { tabId: number; popup: string }) => void state.popups.set(tabId, popup.includes("://") ? popup : url(popup)),
    setBadgeText: async ({ tabId, text }: { tabId: number; text: string }) => void state.badges.set(tabId, text),
    openPopup: async ({ windowId }: { windowId: number }) => void state.opened.push(windowId),
  };
  return state;
}
