// Probe: does the real add-on open its decision window in real Firefox? (6.6b)
// usage: bun decide.ts <absolute path to dist-firefox-e2e>
// Page with a login form -> focus the username field -> LocalPass badge appears -> a trusted click
// (BiDi input.performActions) -> the background opens decide.html in a popup window -> read it.
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const extDir = process.argv[2];
const web = Bun.serve({
  port: 8089,
  hostname: "127.0.0.1",
  fetch: () => new Response(`<!doctype html><title>probe</title><form><input name=user id=u style="width:300px;height:30px"><input type=password name=pw></form>`, { headers: { "content-type": "text/html" } }),
});
const tmp = mkdtempSync(join(tmpdir(), "lp-decide-"));
const profile = join(tmp, "profile");
const home = join(tmp, "home");
mkdirSync(profile);
mkdirSync(home);
writeFileSync(join(profile, "user.js"), ['user_pref("browser.shell.checkDefaultBrowser", false);', 'user_pref("extensions.autoDisableScopes", 0);', 'user_pref("app.update.enabled", false);'].join("\n"));
const port = 9334;
const ff = Bun.spawn(["firefox", "--headless", "--no-remote", "--profile", profile, "--remote-debugging-port", String(port), "--remote-allow-system-access", "about:blank"], {
  env: { ...process.env, HOME: home, MOZ_HEADLESS: "1" },
  stdout: "ignore",
  stderr: "ignore",
});
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
let id = 0;
const pending = new Map<number, (m: any) => void>();
let ws: WebSocket;
function send(method: string, params: object = {}): Promise<any> {
  const n = ++id;
  return new Promise((resolve, reject) => {
    pending.set(n, (m) => (m.type === "error" ? reject(new Error(`${method}: ${m.error} ${m.message}`)) : resolve(m.result)));
    ws.send(JSON.stringify({ id: n, method, params }));
  });
}
const ev = async (context: string, expression: string) => {
  const r = await send("script.evaluate", { expression, target: { context }, awaitPromise: true });
  return r.type === "success" ? r.result.value : `exception: ${JSON.stringify(r.exceptionDetails?.text)}`;
};
try {
  for (let i = 0; i < 60 && !ws; i++) {
    try {
      const s = new WebSocket(`ws://127.0.0.1:${port}/session`);
      await new Promise<void>((res, rej) => ((s.onopen = () => res()), (s.onerror = () => rej(new Error("x")))));
      ws = s;
    } catch { await sleep(500); }
  }
  ws!.onmessage = (e) => { const m = JSON.parse(String(e.data)); if (m.id && pending.has(m.id)) { pending.get(m.id)!(m); pending.delete(m.id); } };
  await send("session.new", { capabilities: {} });
  console.log("installed:", JSON.stringify(await send("webExtension.install", { extensionData: { type: "path", path: extDir } })));
  await sleep(1500);
  const { context } = await send("browsingContext.create", { type: "tab" });
  await send("browsingContext.navigate", { context, url: "http://localhost:8089/", wait: "complete" });
  await sleep(500);
  console.log("focus:", await ev(context, "document.getElementById('u').focus(), document.activeElement.id"));
  let rect: any = null;
  for (let i = 0; i < 20 && !rect; i++) {
    await sleep(300);
    const r = await ev(context, `(() => { const h = [...document.documentElement.children].find((e) => e.shadowRoot); const b = h && h.shadowRoot.querySelector('.badge'); if (!b) return ''; const r = b.getBoundingClientRect(); return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2, ready: b.hasAttribute('data-ready') }); })()`);
    if (r && !String(r).startsWith("exception")) rect = JSON.parse(r as string);
  }
  console.log("badge:", JSON.stringify(rect));
  if (rect) {
    await send("input.performActions", { context, actions: [{ type: "pointer", id: "m", parameters: { pointerType: "mouse" }, actions: [
      { type: "pointerMove", x: Math.round(rect.x), y: Math.round(rect.y), origin: "viewport" }, { type: "pointerDown", button: 0 }, { type: "pointerUp", button: 0 } ] }] });
    await sleep(2500);
  }
  const tree = await send("browsingContext.getTree", {});
  console.log("contexts:", tree.contexts.map((c: any) => c.url).join(" | "));
  const win = tree.contexts.find((c: any) => String(c.url).includes("decide.html"));
  if (win) {
    console.log("decide window context found:", win.url);
    console.log("main text:", JSON.stringify(await ev(win.context, "document.getElementById('main')?.textContent")));
    console.log("site:", JSON.stringify(await ev(win.context, "document.getElementById('site')?.textContent")));
    // The IndexedDB area the background made (same origin as this window): list databases first, because opening a
    // missing one would create it.
    console.log("indexedDB:", await ev(win.context, `indexedDB.databases().then(async (dbs) => { const out = []; for (const d of dbs) { const db = await new Promise((res, rej) => { const o = indexedDB.open(d.name); o.onsuccess = () => res(o.result); o.onerror = () => rej(o.error); }); out.push({ name: d.name, version: d.version, stores: [...db.objectStoreNames] }); db.close(); } return JSON.stringify(out); })`));
    // Event page lifetime: the decision must still be there after Firefox's idle timeout would have suspended an idle page.
    const ask = `chrome.runtime.sendMessage({ kind: "decisionInfo", id: new URLSearchParams(location.search).get("id") }).then((r) => JSON.stringify(r), (e) => "reject: " + e)`;
    console.log("info now  :", await ev(win.context, ask));
    await sleep(Number(process.env.WAIT_MS ?? 0));
    console.log("info later:", await ev(win.context, ask));
    // Close: the answer is "cancel", the background closes the window.
    await ev(win.context, "[...document.querySelectorAll('button')].find((b) => b.textContent === 'Close')?.click(), 'clicked'");
    await sleep(1000);
    console.log("after Close:", (await send("browsingContext.getTree", {})).contexts.map((c: any) => c.url).join(" | "));
  }
} catch (e) {
  console.error("PROBE ERROR:", e);
} finally {
  try { ws!.close(); } catch {}
  ff.kill();
  await ff.exited;
  web.stop(true);
  rmSync(tmp, { recursive: true, force: true });
}
