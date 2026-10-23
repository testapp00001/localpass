// Saving a new password from Android's autofill prompt, on a real system image: Chrome -> the
// system's "Save password?" sheet -> the LocalPass autofill service -> the vault.
//
//   cd app/src-tauri/gen/android && ./gradlew.bat :app:assembleX86_64Debug -x rustBuildX86_64Debug
//   (start AVD lp_api37, unlock it; a vault with fingerprint unlock is created if there is none)
//   cd extension && bun run e2e/android-save.ts [path\to\app-x86_64-debug.apk]
//   (LP_SKIP_INSTALL=1 reuses the installed build)
//
// A local login page is served on the host and reached through `adb reverse`; Chrome opens it as
// http://127.0.0.1:PORT (Android reports the website without the port). The page is driven over
// Chrome's DevTools, the system sheets with uiautomator, the vault is read back through the app's
// own web view. Every login the test saves is removed again, and the autofill setting and port
// forwards are restored.

import { resolve } from "node:path";
import puppeteer, { type Browser, type Page } from "puppeteer-core";
import { adb, find, key, labels, screen, scrollTo, shell, sleep, tap, tapNode, type, waitFor } from "./adb";
import { MASTER_PASSWORD, PACKAGE, connectToApp } from "./android";

const CHROME = "com.android.chrome";
const AUTOFILL = `${PACKAGE}/${PACKAGE}.LpAutofillService`;
const APK = resolve(import.meta.dir, "../../app/src-tauri/gen/android/app/build/outputs/apk/x86_64/debug/app-x86_64-debug.apk");
const HOST = "127.0.0.1";
const run = Date.now().toString(36);
const user = (name: string) => `e2e-${name}-${run}`;

let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};

const html = (body: string) => new Response(`<!doctype html><meta name="viewport" content="width=device-width"><title>Test</title>${body}`, { headers: { "content-type": "text/html" } });
const loginForm = `<form method="post" action="/session"><input name="username" autocomplete="username" placeholder="Username"><input name="password" type="password" placeholder="Password"><button id="go">Sign in</button></form>`;
// A single-page login: the form is hidden by script and the page never reloads.
const spaForm = `<form id="f" onsubmit="this.style.display='none';document.getElementById('done').hidden=false;return false"><input name="username" autocomplete="username" placeholder="Username"><input name="password" type="password" placeholder="Password"><button id="go">Sign in</button></form><p id="done" hidden>Signed in</p>`;
const searchForm = `<form method="get" action="/found"><input name="q" placeholder="Search"><button id="go">Search</button></form>`;
const site = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(req) {
    const path = new URL(req.url).pathname;
    if (req.method === "POST") return html("<p>Signed in</p>");
    if (path === "/spa") return html(spaForm);
    if (path === "/search") return html(searchForm);
    if (path === "/found") return html("<p>Results</p>");
    return html(loginForm);
  },
});
const port = String(site.port);

const autofillBefore = shell("settings", "get", "secure", "autofill_service");
let browser: Browser | undefined;
let app: Awaited<ReturnType<typeof connectToApp>> | undefined;
let inApp: <T>(call: () => Promise<T>) => Promise<T> = () => Promise.reject(new Error("not ready"));

