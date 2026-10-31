// LocalPass in real Firefox against the real broker (Linux, headless, BiDi). See harness.ts for the set-up.
//
//   cargo build -p lp-broker --examples && cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts
//
// Pairing through the real native-messaging helper, fill, save, the decision window's guards, and passkeys
// checked by an independent relying party (SimpleWebAuthn, e2e/rp.ts). The shipped add-on (`dist-firefox`)
// differs from the E2E build only in the on-page UI's shadow-root mode (the first check proves it), so one
// run covers both. SLOW=1 adds a wait of over a minute inside a decision window.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { startRelyingParty } from "../rp";
import { FirefoxHarness, extensionDir, shippedDir, sleep } from "./harness";

const h = new FirefoxHarness();
const check = (name: string, ok: boolean, detail = "") => h.check(name, ok, detail);

const loginPage = (title: string, extra = "") => `<!doctype html><title>${title}</title>
<form method="post" action="/session" id="login">
  <input name="username" id="u" autocomplete="username" placeholder="Username" style="width:300px;height:30px">
  <input name="password" id="p" type="password" placeholder="Password">
  <button type="submit" id="go">Sign in</button>
</form>${extra}`;

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    const url = new URL(request.url);
    const html = { "content-type": "text/html" };
    if (url.pathname === "/login") return new Response(loginPage("Login"), { headers: html });
    // Pages that try to collect clicks on the extension's badge (RA-1 in Chrome): one hides it with !important CSS, one paints a
    // see-through decoy over it that ignores the mouse.
    if (url.pathname === "/attack-style") {
      const css = "<style>html > div { opacity: 0 !important; display: none !important; visibility: hidden !important; transform: scale(0) !important; }</style>";
      return new Response(loginPage("Hidden") + css, { headers: html });
    }
    if (url.pathname === "/attack-cover") {
      const script = `<script>const place = () => { let d = document.getElementById("decoy");
        if (!d) { d = document.createElement("div"); d.id = "decoy"; d.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none;background:#fff"; }
        if (document.documentElement.lastElementChild !== d) document.documentElement.append(d); };
        new MutationObserver(place).observe(document.documentElement, { childList: true }); place();</script>`;
      return new Response(loginPage("Covered") + script, { headers: html });
    }
    if (url.pathname === "/session") return new Response(null, { status: 303, headers: { location: "/dashboard" } });
    if (url.pathname === "/dashboard") return new Response("<!doctype html><title>Dashboard</title><h1>Welcome</h1>", { headers: html });
    // A frame whose origin is opaque (CSP sandbox): Firefox reports origin "null" and LocalPass must refuse it.
    if (url.pathname === "/sandboxed") return new Response(loginPage("Sandboxed"), { headers: { ...html, "content-security-policy": "sandbox allow-scripts allow-forms" } });
    if (url.pathname === "/plainframe") return new Response(`<!doctype html><title>Plain frame</title><iframe src="/login" width="500" height="200"></iframe>`, { headers: html });
    if (url.pathname === "/framed") return new Response(`<!doctype html><title>Framed</title><iframe src="/sandboxed" width="500" height="200"></iframe>`, { headers: html });
    return new Response("not found", { status: 404 });
  },
});
const site = `http://localhost:${server.port}`;
const rp = startRelyingParty();

const sha = (path: string) => createHash("sha256").update(readFileSync(path)).digest("hex");

/** Waits until nothing opens: no decision window appears in `ms`. */
async function noWindow(why: string, ms = 2500) {
  await sleep(ms);
  const open = await h.find("decide.html");
  check(why, open.length === 0, open.map((c) => c.url).join(","));
}

async function status(tab: string, expected: string | RegExp, timeoutMs = 15000): Promise<string> {
  let text = "";
  try {
    await h.waitFor(`status ${expected}`, async () => ((text = await h.ev<string>(tab, "document.getElementById('status').textContent")), typeof expected === "string" ? text === expected : expected.test(text)), timeoutMs);
  } catch {
    // the caller's check shows the text
  }
  return text;
}

async function passkeyTab(path = ""): Promise<string> {
  return h.newTab(`${rp.origin}/${path}`);
}

