// The "Autofill and passkeys" set-up section in the Android app's Settings, on a real system image:
// the status it shows, the system pages its buttons open, and the status changing when the user
// comes back from them.
//
//   cd app/src-tauri/gen/android && ./gradlew.bat :app:assembleX86_64Debug -x rustBuildX86_64Debug
//   (start AVD lp_api37, unlock it; a vault is created if there is none)
//   cd extension && bun run e2e/android-setup.ts [path\to\app-x86_64-debug.apk]
//   (LP_SKIP_INSTALL=1 reuses the installed build)
//
// The app's screen is read through its web view (debug builds only); the system dialogs are driven
// with uiautomator. The autofill service, the credential provider and the local-network permission
// are put back as they were.

import { resolve } from "node:path";
import { adb, find, key, labels, shell, sleep, tapNode, waitFor } from "./adb";
import { MASTER_PASSWORD, PACKAGE, connectToApp } from "./android";

const AUTOFILL = `${PACKAGE}/${PACKAGE}.LpAutofillService`;
const APK = resolve(import.meta.dir, "../../app/src-tauri/gen/android/app/build/outputs/apk/x86_64/debug/app-x86_64-debug.apk");
const LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK";

let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};

const secure = (key: string) => shell("settings", "get", "secure", key);
const before = { autofill: secure("autofill_service"), providers: secure("credential_service"), primary: secure("credential_service_primary") };
const restore = (key: string, value: string) => (value && value !== "null" ? shell("settings", "put", "secure", key, value) : shell("settings", "delete", "secure", key));
let app: Awaited<ReturnType<typeof connectToApp>> | undefined;

