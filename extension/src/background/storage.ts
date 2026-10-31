// Where the service worker keeps what must outlive it (the pairing secret, the "never save" list,
// the passkey switch).
//
// Chrome: chrome.storage.local is readable by content scripts by default, and content scripts run
// in every page (a compromised renderer there could read the broker secret). The worker therefore
// restricts the area to extension pages and itself, and every read or write waits for that. If the
// restriction cannot be applied, nothing is read or written (fail closed).
//
// Firefox has no `setAccessLevel` and lets content scripts read and write storage.local, so there
// the same data lives in an IndexedDB database of the extension's own origin, which no content
// script can open (a content script's IndexedDB is the page's). It never falls back to storage.local.

/** The few calls the worker makes on its persistent storage (the same shape as chrome.storage). */
export interface Area {
  get(keys: string | string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove(keys: string | string[]): Promise<void>;
}

/** Set by `bun run build --firefox`. */
declare const __FIREFOX__: boolean;

const gates = new WeakMap<object, Promise<void>>();

/** chrome.storage.local, once it is restricted to trusted contexts. */
async function chromeArea(): Promise<Area> {
  const area = chrome.storage.local;
  let gate = gates.get(area);
  if (!gate) {
    gate = area.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
    gates.set(area, gate);
  }
  await gate;
  return area;
}

const DB_NAME = "localpass";
const STORE = "kv";

function wait<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB request failed"));
  });
}

function done(tx: IDBTransaction): Promise<void> {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("IndexedDB transaction failed"));
    tx.onabort = () => reject(tx.error ?? new Error("IndexedDB transaction aborted"));
  });
}

/** A key-value area in an IndexedDB database of the calling origin. */
export async function indexedDbArea(factory: IDBFactory): Promise<Area> {
  const open = factory.open(DB_NAME, 1);
  open.onupgradeneeded = () => open.result.createObjectStore(STORE);
  const db = await wait(open);
  const names = (keys: string | string[]) => (Array.isArray(keys) ? keys : [keys]);
  return {
    async get(keys) {
      const tx = db.transaction(STORE, "readonly");
      const store = tx.objectStore(STORE);
      const found: Record<string, unknown> = {};
      await Promise.all(
        names(keys).map(async (key) => {
          const value = await wait(store.get(key));
          if (value !== undefined) found[key] = value;
        }),
      );
      return found;
    },
    async set(items) {
      const tx = db.transaction(STORE, "readwrite");
      for (const [key, value] of Object.entries(items)) tx.objectStore(STORE).put(value, key);
      await done(tx);
    },
    async remove(keys) {
      const tx = db.transaction(STORE, "readwrite");
      for (const key of names(keys)) tx.objectStore(STORE).delete(key);
      await done(tx);
    },
  };
}

let firefoxArea: Promise<Area> | undefined;

/** The persistent area of this browser's build (opened once; a failure to open is remembered and fails every call). */
export function local(): Promise<Area> {
  if (!__FIREFOX__) return chromeArea();
  return (firefoxArea ??= indexedDbArea(indexedDB));
}
