// Smoke test of the signed RELEASE apk on an emulator (AVD lp_api37): first start, creating a vault,
// the "Autofill and passkeys" section, saving a password, filling it, and creating and using a passkey.
//
// A release build has no web view debugging, so the app itself is driven only with uiautomator (this
// is what `android.ts` / `android-save.ts` / `android-setup.ts` cannot do: they read and call the
// app through its debug web view, so they need a debug build). Chrome is still debuggable, so the
// pages are driven over its DevTools exactly as in those scripts.
//
//   cd extension && bun run e2e/android-release.ts path\to\localpass-<version>-universal-release.apk
//
// It removes any installed LocalPass first (a clean first start; a release build is signed with
// another key than a debug build, which Android refuses to install over), so the emulator's
// LocalPass data is wiped. The
// autofill and credential provider settings and the port forwards are put back at the end.

import puppeteer, { type Browser, type Page } from "puppeteer-core";
import { adb, find, key, labels, screen, scrollTo, shell, sleep, tapNode, type, waitFor } from "./adb";
import { startRelyingParty } from "./rp";

const PACKAGE = "app.localpass";
const CHROME = "com.android.chrome";
const AUTOFILL = `${PACKAGE}/${PACKAGE}.LpAutofillService`;
const PROVIDER = `${PACKAGE}/${PACKAGE}.LpCredentialProviderService`;
const MASTER_PASSWORD = "release smoke master password";
const HOST = "127.0.0.1";
const apk = process.argv[2];
if (!apk) {
  console.error("usage: bun run e2e/android-release.ts <path to the release apk>");
  process.exit(2);
}

let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};
const secure = (k: string) => shell("settings", "get", "secure", k);
const restore = (k: string, v: string) => (v && v !== "null" ? shell("settings", "put", "secure", k, v) : shell("settings", "delete", "secure", k));
const before = { autofill: secure("autofill_service"), providers: secure("credential_service"), primary: secure("credential_service_primary") };

const rp = startRelyingParty();
const rpPort = new URL(rp.origin).port;
const login = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: (req) =>
    new Response(
      req.method === "POST"
        ? "<!doctype html><title>Done</title><p>Signed in</p>"
        : `<!doctype html><meta name="viewport" content="width=device-width"><title>Login</title><form method="post" action="/session"><input name="username" autocomplete="username" placeholder="Username"><input name="password" type="password" placeholder="Password"><button id="go">Sign in</button></form>`,
      { headers: { "content-type": "text/html" } },
    ),
});
const loginPort = String(login.port);
let browser: Browser | undefined;

const fingerprint = async (prompt: string | RegExp) => {
  await waitFor(prompt, 25000);
  await sleep(1200);
  adb("emu", "finger", "touch", "1");
};
/** Brings LocalPass to the front. */
const front = async () => {
  shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(1500);
};
const inputs = () => screen().filter((n) => n.cls.endsWith("EditText"));
const typeInto = async (index: number, text: string) => {
  const field = inputs()[index];
  if (!field) throw new Error(`no input field #${index}; the screen shows: ${labels().slice(0, 14).join(" | ")}`);
  tapNode(field);
  await sleep(500);
  type(text);
  await sleep(400);
};

