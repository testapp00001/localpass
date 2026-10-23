// Access to chrome.storage.local from the service worker.
//
// storage.local is readable by content scripts by default, and content scripts run in every page
// (a compromised renderer there could read the broker secret). The service worker therefore
// restricts the area to extension pages and itself, and every read or write here waits for that.
// If the restriction cannot be applied, nothing is read or written (fail closed).

const gates = new WeakMap<object, Promise<void>>();

/** The local storage area, once it is restricted to trusted contexts. */
export async function local(): Promise<chrome.storage.LocalStorageArea> {
  const area = chrome.storage.local;
  let gate = gates.get(area);
  if (!gate) {
    gate = area.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
    gates.set(area, gate);
  }
  await gate;
  return area;
}
