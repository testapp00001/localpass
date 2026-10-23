// Passkeys are refused on a phone without a screen lock, with a clear message, and Settings says why.
//
//   (start AVD lp_api37 and REMOVE its screen lock first: `adb shell locksettings clear --old 1234`.
//    That also removes the enrolled fingerprint; to put it back set the PIN again and enrol the
//    fingerprint in Settings, or restore a copy of the AVD's data files. This script only checks.)
//   cd extension && bun run e2e/android-nolock.ts [path\to\app-x86_64-debug.apk]   (LP_SKIP_INSTALL=1 reuses the installed build)
//
// Chrome -> Credential Manager -> LocalPass with a local relying party (SimpleWebAuthn). The screen
// lock decision itself is unit tested (`PasskeyPolicyTest`); this shows it on a real system image.

import { resolve } from "node:path";
import puppeteer, { type Browser } from "puppeteer-core";
import { adb, find, labels, shell, sleep, tap, tapNode, waitFor } from "./adb";
import { MASTER_PASSWORD, PACKAGE, PROVIDER, connectToApp } from "./android";
import { startRelyingParty } from "./rp";

const CHROME = "com.android.chrome";
const APK = resolve(import.meta.dir, "../../app/src-tauri/gen/android/app/build/outputs/apk/x86_64/debug/app-x86_64-debug.apk");

let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};

const rp = startRelyingParty();
const secure = (key: string) => shell("settings", "get", "secure", key);
const before = { all: secure("credential_service"), primary: secure("credential_service_primary") };
const restore = (key: string, value: string) => (value && value !== "null" ? shell("settings", "put", "secure", key, value) : shell("settings", "delete", "secure", key));
let browser: Browser | undefined;
let app: Awaited<ReturnType<typeof connectToApp>> | undefined;

try {
  const apk = process.argv[2] ?? APK;
  if (process.env.LP_SKIP_INSTALL !== "1") check("the app installs", adb("install", "-r", apk).includes("Success"), apk);
  shell("pm", "grant", PACKAGE, "android.permission.ACCESS_LOCAL_NETWORK");
  shell("settings", "put", "secure", "credential_service", PROVIDER);
  shell("settings", "put", "secure", "credential_service_primary", PROVIDER);
  adb("reverse", `tcp:${new URL(rp.origin).port}`, `tcp:${new URL(rp.origin).port}`);
  shell("am", "force-stop", CHROME);
  shell("am", "force-stop", "com.android.credentialmanager");
  shell("input", "keyevent", "KEYCODE_HOME");
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(8000);
  const wait = find(/^Wait$/);
  if (wait) tapNode(wait);
  app = await connectToApp();
  await app.invoke("app_info");
  await sleep(5000);
  const inApp = async <T>(call: () => Promise<T>): Promise<T> => {
    shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
    await sleep(700);
    return call();
  };
  const status = () => inApp(async () => ((await app!.invoke("app_info")) as any).status as string);
  const ensureUnlocked = async () => {
    if ((await status()) === "locked") await inApp(() => app!.invoke("unlock", { password: MASTER_PASSWORD }));
  };
  if ((await status()) === "noVault") await inApp(() => app!.invoke("create_vault", { password: MASTER_PASSWORD, deviceName: "Test phone" }));
  await ensureUnlocked();
  check("the vault is open", (await status()) === "unlocked");
  const setup = (await inApp(() => app!.invoke("android_setup_status"))) as { screenLock: string };
  check("the phone has no screen lock (the precondition of this test)", setup.screenLock === "off", JSON.stringify(setup));
  if (setup.screenLock !== "off") throw new Error("this phone has a screen lock; remove it first (see the header)");
  const passkeys = () => inApp(async () => ((await app!.invoke("list_items", { query: {} })) as { kind: string }[]).filter((i) => i.kind === "passkey").length);
  const passkeysBefore = await passkeys();

  // ---- Settings tells the user ----
  const section = () =>
    inApp(() =>
      app!.page.evaluate(() => {
        const h = [...document.querySelectorAll("h2")].find((x) => x.textContent?.trim() === "Autofill and passkeys");
        return (h?.parentElement as HTMLElement | undefined)?.innerText ?? "";
      }),
    );
  await inApp(() =>
    app!.page.evaluate(() => {
      ([...document.querySelectorAll("nav button")].find((x) => x.textContent?.trim() === "Settings") as HTMLElement | undefined)?.click();
    }),
  );
  await sleep(1500);
  const text = await section();
  check("Settings shows a Screen lock warning with a way to fix it", /Screen lock\s*\n?Off/.test(text) && text.includes("will not create or use passkeys") && text.includes("Open settings"), JSON.stringify(text.slice(0, 300)));

  // ---- Chrome and the relying party ----
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
  const pageStatus = async (expected: RegExp, timeoutMs = 25000) => {
    const start = Date.now();
    let t = "";
    while (Date.now() - start < timeoutMs) {
      t = await page.$eval("#status", (el) => el.textContent ?? "");
      if (expected.test(t)) return t;
      await sleep(400);
    }
    return t;
  };
  const fresh = async () => {
    shell("am", "start", "-n", `${CHROME}/com.google.android.apps.chrome.Main`);
    await sleep(700);
    const nag = find("No thanks");
    if (nag) tapNode(nag);
    await page.bringToFront();
    await page.reload();
    await sleep(500);
  };

  // ---- 1. creating a passkey is refused, before anything is asked or made ----
  await fresh();
  await page.click("#register");
  await waitFor(/Create passkey to sign in/, 20000);
  await tap("Continue");
  await waitFor("Set a screen lock to use passkeys", 20000);
  const message = labels().join(" | ");
  check("the message says what to do", /no screen lock/.test(message) && /try again/.test(message), message.slice(0, 240));
  check("and offers to open the security settings", !!find(/^Open settings$/i));
  await tap("OK");
  check("the page's request fails cleanly (NotAllowedError)", /^error:NotAllowedError/.test(await pageStatus(/^error:/)), await pageStatus(/./, 100));
  check("no passkey was made", (await passkeys()) === passkeysBefore);

  // ---- 2. signing in is refused too, also from the unlock step of the picker ----
  await inApp(() => app!.invoke("lock"));
  await fresh();
  await page.click("#signin");
  await waitFor(/Unlock sign-in options|Use your saved passkey/, 20000);
  await tap("Continue");
  await waitFor("Set a screen lock to use passkeys", 20000);
  check("the unlock step of the sign-in picker shows the same message instead of unlocking", !find(/^Unlock LocalPass$/));
  await tap("OK");
  check("and the sign-in fails cleanly", /^error:/.test(await pageStatus(/^error:/)), await pageStatus(/./, 100));
  check("the vault stayed locked (nothing was unlocked for it)", (await status()) === "locked");
} catch (e) {
  console.error(e);
  console.error("screen:", labels().slice(0, 20).join(" | "));
  failures++;
} finally {
  await browser?.disconnect().catch(() => {});
  await app?.browser.disconnect().catch(() => {});
  shell("am", "force-stop", CHROME);
  restore("credential_service", before.all);
  restore("credential_service_primary", before.primary);
  adb("reverse", "--remove", `tcp:${new URL(rp.origin).port}`);
  adb("forward", "--remove", "tcp:9333");
  adb("forward", "--remove", "tcp:9444");
  rp.stop();
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("without a screen lock LocalPass refuses passkeys with a clear message and says so in Settings");
process.exit(0);
