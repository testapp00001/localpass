// End-to-end test in a real browser:
//   cargo build -p lp-broker --examples && cd extension && bun run build --e2e && bun run e2e/run.ts
//
// Starts the dev host (a broker with a throwaway vault) and two local web sites, launches Chrome (or
// Edge: `BROWSER=edge`) with the extension in a fresh profile, and drives the real flow: connect,
// fill, save a new login, refuse other sites, locked vault. The browser finds the native-messaging
// helper through a registry entry that this script sets for the run and restores afterwards.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import puppeteer, { type Browser, type Page } from "puppeteer-core";

const root = resolve(import.meta.dir, "../..");
const extensionDir = resolve(import.meta.dir, "../dist-e2e");
const devHostExe = join(root, "target/debug/examples/dev_host.exe");
const helperExe = join(root, "target/debug/examples/nmhost.exe");
const EXTENSION_ID = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const HOST_NAME = "app.localpass.host";

const browserKind = (process.env.BROWSER ?? "chrome").toLowerCase();
const browser = {
  chrome: { exe: "C:/Program Files/Google/Chrome/Application/chrome.exe", registry: `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}` },
  edge: { exe: "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", registry: `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${HOST_NAME}` },
}[browserKind as "chrome" | "edge"];
if (!browser) throw new Error(`unknown BROWSER ${browserKind}`);

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ---- the web sites under test ----

const loginPage = (title: string) => `<!doctype html><title>${title}</title>
<form method="post" action="/session" id="login">
  <input name="username" autocomplete="username" placeholder="Username">
  <input name="password" type="password" placeholder="Password">
  <button type="submit">Sign in</button>
</form>`;

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/login") return new Response(loginPage("Login"), { headers: { "content-type": "text/html" } });
    // Pages that try to collect clicks on the extension's interface (RA-1): one hides everything it can
    // reach with !important CSS, one paints a see-through decoy over it that ignores the mouse.
    if (url.pathname === "/attack-style") {
      const css = "<style>html > div { opacity: 0 !important; display: none !important; visibility: hidden !important; transform: scale(0) !important; }</style>";
      return new Response(loginPage("Hidden") + css, { headers: { "content-type": "text/html" } });
    }
    if (url.pathname === "/attack-cover") {
      // The decoy always sits after the extension's host element in the document, so it paints above it.
      const background = url.searchParams.get("bg") ?? "#fff";
      const script = `<script>const place = () => { if (window.stopDecoy) return;
        let d = document.getElementById("decoy");
        if (!d) { d = document.createElement("div"); d.id = "decoy";
          d.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none;background:${background}"; }
        if (document.documentElement.lastElementChild !== d) document.documentElement.append(d); };
        new MutationObserver(place).observe(document.documentElement, { childList: true });</script>`;
      return new Response(loginPage("Covered") + script, { headers: { "content-type": "text/html" } });
    }
    if (url.pathname === "/session") return new Response(null, { status: 303, headers: { location: "/dashboard" } });
    if (url.pathname === "/dashboard") return new Response("<!doctype html><title>Dashboard</title><h1>Welcome</h1>", { headers: { "content-type": "text/html" } });
    return new Response("not found", { status: 404 });
  },
});
const port = server.port;

// ---- the dev host (a broker with a throwaway vault) ----

const profile = `e2e${process.pid}`;
const host = Bun.spawn([devHostExe, "--login", `Local site|http://localhost:${port}|alice|alice-secret`], {
  env: { ...process.env, LOCALPASS_PROFILE: profile },
  stdin: "pipe",
  stdout: "pipe",
  stderr: "inherit",
});
const hostLines: string[] = [];
(async () => {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of host.stdout) {
    buffer += decoder.decode(chunk);
    let index;
    while ((index = buffer.indexOf("\n")) >= 0) {
      hostLines.push(buffer.slice(0, index).trim());
      buffer = buffer.slice(index + 1);
    }
  }
})();
async function waitForHost(predicate: (line: string) => boolean, timeoutMs = 15000): Promise<string> {
  const start = Date.now();
  let seen = 0;
  while (Date.now() - start < timeoutMs) {
    for (; seen < hostLines.length; seen++) if (predicate(hostLines[seen]!)) return hostLines[seen]!;
    await sleep(100);
  }
  throw new Error(`the dev host never printed the expected line; saw: ${hostLines.slice(-5).join(" | ")}`);
}
function hostCommand(command: string) {
  host.stdin.write(`${command}\n`);
  host.stdin.flush();
}
function vault(): { title: string; username: string; password: string; urls: string[] }[] {
  const last = [...hostLines].reverse().find((l) => l.startsWith("VAULT "));
  return last ? JSON.parse(last.slice(6)) : [];
}

// ---- the native-messaging registration, set for this run only ----

