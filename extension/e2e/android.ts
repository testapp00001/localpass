// Passkeys on a real Android system image: Chrome -> Credential Manager -> the LocalPass
// provider, with a local relying party (SimpleWebAuthn) checking the result.
//
//   cd app/src-tauri/gen/android && ./gradlew.bat :app:assembleX86_64Debug -x rustBuildX86_64Debug
//   (start an emulator with Chrome and a fingerprint enrolled, e.g. AVD lp_api37, and unlock it)
//   cd extension && bun run e2e/android.ts [path\to\app-x86_64-debug.apk]
//
// The emulator reaches the host's relying party through `adb reverse`, so Chrome opens it as
// http://localhost:PORT (a secure context, which WebAuthn needs). The app's own vault is driven
// through its WebView debugging port; the system sheets and prompts are driven with uiautomator
// and `adb emu finger touch`. The provider setting and the port forwards are restored at the end.

import { join, resolve } from "node:path";
import puppeteer, { type Browser, type Page } from "puppeteer-core";
import { adb, find, key, labels, screen, scrollTo, shell, sleep, tap, tapNode, type, waitFor } from "./adb";
import { startRelyingParty } from "./rp";

export const PACKAGE = "app.localpass";
export const PROVIDER = `${PACKAGE}/${PACKAGE}.LpCredentialProviderService`;
export const MASTER_PASSWORD = "end to end master password";
const CHROME = "com.android.chrome";
const APK = resolve(import.meta.dir, "../../app/src-tauri/gen/android/app/build/outputs/apk/x86_64/debug/app-x86_64-debug.apk");

/** Connects to the LocalPass web view (debug builds only) so tests can call the app's commands. */
export async function connectToApp(): Promise<{ page: Page; browser: Browser; invoke: (command: string, args?: object) => Promise<unknown> }> {
  let pid = "";
  for (let i = 0; i < 30 && !pid; i++) {
    pid = shell("pidof", PACKAGE).split(/\s+/)[0] ?? "";
    if (!pid) await sleep(500);
  }
  if (!pid) throw new Error("LocalPass is not running");
  const port = 9333;
  adb("forward", `tcp:${port}`, `localabstract:webview_devtools_remote_${pid}`);
  let browser: Browser | undefined;
  for (let i = 0; i < 30 && !browser; i++) {
    try {
      browser = await puppeteer.connect({ browserURL: `http://127.0.0.1:${port}`, defaultViewport: null });
    } catch {
      await sleep(500);
    }
  }
  if (!browser) throw new Error("the app's web view never answered");
  let page: Page | undefined;
  for (let i = 0; i < 40 && !page; i++) {
    page = (await browser.pages()).find((p) => {
      try {
        return p.url().length > 0 && !p.url().startsWith("about:");
      } catch {
        return false; // still loading
      }
    });
    if (!page) await sleep(500);
  }
  if (!page) throw new Error("the app's web view never appeared");
  const invoke = (command: string, args: object = {}) => page!.evaluate((c, a) => (window as any).__TAURI_INTERNALS__.invoke(c, a), command, args as any);
  return { page, browser, invoke };
}