try {
  // ---- install ----
  adb("uninstall", PACKAGE); // a clean first start; also needed when a debug build (another key) is installed
  const out = adb("install", apk);
  check("the release apk installs", out.includes("Success"), out.trim().split("\n").pop());
  const runAs = shell("run-as", PACKAGE, "id");
  check("run-as is refused (the release build is not debuggable)", /not debuggable|unknown package|is not an application/i.test(runAs) && !/uid=/.test(runAs), runAs.slice(0, 80));

  // ---- first start ----
  adb("shell", "input", "keyevent", "KEYCODE_HOME");
  shell("am", "force-stop", PACKAGE);
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(3000);
  const pid = shell("pidof", PACKAGE).split(/\s+/)[0] ?? "";
  // A userdebug system image (the emulator) opens every web view to DevTools; only a "user" build
  // such as a real phone shows whether the app itself leaves it off. The apk's library has no call
  // that turns it on (checked with `unzip -p <apk> lib/<abi>/liblocalpass_lib.so | grep -c setWebContentsDebuggingEnabled`).
  if (shell("getprop", "ro.debuggable") === "1") {
    console.log("info  this system image is debuggable, so its web views are open to DevTools whatever the app does");
  } else {
    check("web view debugging is off (no devtools socket for the app's process)", !!pid && !shell("cat", "/proc/net/unix").includes(`webview_devtools_remote_${pid}`), `pid ${pid}`);
  }
  const allow = await waitFor(/^Allow$/, 20000).catch(() => undefined);
  if (allow) tapNode(allow);
  check("Android 17 asks for the local-network permission at the first start", !!allow);
  await waitFor(/Create a new vault/, 40000);
  check("the welcome screen shows", true);

  // ---- create a vault ----
  tapNode(find(/Create a new vault/)!);
  await waitFor(/Create your vault/, 10000);
  await typeInto(0, "Release smoke phone");
  await typeInto(1, MASTER_PASSWORD);
  await typeInto(2, MASTER_PASSWORD);
  key("KEYCODE_BACK"); // close the keyboard
  await sleep(600);
  tapNode(await waitFor(/^Create vault$/, 8000));
  await waitFor(/^Settings$/, 60000);
  check("the vault is created and the app shows its main screen", !!find(/^Items$/) || !!find(/^Settings$/));

  // ---- the set-up section ----
  tapNode(find(/^Settings$/)!);
  await waitFor(/Autofill and passkeys/i, 8000);
  check("Settings has the Autofill and passkeys section", true, labels().filter((l) => /Autofill|Passkeys|Local network/.test(l)).join(" | ").slice(0, 160));
  check("Local network reads On (the permission was allowed)", labels().some((l) => /Allowed, so your devices/.test(l)));
  tapNode(await waitFor(/^Turn on$/, 8000));
  await waitFor(/^Preferred service$/, 10000);
  tapNode(await waitFor(/^LocalPass$/, 5000));
  tapNode(await waitFor(/^Change$/, 8000));
  await sleep(1500);
  check("the Turn on button makes LocalPass the autofill service", secure("autofill_service") === AUTOFILL, secure("autofill_service"));
  key("KEYCODE_BACK");
  await sleep(1200);
  await front();
  await waitFor(/^On$/, 10000).catch(() => {});
  check("the section shows On afterwards", labels().includes("On"));
  // Fingerprint unlock, so the services have a fingerprint to ask for.
  const toggle = await waitFor(/Unlock with fingerprint or face/, 8000).catch(() => undefined);
  if (toggle) {
    tapNode(toggle);
    await fingerprint("Turn on fingerprint unlock");
    await sleep(2500);
  }
  check("fingerprint unlock is switched on", !!toggle);

  // ---- Chrome hands pages to a third-party autofill service ----
  adb("reverse", `tcp:${loginPort}`, `tcp:${loginPort}`);
  adb("reverse", `tcp:${rpPort}`, `tcp:${rpPort}`);
  shell("am", "force-stop", CHROME);
  shell("am", "start", "-a", "android.intent.action.VIEW", "-d", "about:blank", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
  await sleep(4000);
  tapNode(await waitFor(/More options/, 15000));
  await sleep(1000);
  tapNode(await waitFor(/^Settings$/, 8000));
  await sleep(1500);
  tapNode(await scrollTo(/^Autofill services$/));
  await sleep(1000);
  tapNode(await waitFor(/^Autofill using another service$/, 8000));
  await sleep(1500);
  const restart = find("Restart Chrome");
  if (restart) tapNode(restart);
  await sleep(5000);
  shell("am", "force-stop", CHROME);
  shell("am", "start", "-a", "android.intent.action.VIEW", "-d", `http://${HOST}:${loginPort}/`, "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
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
  const openPage = async (url: string): Promise<Page> => {
    shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(700);
    const nag = find("No thanks");
    if (nag) tapNode(nag);
    const pages = await browser!.pages();
    const tab = pages.find((p) => p.url().startsWith("http")) ?? pages[0]!;
    await tab.goto(url);
    await tab.bringToFront();
    await sleep(900);
    return tab;
  };

  // ---- save a password from Android's prompt ----
  let tab = await openPage(`http://${HOST}:${loginPort}/`);
  await tab.click('input[name="username"]');
  await sleep(1200);
  await tab.keyboard.type("release-user", { delay: 30 });
  key("KEYCODE_TAB");
  await sleep(1000);
  await tab.keyboard.type("release-pass-1", { delay: 30 });
  await sleep(400);
  key("KEYCODE_ENTER");
  tapNode(await waitFor(/^Save$/, 30000));
  const unlock = await waitFor(/^Unlock LocalPass$/, 6000).catch(() => undefined);
  if (unlock) await fingerprint(/^Unlock LocalPass$/);
  await sleep(3500);
  check("Save in Android's prompt stores the login", true);
  await front();
  tapNode((await waitFor(/^Items$/, 8000).catch(() => undefined)) ?? find(/^Settings$/)!);
  await sleep(1500);
  if (find(/Unlock LocalPass/) && !find(HOST)) {
    // The app locked itself while away; open it again with the fingerprint.
    const quick = find(/Use fingerprint|Unlock with fingerprint/i);
    if (quick) tapNode(quick);
    await fingerprint("Unlock LocalPass").catch(() => {});
    await sleep(2500);
  }
  check("the saved login is in the app, titled with the site", !!(await waitFor(new RegExp(HOST.replace(/\./g, "\\.")), 12000).catch(() => undefined)), labels().slice(0, 12).join(" | "));

  // ---- fill it ----
  tab = await openPage(`http://${HOST}:${loginPort}/`);
  await tab.click('input[name="username"]');
  const row = await waitFor(new RegExp(`${HOST.replace(/\./g, "\\.")}|Tap to unlock`), 25000);
  if (/Tap to unlock/.test(row.text + row.desc)) {
    tapNode(row);
    await fingerprint("Unlock LocalPass");
    tapNode(await waitFor(new RegExp(HOST.replace(/\./g, "\\.")), 25000));
  } else {
    tapNode(row);
  }
  const values = () => tab.evaluate(() => [(document.querySelector('input[name="username"]') as HTMLInputElement).value, (document.querySelector('input[name="password"]') as HTMLInputElement).value]);
  for (let i = 0; i < 24 && (await values())[1] === ""; i++) await sleep(500);
  check("picking the login in the dropdown fills the username and password", JSON.stringify(await values()) === '["release-user","release-pass-1"]', JSON.stringify(await values()));

  // ---- a passkey: create, then sign in ----
  const status = async (expected: RegExp | string, p: Page, ms = 30000) => {
    const start = Date.now();
    let text = "";
    while (Date.now() - start < ms) {
      text = await p.$eval("#status", (el) => el.textContent ?? "");
      if (typeof expected === "string" ? text === expected : expected.test(text)) return text;
      await sleep(400);
    }
    return text;
  };
  const fresh = async () => {
    const t = await openPage(`${rp.origin}/`);
    return t;
  };
  let rpTab = await fresh();
  await rpTab.click("#register");
  await waitFor(/Create passkey to sign in/, 25000);
  check("the system sheet offers to save the passkey in LocalPass", !!find("LocalPass"));
  tapNode(await waitFor("Continue", 8000));
  await fingerprint(/Unlock LocalPass|Create a passkey/);
  if (find(/Unlock LocalPass/)) {
    // The vault was locked: the unlock came first, then the creation asks once more.
    await fingerprint(/^Create a passkey for /).catch(() => {});
  }
  check("the site accepts the passkey LocalPass made", (await status("registered", rpTab)) === "registered", await status(/./, rpTab, 100));
  rpTab = await fresh();
  await rpTab.click("#signin");
  await waitFor(/Use your saved passkey|Unlock sign-in options/, 25000);
  tapNode(await waitFor("Continue", 8000));
  await fingerprint(/^Sign in to .* with a passkey$|Unlock LocalPass/);
  if (find(/Unlock LocalPass/)) await fingerprint(/^Sign in to .* with a passkey$/).catch(() => {});
  check("signing in with that passkey works", (await status("signed-in", rpTab)) === "signed-in", await status(/./, rpTab, 100));
} catch (e) {
  console.error(e);
  console.error("screen:", labels().slice(0, 24).join(" | "));
  failures++;
} finally {
  await browser?.disconnect().catch(() => {});
  shell("am", "force-stop", CHROME);
  restore("autofill_service", before.autofill);
  restore("credential_service", before.providers);
  restore("credential_service_primary", before.primary);
  adb("reverse", "--remove", `tcp:${loginPort}`);
  adb("reverse", "--remove", `tcp:${rpPort}`);
  adb("forward", "--remove", "tcp:9444");
  login.stop(true);
  rp.stop();
  shell("input", "keyevent", "KEYCODE_HOME");
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("the release apk works: first start, vault, set-up section, save, fill and a passkey");
process.exit(0);
