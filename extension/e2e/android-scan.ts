// QR scanning in the Android app: the camera screen must close again with its Cancel button and with
// the system Back button, and open again afterwards. Regression test for the "stuck on the scanning
// screen" bug (tauri-plugin-barcode-scanner's `cancel` forgets the pending `scan()` call before it
// rejects it, so the page never left the see-through scanning screen; see app/src/lib/scan.ts).
//
// Needs an emulator with a back camera (AVD lp_api37: hw.camera.back=emulated), unlocked. It starts
// from a clean install (the app's data on the emulator is wiped), grants nothing but the local-network
// permission up front and answers the camera permission box itself.
//
//   debug build (reads the page state over DevTools as well, and shows the plugin bug first):
//     cd app/src-tauri/gen/android && ./gradlew.bat :app:assembleUniversalDebug -x rustBuildUniversalDebug
//       (or `bun run tauri android build --debug --target x86_64 --apk --ci` in app/)
//     cd extension && bun run e2e/android-scan.ts [path\to\app-universal-debug.apk]
//   release build (no DevTools; everything is read with uiautomator):
//     cd extension && bun run e2e/android-scan.ts --release path\to\localpass-<version>-universal-release.apk
//
// Part A: edit a saved login > Scan setup QR code > Cancel / Back / reopen / Back out of the app.
// Part B: a fresh app > "I already use LocalPass on another device" > Scan QR code instead > same.
// Emulator notes: the plugin reads QR codes with Google's ML Kit module that Play services download, which the
// emulator cannot do ("Waiting for the barcode module to be downloaded"), and CameraX also wants a front camera
// to exist (start the emulator with `-camera-front emulated` or it logs "No available camera can be found").
// Either way the scanning screen comes up and the scan call stays pending, which is the state under test; a
// successful scan (a QR code decoded) is NOT covered here.

import { resolve } from "node:path";
import { adb, find, key, labels, screen, shell, sleep, tapNode, type, waitFor, type UiNode } from "./adb";
import { PACKAGE, connectToApp } from "./android";

const LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK";
const MASTER_PASSWORD = "scan test master password";
const OVERLAY = /Point the camera at the QR code/;
const argv = process.argv.slice(2);
const release = argv.includes("--release");
const apk = argv.find((a) => !a.startsWith("--")) ?? resolve(import.meta.dir, "../../app/src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk");

let failures = 0;
const check = (name: string, ok: boolean, detail = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
};
const info = (text: string) => console.log(`info  ${text}`);

let app: Awaited<ReturnType<typeof connectToApp>> | undefined;

