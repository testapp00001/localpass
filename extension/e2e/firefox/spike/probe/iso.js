// Isolated-world content script.
const out = { world: "isolated", locationOrigin: location.origin, href: location.href };
out.hasWindowFoo = typeof window.__mainFlag; // page globals are not visible here
const ioEntryHasIsVisible = "isVisible" in IntersectionObserverEntry.prototype;
out.ioV2 = { entryPrototypeHasIsVisible: ioEntryHasIsVisible };
// postMessage bridge: main -> iso with event.source === window
out.bridge = { fromMain: null, sourceIsWindow: null };
window.addEventListener("message", (e) => {
  if (e.data && e.data.probe === "from-main") {
    out.bridge.fromMain = true;
    out.bridge.sourceIsWindow = e.source === window;
    window.postMessage({ probe: "from-iso" }, "*");
    flush();
  }
});
// can a content script read storage.local? (setAccessLevel matters)
function flush() { document.documentElement.dataset.lpIso = JSON.stringify(out); }
flush();
(async () => {
  try {
    await chrome.storage.local.set({ lpProbe: 1 });
    const got = await chrome.storage.local.get("lpProbe");
    out.contentStorageLocal = got.lpProbe === 1 ? "readable" : "empty";
  } catch (e) { out.contentStorageLocal = "blocked: " + e; }
  // storage.session holds a captured password in the real extension: can a content script read it?
  try {
    const got = await chrome.storage.session.get("lpSessionSecret");
    out.contentStorageSession = got.lpSessionSecret === "hunter2" ? "READABLE" : "empty";
    await chrome.storage.session.set({ lpFromContent: 1 });
    out.contentSessionWrite = "allowed";
  } catch (e) { out.contentStorageSession = "blocked: " + e; }
  // The extension origin's IndexedDB (where the 6.6 pairing secret lives) must not be visible from here.
  try {
    const found = await new Promise((res, rej) => {
      const o = indexedDB.open("localpass");
      let existed = true;
      o.onupgradeneeded = () => { existed = false; };
      o.onsuccess = () => { const names = [...o.result.objectStoreNames]; o.result.close(); res({ existed, names }); };
      o.onerror = () => rej(o.error);
    });
    out.contentIdb = found;
  } catch (e) { out.contentIdb = "error: " + e; }
  try {
    out.reply = await chrome.runtime.sendMessage({ type: "probe" });
  } catch (e) { out.reply = "error: " + e; }
  flush();
})();
// IO v2 in practice: observe a visible element once the DOM exists.
function observe() {
  const el = document.createElement("div");
  el.style.cssText = "width:50px;height:50px;background:red;position:fixed;top:0;left:0";
  document.body.appendChild(el);
  try {
    const io = new IntersectionObserver((entries) => {
      const e = entries[entries.length - 1];
      out.ioV2.observed = { isVisible: e.isVisible, ratio: e.intersectionRatio, keys: Object.keys(Object.getPrototypeOf(e)).slice(0, 12) };
      flush();
    }, { threshold: [0.5, 1], trackVisibility: true, delay: 100 });
    io.observe(el);
  } catch (e) { out.ioV2.observed = "throws: " + e; flush(); }
}
if (document.body) observe(); else addEventListener("DOMContentLoaded", observe);