try {
  // ---- set up ----
  const apk = process.argv[2] ?? APK;
  if (process.env.LP_SKIP_INSTALL !== "1") check("the app installs", adb("install", "-r", apk).includes("Success"), apk);
  shell("pm", "grant", PACKAGE, "android.permission.ACCESS_LOCAL_NETWORK");
  adb("reverse", `tcp:${port}`, `tcp:${port}`);
  shell("am", "force-stop", CHROME);
  shell("input", "keyevent", "KEYCODE_HOME");
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(8000);
  const wait = find(/^Wait$/);
  if (wait) tapNode(wait);
  app = await connectToApp();
  await app.invoke("app_info");
  await sleep(5000);
  inApp = async <T,>(call: () => Promise<T>): Promise<T> => {
    shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
    await sleep(700);
    return call();
  };
  const status = () => inApp(async () => ((await app!.invoke("app_info")) as any).status as string);
  const ensureUnlocked = async () => {
    if ((await status()) === "locked") await inApp(() => app!.invoke("unlock", { password: MASTER_PASSWORD }));
  };
  const lock = () => inApp(() => app!.invoke("lock"));
  if ((await status()) === "noVault") await inApp(() => app!.invoke("create_vault", { password: MASTER_PASSWORD, deviceName: "Test phone" }));
  await ensureUnlocked();
  check("the vault is open", (await status()) === "unlocked");

  const quick = () => inApp(async () => ((await app!.invoke("app_info")) as any).quickUnlock.enabled === true);
  if (!(await quick())) {
    const enabling = app.invoke("quick_unlock_enable").catch((e) => String(e));
    await waitFor("Turn on fingerprint unlock", 15000);
    await sleep(800);
    adb("emu", "finger", "touch", "1");
    await enabling;
  }
  check("fingerprint unlock is on", await quick());
  const fingerprint = async (prompt: string) => {
    await waitFor(prompt, 20000);
    await sleep(1200);
    adb("emu", "finger", "touch", "1");
  };

  /** Logins in the vault typed by this run for `name`, with their passwords (read through the app). */
  type Saved = { id: string; title: string; username: string; password: string; urls: string[]; updatedMs: number; history: string[] };
  const savedFor = (name: string): Promise<Saved[]> =>
    inApp(async () => {
      const all = (await app!.invoke("list_items", { query: {} })) as { id: string; kind: string }[];
      const found: Saved[] = [];
      for (const item of all) {
        if (item.kind !== "login") continue;
        const v = (await app!.invoke("get_item", { id: item.id })) as any;
        if (v.username === name) {
          found.push({ id: v.id, title: v.title, username: v.username, password: v.password, urls: v.urls, updatedMs: v.updatedMs, history: (v.passwordHistory ?? []).map((h: any) => h.value) });
        }
      }
      return found;
    });

  /** For failure messages: every login this test has made, whoever the user name is. */
  const allTestLogins = () =>
    inApp(async () => {
      const all = (await app!.invoke("list_items", { query: {} })) as { id: string; subtitle: string }[];
      return all.filter((i) => i.subtitle.startsWith("e2e-")).map((i) => i.subtitle);
    });

  // ---- Chrome: hand pages to a third-party autofill service ----
  shell("settings", "put", "secure", "autofill_service", AUTOFILL);
  shell("am", "force-stop", CHROME);
  shell("am", "start", "-a", "android.intent.action.VIEW", "-d", "about:blank", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
  await sleep(4000);
  await tap(/More options/);
  await sleep(1000);
  await tap(/^Settings$/);
  await sleep(1500);
  tapNode(await scrollTo(/^Autofill services$/));
  await sleep(1000);
  await tap(/^Autofill using another service$/);
  await sleep(1500);
  const restart = find("Restart Chrome");
  if (restart) tapNode(restart);
  await sleep(5000);
  shell("am", "force-stop", CHROME);
  shell("am", "start", "-a", "android.intent.action.VIEW", "-d", `http://${HOST}:${port}/`, "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
  await sleep(4000);
  adb("forward", "tcp:9444", "localabstract:chrome_devtools_remote");
  for (let i = 0; i < 20 && !browser; i++) {
    try {
      browser = await puppeteer.connect({ browserURL: "http://127.0.0.1:9444", defaultViewport: null });
    } catch {
      await sleep(1000);
    }
  }
  if (!browser) throw new Error("could not reach Chrome's DevTools");

  /** Opens a page of the test site in Chrome, in front. */
  const open = async (path = "/"): Promise<Page> => {
    shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(700);
    const pages = await browser!.pages();
    const tab = pages.find((p) => p.url().includes(`:${port}`)) ?? pages[0]!;
    await tab.goto(`http://${HOST}:${port}${path}`);
    await tab.bringToFront();
    await sleep(800);
    return tab;
  };
  /**
   * Types a username and password with real key events in the page (`adb input text` loses
   * characters while the autofill dropdown is open).
   * The password field is reached with the Tab key, not tapped: once a login exists for the site
   * the autofill dropdown opens under the username field, right over the password field.
   */
  const typeLogin = async (tab: Page, name: string, password: string) => {
    await tab.click('input[name="username"]');
    await sleep(1200);
    await tab.keyboard.type(name, { delay: 30 });
    await sleep(300);
    key("KEYCODE_TAB");
    await sleep(1200);
    await tab.keyboard.type(password, { delay: 30 });
    await sleep(500);
  };
  /** Sends the form with the keyboard's Enter key (a tap on the button could hit the dropdown). */
  const submit = async () => {
    key("KEYCODE_ENTER");
  };
  /** Sends the form by tapping its button, for when the app was in front for a moment. */
  const submitByTap = async (tab: Page) => {
    shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(900);
    await tab.bringToFront();
    await tab.click("#go");
  };
  const saveSheet = () => waitFor(/^Save$/, 25000);
  const noSheet = async (ms = 6000) => {
    await sleep(ms);
    return !find(/^Save$/) && !find(/^No thanks$/);
  };
  const dismissToast = () => sleep(1500);

  // ---- 1. a login LocalPass does not have: unlocked vault, tap Save ----
  const alice = user("new");
  let tab = await open("/");
  await typeLogin(tab, alice, "first-pass-1");
  await submit();
  await saveSheet();
  const sheetText = labels().join(" | ");
  check("the system offers to save, naming LocalPass", /LocalPass/.test(sheetText), sheetText.slice(0, 160));
  await tap(/^Save$/);
  await dismissToast();
  let saved = await savedFor(alice);
  check("tapping Save stores the login (one item)", saved.length === 1, `${saved.length} item(s)`);
  check("with the typed password, the site's address and its name as title", saved[0]?.password === "first-pass-1" && JSON.stringify(saved[0]?.urls) === JSON.stringify([`http://${HOST}`]) && saved[0]?.title === HOST, JSON.stringify(saved[0]));

  // ---- 2. dismissing the prompt saves nothing ----
  const bob = user("cancel");
  tab = await open("/");
  await typeLogin(tab, bob, "never-saved-1");
  await submit();
  await saveSheet();
  await tap(/^No thanks$/);
  await sleep(2500);
  check("choosing No thanks saves nothing", (await savedFor(bob)).length === 0);

  // ---- 3. the same account with a changed password updates the item ----
  tab = await open("/");
  await typeLogin(tab, alice, "second-pass-2");
  await submit();
  await saveSheet();
  await tap(/^Save$/);
  await dismissToast();
  saved = await savedFor(alice);
  check("a changed password updates the login instead of adding another", saved.length === 1 && saved[0]!.password === "second-pass-2", `${saved.length} item(s): ${JSON.stringify(saved.map((x) => x.password))}`);
  check("the old password is kept as history", saved[0]?.history.includes("first-pass-1") === true);

  // ---- 4. a locked vault: Save opens the unlock screen, the fingerprint unlocks and saves ----
  const carol = user("locked");
  tab = await open("/");
  await typeLogin(tab, carol, "locked-pass-1");
  await lock();
  await submitByTap(tab);
  await saveSheet();
  await tap(/^Save$/);
  await fingerprint("Unlock LocalPass");
  await sleep(4000);
  check("the vault is unlocked afterwards", (await status()) === "unlocked");
  saved = await savedFor(carol);
  check("after the fingerprint the login is saved", saved.length === 1 && saved[0]!.password === "locked-pass-1", `${saved.length} item(s); vault has ${JSON.stringify(await allTestLogins())}`);

  // ---- 5. a locked vault: backing out of the unlock saves nothing ----
  const dave = user("locked-cancel");
  tab = await open("/");
  await typeLogin(tab, dave, "locked-pass-2");
  await lock();
  await submitByTap(tab);
  await saveSheet();
  await tap(/^Save$/);
  await waitFor("Unlock LocalPass", 20000);
  await tap("Use master password");
  await waitFor(/^cancel$/i, 10000);
  await tap(/^cancel$/i);
  await sleep(2500);
  check("the vault stayed locked", (await status()) === "locked");
  await ensureUnlocked();
  check("backing out of the unlock saves nothing", (await savedFor(dave)).length === 0);

  // ---- 5b. a locked vault: the master password unlocks and saves too ----
  const frank = user("password");
  tab = await open("/");
  await typeLogin(tab, frank, "locked-pass-3");
  await lock();
  await submitByTap(tab);
  await saveSheet();
  await tap(/^Save$/);
  await waitFor("Unlock LocalPass", 20000);
  await tap("Use master password");
  const field = await waitFor(/EditText/, 10000).catch(() => undefined);
  tapNode(field ?? screen().find((n) => n.cls.endsWith("EditText"))!);
  await sleep(500);
  type(MASTER_PASSWORD);
  key("KEYCODE_BACK"); // close the keyboard
  await sleep(500);
  await tap(/^unlock$/i);
  await sleep(4000);
  saved = await savedFor(frank);
  check("the master password unlocks and the login is saved", saved.length === 1 && saved[0]!.password === "locked-pass-3", `${saved.length} item(s)`);

  // ---- 6. a login form that never reloads is offered for saving once its fields disappear ----
  const erin = user("spa");
  tab = await open("/spa");
  await typeLogin(tab, erin, "spa-pass-1");
  await submit();
  let spaOffered = true;
  try {
    await saveSheet();
    await tap(/^Save$/);
    await dismissToast();
  } catch {
    spaOffered = false;
  }
  check("a single-page login is offered for saving", spaOffered, labels().slice(0, 8).join(" | "));
  if (spaOffered) check("and saved", (await savedFor(erin)).length === 1);

  // ---- 7. a search box is not a login ----
  tab = await open("/search");
  await tab.click('input[name="q"]');
  await sleep(900);
  await tab.keyboard.type("some words", { delay: 30 });
  await sleep(300);
  await submit();
  check("a search is never offered for saving", await noSheet());
} catch (e) {
  console.error(e);
  console.error("screen:", labels().slice(0, 20).join(" | "));
  failures++;
} finally {
  // Remove what the run saved, then put the phone's settings back.
  try {
    if (app) {
      await inApp(async () => {
        if (((await app!.invoke("app_info")) as any).status === "locked") await app!.invoke("unlock", { password: MASTER_PASSWORD });
        const all = (await app!.invoke("list_items", { query: {} })) as { id: string; subtitle: string }[];
        for (const item of all) {
          if (item.subtitle.startsWith("e2e-")) {
            await app!.invoke("trash_item", { id: item.id });
            await app!.invoke("purge_item", { id: item.id });
          }
        }
      });
    }
  } catch (e) {
    console.error("could not remove the test logins:", String(e));
  }
  await browser?.disconnect().catch(() => {});
  await app?.browser.disconnect().catch(() => {});
  shell("am", "force-stop", CHROME);
  shell("settings", "put", "secure", "autofill_service", autofillBefore);
  adb("reverse", "--remove", `tcp:${port}`);
  adb("forward", "--remove", "tcp:9444");
  adb("forward", "--remove", "tcp:9333");
  site.stop(true);
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("saving a new password from Android's autofill prompt works: Chrome, the system sheet and LocalPass");
process.exit(0);
