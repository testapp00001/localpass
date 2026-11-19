// ASSISTED Chrome check against the REAL desktop app (Linux; also BROWSER=chromium|brave|vivaldi|edge). A person has to be at
// the screen.
//
//   terminal 1: cd app && LOCALPASS_PROFILE=audit bun run tauri dev     (any throwaway profile; its vault must be unlockable by
//               you; the app uses your real keyring. Optionally LOCALPASS_RAISE=present|remap for the 8.4 raise variants.)
//   terminal 2: cd extension && bun run build --e2e && [BROWSER=chromium] bun run e2e/assisted.ts [profile] [path to the localpass binary]
//
// A visible Chrome (puppeteer, throwaway user data dir, HOME and XDG folders; `--password-store=basic`; the native-messaging
// manifest goes into the throwaway user data dir, never into ~/.config/<browser>) gets `dist-e2e` (puppeteer installs the
// unpacked extension over CDP: Chrome 137+ ignores --load-extension; the manifest's key keeps the usual id). Its helper is the
// app's own binary (default target/debug/localpass, the one `tauri dev` builds), started with the real runtime folder and
// LOCALPASS_PROFILE, so it reaches the running app. The window shows "controlled by automated test software"; the LocalPass
// icon is in the Extensions (puzzle) menu, pin it to use the toolbar panel. Two tabs: the test page (a login form on
// http://localhost:<port>) and, in the background, the extension's popup page (the control tab). Steps, as in the Firefox script:
//   A. pairing; B/C. "Show all my logins" with the window hidden / open; D/E. the Unlock round trip from the field's badge with
//   the app hidden in the tray / open behind the browser (the panel, the vault status, the fill, the times, prompts.log).
// After each step it asks what only you saw. Every wait times out after 3 minutes (LP_ASSIST_STEP_S). The answers are printed
// at the end as a list for docs/handoff-phase8.md.
//
// Chrome opens the panel (`action.openPopup`) only for a window that has the focus: GNOME may open this window behind the
// others, so click into it once before you start.

import { join, resolve } from "node:path";
import type { Browser, Page, Target } from "puppeteer-core";
import {
  confirmTrial,
  describeBuilds,
  ensureLogin,
  Person,
  PromptsLog,
  requireAppListening,
  requireBuilds,
  say,
  TEST_PAGE,
  unlockRoundTrip,
  ARM_UNLOCK,
  type DecisionView,
  type RoundTripProbe,
  type StepDriver,
} from "./assist";
import { BrowserSetup, EXTENSION_ID, browserExe, browserKind, extensionDir, panelClick, panelText, sleep, toolbarBadge } from "./harness";

const withTimeout = <T>(p: Promise<T>, ms: number, what: string): Promise<T> =>
  Promise.race([p, sleep(ms).then(() => Promise.reject(new Error(`timed out: ${what}`)))]);

const isDecision = (t: Target) => t.url().startsWith(`chrome-extension://${EXTENSION_ID}/decide.html?`);

/** What the script sees in Chrome: decision pages (panel or window targets), the test page, the control page. */
export function chromeDriver(browser: Browser, page: Page, control: Page): StepDriver {
  const ids = new WeakMap<Target, string>();
  const pages = new Map<string, Promise<Page | null>>();
  let next = 0;
  const idOf = (t: Target) => {
    let id = ids.get(t);
    if (!id) ids.set(t, (id = `t${++next}`));
    return id;
  };
  const targetOf = (v: DecisionView) => browser.targets().find((t) => ids.get(t) === v.id);
  const pageOf = (v: DecisionView): Promise<Page | null> => {
    let p = pages.get(v.id);
    if (!p) {
      const t = targetOf(v);
      p = t ? t.asPage().catch(() => null) : Promise.resolve(null);
      pages.set(v.id, p);
    }
    return p;
  };
  const probe: RoundTripProbe = {
    views: async () => browser.targets().filter(isDecision).map((t): DecisionView => ({ id: idOf(t), kind: t.url().includes("presentation=panel") ? "panel" : "window" })),
    text: async (v) => {
      const p = await pageOf(v);
      return p ? withTimeout(panelText(p), 2000, "panel text") : "";
    },
    arm: async (v) => {
      const p = await pageOf(v);
      if (p) await withTimeout(p.evaluate(ARM_UNLOCK), 2000, "arm");
    },
    unlockPressedAt: () => control.evaluate(() => Number(localStorage.getItem("lp-assist-unlock") || 0)),
    clearUnlockPressed: () => control.evaluate(() => localStorage.removeItem("lp-assist-unlock")),
    vault: () =>
      control.evaluate(() => chrome.runtime.sendMessage({ kind: "state" }).then((r: { ok: boolean; data?: { vault: string | null }; code?: string }) => (r.ok ? String(r.data!.vault) : `error ${r.code}`))),
    fields: () =>
      page.evaluate(() => ({
        username: (document.getElementById("u") as HTMLInputElement).value,
        password: (document.getElementById("p") as HTMLInputElement).value.length,
      })),
    badge: () => withTimeout(toolbarBadge(browser), 3000, "toolbar badge"),
  };
  return {
    probe,
    async prepare() {
      for (const t of browser.targets().filter(isDecision)) {
        const p = await t.asPage().catch(() => null);
        await p?.close().catch(() => {});
      }
      await page.bringToFront().catch(() => {});
      await page.evaluate(() => {
        (document.getElementById("u") as HTMLInputElement).value = "";
        (document.getElementById("p") as HTMLInputElement).value = "";
        (document.activeElement as HTMLElement | null)?.blur();
      });
    },
    loadLogins: () =>
      control.evaluate(() =>
        chrome.runtime.sendMessage({ kind: "loadLogins" }).then((r: { ok: boolean; data?: unknown[]; code?: string }) => JSON.stringify(r.ok ? { ok: true, logins: r.data!.length } : { ok: false, code: r.code })),
      ),
    state: () =>
      control.evaluate(() =>
        chrome.runtime.sendMessage({ kind: "state" }).then((r: { ok: boolean; data?: { vault: string | null; logins: unknown[] }; code?: string }) =>
          r.ok ? { vault: String(r.data!.vault), logins: r.data!.logins.length } : { vault: `error ${r.code}`, logins: 0 },
        ),
      ),
  };
}

