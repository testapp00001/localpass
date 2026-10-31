// Module background: top-level await proves the module loads.
const R = { bgModule: true };
await Promise.resolve();
R.runtimeId = chrome.runtime.id;
R.setAccessLevelType = typeof chrome.storage.local.setAccessLevel;
if (R.setAccessLevelType === "function") {
  try { await chrome.storage.local.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" }); R.setAccessLevel = "ok"; } catch (e) { R.setAccessLevel = "error: " + e; }
}
try {
  const s = await chrome.privacy.services.passwordSavingEnabled.get({});
  R.privacyGet = { value: s.value, levelOfControl: s.levelOfControl };
} catch (e) { R.privacyGet = "error: " + e; }
try {
  const w = await chrome.windows.create({ type: "popup", url: chrome.runtime.getURL("p.html"), width: 400, height: 300 });
  R.windowCreate = { type: w.type, hasId: typeof w.id === "number" };
  await chrome.windows.remove(w.id);
} catch (e) { R.windowCreate = "error: " + e; }
try {
  const port = chrome.runtime.connectNative("app.localpass.host");
  R.connectNative = "returned a port";
  await new Promise((resolve) => {
    port.onDisconnect.addListener((p) => { R.nativeDisconnect = String((p && p.error && p.error.message) || chrome.runtime.lastError?.message || "none"); resolve(); });
    setTimeout(resolve, 3000);
  });
} catch (e) { R.connectNative = "throws: " + e; }
R.storageSession = typeof chrome.storage.session;
try { await chrome.storage.session.set({ lpSessionSecret: "hunter2" }); R.sessionSet = "ok"; } catch (e) { R.sessionSet = "error: " + e; }
try {
  const db = await new Promise((res, rej) => { const o = indexedDB.open("localpass", 1); o.onupgradeneeded = () => o.result.createObjectStore("kv"); o.onsuccess = () => res(o.result); o.onerror = () => rej(o.error); });
  await new Promise((res, rej) => { const tx = db.transaction("kv", "readwrite"); tx.objectStore("kv").put("hunter2", "secret"); tx.oncomplete = res; tx.onerror = () => rej(tx.error); });
  R.idb = "background wrote";
} catch (e) { R.idb = "error: " + e; }
R.tabsQuery = typeof chrome.tabs.query;
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message && message.type === "probe") {
    // Answer later with `return true`, the way the real worker does.
    Promise.resolve().then(() => new Promise((r) => setTimeout(r, 200))).then(() => sendResponse({
      sender: { id: sender.id, origin: sender.origin ?? null, url: sender.url ?? null, frameId: sender.frameId, hasTab: !!sender.tab, tabUrl: sender.tab?.url ?? null, documentLifecycle: sender.documentLifecycle ?? null },
      background: R,
      async: true,
    }));
    return true;
  }
  return false;
});