if (import.meta.main) {
  let failures = 0;
  const check = (name: string, ok: boolean, detail = "") => {
    console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
    if (!ok) failures++;
  };

  const rp = startRelyingParty();
  const port = new URL(rp.origin).port;
  const providerBefore = {
    all: shell("settings", "get", "secure", "credential_service"),
    primary: shell("settings", "get", "secure", "credential_service_primary"),
  };
  const autofillBefore = shell("settings", "get", "secure", "autofill_service");
  const loginSite = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: () =>
      new Response(
        `<!doctype html><title>Login</title><form method="post" action="/session"><input name="username" autocomplete="username" placeholder="Username"><input name="password" type="password" placeholder="Password"><button>Sign in</button></form>`,
        { headers: { "content-type": "text/html" } },
      ),
  });
  const loginPort = String(loginSite.port);
  let browser: Browser | undefined;
  let app: Awaited<ReturnType<typeof connectToApp>> | undefined;

  try {
    // ---- set up ----
    const apk = process.argv[2] ?? APK;
    // LP_SKIP_INSTALL=1 reuses the installed build (the first start after an install is the slow one).
    if (process.env.LP_SKIP_INSTALL !== "1") check("the app installs", adb("install", "-r", apk).includes("Success"), apk);
    shell("pm", "grant", PACKAGE, "android.permission.ACCESS_LOCAL_NETWORK");
    shell("settings", "put", "secure", "credential_service", PROVIDER);
    shell("settings", "put", "secure", "credential_service_primary", PROVIDER);
    adb("reverse", `tcp:${port}`, `tcp:${port}`);
    // Start from a clean screen: no browser, no credential sheet left over from an earlier run.
    shell("am", "force-stop", CHROME);
    shell("am", "force-stop", "com.android.credentialmanager");
    shell("input", "keyevent", "KEYCODE_HOME");
    // The first start after an install is slow on an emulator (no compiled code yet); let it finish
    // and close an "isn't responding" box if the system raised one meanwhile.
    shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
    await sleep(8000);
    const wait = find(/^Wait$/);
    if (wait) tapNode(wait);
    app = await connectToApp();
    await app.invoke("app_info");
    await sleep(5000); // let the web view settle before another app takes the screen
    // Android freezes a background app, which would stall its web view: bring it to the front for
    // every call into it.
    const inApp = async <T>(call: () => Promise<T>): Promise<T> => {
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

    // Fingerprint unlock, so the provider's prompts have a fingerprint to ask for.
    const quick = () => inApp(async () => ((await app!.invoke("app_info")) as any).quickUnlock.enabled === true);
    if (!(await quick())) {
      const enabling = app.invoke("quick_unlock_enable").catch((e) => String(e));
      await waitFor("Turn on fingerprint unlock", 15000);
      await sleep(800);
      adb("emu", "finger", "touch", "1");
      await enabling;
    }
    check("fingerprint unlock is on", await quick());

    const passkeys = () => inApp(async () => ((await app!.invoke("list_items", { query: {} })) as { kind: string }[]).filter((i) => i.kind === "passkey").length);
    // Registering the same account again replaces its passkey rather than adding one.

    // ---- Chrome and the page ----
    shell("am", "force-stop", CHROME);
    shell("am", "start", "-a", "android.intent.action.VIEW", "-d", `${rp.origin}/`, "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(3000);
    adb("forward", "tcp:9444", "localabstract:chrome_devtools_remote");
    for (let i = 0; i < 20 && !browser; i++) {
      try {
        browser = await puppeteer.connect({ browserURL: "http://127.0.0.1:9444", defaultViewport: null });
      } catch {
        await sleep(1000);
      }
    }
    if (!browser) throw new Error("could not reach Chrome's DevTools");
    const page = (await browser.pages()).find((p) => p.url().startsWith(rp.origin))!;

    const pageStatus = async (expected: RegExp | string, timeoutMs = 25000) => {
      const start = Date.now();
      let text = "";
      while (Date.now() - start < timeoutMs) {
        text = await page.$eval("#status", (el) => el.textContent ?? "");
        if (typeof expected === "string" ? text === expected : expected.test(text)) return text;
        await sleep(400);
      }
      return text;
    };
    /** Starts from a fresh page (the status text resets) with Chrome in front. */
    const fresh = async () => {
      shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
      await sleep(700);
      const nag = find("No thanks");
      if (nag) tapNode(nag);
      await page.bringToFront();
      await page.reload();
      await sleep(500);
    };
    const fingerprint = async (prompt: string | RegExp) => {
      await waitFor(prompt, 20000);
      // The sensor may not be listening the moment the prompt appears (a loaded emulator is slow
      // to start it): touch again while the prompt is still there.
      for (let i = 0; i < 8; i++) {
        await sleep(1200);
        adb("emu", "finger", "touch", "1");
        await sleep(4000);
        if (!find(prompt)) return;
      }
    };

    // ---- 1. create a passkey ----
    await fresh();
    await page.click("#register");
    const createSheet = (await waitFor(/Create passkey to sign in/, 20000)).text;
    check("the system asks where to save the passkey, and LocalPass is offered", !!find("LocalPass") && !!find(/alice/), createSheet);
    await tap("Continue");
    // A passkey left in the vault by an earlier run is not known to this new site: LocalPass asks before replacing it.
    const first = await waitFor(/^Create a passkey for |^Replace your passkey\?$/, 20000);
    if (/^Replace your passkey\?$/.test(first.text)) await tap(/^Replace$/i);
    await fingerprint(/^(Create a|Replace your) passkey for /);
    check("the site accepts the passkey LocalPass made", (await pageStatus("registered")) === "registered", await pageStatus(/./, 100));
    check("the passkey is in the vault", (await passkeys()) >= 1);
    const passkeyCount = await passkeys();

    // ---- 2. sign in with it ----
    await ensureUnlocked();
    await fresh();
    await page.click("#signin");
    await waitFor(/Use your saved passkey/, 20000);
    check("the sign-in sheet lists the passkey", !!find("alice"));
    await tap("Continue");
    await fingerprint(/^Sign in to .* with a passkey$/);
    check("the site accepts the sign-in", (await pageStatus("signed-in")) === "signed-in", await pageStatus(/./, 100));

    // ---- 3. no second passkey for the same account ----
    await ensureUnlocked();
    await fresh();
    await page.click("#register");
    await tap("Continue", 20000);
    check("a second registration is refused as the specification asks", /^error:InvalidStateError/.test(await pageStatus(/^error:/)), await pageStatus(/./, 100));
    check("no second passkey was made", (await passkeys()) === passkeyCount);

    // ---- 3b. a site that does not list the old passkey: LocalPass asks before replacing it ----
    // The page sends the exclude list unless it has an unchecked "exclude" box (the assisted page has one).
    const dropExcludeList = () => page.evaluate(() => document.body.insertAdjacentHTML("afterbegin", '<input type="checkbox" id="exclude">'));
    await ensureUnlocked();
    await fresh();
    await dropExcludeList();
    await page.click("#register");
    await tap("Continue", 20000);
    const ask = await waitFor("Replace your passkey?", 20000);
    const askText = labels().join(" | ");
    check("a clear question names the site and the account", /This replaces your existing passkey for localhost \/ alice/.test(askText), askText.slice(0, 200));
    await tap(/^Cancel$/i);
    check("answering Cancel refuses the request", /^error:NotAllowedError/.test(await pageStatus(/^error:/)), await pageStatus(/./, 100));
    check("and keeps the old passkey", (await passkeys()) === passkeyCount && !!ask);
    await ensureUnlocked();
    await fresh();
    await dropExcludeList();
    await page.click("#register");
    await tap("Continue", 20000);
    await waitFor("Replace your passkey?", 20000);
    await tap(/^Replace$/i);
    await fingerprint(/^Replace your passkey for /);
    check("answering Replace, then the fingerprint, makes the new passkey", (await pageStatus("registered")) === "registered", await pageStatus(/./, 100));
    check("it took the place of the old one, not added to it", (await passkeys()) === passkeyCount, String(await passkeys()));

    // ---- 4. a locked vault: unlock with the fingerprint, then choose the passkey ----
    await lock();
    await fresh();
    await page.click("#signin");
    await waitFor(/Unlock sign-in options/, 20000);
    await tap("Continue");
    await fingerprint("Unlock LocalPass");
    await waitFor(/Use your saved passkey/, 20000);
    await tap("Continue");
    check("after unlocking, the sign-in completes with no second prompt", (await pageStatus("signed-in")) === "signed-in", await pageStatus(/./, 100));
    check("the vault is unlocked afterwards", (await status()) === "unlocked");

    // ---- 5. the master password works when the fingerprint isn't used ----
    await lock();
    await fresh();
    await page.click("#signin");
    await waitFor(/Unlock sign-in options/, 20000);
    await tap("Continue");
    await waitFor("Unlock LocalPass", 20000);
    await tap("Use master password");
    const field = await waitFor(/EditText/, 10000).catch(() => undefined);
    if (field) tapNode(field);
    else tapNode(screen().find((n) => n.cls.endsWith("EditText"))!);
    await sleep(500);
    type(MASTER_PASSWORD);
    key("KEYCODE_BACK"); // close the keyboard
    await sleep(500);
    await tap(/^unlock$/i);
    await waitFor(/Use your saved passkey/, 25000);
    await tap("Continue");
    check("the master password unlocks and the sign-in completes", (await pageStatus("signed-in", 30000)) === "signed-in", await pageStatus(/./, 100));

    // ---- 6. backing out refuses the request ----
    await ensureUnlocked();
    await fresh();
    await page.click("#signin");
    await tap("Continue", 20000);
    await waitFor(/^Sign in to .* with a passkey$/, 20000);
    await tap("Cancel");
    check("cancelling the fingerprint prompt refuses the request", /^error:NotAllowedError/.test(await pageStatus(/^error:/)), await pageStatus(/./, 100));

    // ---- 7. autofill: a saved login for the website in Chrome ----
    adb("reverse", `tcp:${loginPort}`, `tcp:${loginPort}`);
    shell("settings", "put", "secure", "autofill_service", `${PACKAGE}/${PACKAGE}.LpAutofillService`);
    // Chrome only hands pages to a third-party autofill service when its own setting says so (it
    // asks to restart; choosing the already-selected option does nothing).
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
    // Chrome restarted: reconnect to a fresh tab.
    await browser.disconnect().catch(() => {});
    browser = undefined;
    shell("am", "force-stop", CHROME);
    shell("am", "start", "-a", "android.intent.action.VIEW", "-d", `http://localhost:${loginPort}/`, "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(4000);
    for (let i = 0; i < 20 && !browser; i++) {
      try {
        browser = await puppeteer.connect({ browserURL: "http://127.0.0.1:9444", defaultViewport: null });
      } catch {
        await sleep(1000);
      }
    }
    if (!browser) throw new Error("could not reach Chrome's DevTools again");
    const loginPage = async (host: string) => {
      const pages = await browser!.pages();
      const tab = pages.find((p) => p.url().includes(`:${loginPort}`)) ?? pages[0]!;
      await tab.goto(`http://${host}:${loginPort}/`);
      await tab.bringToFront();
      await sleep(800);
      return tab;
    };
    const values = (tab: Page) =>
      tab.evaluate(() => [(document.querySelector('input[name="username"]') as HTMLInputElement).value, (document.querySelector('input[name="password"]') as HTMLInputElement).value]);

    // The vault needs a login for the site (saved for the host; Android reports no port).
    await ensureUnlocked();
    const hasLogin = await inApp(async () => ((await app!.invoke("list_items", { query: {} })) as { title: string }[]).some((i) => i.title === "Local site"));
    if (!hasLogin) await inApp(() => app!.invoke("save_item", { input: { title: "Local site", username: "alice", password: "alice-secret", urls: ["http://localhost"] } }));

    let tab = await loginPage("localhost");
    await tab.click('input[name="username"]');
    await waitFor("Local site", 20000);
    check("the autofill dropdown offers the saved login", !!find("alice"));
    tapNode(find("Local site")!);
    for (let i = 0; i < 20 && (await values(tab))[1] === ""; i++) await sleep(500);
    check("choosing it fills the username and password", JSON.stringify(await values(tab)) === '["alice","alice-secret"]', JSON.stringify(await values(tab)));

    // Another website (the same machine under a different name) gets nothing.
    tab = await loginPage("127.0.0.1");
    await tab.click('input[name="username"]');
    await sleep(4000);
    check("a different website is offered no login", !find("Local site"), labels().slice(0, 8).join(" | "));

    // A locked vault: unlock from the dropdown with the fingerprint, then choose the login.
    await lock();
    // The app's own lock screen starts a fingerprint prompt; a touch meant for the autofill prompt
    // would go to that one instead (and unlock the app, not the page's dropdown), so back out of it.
    await sleep(1500);
    key("KEYCODE_BACK");
    await sleep(500);
    shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(700);
    tab = await loginPage("localhost");
    // Not awaited: when the unlock prompt opens straight away it covers Chrome and DevTools stalls until it is gone.
    tab.click('input[name="username"]').catch(() => {});
    // Depending on the system, the dropdown row is shown first or the unlock prompt opens straight away.
    const unlockStep = await waitFor(/Tap to unlock|^Unlock LocalPass$/, 20000);
    if (/Tap to unlock/.test(unlockStep.text + unlockStep.desc)) tapNode(unlockStep);
    await fingerprint("Unlock LocalPass");
    await waitFor("Local site", 25000);
    tapNode(find("Local site")!);
    for (let i = 0; i < 20 && (await values(tab))[1] === ""; i++) await sleep(500);
    check("after unlocking from the dropdown the login is filled", JSON.stringify(await values(tab)) === '["alice","alice-secret"]', JSON.stringify(await values(tab)));

    // ---- 8. an unlock done for autofill is not the user check for a passkey ----
    // The vault was just unlocked from the autofill dropdown with the fingerprint (the app may lock
    // itself again soon after, so this does not look at the vault first). Signing in with a passkey
    // must still ask the user: the unlock prompt if it is locked again, else the passkey's own
    // prompt, never a silent sign-in. Only the unlock inside the passkey picker counts for the pick after it.
    await tab.goto(`${rp.origin}/`);
    await tab.bringToFront();
    await sleep(800);
    tab.click("#signin").catch(() => {}); // not awaited: the system sheet covers Chrome and DevTools stalls meanwhile
    await tap("Continue", 25000);
    const asked = await waitFor(/^Fingerprint sensor$/, 25000).catch(() => undefined);
    check("the passkey sign-in still asks for the fingerprint after an autofill unlock", !!asked, labels().slice(0, 8).join(" | "));
    await tap(/^Cancel$/);
    await sleep(1500);
    const outcome = await tab.$eval("#status", (el) => el.textContent ?? "");
    check("backing out refuses it", /^error:NotAllowedError/.test(outcome), outcome);
  } catch (e) {
    console.error(e);
    console.error("screen:", labels().slice(0, 20).join(" | "));
    failures++;
  } finally {
    await browser?.disconnect().catch(() => {});
    await app?.browser.disconnect().catch(() => {});
    shell("am", "force-stop", CHROME);
    shell("settings", "put", "secure", "credential_service", providerBefore.all);
    shell("settings", "put", "secure", "credential_service_primary", providerBefore.primary);
    shell("settings", "put", "secure", "autofill_service", autofillBefore);
    adb("reverse", "--remove", `tcp:${port}`);
    adb("reverse", "--remove", `tcp:${loginPort}`);
    loginSite.stop(true);
    rp.stop();
  }

  if (failures) {
    console.error(`${failures} check(s) failed`);
    process.exit(1);
  }
  console.log("passkeys and autofill work on Android: Chrome, Credential Manager and LocalPass");
  process.exit(0);
}
