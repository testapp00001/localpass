// Passkeys in a real browser, checked by an independent relying party (SimpleWebAuthn):
//   cargo build -p lp-broker --examples && cd extension && bun run build --e2e && bun run e2e/passkeys.ts
//
// A local site registers and signs in with passkeys through navigator.credentials. LocalPass
// answers (via the extension, the helper and the broker); the site verifies everything the way a
// real website would. Also checked: conditional sign-in from a field, the locked-vault flow, and
// that calls LocalPass declines still reach the browser's own implementation.

import type { Page } from "puppeteer-core";
import { EXTENSION_ID, Harness, sleep } from "./harness";
import { startRelyingParty } from "./rp";

const h = new Harness();

const rp = startRelyingParty();
const origin = rp.origin;

// ---- helpers ----

async function status(p: Page, expected: string | RegExp, timeoutMs = 15000): Promise<string> {
  const start = Date.now();
  let text = "";
  while (Date.now() - start < timeoutMs) {
    text = await p.$eval("#status", (el) => el.textContent ?? "");
    if (typeof expected === "string" ? text === expected : expected.test(text)) return text;
    await sleep(150);
  }
  return text;
}

async function sheetText(p: Page): Promise<string> {
  const sheet = await p.waitForSelector("pierce/.sheet", { timeout: 10000 });
  return (await sheet!.evaluate((el) => el.textContent ?? "")).replace(/\s+/g, " ").trim();
}

/** Waits until the dialog's text matches (it changes as LocalPass answers). */
async function sheetMatching(p: Page, pattern: RegExp, timeoutMs = 10000): Promise<string> {
  const start = Date.now();
  let text = "";
  while (Date.now() - start < timeoutMs) {
    text = await sheetText(p).catch(() => "");
    if (pattern.test(text)) return text;
    await sleep(150);
  }
  return text;
}

// Controls that act for the user (rows, the primary button) ignore clicks until the browser has shown them
// plainly for a moment: wait for `data-ready`, as a person would.
const clickIn = async (p: Page, selector: string) => {
  const ready = /\.(row|btn\.primary)$/.test(selector) ? `${selector}[data-ready]` : selector;
  return (await p.waitForSelector(`pierce/${ready}`, { timeout: 8000 }))!.click();
};

async function open(path = ""): Promise<Page> {
  const p = await h.chrome.newPage();
  await p.goto(`${origin}/${path}`);
  await p.bringToFront();
  return p;
}