// ---- the phone ----
const focus = () => shell("dumpsys", "window").split("\n").find((l) => l.includes("mCurrentFocus"))?.trim().replace(/^mCurrentFocus=Window\{\w+ u0 /, "").replace(/\}$/, "") ?? "";
const inApp = () => focus().startsWith(`${PACKAGE}/`);
const keyboardShown = () => /mInputShown=true/.test(shell("dumpsys", "input_method"));
const hideKeyboard = async () => {
  if (keyboardShown()) {
    key("KEYCODE_BACK"); // only closes the keyboard while it is up
    await sleep(600);
  }
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
/**
 * The label's node once it is really on screen. A web view's accessibility tree does not follow a scroll
 * (nodes that were cut off keep a zero-height box at the page edge), so nothing here scrolls: the screens
 * under test keep what the test needs in view.
 */
const visible = async (match: string | RegExp, ms = 10000): Promise<UiNode> => {
  const start = Date.now();
  while (Date.now() - start < ms) {
    const node = find(match);
    if (node && node.bounds[3] - node.bounds[1] > 20 && node.bounds[1] > 150 && node.bounds[3] < 2300) return node;
    await sleep(500);
  }
  throw new Error(`"${match}" is not on screen; the screen shows: ${labels().slice(0, 20).join(" | ")}`);
};
const unlockPhone = async () => {
  for (let i = 0; i < 4 && /isKeyguardShowing=true/.test(shell("dumpsys", "window")); i++) {
    shell("input", "keyevent", "KEYCODE_WAKEUP");
    shell("input", "swipe", "540", "2000", "540", "600", "300");
    await sleep(1500);
    type("1234"); // lp_api37's PIN
    await sleep(500);
    key("KEYCODE_ENTER");
    await sleep(2000);
  }
};
const startApp = async () => {
  key("KEYCODE_HOME");
  shell("am", "force-stop", PACKAGE);
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(3500);
};
/** Brings the app back after it left (the activity is rebuilt, the process may live on). */
const relaunch = async () => {
  shell("am", "start", "-W", "-n", `${PACKAGE}/.MainActivity`);
  await sleep(3000);
};
const connect = async () => {
  if (release) return;
  app?.browser.disconnect().catch(() => {});
  app = await connectToApp();
  await app.invoke("app_info");
};

// ---- the page, as far as a debug build lets us see it ----
const dom = () =>
  app?.page.evaluate(() => ({
    scanning: document.documentElement.classList.contains("scanning"),
    rootHidden: getComputedStyle(document.getElementById("root")!).visibility === "hidden",
    heading: document.querySelector("h1, h2")?.textContent ?? "",
  }));

// ---- scanner steps ----
/** Taps the button that opens the camera and waits for the scanning screen; answers the camera permission box. */
async function openScanner(button: RegExp): Promise<boolean> {
  const node = await visible(button);
  tapNode(node);
  const start = Date.now();
  while (Date.now() - start < 40000) {
    const nodes = screen();
    const grant = find(/^While using the app$/, nodes);
    if (grant) {
      info("the camera permission box appeared; choosing 'While using the app'");
      tapNode(grant);
      await sleep(1000);
      continue;
    }
    if (find(OVERLAY, nodes)) return true;
    const wait = find(/isn't responding/, nodes) && find(/^Wait$/, nodes);
    if (wait) tapNode(wait);
    await sleep(500);
  }
  return false;
}
/** Waits until `ready()` is true; returns how long it took in ms, or -1. */
async function settles(ready: (nodes: UiNode[]) => boolean, ms = 8000): Promise<number> {
  const start = Date.now();
  while (Date.now() - start < ms) {
    if (ready(screen())) return Date.now() - start;
    await sleep(300);
  }
  return -1;
}
const scannerGone = (nodes: UiNode[]) => !find(OVERLAY, nodes);

/**
 * One scan session: open, check the scanning screen, then close it with `how` and check that the screen
 * we came from is back and usable. `back` is the page title to expect (e.g. "Edit item").
 */
async function session(name: string, button: RegExp, how: "cancel" | "back", expect: RegExp, field: number, word: string) {
  const opened = await openScanner(button);
  check(`${name}: the scanning screen opens`, opened, opened ? "" : labels().slice(0, 12).join(" | "));
  if (!opened) return false;
  const d = await dom();
  if (d) check(`${name}: the page is in the scanning state`, d.scanning && d.rootHidden, JSON.stringify(d));
  check(`${name}: the app's own screen is hidden behind the camera`, !find(expect));
  if (how === "cancel") {
    const cancel = screen().filter((n) => n.text === "Cancel").pop();
    if (!cancel) throw new Error("no Cancel button on the scanning screen");
    tapNode(cancel);
  } else {
    key("KEYCODE_BACK");
  }
  const ms = await settles((nodes) => scannerGone(nodes) && !!find(expect, nodes), 8000);
  check(`${name}: ${how === "cancel" ? "Cancel" : "Back"} closes the scanning screen and shows ${expect}`, ms >= 0, ms >= 0 ? `${ms} ms` : `still: ${labels().slice(0, 10).join(" | ")}`);
  const after = await dom();
  if (after) check(`${name}: the page left the scanning state`, !after.scanning && !after.rootHidden, JSON.stringify(after));
  check(`${name}: the app is still in front (Back did not leave it)`, inApp(), focus());
  if (ms < 0) return false;
  // Usable again: type into a field and read it back.
  await typeInto(field, word);
  const typed = inputs()[field];
  check(`${name}: the screen takes input again`, !!typed && typed.text.includes(word), typed?.text ?? "no field");
  await hideKeyboard();
  return true;
}

try {
  await unlockPhone();
  // ---- install ----
  adb("uninstall", PACKAGE); // a clean start, and a build signed with another key cannot be installed over
  const out = adb("install", apk);
  check(`the ${release ? "release" : "debug"} apk installs`, out.includes("Success"), out.trim().split("\n").pop());
  const badge = shell("dumpsys", "package", PACKAGE).match(/versionName=(\S+)/)?.[1];
  info(`installed version ${badge}`);
  shell("pm", "grant", PACKAGE, LOCAL_NETWORK);
  await startApp();
  await connect();

  // ================= Part A: edit a saved login =================
  info("Part A: editing a saved login");
  tapNode(await waitFor(/Create a new vault/, 60000));
  await waitFor(/Create your vault/, 10000);
  await typeInto(0, "Scan test phone");
  await typeInto(1, MASTER_PASSWORD);
  await typeInto(2, MASTER_PASSWORD);
  await hideKeyboard();
  tapNode(await waitFor(/^Create vault$/, 8000));
  await waitFor(/^Settings$/, 60000);
  tapNode(await waitFor(/^New$/, 10000));
  await waitFor(/^New item$/, 10000);
  await typeInto(0, "ScanTarget");
  await typeInto(1, "scan-user");
  await typeInto(2, "scan-pass-1");
  await hideKeyboard();
  tapNode(inputs()[1]!); // Enter in a field submits the form (Save is below the fold, and the tree does not scroll)
  await sleep(400);
  key("KEYCODE_ENTER");
  tapNode(await waitFor(/^Edit$/, 10000)); // the saved login's page
  await waitFor(/^Edit item$/, 10000);
  check("a saved login is open for editing", true);
  const EDIT = /^Edit item$/;
  const SCAN = /^Scan setup QR code$/;

  const first = await dom();
  if (first) check("before any scan the page is not in the scanning state", !first.scanning, JSON.stringify(first));

  if (!release) {
    // The bug itself: the plugin's own `cancel` leaves the page waiting. This is what the old Cancel button
    // and Back handler did (they called nothing else), so the page stayed on the see-through screen.
    check("scanner opens for the root-cause check", await openScanner(SCAN));
    await app!.page.evaluate(() => (window as any).__TAURI_INTERNALS__.invoke("plugin:barcode-scanner|cancel"));
    await sleep(2500);
    const stuck = await dom();
    check("BUG REPRODUCED by the plugin's cancel alone: the page is still in the scanning state", !!stuck?.scanning && !!find(OVERLAY), JSON.stringify(stuck));
    const cancel = screen().filter((n) => n.text === "Cancel").pop();
    if (cancel) tapNode(cancel);
    const ms = await settles((nodes) => scannerGone(nodes) && !!find(EDIT, nodes), 8000);
    check("the fixed Cancel button recovers even from that stuck state", ms >= 0, `${ms} ms`);
    const rec = await dom();
    check("and the page is out of the scanning state", !rec?.scanning, JSON.stringify(rec));
  }

  // (a) Cancel, then reopen, then (b) system Back
  await session("A1 Cancel", SCAN, "cancel", EDIT, 0, "x");
  await session("A2 reopen, Back", SCAN, "back", EDIT, 0, "y");
  await session("A3 reopen, Cancel", SCAN, "cancel", EDIT, 0, "z");
  await session("A4 reopen, Back", SCAN, "back", EDIT, 0, "w");
  // (c) with the scanner closed, Back is the app's own again
  key("KEYCODE_BACK");
  await sleep(2500);
  check("A5 with the scanner closed, Back leaves the app as it always does (its Back handler is gone)", !inApp(), focus());
  if (!release) info(`A5 now in front: ${focus()}`);

  // ================= Part B: joining a vault by scanning =================
  info("Part B: Join, scan the pairing QR code");
  shell("pm", "clear", PACKAGE);
  shell("pm", "grant", PACKAGE, LOCAL_NETWORK);
  await startApp();
  await connect();
  await waitFor(/Create a new vault/, 60000);
  // Baseline: on a fresh start, with no scan ever opened, Back leaves the app.
  key("KEYCODE_BACK");
  await sleep(2500);
  const baseline = !inApp();
  check("B0 baseline: on the welcome screen Back leaves the app", baseline, focus());
  await relaunch();
  await connect();
  tapNode(await waitFor(/I already use LocalPass/, 60000));
  await waitFor(/Join your vault/, 10000);
  const JOIN = /Join your vault/;
  await session("B1 Cancel", /^Scan QR code instead$/, "cancel", JOIN, 0, "12");
  await session("B2 reopen, Back", /^Scan QR code instead$/, "back", JOIN, 0, "34");
  key("KEYCODE_BACK");
  await sleep(2500);
  check("B3 with the scanner closed, Back leaves the app like the baseline", !inApp() === baseline, focus());
} catch (e) {
  console.error(e);
  console.error("screen:", labels().slice(0, 24).join(" | "));
  console.error("focus:", focus());
  failures++;
} finally {
  app?.browser.disconnect().catch(() => {});
  adb("forward", "--remove", "tcp:9333");
  shell("input", "keyevent", "KEYCODE_HOME");
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("the scanning screen closes with Cancel and with Back, and opens again");
process.exit(0);
