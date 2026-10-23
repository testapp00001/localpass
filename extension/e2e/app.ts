// Full-system test: the real LocalPass app (its UI driven over WebView2's debugging port), the
// native-messaging registration it writes, Chrome (or Edge) with the extension, and a local site.
//
//   cargo build -p localpass --features custom-protocol && cd extension && bun run build --e2e
//   bun run e2e/app.ts          (BROWSER=edge for Edge)
//
// The app runs with its own profile and WebView2 folder, so a LocalPass you have open is not touched.
// The browser registration is added through the app's own Settings page and removed again at the end.

import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import puppeteer, { type Browser, type Page } from "puppeteer-core";
import { startRelyingParty } from "./rp";

const root = resolve(import.meta.dir, "../..");
const extensionDir = resolve(import.meta.dir, "../dist-e2e");
// LP_APP_EXE: a build made in another target directory (so a localpass.exe you use is not replaced).
const appExe = process.env.LP_APP_EXE ?? join(root, "target/debug/localpass.exe");
const EXTENSION_ID = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const HOST_KEY = (browser: string) => `HKCU\\Software\\${browser === "edge" ? "Microsoft\\Edge" : "Google\\Chrome"}\\NativeMessagingHosts\\app.localpass.host`;
const browserKind = (process.env.BROWSER ?? "chrome").toLowerCase();
const browserExe = browserKind === "edge" ? "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe" : "C:/Program Files/Google/Chrome/Application/chrome.exe";

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const reg = (...args: string[]) => Bun.spawnSync(["reg", ...args], { stdout: "pipe", stderr: "pipe" });
const registered = (browser: string) => reg("query", HOST_KEY(browser), "/ve").exitCode === 0;

// ---- a local site ----

const site = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/login")
      return new Response(
        `<!doctype html><title>Login</title><form method="post" action="/session"><input name="username" autocomplete="username"><input name="password" type="password"><button>Sign in</button></form>`,
        { headers: { "content-type": "text/html" } },
      );
    return new Response("<!doctype html><title>Done</title>ok", { headers: { "content-type": "text/html" } });
  },
});

// ---- the app, with its own profile and WebView2 folder ----