try {
  await h.start();
  await h.pair();

  // 1. Register a passkey.
  const p1 = await open();
  await p1.click("#register");
  const create = await sheetText(p1);
  check("a dialog asks before creating a passkey", create.includes("Create a passkey?") && create.includes("alice"), create);
  await clickIn(p1, ".btn.primary");
  check("the site accepts the registration", (await status(p1, "registered")) === "registered", await p1.$eval("#status", (e) => e.textContent ?? ""));
  const items = await h.waitForVault((all) => all.some((i) => i.kind === "passkey"));
  const stored = items.find((i) => i.kind === "passkey");
  check("the passkey is in the vault", stored?.passkey?.rpId === "localhost" && stored.passkey.userName === "alice", JSON.stringify(stored?.passkey));
  check("the vault listing carries no private key", !!stored && !JSON.stringify(items).includes("MIGH") && stored.password === "");

  // 2. Registering again for the same account does not duplicate it (the site excludes it).
  await p1.reload();
  await p1.click("#register");
  await sheetText(p1);
  await clickIn(p1, ".btn.primary");
  const refused = await sheetMatching(p1, /could not create/i);
  check("the dialog says a passkey already exists", /already exists/i.test(refused), refused);
  await clickIn(p1, ".btn:not(.primary):last-child"); // Close
  const again = await status(p1, /^(registered|error:.*)$/);
  check("a second registration is refused by the exclude list", again.startsWith("error:InvalidStateError"), again);
  check("still one passkey in the vault", h.vault().filter((i) => i.kind === "passkey").length === 1);

  // 3. Sign in with the passkey (modal).
  const p2 = await open();
  await p2.click("#signin");
  const chooser = await sheetMatching(p2, /Choose a passkey/);
  check("the sign-in dialog lists the passkey", chooser.includes("Sign in with a passkey") && chooser.includes("alice"), chooser);
  await clickIn(p2, ".row");
  check("the site accepts the sign-in", (await status(p2, "signed-in")) === "signed-in", await p2.$eval("#status", (e) => e.textContent ?? ""));

  // 4. Sign in from the username field (conditional mediation).
  const p3 = await open("#conditional");
  await sleep(800);
  await p3.focus("#cond");
  await (await p3.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 }))!.click();
  const menu = await (await p3.waitForSelector("pierce/.panel", { timeout: 8000 }))!.evaluate((el) => el.textContent ?? "");
  check("the field's menu offers the passkey", menu.includes("Passkey: alice"), menu);
  await clickIn(p3, ".row");
  check("conditional sign-in succeeds", (await status(p3, "signed-in-conditional")) === "signed-in-conditional", await p3.$eval("#status", (e) => e.textContent ?? ""));

  // 5. A locked vault is unlocked from the dialog.
  h.hostCommand("lock");
  await h.waitForHost((l) => l === "LOCKED");
  const p4 = await open();
  await p4.click("#signin");
  const locked = await sheetMatching(p4, /locked/);
  check("a locked vault is reported in the dialog", locked.includes("locked"), locked);
  await clickIn(p4, ".btn.primary"); // Unlock
  await p4.waitForSelector("pierce/.row[data-ready]", { timeout: 10000 });
  await clickIn(p4, ".row");
  check("after unlocking the sign-in works", (await status(p4, "signed-in")) === "signed-in");

  // 6. Cancelling the dialog is a refusal the page can handle.
  const p5 = await open();
  await p5.click("#signin");
  await sheetText(p5);
  await p5.keyboard.press("Escape");
  const cancelled = await status(p5, /^error:/);
  check("closing the dialog refuses the request", cancelled.startsWith("error:NotAllowedError"), cancelled);

  // 7. A page can't ask for another site's passkeys.
  const p7 = await open();
  await p7.click("#wrong-rp");
  const wrong = await status(p7, /^(created|error:.*)$/);
  check("another site's RP ID is refused with SecurityError", wrong === "error:SecurityError", wrong);
  check("no passkey was made for it", h.vault().filter((i) => i.kind === "passkey").length === 1);

  // 8. What LocalPass declines still reaches the browser's own implementation.
  const p6 = await open();
  const cdp = await p6.createCDPSession();
  await cdp.send("WebAuthn.enable");
  await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: { protocol: "ctap2", transport: "usb", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true },
  });
  await p6.click("#register-key");
  check("a security-key request goes to the browser, not LocalPass", (await status(p6, "registered-key", 20000)) === "registered-key", await p6.$eval("#status", (e) => e.textContent ?? ""));
  check("LocalPass did not take that registration", h.vault().filter((i) => i.kind === "passkey").length === 1);

  // 9. The popup's switch turns passkey handling off (and back on).
  const popup = await h.chrome.newPage();
  await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
  await popup.click("#passkeys");
  check("the switch is saved", (await popup.evaluate(() => chrome.storage.local.get("passkeysEnabled"))).passkeysEnabled === false);
  const p8 = await open();
  await p8.click("#signin");
  await sleep(1500);
  check("with the switch off LocalPass shows no dialog", (await p8.evaluateHandle(() => document.querySelector("#status")?.textContent)).toString() !== "" && !(await p8.$("pierce/.sheet")));
  await popup.bringToFront();
  await popup.click("#passkeys");
  await popup.close();
  const p9 = await open();
  await p9.click("#signin");
  check("with the switch on again the dialog is back", (await sheetText(p9)).includes("Sign in with a passkey"));

  // 10. A sleeping service worker (MV3 stops idle ones) doesn't send the request to the browser.
  const host = await open();
  const session = await host.createCDPSession();
  const versions: { versionId: string; scriptURL: string; runningStatus: string }[] = [];
  session.on("ServiceWorker.workerVersionUpdated", (e: { versions: typeof versions }) => versions.push(...e.versions));
  await session.send("ServiceWorker.enable");
  await sleep(500);
  const version = versions.find((v) => v.scriptURL.includes(EXTENSION_ID) && v.runningStatus === "running");
  check("the extension's service worker is running", !!version, JSON.stringify(versions.map((v) => [v.scriptURL, v.runningStatus])));
  if (version) {
    await session.send("ServiceWorker.stopWorker", { versionId: version.versionId });
    await sleep(800);
    const asleep = h.chrome.targets().every((t) => !(t.type() === "service_worker" && t.url().includes(EXTENSION_ID)));
    check("the service worker was stopped", asleep);
    const p10 = await open();
    await p10.click("#signin");
    const woke = await sheetMatching(p10, /Choose a passkey/);
    check("a passkey request wakes the service worker and still gets the LocalPass dialog", woke.includes("Choose a passkey"), woke);
  }
} catch (e) {
  console.error(e);
  h.failures++;
} finally {
  rp.stop();
  await h.stop();
}

function check(name: string, ok: boolean, detail = "") {
  h.check(name, ok, detail);
}

h.finish("all passkey end-to-end checks passed");