const reg = (...args: string[]) => Bun.spawnSync(["reg", ...args], { stdout: "pipe", stderr: "pipe" });
const previous = (() => {
  const result = reg("query", browser.registry, "/ve");
  if (result.exitCode !== 0) return null;
  return /REG_SZ\s+(.*)\r?$/m.exec(result.stdout.toString())?.[1]?.trim() ?? null;
})();
const workDir = mkdtempSync(join(tmpdir(), "lp-e2e-"));
const manifestPath = join(workDir, `${HOST_NAME}.json`);
writeFileSync(
  manifestPath,
  JSON.stringify({ name: HOST_NAME, description: "LocalPass e2e", path: helperExe, type: "stdio", allowed_origins: [`chrome-extension://${EXTENSION_ID}/`] }),
);
function restoreRegistry() {
  if (previous) reg("add", browser.registry, "/ve", "/d", previous, "/f");
  else reg("delete", browser.registry, "/f");
}
process.on("exit", restoreRegistry);
reg("add", browser.registry, "/ve", "/d", manifestPath, "/f");

// ---- helpers for the pages ----

/** Text of the on-page menu (inside the extension's open shadow root in the e2e build). */
async function menuText(page: Page): Promise<string> {
  const panel = await page.waitForSelector("pierce/.panel", { timeout: 8000 });
  return (await panel!.evaluate((el) => el.textContent ?? "")).trim();
}

async function openMenu(page: Page) {
  // Focus events only fire in the tab that is in front.
  await page.bringToFront();
  await page.focus('input[name="username"]');
  const badge = await page.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 });
  // A person rests the pointer on the badge before clicking: hover styling must not make the browser
  // report it as hidden (a CSS filter does), or the click would be ignored.
  await badge!.hover();
  await sleep(600);
  await badge!.click();
}

async function value(page: Page, name: string): Promise<string> {
  return page.$eval(`input[name="${name}"]`, (el) => (el as HTMLInputElement).value);
}

