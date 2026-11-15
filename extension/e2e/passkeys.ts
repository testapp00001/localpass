// Passkeys in a real browser, checked by an independent relying party (SimpleWebAuthn):
//   cargo build -p lp-broker --examples && cd extension && bun run build --e2e && bun run e2e/passkeys.ts
//
// A local site registers and signs in with passkeys through navigator.credentials. LocalPass
// answers (via the extension, the helper and the broker); the site verifies everything the way a
// real website would. Also checked: conditional sign-in from a field, the locked-vault flow, and
// that calls LocalPass declines still reach the browser's own implementation.

import type { Page } from "puppeteer-core";
import { EXTENSION_ID, Harness, Panels, inPageControls, panelClick, panelShows, sleep, toolbarBadge } from "./harness";
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

let panels!: Panels;

async function open(path = ""): Promise<Page> {
  const p = await h.chrome.newPage();
  await p.goto(`${origin}/${path}`);
  await p.bringToFront();
  return p;
}

/** Clicks a button of the site and returns the LocalPass panel its passkey request opens. */
async function ask(p: Page, button: string): Promise<Page> {
  const next = panels.next();
  await p.click(button);
  return next;
}

try {
  await h.start();
  await h.pair();
  panels = new Panels(h.chrome);

  // 1. Register a passkey: the request is shown in LocalPass's panel, never in the page.
  const p1 = await open();
  let panel = await ask(p1, "#register");
  const create = await panelShows(panel, /Create passkey/);
  check("the panel asks before creating a passkey, naming the site and the account", create.includes("Create passkey") && create.includes("localhost") && create.includes("Account: alice"), create);
  check("nothing is drawn into the page for it", (await inPageControls(p1)).length === 0, JSON.stringify(await inPageControls(p1)));
  check("the waiting request shows the badge on the toolbar icon", (await toolbarBadge(h.chrome)) === "1");
  await panelClick(panel, "Create passkey");
  check("the site accepts the registration", (await status(p1, "registered")) === "registered", await p1.$eval("#status", (e) => e.textContent ?? ""));
  const items = await h.waitForVault((all) => all.some((i) => i.kind === "passkey"));
  const stored = items.find((i) => i.kind === "passkey");
  check("the passkey is in the vault", stored?.passkey?.rpId === "localhost" && stored.passkey.userName === "alice", JSON.stringify(stored?.passkey));
  check("the vault listing carries no private key", !!stored && !JSON.stringify(items).includes("MIGH") && stored.password === "");

  // 2. Registering again for the same account does not duplicate it (the site excludes it).
  await p1.reload();
  panel = await ask(p1, "#register");
  await panelClick(panel, "Create passkey");
  const refused = await panelShows(panel, /Could not create/i);
  check("the panel says a passkey already exists", /already exists/i.test(refused), refused);
  await panelClick(panel, "Close");
  const again = await status(p1, /^(registered|error:.*)$/);
  check("a second registration is refused by the exclude list", again.startsWith("error:InvalidStateError"), again);
  check("still one passkey in the vault", h.vault().filter((i) => i.kind === "passkey").length === 1);

  // 3. Sign in with the passkey (modal).
  const p2 = await open();
  panel = await ask(p2, "#signin");
  const chooser = await panelShows(panel, /Choose a passkey/);
  check("the sign-in panel lists the passkey", chooser.includes("Choose a passkey to sign in with:") && chooser.includes("alice"), chooser);
  await panelClick(panel, "alice");
  check("the site accepts the sign-in", (await status(p2, "signed-in")) === "signed-in", await p2.$eval("#status", (e) => e.textContent ?? ""));

  // 4. Sign in from the username field (conditional mediation): the badge opens the picker, which lists the passkey.
  const p3 = await open("#conditional");
  await sleep(800);
  await p3.focus("#cond");
  const badge = await p3.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 });
  const next = panels.next();
  await badge!.click();
  panel = await next;
  const menu = await panelShows(panel, /Passkey: alice/);
  check("the field's picker offers the passkey", menu.includes("Passkey: alice"), menu);
  await panelClick(panel, "Passkey: alice");
  check("conditional sign-in succeeds", (await status(p3, "signed-in-conditional")) === "signed-in-conditional", await p3.$eval("#status", (e) => e.textContent ?? ""));

  // 5. A locked vault is unlocked from the panel.
  h.hostCommand("lock");
  await h.waitForHost((l) => l === "LOCKED");
  const p4 = await open();
  panel = await ask(p4, "#signin");
  const locked = await panelShows(panel, /locked/);
  check("a locked vault is reported in the panel", locked.includes("LocalPass is locked."), locked);
  await panelClick(panel, "Unlock");
  await panelShows(panel, /Choose a passkey/);
  await panelClick(panel, "alice");
  check("after unlocking the sign-in works", (await status(p4, "signed-in")) === "signed-in");

  // 6. Esc in the panel is a refusal the page can handle.
  const p5 = await open();
  panel = await ask(p5, "#signin");
  await panelShows(panel, /Choose a passkey/);
  await panel.keyboard.press("Escape");
  const cancelled = await status(p5, /^error:/);
  check("Esc in the panel refuses the request", cancelled.startsWith("error:NotAllowedError"), cancelled);

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
  // The popup in a tab is held for 0.6 s after each view, its switches too (8.3c).
  const switchFree = () => popup.waitForFunction(() => document.querySelector("#passkeys")?.getAttribute("aria-disabled") !== "true", { timeout: 5000 });
  await popup.waitForFunction(() => !(document.querySelector("#main")?.textContent ?? "Connecting").includes("Connecting"), { timeout: 10000 });
  await switchFree();
  await popup.click("#passkeys");
  check("the switch is saved", (await popup.evaluate(() => chrome.storage.local.get("passkeysEnabled"))).passkeysEnabled === false);
  const p8 = await open();
  const off = panels.opens(1500);
  await p8.click("#signin");
  check("with the switch off LocalPass shows no panel", !(await off) && (await p8.$eval("#status", (e) => e.textContent ?? "")) !== "");
  await popup.bringToFront();
  await sleep(100); // coming back to the tab holds it again
  await switchFree();
  await popup.click("#passkeys");
  await popup.close();
  const p9 = await open();
  panel = await ask(p9, "#signin");
  check("with the switch on again the panel is back", (await panelShows(panel, /Choose a passkey/)).includes("Choose a passkey"));
  await panelClick(panel, "Cancel");

  // 10. A stopped service worker (MV3 stops idle ones) doesn't send the request to the browser. Since 8.3b the
  // Chrome worker also listens to tab and window events (for the decisions), so Chrome may start it again at
  // once for one of those: what counts is that the request then meets a fresh worker with no connection.
  const p10 = await open();
  await sleep(500);
  const session = await p10.createCDPSession();
  const versions: { versionId: string; scriptURL: string; runningStatus: string }[] = [];
  session.on("ServiceWorker.workerVersionUpdated", (e: { versions: typeof versions }) => versions.push(...e.versions));
  await session.send("ServiceWorker.enable");
  await sleep(500);
  const version = versions.find((v) => v.scriptURL.includes(EXTENSION_ID) && v.runningStatus === "running");
  check("the extension's service worker is running", !!version, JSON.stringify(versions.map((v) => [v.scriptURL, v.runningStatus])));
  if (version) {
    const seen = versions.length;
    await session.send("ServiceWorker.stopWorker", { versionId: version.versionId });
    const start = Date.now();
    while (Date.now() - start < 5000 && !versions.slice(seen).some((v) => v.versionId === version.versionId && v.runningStatus === "stopped")) await sleep(50);
    check("the service worker was stopped", versions.slice(seen).some((v) => v.versionId === version.versionId && v.runningStatus === "stopped"), JSON.stringify(versions.slice(seen).map((v) => v.runningStatus)));
    // A harness detail, not the extension's: DevTools holds a worker that starts again after `stopWorker` until a
    // client lets it run, and puppeteer does not do that for a worker target it has seen before (measured: it stays
    // "starting"). Let it run, as a browser without DevTools attached does.
    let resuming = true;
    const resume = (async () => {
      while (resuming) {
        for (const t of h.chrome.targets().filter((t) => t.type() === "service_worker" && t.url().includes(EXTENSION_ID))) {
          const sw = await t.createCDPSession().catch(() => null);
          await sw?.send("Runtime.runIfWaitingForDebugger").catch(() => {});
          await sw?.detach().catch(() => {});
        }
        await sleep(300);
      }
    })();
    panel = await ask(p10, "#signin").finally(() => (resuming = false));
    await resume;
    const woke = await panelShows(panel, /Choose a passkey/);
    check("a passkey request to a stopped worker still gets the LocalPass panel", woke.includes("Choose a passkey"), woke);
    await panelClick(panel, "alice");
    check("and the sign-in through it works", (await status(p10, "signed-in")) === "signed-in");
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