const passkeys = () => h.vault().filter((i) => i.kind === "passkey");

try {
  // 0. The shipped add-on is the E2E add-on but for the shadow-root mode.
  if (existsSync(join(shippedDir, "manifest.json"))) {
    const differing = readdirSync(shippedDir).filter((f) => sha(join(shippedDir, f)) !== sha(join(extensionDir, f)));
    const same = differing.length === 1 && differing[0] === "content.js" && readFileSync(join(shippedDir, "content.js"), "utf8").replace('mode: "closed"', 'mode: "open"') === readFileSync(join(extensionDir, "content.js"), "utf8");
    check("dist-firefox differs from the tested build only in the shadow-root mode", same, differing.join(","));
  } else console.log("skip dist-firefox is not built (bun run build --firefox)");

  await h.startHost([`Local site|${site}|alice|alice-secret`]);
  await h.startFirefox(extensionDir);

  // 1. Pair. Unpaired, the badge still opens the decision window, and it says how to connect.
  const login = await h.newTab(`${site}/login`);
  await h.ev(login, "document.getElementById('u').focus()");
  const badge = await h.waitFor("the badge", async () => (await h.badgeRect(login)) ?? false);
  await h.trustedClick(login, badge);
  let win = await h.decisionWindow();
  const why = await h.waitWindowText(win, /LocalPass icon/);
  check("an unpaired browser is told to connect", /LocalPass icon.*Connect/.test(why), why);

  // BiDi cannot navigate a tab to moz-extension://, but the window (an extension page) can open the popup in a tab.
  await h.ev(win, `chrome.tabs.create({ url: chrome.runtime.getURL("popup.html") }).then(() => "ok")`);
  const popup = (await h.waitFor("the popup tab", async () => (await h.find("popup.html"))[0])).context;
  await h.waitFor("the connect offer", async () => (await h.ev<string>(popup, "document.body.innerText")).includes("Connect to LocalPass"), 20000);
  await h.ev(popup, `[...document.querySelectorAll("button")].find((b) => b.textContent === "Connect").click()`);
  const paired = await h.waitForHost((l) => l.startsWith("PAIRED"));
  check("the app is asked to approve a Firefox extension", /PAIRED Firefox extension code=\d{6}/.test(paired), paired);
  await h.waitFor("the popup to be connected", async () => !(await h.ev<string>(popup, "document.body.innerText")).includes("Connect to LocalPass"), 10000);
  check("the popup is connected", true);
  // (The popup stays open as a control surface for the extension's own pages, e.g. its passkeys switch.)
  await h.closeContext(win);
  check("the window closes", await h.windowGone(win));

  // 2. The badge: a page-made click opens nothing, a real click opens one window, a second real click replaces it.
  await h.ev(login, "document.getElementById('u').blur(), document.getElementById('u').focus()");
  const ready = await h.waitFor("a ready badge", async () => {
    const b = await h.badgeRect(login);
    return b?.ready ? b : false;
  });
  await h.ev(login, `[...document.documentElement.children].find((e) => e.shadowRoot).shadowRoot.querySelector('.badge').click()`);
  await noWindow("a click the page made up on the badge opens nothing");
  await h.trustedClick(login, ready);
  win = await h.decisionWindow();
  const list = await h.waitWindowText(win, /Fill a login/);
  check("the window shows the site the browser reported and the saved login", /^localhost \|.*Fill a login for localhost.*Local site.*alice/.test(list), list);
  check("the window never carries the password", !list.includes("alice-secret"));
  check("one window is open", (await h.find("decide.html")).length === 1);
  await h.trustedClick(login, ready);
  await sleep(1500);
  const again = await h.find("decide.html");
  check("a second click replaces the window (one per tab)", again.length === 1 && again[0]!.context !== win, `${again.length} open`);
  win = again[0]!.context;
  await h.waitWindowText(win, /Fill a login/);

  // The window holds its acting controls for a moment after it shows or regains focus.
  const held = await h.ev<string>(win, `(() => { window.dispatchEvent(new Event('focus')); const b = document.querySelector('#main button.row'); return b ? String(b.disabled) : 'no row'; })()`);
  check("a login row is disabled right after the window gets focus", held === "true", held);
  const cancelEnabled = await h.ev<string>(win, `String([...document.querySelectorAll('#main button')].find((b) => b.textContent === 'Cancel').disabled)`);
  check("Cancel works at once", cancelEnabled === "false", cancelEnabled);
  await sleep(900);
  check("the row is enabled again after the hold", (await h.ev<string>(win, `String(document.querySelector('#main button.row').disabled)`)) === "false");
  // A page cannot see or press anything in the window; a click inside it chooses.
  check("nothing is filled before a row is clicked in the window", (await h.ev<string>(login, "document.getElementById('u').value + document.getElementById('p').value")) === "");
  await h.clickWindow(win, /^Local site/);
  await h.waitFor("the fill", async () => (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret");
  check("the username is filled", (await h.ev<string>(login, "document.getElementById('u').value")) === "alice");
  check("the password is filled", (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret");
  check("the window closes after a fill", await h.windowGone(win));

  // 3. A login that LocalPass filled is not offered for saving again.
  await h.ev(login, "document.getElementById('login').requestSubmit()");
  await noWindow("signing in with a filled login does not offer to save it", 3000);

  // 3b. Decoys. Firefox has no proof that a badge could be seen (no IntersectionObserver `isVisible`), so the badge only needs a
  // real click (`armOpener`): a page that hides or covers it can still get that click, and the result is a window that shows the
  // browser-reported site and fills nothing until a row is clicked IN the window. Record what happens and hold that line.
  for (const [path, what] of [["attack-style", "hidden with !important CSS"], ["attack-cover", "covered by a see-through decoy"]] as const) {
    const t = await h.newTab(`${site}/${path}`);
    await h.ev(t, "document.getElementById('u').focus()");
    await sleep(1500);
    const rect = await h.badgeRect(t);
    let opened = false;
    let detail = "no badge box";
    if (rect && rect.x > 0 && rect.y > 0) {
      await h.trustedClick(t, rect);
      await sleep(2000);
      const windows = await h.find("decide.html");
      opened = windows.length > 0;
      detail = opened ? await h.windowText(windows[0]!.context) : "no window";
      for (const w of windows) await h.closeContext(w.context);
    }
    check(`badge ${what}: a click on it fills nothing (window opened: ${opened}; ${detail})`, (await h.ev<string>(t, "document.getElementById('u').value + document.getElementById('p').value")) === "");
    check(`badge ${what}: any window shows only the browser-reported site`, !opened || /^localhost \| (Looking|Fill a login|LocalPass)/.test(detail), detail);
    await h.closeContext(t);
  }

  // 4. Save a new login: "Not now" drops it, Save stores it, a window closed with X is "not now".
  const typeAndSubmit = async (user: string, pass: string) => {
    const tab = await h.newTab(`${site}/login`);
    await h.ev(tab, `document.getElementById('u').value = ${JSON.stringify(user)}, document.getElementById('p').value = ${JSON.stringify(pass)}, document.getElementById('login').requestSubmit()`);
    return tab;
  };
  await typeAndSubmit("bob", "bob-password");
  win = await h.decisionWindow();
  const offer = await h.waitWindowText(win, /Save this login/);
  check("a new login is offered for saving, with the site the browser reported", /^localhost \|.*Save this login to LocalPass\?.*bob on localhost/.test(offer), offer);
  check("the save window never shows the password", !offer.includes("bob-password"));
  await h.clickWindow(win, "Not now");
  check("Not now closes the window", await h.windowGone(win));
  check("Not now saved nothing", !h.vault().some((i) => i.username === "bob"));
  await sleep(1500);
  check("the same sign-in is not offered twice", (await h.find("decide.html")).length === 0);

  await typeAndSubmit("bob", "bob-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.closeContext(win); // the window's own X
  check("closing the save window with X counts as Not now", await h.windowGone(win));
  check("nothing saved after X", !h.vault().some((i) => i.username === "bob"));

  await typeAndSubmit("bob", "bob-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.clickWindow(win, "Save");
  const saved = await h.waitWindowText(win, /Saved/, 8000);
  check("the window says it was saved", /Saved/.test(saved), saved);
  const items = await h.waitForVault((all) => all.some((i) => i.username === "bob"));
  const bob = items.find((i) => i.username === "bob");
  check("the login is in the vault with its password", bob?.password === "bob-password" && bob.urls.some((u) => u.startsWith(site)), JSON.stringify(bob?.urls));
  const before = Date.now();
  check("the window closes by itself after about two seconds", await h.windowGone(win, 6000), `${Date.now() - before} ms`);

  // 5. "Never for this site" ends the offers for that origin.
  await typeAndSubmit("carol", "carol-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.clickWindow(win, "Never for this site");
  check("Never closes the window", await h.windowGone(win));
  await typeAndSubmit("dave", "dave-password");
  await noWindow("a site marked Never is not offered again", 3000);
  check("neither Never nor the later sign-in saved anything", !h.vault().some((i) => ["carol", "dave"].includes(i.username)));

  // 6. A sandboxed frame has an opaque origin: nothing of LocalPass answers there.
  const framed = await h.newTab(`${site}/framed`);
  const frames = (await h.contexts()).find((c) => c.context === framed)?.children ?? [];
  check("the sandboxed frame exists", frames.length === 1);
  if (frames[0]) {
    // The page-side wrapper refuses first (a frame whose origin it cannot compare with the top page's is not "the same site"),
    // so this call never reaches the content script; the background's own refusal (sender.origin "null") is pinned by test/background.test.ts.
    const wrapped = await h.ev<string>(frames[0].context, `typeof window.__localpassPasskeys`);
    check("the wrapper is installed in the sandboxed frame", wrapped !== "undefined", wrapped);
    const outcome = await h.ev<string>(frames[0].context, `navigator.credentials.create({ publicKey: { challenge: new Uint8Array(32), rp: { name: "x" }, user: { id: new Uint8Array([1]), name: "u", displayName: "u" }, pubKeyCredParams: [{ type: "public-key", alg: -7 }], timeout: 3000 } }).then(() => "created", (e) => e.name)`);
    check("a passkey request from the sandboxed frame opens no LocalPass window and gets nothing from it", outcome !== "created" && (await h.find("decide.html")).length === 0, outcome);
  }
  // Fill: a login form in a frame of the same site gets the badge (the control); the same form in a sandboxed frame does not.
  const control = await h.newTab(`${site}/plainframe`);
  const plain = (await h.contexts()).find((c) => c.context === control)?.children?.[0];
  await h.ev(plain!.context, "document.getElementById('u').focus()");
  const plainBadge = await h.waitFor("the badge in a same-site frame", async () => (await h.badgeRect(plain!.context)) ?? false).catch(() => undefined);
  check("control: a login form in a frame of the same site gets the badge", !!plainBadge);
  const sandboxTab = await h.newTab(`${site}/framed`);
  const sandboxed = (await h.contexts()).find((c) => c.context === sandboxTab)?.children?.[0];
  await h.ev(sandboxed!.context, "document.getElementById('u').focus()");
  await sleep(2500);
  check("a login form in a sandboxed frame (origin null) gets no badge", (await h.badgeRect(sandboxed!.context)) === undefined);

  // 7. Passkeys, checked by SimpleWebAuthn.
  let p = await passkeyTab();
  await h.clickElement(p, "#register");
  win = await h.decisionWindow();
  const create = await h.waitWindowText(win, /Create a passkey/);
  check("a window asks before creating a passkey", /^localhost \| localhost \| Account: alice/.test(create), create);
  const heldCreate = await h.ev<string>(win, `(() => { window.dispatchEvent(new Event('focus')); return String([...document.querySelectorAll('#main button')].find((b) => b.textContent === 'Create passkey').disabled); })()`);
  check("Create passkey is held right after the window gets focus", heldCreate === "true", heldCreate);
  await h.clickWindow(win, "Create passkey");
  check("the site accepts the registration", (await status(p, "registered")) === "registered", await h.ev<string>(p, "document.getElementById('status').textContent"));
  const stored = (await h.waitForVault((all) => all.some((i) => i.kind === "passkey"))).find((i) => i.kind === "passkey");
  check("the passkey is in the vault", stored?.passkey?.rpId === "localhost" && stored.passkey.userName === "alice", JSON.stringify(stored?.passkey));
  check("the vault listing carries no private key", !!stored && !JSON.stringify(h.vault()).includes("MIGH") && stored.password === "");
  check("the create window closes", await h.windowGone(win));

  // Registering again is refused through the exclude list.
  p = await passkeyTab();
  await h.clickElement(p, "#register");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Create a passkey/);
  await h.clickWindow(win, "Create passkey");
  const refused = await h.waitWindowText(win, /Could not create/);
  check("the window says a passkey already exists", /already exists/i.test(refused), refused);
  await h.clickWindow(win, "Close");
  const exists = await status(p, /^error:/);
  check("the site's exclude list is honoured (InvalidStateError)", exists.startsWith("error:InvalidStateError"), exists);
  check("still one passkey", passkeys().length === 1);

  // Sign in with the passkey.
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  const chooser = await h.waitWindowText(win, /Choose a passkey/);
  check("the sign-in window lists the passkey", chooser.startsWith("localhost |") && chooser.includes("alice"), chooser);
  await h.clickWindow(win, /^alice|^Passkey|alice/);
  check("the site verifies the sign-in", (await status(p, "signed-in")) === "signed-in", await h.ev<string>(p, "document.getElementById('status').textContent"));

  // Conditional mediation (sign in from the username field) is not handled on Firefox: the page gets the browser's own flow at once.
  p = await passkeyTab("#conditional");
  await noWindow("a conditional sign-in opens no LocalPass window on Firefox");
  check("a conditional sign-in is left to Firefox", (await h.ev<string>(p, "document.getElementById('status').textContent")) === "idle");

  // A locked vault is unlocked from the window.
  h.hostCommand("lock");
  await h.waitForHost((l) => l === "LOCKED");
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  const locked = await h.waitWindowText(win, /locked/);
  check("a locked vault is reported in the window", /LocalPass is locked/.test(locked), locked);
  await h.clickWindow(win, "Unlock");
  await h.waitWindowText(win, /Choose a passkey/);
  await h.clickWindow(win, /alice/);
  check("after unlocking the sign-in works", (await status(p, "signed-in")) === "signed-in");

  // Cancel, and the window's own X, are refusals the page can handle.
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.clickWindow(win, "Cancel");
  const cancelled = await status(p, /^error:/);
  check("Cancel refuses the request (NotAllowedError)", cancelled.startsWith("error:NotAllowedError"), cancelled);
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.closeContext(win);
  const closed = await status(p, /^error:/);
  check("closing the window with X refuses the request (NotAllowedError)", closed.startsWith("error:NotAllowedError"), closed);

  // A page cannot ask for another site's passkeys; nothing opens for it.
  p = await passkeyTab();
  await h.clickElement(p, "#wrong-rp");
  const wrong = await status(p, /^(created|error:.*)$/);
  check("another site's RP ID is refused with SecurityError", wrong === "error:SecurityError", wrong);
  check("no window and no passkey for it", (await h.find("decide.html")).length === 0 && passkeys().length === 1);

  // What LocalPass declines still reaches the browser: a request for a security key.
  p = await passkeyTab();
  await h.clickElement(p, "#register-key");
  await noWindow("a security-key request opens no LocalPass window");
  check("LocalPass did not take the security-key registration", passkeys().length === 1 && (await h.ev<string>(p, "document.getElementById('status').textContent")) !== "registered-key");

  // The popup's passkey switch turns LocalPass off for passkeys (and back on).
  await h.ev(popup, `document.getElementById('passkeys').click()`);
  await sleep(500);
  check("the switch is off", (await h.ev<string>(popup, "String(document.getElementById('passkeys').checked)")) === "false");
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  await noWindow("with the switch off LocalPass shows no window");
  await h.ev(popup, `document.getElementById('passkeys').click()`);
  await sleep(500);
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  check("with the switch on again the window is back", /Choose a passkey/.test(await h.waitWindowText(win, /Choose a passkey/)));
  await h.clickWindow(win, "Cancel");

  // 8. Opening a window while the vault has many windows: at most one per tab, eight in all.
  const tabs: string[] = [];
  for (let i = 0; i < 9; i++) {
    const t = await passkeyTab();
    tabs.push(t);
    await h.clickElement(t, "#signin");
    await sleep(400);
  }
  await sleep(1500);
  const crowd = (await h.find("decide.html")).length;
  check("no more than eight LocalPass windows are open at once", crowd <= 8, `${crowd} open`);
  for (const w of await h.find("decide.html")) await h.closeContext(w.context);

  if (process.env.SLOW) {
    // The background is an event page that Firefox suspends when idle; the open window keeps its request alive.
    p = await passkeyTab();
    await h.clickElement(p, "#signin");
    win = await h.decisionWindow();
    await h.waitWindowText(win, /Choose a passkey/);
    await sleep(75_000);
    await h.clickWindow(win, /alice/);
    check("a request still works after 75 s in the window", (await status(p, "signed-in")) === "signed-in");
  }
  // 9. The wrapper's objects behave like the browser's (plan 6.6): shape of the credential a page gets back. Last, because the
  // registration replaces the passkey the relying party knows.
  p = await passkeyTab();
  const shapeGet = h.ev<string>(p, `(async () => {
    const options = await (await fetch('/api/auth/options', { method: 'POST' })).json();
    const c = await navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options) });
    return JSON.stringify({ isPKC: c instanceof PublicKeyCredential, isCred: c instanceof Credential, type: c.type, idIsString: typeof c.id === 'string', raw: c.rawId instanceof ArrayBuffer,
      resp: c.response instanceof AuthenticatorAssertionResponse, respBuffers: [c.response.clientDataJSON, c.response.authenticatorData, c.response.signature].every((b) => b instanceof ArrayBuffer),
      toJSON: typeof c.toJSON, jsonKeys: Object.keys(c.toJSON()).sort().join(), ext: JSON.stringify(c.getClientExtensionResults()), cond: await PublicKeyCredential.isConditionalMediationAvailable() });
  })()`);
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.clickWindow(win, /alice/);
  const g = JSON.parse(await shapeGet) as Record<string, unknown>;
  check("a sign-in result is a PublicKeyCredential with an assertion response", g.isPKC === true && g.isCred === true && g.type === "public-key" && g.idIsString === true && g.raw === true && g.resp === true && g.respBuffers === true, JSON.stringify(g));
  check("it has toJSON and getClientExtensionResults like the browser's", g.toJSON === "function" && /clientExtensionResults/.test(String(g.jsonKeys)) && g.ext === "{}", `${g.jsonKeys} ${g.ext}`);
  check("isConditionalMediationAvailable answers (Firefox's own answer: LocalPass leaves conditional requests to it)", typeof g.cond === "boolean", String(g.cond));

  p = await passkeyTab();
  const shapeCreate = h.ev<string>(p, `(async () => {
    const options = await (await fetch('/api/register/options', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ exclude: false }) })).json();
    const c = await navigator.credentials.create({ publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options) });
    const r = c.response;
    return JSON.stringify({ isPKC: c instanceof PublicKeyCredential, resp: r instanceof AuthenticatorAttestationResponse, getPublicKey: r.getPublicKey() instanceof ArrayBuffer, alg: r.getPublicKeyAlgorithm(), authData: r.getAuthenticatorData() instanceof ArrayBuffer,
      transports: JSON.stringify(r.getTransports()), jsonResp: Object.keys(c.toJSON().response).sort().join() });
  })()`);
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Create a passkey/);
  await h.clickWindow(win, "Create passkey");
  const c = JSON.parse(await shapeCreate) as Record<string, unknown>;
  check("a registration result is a PublicKeyCredential with an attestation response and its accessors", c.isPKC === true && c.resp === true && c.getPublicKey === true && c.alg === -7 && c.authData === true, JSON.stringify(c));
} catch (e) {
  console.error(e);
  h.failures++;
} finally {
  rp.stop();
  server.stop(true);
  await h.stop();
}

h.finish("all Firefox end-to-end checks passed");