try {
  // ---- set up ----
  const apk = process.argv[2] ?? APK;
  if (process.env.LP_SKIP_INSTALL !== "1") check("the app installs", adb("install", "-r", apk).includes("Success"), apk);
  shell("pm", "grant", PACKAGE, LOCAL_NETWORK);
  shell("input", "keyevent", "KEYCODE_HOME");
  shell("am", "force-stop", PACKAGE);
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(8000);
  const wait = find(/^Wait$/);
  if (wait) tapNode(wait);
  app = await connectToApp();
  await app.invoke("app_info");
  await sleep(4000);

  /** The app in front (Android freezes a background web view), then `call`. */
  const inApp = async <T>(call: () => Promise<T>): Promise<T> => {
    shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
    await sleep(900);
    return call();
  };
  const status = () => inApp(async () => ((await app!.invoke("app_info")) as any).status as string);
  /** An app that starts locked shows its fingerprint prompt, which stays over the screen after the unlock above: close it. */
  const dismissPrompt = async () => {
    await sleep(1500);
    if (find(/^Fingerprint sensor$/)) {
      key("KEYCODE_BACK");
      await sleep(800);
    }
  };
  if ((await status()) === "noVault") await inApp(() => app!.invoke("create_vault", { password: MASTER_PASSWORD, deviceName: "Test phone" }));
  if ((await status()) === "locked") await inApp(() => app!.invoke("unlock", { password: MASTER_PASSWORD }));
  check("the vault is open", (await status()) === "unlocked");
  await dismissPrompt();

  /** The text of the "Autofill and passkeys" section as the user sees it. */
  const section = () =>
    inApp(() =>
      app!.page.evaluate(() => {
        const h = [...document.querySelectorAll("h2")].find((x) => x.textContent?.trim() === "Autofill and passkeys");
        return (h?.parentElement as HTMLElement | undefined)?.innerText ?? "";
      }),
    );
  const clickButton = (label: string) =>
    inApp(() =>
      app!.page.evaluate((text) => {
        const h = [...document.querySelectorAll("h2")].find((x) => x.textContent?.trim() === "Autofill and passkeys");
        const b = [...(h?.parentElement?.querySelectorAll("button") ?? [])].find((x) => x.textContent?.trim() === text);
        b?.click();
        return !!b;
      }, label),
    );
  /** Opens Settings in the app's own UI. */
  const openSettings = async () => {
    await inApp(() =>
      app!.page.evaluate(() => {
        const nav = [...document.querySelectorAll("nav button")].find((x) => x.textContent?.trim() === "Settings") as HTMLElement | undefined;
        nav?.click();
      }),
    );
    await sleep(1200);
  };
  const row = (text: string, title: string) => new RegExp(`${title}\\s*\\n?(On|Off)?`).exec(text)?.[1] ?? "";
  const settle = async (until: () => Promise<boolean>, ms = 15000) => {
    const start = Date.now();
    while (Date.now() - start < ms) {
      if (await until()) return true;
      await sleep(700);
    }
    return false;
  };

  // ---- autofill: off -> the button opens Android's chooser -> on, without reloading ----
  for (const name of ["autofill_service", "credential_service", "credential_service_primary"]) shell("settings", "delete", "secure", name);
  await openSettings();
  let text = await section();
  check("the section is there, titled Autofill and passkeys", text.includes("Autofill passwords") && text.includes("Passkeys"), text.slice(0, 80));
  check("no screen-lock warning while the phone has a screen lock", !text.includes("Screen lock"), JSON.stringify(text.slice(0, 160)));
  check("autofill reads Off while no service is selected", row(text, "Autofill passwords") === "Off", JSON.stringify(text.slice(0, 120)));
  check("and offers Turn on", await clickButton("Turn on"));
  // Android 17 shows its "Preferred service" list; older Android asks "Make LocalPass your autofill service?".
  const page = await waitFor(/Preferred service|autofill service/i, 10000);
  check("Android's own page for choosing the service opens", !!page, labels().slice(0, 8).join(" | "));
  const chooser = !!find(/^Preferred service$/);
  if (chooser) tapNode(await waitFor(/^LocalPass$/, 5000));
  // Backing out of the confirmation changes nothing.
  const cancel = await waitFor(/^Cancel$/, 8000).catch(() => undefined);
  if (cancel) tapNode(cancel);
  await sleep(1500);
  check("cancelling keeps autofill off", secure("autofill_service") === "null", secure("autofill_service"));
  if (chooser) tapNode(await waitFor(/^LocalPass$/, 5000));
  tapNode(await waitFor(chooser ? /^Change$/ : /^OK$/i, 8000));
  await sleep(1500);
  check("confirming makes LocalPass the autofill service", secure("autofill_service") === AUTOFILL, secure("autofill_service"));
  console.log(`info  credential provider after that: ${secure("credential_service_primary")}`);
  key("KEYCODE_BACK");
  await sleep(1200);
  shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
  check(
    "the section shows On after coming back, without a reload",
    await settle(async () => row(await section(), "Autofill passwords") === "On"),
    JSON.stringify((await section()).slice(0, 120)),
  );
  text = await section();
  check("and the button now reads Change", text.includes("Change") && !text.includes("Turn on"), JSON.stringify(text.slice(0, 120)));
  await clickButton("Change");
  check("Change opens the same page again", !!(await waitFor(/Preferred service/, 8000).catch(() => undefined)));
  key("KEYCODE_BACK");
  await sleep(1000);

  // ---- passkeys: no invented status, and the button opens the provider page ----
  shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
  text = await section();
  check("passkeys show no On/Off claim (Android hides it from apps)", row(text, "Passkeys") === "" && text.includes("Open settings and switch LocalPass on."), JSON.stringify(text));
  check("and the Open settings button is there", text.includes("Open settings"));
  await clickButton("Open settings");
  const provider = await waitFor(/Passwords & passkeys/, 8000).catch(() => undefined);
  const providerPage = labels().join(" | ");
  check("the passwords and passkeys page opens, with LocalPass on it", !!provider && /LocalPass/.test(providerPage), providerPage.slice(0, 200));
  key("KEYCODE_BACK");
  await sleep(1200);

  // ---- local network: denied -> Allow opens the app's permissions -> granted ----
  shell("pm", "revoke", PACKAGE, LOCAL_NETWORK); // this ends the app's process; it starts again below
  await sleep(2000);
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(7000);
  const dont = find(/^Don.t allow$/i);
  if (dont) tapNode(dont); // the app asks at start-up on Android 17; the answer here is no
  await sleep(1500);
  app.browser.disconnect().catch(() => {});
  app = await connectToApp();
  if ((await status()) === "locked") await inApp(() => app!.invoke("unlock", { password: MASTER_PASSWORD }));
  await dismissPrompt();
  await openSettings();
  text = await section();
  check("without the permission the section says so and offers Allow", /Local network\s*\n?Off/.test(text) && text.includes("Allow"), JSON.stringify(text));
  await clickButton("Allow");
  await sleep(2500);
  const appPage = labels().join(" | ");
  check("Allow opens the app's own settings page", /LocalPass/.test(appPage) && /Permissions|App info|Uninstall|Force stop/.test(appPage), appPage.slice(0, 200));
  shell("pm", "grant", PACKAGE, LOCAL_NETWORK);
  await sleep(2500);
  app.browser.disconnect().catch(() => {});
  shell("am", "start", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(6000);
  app = await connectToApp();
  if ((await status()) === "locked") await inApp(() => app!.invoke("unlock", { password: MASTER_PASSWORD }));
  await dismissPrompt();
  await openSettings();
  text = await section();
  check("once allowed the section shows Local network On", /Local network\s*\n?On/.test(text), JSON.stringify(text));
} catch (e) {
  console.error(e);
  console.error("screen:", labels().slice(0, 20).join(" | "));
  failures++;
} finally {
  await app?.browser.disconnect().catch(() => {});
  restore("autofill_service", before.autofill);
  restore("credential_service", before.providers);
  restore("credential_service_primary", before.primary);
  shell("pm", "grant", PACKAGE, LOCAL_NETWORK);
  adb("forward", "--remove", "tcp:9333");
  shell("input", "keyevent", "KEYCODE_HOME");
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("the Autofill and passkeys section shows the real state and opens the right system pages");
process.exit(0);
