// Spike driver: Firefox (headless, throwaway profile and HOME) + WebDriver BiDi.
// usage: bun run.ts <extension dir> [--real]
import { mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const extDir = process.argv[2];
const real = process.argv.includes("--real");
// --ext-page <addon id>: pin the add-on's internal UUID, open its popup in a tab and ask the background a question.
const extIdx = process.argv.indexOf("--ext-page");
const extId = extIdx > 0 ? process.argv[extIdx + 1] : undefined;
const UUID = "6f1d5a64-2f4c-4b0e-9a55-3c2b7c9d1e10";
const web = Bun.serve({
  port: 8089,
  hostname: "127.0.0.1",
  fetch(req) {
    const path = new URL(req.url).pathname;
    if (path === "/frame") {
      return new Response("<!doctype html><title>sandboxed</title><p>sandboxed frame</p>", {
        headers: { "content-type": "text/html", "content-security-policy": "sandbox allow-scripts" },
      });
    }
    return new Response(
      `<!doctype html><title>spike</title><form><input name=user><input type=password name=pw></form>
       <iframe src="/frame"></iframe>`,
      { headers: { "content-type": "text/html" } },
    );
  },
});

const tmp = mkdtempSync(join(tmpdir(), "lp-spike-"));
const profile = join(tmp, "profile");
const home = join(tmp, "home");
mkdirSync(profile);
mkdirSync(home);
writeFileSync(join(profile, "user.js"), [
  'user_pref("browser.shell.checkDefaultBrowser", false);',
  'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
  'user_pref("app.update.enabled", false);',
  'user_pref("extensions.autoDisableScopes", 0);',
  ...(extId ? [`user_pref("extensions.webextensions.uuids", ${JSON.stringify(JSON.stringify({ [extId]: UUID }))});`] : []),
].join("\n"));

const port = 9333;
const ff = Bun.spawn(["firefox", "--headless", "--no-remote", "--profile", profile, "--remote-debugging-port", String(port), "about:blank"], {
  env: { ...process.env, HOME: home, MOZ_HEADLESS: "1" },
  stdout: "ignore",
  stderr: "ignore",
});

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function connect(): Promise<WebSocket> {
  for (let i = 0; i < 60; i++) {
    try {
      const ws = new WebSocket(`ws://127.0.0.1:${port}/session`);
      await new Promise<void>((res, rej) => { ws.onopen = () => res(); ws.onerror = () => rej(new Error("x")); });
      return ws;
    } catch { await sleep(500); }
  }
  throw new Error("no BiDi endpoint");
}

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

try {
  ws = await connect();
  ws.onmessage = (e) => { const m = JSON.parse(String(e.data)); if (m.id && pending.has(m.id)) { pending.get(m.id)!(m); pending.delete(m.id); } };
  await send("session.new", { capabilities: {} });
  const installed = await send("webExtension.install", { extensionData: { type: "path", path: extDir } });
  console.log("installed:", JSON.stringify(installed));
  await sleep(3500); // let the module background finish (connectNative waits up to 3 s)
  const { context } = await send("browsingContext.create", { type: "tab" });
  await send("browsingContext.navigate", { context, url: "http://localhost:8089/", wait: "complete" });
  await sleep(2500);
  if (extId) {
    // Through the real content script and background: a passkey call makes the page ask the background (passkeysEnabled
    // reads the IndexedDB area, findLogins connects to a host that is not there) and then fall back to Firefox's own flow.
    const t0 = Date.now();
    const r = await send("script.evaluate", { expression: `navigator.credentials.get({ publicKey: { challenge: new Uint8Array(32), rpId: "localhost", timeout: 4000 } }).then(() => "resolved", (e) => e.name + ": " + e.message)`, target: { context }, awaitPromise: true });
    console.log("passkey get ->", r.type === "success" ? r.result.value : JSON.stringify(r.exceptionDetails), `(${Date.now() - t0} ms)`);
  }
  const tree = await send("browsingContext.getTree", { root: context });
  const all: any[] = [];
  const walk = (c: any) => { all.push(c); (c.children || []).forEach(walk); };
  tree.contexts.forEach(walk);
  const ev = async (ctx: string, expression: string) => {
    const r = await send("script.evaluate", { expression, target: { context: ctx }, awaitPromise: false });
    return r.type === "success" ? r.result.value : `exception: ${JSON.stringify(r.exceptionDetails?.text)}`;
  };
  for (const c of all) {
    console.log(`--- context ${c === all[0] ? "TOP" : "FRAME"} ${c.url}`);
    console.log("lpIso :", await ev(c.context, 'document.documentElement.dataset.lpIso || "(none)"'));
    console.log("lpMain:", await ev(c.context, 'document.documentElement.dataset.lpMain || "(none)"'));
    if (real) {
      console.log("page  :", await ev(c.context, 'JSON.stringify({passkeysWrapper: !!window.__localpassPasskeys, credentialsCreate: String(navigator.credentials.create).slice(0, 60)})'));
    }
  }
} catch (e) {
  console.error("SPIKE ERROR:", e);
} finally {
  try { ws?.close(); } catch {}
  ff.kill();
  await ff.exited;
  web.stop(true);
  rmSync(tmp, { recursive: true, force: true });
}