let chrome: Browser | undefined;
try {
  await waitForHost((l) => l.startsWith("READY"));

  chrome = await puppeteer.launch({
    executablePath: browser.exe,
    headless: true,
    userDataDir: join(workDir, "profile"),
    // Recent Chrome ignores --load-extension; puppeteer installs unpacked extensions over CDP.
    enableExtensions: [extensionDir],
    args: ["--no-first-run", "--no-default-browser-check"],
    env: { ...process.env, LOCALPASS_PROFILE: profile } as Record<string, string>,
  });

  // 1. Connect the browser to the app.
  const popup = await chrome.newPage();
  await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
  await popup.waitForFunction(() => document.body.innerText.includes("Connect to LocalPass"), { timeout: 20000 });
  check("the popup offers to connect an unknown browser", true);
  await popup.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.textContent === "Connect")!.click());
  const paired = await waitForHost((l) => l.startsWith("PAIRED"));
  check("the app was asked to approve the browser", /PAIRED .*extension code=\d{6}/.test(paired), paired);
  await popup.waitForFunction(() => !document.body.innerText.includes("Connect to LocalPass"), { timeout: 10000 });

  // 2. Fill a saved login on the site it belongs to.
  const page = await chrome.newPage();
  await page.goto(`http://localhost:${port}/login`);
  await sleep(500);
  await openMenu(page);
  const menu = await menuText(page);
  check("the menu lists the saved login for this site", menu.includes("Local site") && menu.includes("alice"), menu);
  await (await page.waitForSelector("pierce/.row[data-ready]", { timeout: 5000 }))!.click();
  await sleep(500);
  check("the username is filled", (await value(page, "username")) === "alice");
  check("the password is filled", (await value(page, "password")) === "alice-secret");

  // The stored broker secret is out of reach of content scripts (V26): ask from the content
  // script's own context (an isolated world named after the extension), and from the extension's
  // popup page as the control. Only key names are printed, never values.
  {
    const cdp = await page.createCDPSession();
    const contexts: { id: number; origin: string }[] = [];
    cdp.on("Runtime.executionContextCreated", (e) => contexts.push({ id: e.context.id, origin: e.context.origin }));
    await cdp.send("Runtime.enable");
    await sleep(300);
    const script = contexts.find((c) => c.origin === `chrome-extension://${EXTENSION_ID}`);
    check("the page's content-script context was found", !!script);
    const probe = async (area: string) =>
      script
        ? String(
            (
              await cdp.send("Runtime.evaluate", {
                contextId: script.id,
                awaitPromise: true,
                returnByValue: true,
                expression: `chrome.storage.${area}.get(null).then((r) => "READ " + Object.keys(r).join(","), (e) => "DENIED " + e.message)`,
              })
            ).result.value,
          )
        : "no context";
    const local = await probe("local");
    check("a content script cannot read storage.local", local.startsWith("DENIED"), local);
    const own = await popup.evaluate(() => chrome.storage.local.get(null).then((r) => Object.keys(r).sort().join(",")));
    check("the extension's own page can (so the check above means something)", own.includes("secret") && own.includes("clientId"), own);
    await cdp.detach();
  }

  // Signing in with the filled login does not offer to save it again.
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  await sleep(2500);
  check("no save prompt after signing in with a filled login", (await page.$("pierce/.bar")) === null);

  // 3. A login typed by hand is offered for saving, and saved on request.
  await page.goto(`http://localhost:${port}/login`);
  await page.type('input[name="username"]', "bob");
  await page.type('input[name="password"]', "bob-secret");
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  const bar = await page.waitForSelector("pierce/.bar", { timeout: 8000 });
  const barText = (await bar!.evaluate((el) => el.textContent ?? "")).replace(/\s+/g, " ");
  check("a new login is offered for saving", barText.includes("Save this login to LocalPass") && barText.includes("bob"), barText);
  await (await page.waitForSelector("pierce/.btn.primary[data-ready]", { timeout: 5000 }))!.click();
  await waitForHost((l) => l.startsWith("VAULT ") && l.includes('"bob"'), 10000);
  const bob = vault().find((i) => i.username === "bob");
  check("the new login reached the vault", bob?.password === "bob-secret" && bob.urls.some((u) => u.startsWith(`http://localhost:${port}`)), JSON.stringify(bob));

  // 4. Another site never gets these logins.
  const other = await chrome.newPage();
  await other.goto(`http://127.0.0.1:${port}/login`);
  await sleep(500);
  await openMenu(other);
  const otherMenu = await menuText(other);
  check("another site sees no saved logins", otherMenu.includes("No saved logins") && !otherMenu.includes("alice"), otherMenu);
  check("nothing was filled on the other site", (await value(other, "username")) === "");

  // 5. A locked vault is unlocked from the page.
  hostCommand("lock");
  await waitForHost((l) => l === "LOCKED");
  await page.goto(`http://localhost:${port}/login`);
  await sleep(500);
  await openMenu(page);
  check("a locked vault is reported", (await menuText(page)).includes("locked"));
  const unlock = await page.waitForSelector("pierce/.row[data-ready]", { timeout: 5000 });
  await unlock!.click();
  await sleep(1500);
  await page.waitForSelector("pierce/.panel", { timeout: 8000 });
  const afterUnlock = await menuText(page);
  check("after unlocking the logins appear", afterUnlock.includes("Local site"), afterUnlock);

  // 6. A page cannot hide the interface, or cover it, to collect clicks (RA-1).
  {
    const styled = await chrome.newPage();
    await styled.goto(`http://localhost:${port}/attack-style`);
    await sleep(500);
    await styled.bringToFront();
    await styled.focus('input[name="username"]');
    const badge = await styled.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 }).catch(() => null);
    check("page CSS with !important cannot hide the interface", badge !== null);
    const intact = await badge?.evaluate((el) => {
      const s = getComputedStyle(((el.getRootNode() as ShadowRoot).host as HTMLElement));
      return s.display !== "none" && s.visibility === "visible" && s.opacity === "1" && s.transform === "none";
    });
    check("the interface keeps its own display, visibility, opacity and transform", intact === true);

    for (const [name, background] of [
      ["an opaque decoy", "#fff"],
      ["a 99% opaque decoy", "rgba(255,255,255,.99)"],
      ["a 95% opaque decoy", "rgba(255,255,255,.95)"],
    ] as const) {
      const covered = await chrome.newPage();
      await covered.goto(`http://localhost:${port}/attack-cover?bg=${encodeURIComponent(background)}`);
      await sleep(500);
      await covered.bringToFront();
      await covered.focus('input[name="username"]');
      const shown = await covered.waitForSelector("pierce/.badge", { timeout: 8000 });
      await sleep(1500); // longer than the visibility delay and the hold-off together
      check(`${name} painted over the interface keeps it from reacting`, (await covered.$("pierce/.badge[data-ready]")) === null);
      await shown!.click(); // the decoy ignores the mouse, so this reaches the badge underneath
      await sleep(500);
      check(`a click through ${name} opens nothing`, (await covered.$("pierce/.panel")) === null);
      await covered.evaluate(() => {
        (window as unknown as { stopDecoy: boolean }).stopDecoy = true;
        document.getElementById("decoy")!.remove();
      });
      const ready = await covered.waitForSelector("pierce/.badge[data-ready]", { timeout: 6000 }).catch(() => null);
      const state = ready ? "" : await covered.$eval("pierce/.badge", (el) => el.outerHTML.slice(0, 200)).catch(() => "no badge");
      check("with the decoy gone the interface reacts again", ready !== null, state);
      await ready?.click();
      check("and then a click opens the menu", ready !== null && (await menuText(covered)).includes("Local site"));
      await covered.close();
    }
  }
} catch (e) {
  console.error(e);
  failures++;
} finally {
  await chrome?.close().catch(() => {});
  hostCommand("quit");
  host.kill();
  server.stop(true);
  restoreRegistry();
  try {
    rmSync(workDir, { recursive: true, force: true });
  } catch {}
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("all end-to-end checks passed");