const workDir = mkdtempSync(join(tmpdir(), "lp-app-e2e-"));
const profile = `e2eapp${process.pid}`;
const debugPort = 9300 + (process.pid % 500);
const app = Bun.spawn([appExe, "--minimized"], {
  env: {
    ...process.env,
    LOCALPASS_PROFILE: profile,
    WEBVIEW2_USER_DATA_FOLDER: join(workDir, "webview"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
  },
  stdout: "ignore",
  stderr: "ignore",
});

/** Minimal DevTools client for the app's web view. */
async function connectToApp() {
  let target: any;
  for (let i = 0; i < 100 && !target; i++) {
    try {
      const targets: any[] = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
      target = targets.find((t) => t.type === "page");
    } catch {}
    if (!target) await sleep(200);
  }
  if (!target) throw new Error("the app's web view never appeared");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise<void>((res, rej) => {
    ws.onopen = () => res();
    ws.onerror = rej;
  });
  let id = 0;
  const pending = new Map<number, (m: any) => void>();
  ws.onmessage = (ev) => {
    const m = JSON.parse(String(ev.data));
    pending.get(m.id)?.(m);
  };
  return {
    async eval<T>(expression: string): Promise<T> {
      const my = ++id;
      const reply = await new Promise<any>((resolve) => {
        pending.set(my, resolve);
        ws.send(JSON.stringify({ id: my, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
      });
      if (reply.result?.exceptionDetails) throw new Error(JSON.stringify(reply.result.exceptionDetails.exception?.description ?? reply.result.exceptionDetails));
      return reply.result.result.value as T;
    },
    close: () => ws.close(),
  };
}

const text = (ui: Awaited<ReturnType<typeof connectToApp>>) => ui.eval<string>("document.body.innerText.replace(/\\s+/g, ' ')");
async function waitForText(ui: Awaited<ReturnType<typeof connectToApp>>, needle: string, timeoutMs = 20000) {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    // innerText applies CSS text-transform, so section titles come back in capitals.
    if ((await text(ui)).toLowerCase().includes(needle.toLowerCase())) return;
    await sleep(250);
  }
  throw new Error(`the app never showed "${needle}"; it shows: ${(await text(ui)).slice(0, 300)}`);
}
const clickButton = (ui: Awaited<ReturnType<typeof connectToApp>>, label: string) =>
  ui.eval(`(() => { const b = [...document.querySelectorAll('button')].filter(x => x.offsetParent !== null).find(x => x.textContent.trim().includes(${JSON.stringify(label)})); if (!b) throw new Error('no button ${label}'); b.click(); return true; })()`);

let chrome: Browser | undefined;
let rp: ReturnType<typeof startRelyingParty> | undefined;
let ui: Awaited<ReturnType<typeof connectToApp>> | undefined;
const hadRegistration = registered(browserKind);
try {
  ui = await connectToApp();
  await waitForText(ui, "Create a new vault");
  check("the app starts with the welcome screen", true);

  // A vault with one login for the local site (through the app's own commands).
  const invoke = (command: string, args: object) => ui!.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`);
  await invoke("create_vault", { password: "end to end master password", deviceName: "E2E PC" });
  await invoke("save_item", { input: { title: "Local site", username: "alice", password: "alice-secret", urls: [`http://localhost:${site.port}`] } });
  await waitForText(ui, "Local site");
  check("the vault shows the login", true);

  // Settings → Browser extension.
  await clickButton(ui, "Settings");
  await waitForText(ui, "Browser extension");
  check("Settings has a Browser extension section", true);
  check("the browser is not set up yet", (await text(ui)).includes("Not set up yet") && !hadRegistration);
  await clickButton(ui, "Set up");
  await waitForText(ui, "Set up for Chrome and Edge");
  check("Set up registers Chrome and Edge", registered("chrome") && registered("edge"));
  check("the extension folder is offered", (await text(ui)).includes("extension"));

  // The browser connects; the app asks the user to approve it.
  chrome = await puppeteer.launch({
    executablePath: browserExe,
    headless: true,
    userDataDir: join(workDir, "browser"),
    enableExtensions: [extensionDir],
    args: ["--no-first-run", "--no-default-browser-check"],
    env: { ...process.env, LOCALPASS_PROFILE: profile } as Record<string, string>,
  });
  const popup = await chrome.newPage();
  await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
  await popup.waitForFunction(() => document.body.innerText.includes("Connect to LocalPass"), { timeout: 30000 });
  await popup.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.textContent === "Connect")!.click());
  await popup.waitForFunction(() => /\d{6}/.test(document.querySelector(".code")?.textContent ?? ""), { timeout: 15000 });
  const shown = await popup.$eval(".code", (el) => el.textContent ?? "");
  await waitForText(ui, "Connect a browser?");
  const prompt = await text(ui);
  check("the app asks to approve the browser with the same code", prompt.includes(shown), `popup shows ${shown}`);
  await clickButton(ui, "Connect");
  await popup.waitForFunction(() => !document.body.innerText.includes("Confirm in LocalPass"), { timeout: 15000 });
  await waitForText(ui, "Connected browsers");
  await sleep(500);
  check("the connected browser is listed in Settings", /Last used/.test(await text(ui)));

  // A login is filled on its own site.
  const page = await chrome.newPage();
  await page.goto(`http://localhost:${site.port}/login`);
  await page.bringToFront();
  await page.focus('input[name="username"]');
  const badge = await page.waitForSelector("pierce/.badge[data-ready]", { timeout: 10000 });
  await badge!.click();
  const row = await page.waitForSelector("pierce/.row[data-ready]", { timeout: 8000 });
  await row!.click();
  await sleep(600);
  const filled = await page.evaluate(() => [(document.querySelector('input[name="username"]') as HTMLInputElement).value, (document.querySelector('input[name="password"]') as HTMLInputElement).value]);
  check("the login is filled from the real app", filled[0] === "alice" && filled[1] === "alice-secret", JSON.stringify(filled));

  // Passkeys through the real app. Windows Hello is switched off for this test profile only
  // (its settings live in the profile's own folder), so no prompt appears on the desktop.
  const settings = await invoke("get_settings", {});
  await invoke("save_settings", { settings: { ...(settings as object), passkeyVerifyEveryTime: false } });
  // "discouraged": with Windows Hello switched off above, the real app then asks nothing (any other value makes it ask Hello, which nobody answers here).
  rp = startRelyingParty({ userVerification: "discouraged" });
  const rpStatus = async (p: Page, expected: string) => {
    for (let i = 0; i < 100; i++) {
      const t = await p.$eval("#status", (el) => el.textContent ?? "");
      if (t === expected || t.startsWith("error:")) return t;
      await sleep(150);
    }
    return "timeout";
  };
  const rpPage = await chrome.newPage();
  await rpPage.goto(`${rp.origin}/`);
  await rpPage.bringToFront();
  await rpPage.click("#register");
  await (await rpPage.waitForSelector("pierce/.btn.primary[data-ready]", { timeout: 10000 }))!.click();
  const registered_ = await rpStatus(rpPage, "registered");
  const sheetNow = await rpPage.$eval("pierce/.sheet", (el) => el.textContent ?? "").catch(() => "(no dialog)");
  check("a passkey is created through the real app and the site accepts it", registered_ === "registered", registered_ === "registered" ? "" : `${registered_}; dialog: ${sheetNow.slice(0, 200)}`);
  const vaultItems = (await invoke("list_items", { query: {} })) as { kind: string; title: string }[];
  check("the passkey is in the app's vault", vaultItems.some((i) => i.kind === "passkey"), JSON.stringify(vaultItems.map((i) => i.kind)));
  const signIn = await chrome.newPage();
  await signIn.goto(`${rp.origin}/`);
  await signIn.bringToFront();
  await signIn.click("#signin");
  await (await signIn.waitForSelector("pierce/.row[data-ready]", { timeout: 10000 }))!.click();
  check("signing in with it through the real app works", (await rpStatus(signIn, "signed-in")) === "signed-in");

  // Locking the app makes the extension ask for an unlock; unlocking in the app lets it continue.
  await invoke("lock", {});
  await page.goto(`http://localhost:${site.port}/login`);
  await page.bringToFront(); // focus events only fire in the tab in front
  await sleep(500);
  await page.focus('input[name="username"]');
  await (await page.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 }))!.click();
  const lockedMenu = await (await page.waitForSelector("pierce/.panel", { timeout: 8000 }))!.evaluate((el) => el.textContent ?? "");
  check("a locked app is reported to the page", lockedMenu.includes("locked"), lockedMenu);

  // Disconnecting in Settings locks the browser out.
  await invoke("unlock", { password: "end to end master password" });
  await waitForText(ui, "Local site");
  await clickButton(ui, "Settings");
  await waitForText(ui, "Connected browsers");
  await (await ui.eval<boolean>(`(() => { const b = document.querySelector('button[aria-label="Disconnect"]'); if (!b) return false; b.click(); return true; })()`));
  await sleep(500);
  check("disconnecting removes the browser from the list", (await text(ui)).includes("None yet"));

  // Remove the registration through Settings.
  await clickButton(ui, "Remove");
  await waitForText(ui, "Not set up yet");
  check("Remove undoes the registration", !registered("chrome") && !registered("edge"));
} catch (e) {
  console.error(e);
  failures++;
} finally {
  await chrome?.close().catch(() => {});
  ui?.close();
  app.kill();
  site.stop(true);
  rp?.stop();
  // Leave the machine as it was: no registration unless there was one before.
  if (!hadRegistration) for (const b of ["chrome", "edge"]) reg("delete", HOST_KEY(b), "/f");
  await sleep(1000);
  try {
    rmSync(workDir, { recursive: true, force: true });
    const appData = process.env.APPDATA ?? "";
    rmSync(join(appData, "app.localpass", "profiles", profile), { recursive: true, force: true });
  } catch {}
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("the real app, the registration and the browser work together");