/** Opens the extension's popup page in a background tab (the service worker creates it, so the test tab stays in front). */
export async function openControlTab(browser: Browser): Promise<Page> {
  const worker = (await (await browser.waitForTarget((t) => t.type() === "service_worker" && t.url().includes(EXTENSION_ID), { timeout: 20000 })).worker())!;
  await worker.evaluate(() => chrome.tabs.create({ url: chrome.runtime.getURL("popup.html"), active: false }).then(() => "ok"));
  const target = await browser.waitForTarget((t) => t.url() === `chrome-extension://${EXTENSION_ID}/popup.html`, { timeout: 10000 });
  return (await target.asPage())!;
}

async function main() {
  const profile = process.argv[2] ?? "audit";
  const appExe = resolve(process.argv[3] ?? join(import.meta.dir, "../../target/debug/localpass"));
  const version = Bun.spawnSync([browserExe, "--version"], { stdout: "pipe", stderr: "ignore", env: { ...process.env, HOME: "/nonexistent" } }).stdout.toString().trim();
  const name = `${browserKind}: ${version || browserExe}`;
  const missing = describeBuilds({ browser: name, extensionDir, buildCommand: `bun run build --e2e && BROWSER=${browserKind} bun run e2e/assisted.ts`, appExe, profile });
  await requireAppListening(profile);
  requireBuilds(missing);
  const log = new PromptsLog(profile);
  console.log(`prompts.log: ${log.path}`);
  const person = new Person();
  const raise = await person.text("Set-up", "Which LOCALPASS_RAISE did you start the app with (present, remap, or Enter for none)?");

  const setup = new BrowserSetup({ appExe, profile });
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(TEST_PAGE, { headers: { "content-type": "text/html" } }) });
  const site = `http://localhost:${server.port}`;
  let browser: Browser | undefined;
  let failed = false;
  try {
    setup.registerHelper();
    log.mark();
    browser = await setup.launch();
    console.log(`browser: ${await browser.version()} (user data dir ${setup.userDataDir})`);
    const page = (await browser.pages())[0] ?? (await browser.newPage());
    await page.goto(`${site}/`);
    const control = await openControlTab(browser);
    const driver = chromeDriver(browser, page, control);
    await page.bringToFront();
    const controlText = () => control.evaluate(() => document.body.innerText);
    // `polling` by time: a background tab runs no animation frames, puppeteer's default.
    await control.waitForFunction(() => document.body.innerText.trim() !== "" && !/^Connecting/.test(document.body.innerText.trim()), { timeout: 20000, polling: 250 }).catch(() => {});
    say(`A ${browserKind} window opened (maybe behind this terminal). Click into it once so it has the focus; the LocalPass icon is in its Extensions (puzzle) menu (pin it if you want the toolbar panel).`);

    if ((await controlText()).includes("Connect to LocalPass")) {
      say("A. Pairing: approve the browser in the LocalPass app when it asks (compare the 6-digit code with the one the app shows).");
      // The control tab comes to the front for the click (puppeteer's wait needs animation frames; the popup in a tab is held
      // for 0.6 s, 8.3c), then the test page again.
      await control.bringToFront();
      await panelClick(control, "Connect");
      await page.bringToFront();
      const paired = await person.until("waiting for the pairing...", async () => !(await controlText()).includes("Connect to LocalPass"));
      person.note("A pairing", paired ? "paired" : "NOT paired");
      log.print("A");
      if (paired) {
        await person.yesNo("A pairing", "Did the app's pairing approval come to the front by itself?");
        await person.yesNo("A pairing", "Did the browser come back to the front by itself afterwards?");
      }
    } else console.log("(already paired in this throwaway profile)");

    await ensureLogin(person, driver, site);
    await confirmTrial(person, log, driver, "B confirm, window hidden", true);
    await confirmTrial(person, log, driver, "C confirm, window open", false);
    await unlockRoundTrip(person, log, driver, "D unlock, app hidden in the tray", true);
    await unlockRoundTrip(person, log, driver, "E unlock, app open behind the browser", false);

    say("Summary for docs/handoff-phase8.md (8.4):");
    console.log(person.summary(`8.4 assisted, ${name}, ${new Date().toISOString().slice(0, 10)}, profile ${profile}, LOCALPASS_RAISE=${raise}, app ${appExe}`));
    say(`This run leaves a paired client (the ${browserKind} extension) in the app's profile: forget it in Settings > Browser extension.`);
  } catch (e) {
    console.error(e);
    failed = true;
  } finally {
    server.stop(true);
    if (browser) await Promise.race([browser.close().catch(() => {}), sleep(8000)]);
    browser?.process()?.kill();
    const touched = setup.cleanup();
    console.log(touched ? `FAIL ${touched}` : `ok   the real ${browserKind} config was not touched`);
    if (touched) failed = true;
  }
  console.log(failed ? "assisted run failed" : "assisted run finished");
  process.exit(failed ? 1 : 0);
}

if (import.meta.main) await main();
