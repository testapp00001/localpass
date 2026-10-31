// MAIN-world content script.
window.__mainFlag = true;
const out = { world: "main", locationOrigin: location.origin, hasChromeNs: typeof chrome, hasBrowserNs: typeof browser };
window.addEventListener("message", (e) => {
  if (e.data && e.data.probe === "from-iso") { out.gotReplyFromIso = true; out.replySourceIsWindow = e.source === window; document.documentElement.dataset.lpMain = JSON.stringify(out); }
});
window.postMessage({ probe: "from-main" }, "*");
document.documentElement.dataset.lpMain = JSON.stringify(out);
