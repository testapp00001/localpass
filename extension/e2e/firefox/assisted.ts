// ASSISTED Firefox check against the REAL desktop app (Linux). A person has to be at the screen.
//
//   terminal 1: cd app && LOCALPASS_PROFILE=audit bun run tauri dev     (any throwaway profile; its vault must be unlockable by
//               you; the app uses your real keyring. Optionally LOCALPASS_RAISE=present|remap for the 8.4 raise variants.)
//   terminal 2: cd extension && bun run build --firefox --e2e && bun run e2e/firefox/assisted.ts [profile] [path to the localpass binary]
//
// A visible Firefox (throwaway profile and HOME; your real ~/.mozilla is not touched) gets the extension, and its native-messaging
// helper is the app's own binary (default target/debug/localpass, the one `tauri dev` builds). Two tabs: the test page (a login
// form on http://localhost:<port>) and, in the background, the extension's popup page (the control tab: the script asks the
// background through it and reads what the panel recorded). The script walks you through what only you can judge and prints
// what it can see itself:
//   A. pairing: approve the 6-digit code in the app;
//   B. "Show all my logins" with the LocalPass window HIDDEN (closed to the tray): the app must raise "Confirm it is you" (or, on
//      Wayland, GNOME may only show a "LocalPass is ready" notification: note which), and the reply says accepted or refused;
//   C. the same with the window open;
//   D. the Unlock round trip (8.4) with the app HIDDEN: you lock the vault in the app and close it to the tray, click the field's
//      badge, press Unlock in the panel, type the master password; the script reports the panel closing and coming back, the
//      vault status, the fill and the times, and the new lines of the app's prompts.log;
//   E. the same with the app window OPEN BEHIND the browser.
// After each step it asks what only you saw (window in front, cursor in the field, notification, browser back). Every wait
// times out after 3 minutes (LP_ASSIST_STEP_S). The answers are printed at the end as a list for docs/handoff-phase8.md.

import { join, resolve } from "node:path";
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
} from "../assist";
import { FirefoxHarness, GECKO_ID, extensionDir } from "./harness";

/**
 * Opens the extension's popup page in a background tab of the test tab's window (the control tab). BiDi cannot navigate a tab
 * to moz-extension://, so the browser window's own chrome scope adds the tab (in the background: the test tab stays in front).
 */
export async function openControlTab(h: FirefoxHarness): Promise<string> {
  const chromeContext = (await h.send("browsingContext.getTree", { "moz:scope": "chrome" })).contexts[0].context;
  await h.ev(
    chromeContext,
    `(() => { gBrowser.addTrustedTab(WebExtensionPolicy.getByID(${JSON.stringify(GECKO_ID)}).getURL("popup.html"), { inBackground: true }); return "ok"; })()`,
  );
  return (await h.waitFor("the control tab", async () => (await h.find("popup.html")).find((c) => !c.context.startsWith("panel:")), 15_000)).context;
}

/** What the script sees in Firefox: decision pages (panels through the harness's chrome-scope lookup), the test tab, the control tab. */
export function firefoxDriver(h: FirefoxHarness, tab: string, control: string): StepDriver {
  const probe: RoundTripProbe = {
    views: async () => (await h.find("decide.html")).map((c): DecisionView => ({ id: c.context, kind: c.context.startsWith("panel:") ? "panel" : "window" })),
    text: (v) => h.windowText(v.id),
    arm: async (v) => void (await h.ev(v.id, ARM_UNLOCK)),
    unlockPressedAt: () => h.ev<number>(control, `Number(localStorage.getItem('lp-assist-unlock') || 0)`),
    clearUnlockPressed: async () => void (await h.ev(control, `localStorage.removeItem('lp-assist-unlock')`)),
    vault: () => h.ev<string>(control, `chrome.runtime.sendMessage({ kind: "state" }).then((r) => r.ok ? String(r.data.vault) : 'error ' + r.code)`),
    fields: async () => JSON.parse(await h.ev<string>(tab, `JSON.stringify({ username: document.getElementById('u').value, password: document.getElementById('p').value.length })`)),
    badge: () => h.ev<string>(control, `chrome.tabs.query({ active: true, currentWindow: true }).then(([t]) => t ? chrome.action.getBadgeText({ tabId: t.id }) : '(no tab)')`),
  };
  return {
    probe,
    async prepare() {
      for (const c of await h.find("decide.html")) await h.closeContext(c.context).catch(() => {});
      await h.send("browsingContext.activate", { context: tab }).catch(() => {});
      await h.ev(tab, `document.getElementById('u').value = ''; document.getElementById('p').value = ''; document.activeElement?.blur(); 'ok'`);
    },
    loadLogins: () =>
      h.ev<string>(control, `chrome.runtime.sendMessage({ kind: "loadLogins" }).then((r) => JSON.stringify(r.ok ? { ok: true, logins: r.data.length } : { ok: false, code: r.code }))`),
    state: async () =>
      JSON.parse(await h.ev<string>(control, `chrome.runtime.sendMessage({ kind: "state" }).then((r) => JSON.stringify(r.ok ? { vault: String(r.data.vault), logins: r.data.logins.length } : { vault: 'error ' + r.code, logins: 0 }))`)),
  };
}

async function firefoxVersion(): Promise<string> {
  const out = Bun.spawnSync(["firefox", "--version"], { stdout: "pipe", stderr: "ignore", env: { ...process.env, HOME: "/nonexistent" } });
  return out.stdout.toString().trim() || "firefox (version unknown)";
}

async function main() {
  const profile = process.argv[2] ?? "audit";
  const appExe = resolve(process.argv[3] ?? join(import.meta.dir, "../../../target/debug/localpass"));
  const browser = await firefoxVersion();
  const missing = describeBuilds({ browser, extensionDir, buildCommand: "bun run build --firefox --e2e && bun run e2e/firefox/assisted.ts", appExe, profile });
  await requireAppListening(profile);
  requireBuilds(missing);
  const log = new PromptsLog(profile);
  console.log(`prompts.log: ${log.path}`);
  const person = new Person();
  const raise = await person.text("Set-up", "Which LOCALPASS_RAISE did you start the app with (present, remap, or Enter for none)?");

  const h = new FirefoxHarness({ appExe, profile });
  const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response(TEST_PAGE, { headers: { "content-type": "text/html" } }) });
  const site = `http://localhost:${server.port}`;
  try {
    h.registerHelper(appExe);
    log.mark();
    await h.startFirefox(extensionDir);
    const tab = await h.newTab(`${site}/`);
    const control = await openControlTab(h);
    const driver = firefoxDriver(h, tab, control);
    const controlText = () => h.ev<string>(control, "document.body.innerText");
    await h.waitFor("the control tab", async () => (await controlText()).trim() !== "" && !/^Connecting/.test((await controlText()).trim()), 20_000).catch(() => {});

    if ((await controlText()).includes("Connect to LocalPass")) {
      say("A. Pairing: approve the browser in the LocalPass app when it asks (compare the 6-digit code with the one the app shows).");
      await h.clickWindow(control, "Connect"); // the popup in a tab is held for 0.6 s (8.3c)
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
    console.log(person.summary(`8.4 assisted, ${browser}, ${new Date().toISOString().slice(0, 10)}, profile ${profile}, LOCALPASS_RAISE=${raise}, app ${appExe}`));
    say('This run leaves a paired client ("Firefox extension") in the app\'s profile: forget it in Settings > Browser extension.');
  } catch (e) {
    console.error(e);
    h.failures++;
  } finally {
    server.stop(true);
    await h.stop();
  }
  h.finish("assisted run finished");
}

if (import.meta.main) await main();
