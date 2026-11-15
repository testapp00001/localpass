// LocalPass in real Firefox against the real broker (Linux, headless, BiDi). See harness.ts for the set-up.
//
//   cargo build -p lp-broker --examples && cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts
//
// Pairing through the real native-messaging helper, fill, save, the decision window's guards, and passkeys
// checked by an independent relying party (SimpleWebAuthn, e2e/rp.ts). The shipped add-on (`dist-firefox`)
// differs from the E2E build only in the on-page UI's shadow-root mode (the first check proves it), so one
// run covers both. SLOW=1 adds a wait of over a minute inside a decision window.

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
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

/** A two-step sign-in: the account name on one page, the password on the next (with no user name box). */
const accountStep = `<!doctype html><title>Account</title>
<form method="post" action="/two-step/password" id="step1">
  <input name="username" id="u" type="text" autocomplete="username" placeholder="Account" style="width:300px;height:30px">
  <button type="submit">Next</button>
</form>`;
const passwordStep = `<!doctype html><title>Password</title>
<form method="post" action="/session" id="login">
  <input name="password" id="p" type="password" autocomplete="current-password" placeholder="Password" style="width:300px;height:30px">
  <button type="submit">Sign in</button>
</form>`;

/** A sign-up form: a new password and its confirmation (8.3b: the picker offers to generate one). */
const signupPage = `<!doctype html><title>Sign up</title>
<form method="post" action="/session" id="signup">
  <input name="username" id="u" autocomplete="username" placeholder="Username">
  <input name="password" id="np" type="password" autocomplete="new-password" placeholder="New password" style="width:300px;height:30px">
  <input name="confirm" id="np2" type="password" autocomplete="new-password" placeholder="Repeat it">
  <button type="submit">Create account</button>
</form>`;

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
    if (url.pathname === "/signup") return new Response(signupPage, { headers: html });
    if (url.pathname === "/two-step") return new Response(accountStep, { headers: html });
    if (url.pathname === "/two-step/password") return new Response(passwordStep, { headers: html });
    if (url.pathname === "/session") return new Response(null, { status: 303, headers: { location: "/dashboard" } });
    // A sign-in whose next page sends the browser on to another address (another origin) a moment later.
    if (url.pathname === "/session-hop") return new Response(null, { status: 303, headers: { location: "/hop" } });
    if (url.pathname === "/hop") {
      const next = `http://127.0.0.1:${url.port}/dashboard`;
      return new Response(`<!doctype html><title>Signed in</title><p>One moment</p><script>setTimeout(() => (location.href = ${JSON.stringify(next)}), 2500)</script>`, { headers: html });
    }
    if (url.pathname === "/dashboard") return new Response("<!doctype html><title>Dashboard</title><h1>Welcome</h1>", { headers: html });
    // A frame whose origin is opaque (CSP sandbox): Firefox reports origin "null" and LocalPass must refuse it.
    if (url.pathname === "/sandboxed") return new Response(loginPage("Sandboxed"), { headers: { ...html, "content-security-policy": "sandbox allow-scripts allow-forms" } });
    if (url.pathname === "/plainframe") return new Response(`<!doctype html><title>Plain frame</title><iframe src="/login" width="500" height="200"></iframe>`, { headers: html });
    if (url.pathname === "/crossframe") return new Response(`<!doctype html><title>Cross-origin frame</title><iframe src="http://127.0.0.1:${url.port}/login" width="500" height="200"></iframe>`, { headers: html });
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

/** The extension's popup page in a tab (opened in step 1): it reads the toolbar state of the active tab. */
let popup = "";
interface Toolbar {
  popup: string;
  badge: string;
  url: string;
  /** Decisions whose toolbar change the background still holds (session storage `decidePanel:<tab>`), in all tabs. */
  open: number;
  /** Whether the background still holds one for this tab. */
  mine: boolean;
}
async function toolbar(): Promise<Toolbar> {
  return JSON.parse(
    await h.ev<string>(
      popup,
      `chrome.tabs.query({ active: true, currentWindow: true }).then(async ([t]) => JSON.stringify({ popup: await chrome.action.getPopup({ tabId: t.id }), badge: await chrome.action.getBadgeText({ tabId: t.id }), url: t.url ?? "", open: Object.keys(await chrome.storage.session.get(null)).filter((k) => k.startsWith("decidePanel:")).length, mine: ("decidePanel:" + t.id) in (await chrome.storage.session.get(null)) }))`,
    ),
  );
}
const normal = (t: Toolbar) => t.popup.endsWith("/popup.html") && t.badge === "";
const waiting = (t: Toolbar) => t.popup.includes("/decide.html?") && t.badge === "1";
/** Waits until the active tab's toolbar is in a state, and returns the last one seen. */
async function toolbarIs(ok: (t: Toolbar) => boolean, timeoutMs = 5000): Promise<Toolbar> {
  let last = await toolbar();
  try {
    await h.waitFor("the toolbar state", async () => ok((last = await toolbar())), timeoutMs);
  } catch {
    // the caller's check shows the state
  }
  return last;
}
/** Starts watching for the host's next line that matches (`waitForHost` also finds earlier ones); await the result. */
function nextHostLine(match: (line: string) => boolean): () => Promise<unknown> {
  const before = h.hostLines(match).length;
  return () => h.waitFor("the dev host's next line", async () => h.hostLines(match).length > before, 15000);
}
const browserWindows = () => h.ev<number>(popup, "chrome.windows.getAll().then((w) => w.length)");

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
  popup = (await h.waitFor("the popup tab", async () => (await h.find("popup.html"))[0])).context;
  await h.waitFor("the connect offer", async () => (await h.ev<string>(popup, "document.body.innerText")).includes("Connect to LocalPass"), 20000);
  await h.clickWindow(popup, "Connect"); // the popup in a tab is held for 0.6 s (8.3c)
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
  check("password picking uses the toolbar panel", win.startsWith("panel:"));
  check("password picking creates no additional browser window", await h.ev<number>(popup, "chrome.windows.getAll().then(w => w.length)") === 1);
  if (process.env.LP_FF_SCREENSHOT) {
    // Browser-window screenshots omit native popup surfaces. Capture the panel's compositor snapshot.
    const [, owner, id] = win.split(":");
    const shot = await h.ev<string>(owner!, `(async () => {
      const bitmap = await BrowsingContext.get(${id}).currentWindowGlobal.drawSnapshot(null, 1, 'white');
      const canvas = document.createElementNS('http://www.w3.org/1999/xhtml', 'canvas');
      canvas.width = bitmap.width; canvas.height = bitmap.height;
      canvas.getContext('2d').drawImage(bitmap, 0, 0); bitmap.close();
      return canvas.toDataURL().split(',')[1];
    })()`);
    writeFileSync(process.env.LP_FF_SCREENSHOT, Buffer.from(shot, "base64"));
  }
  check("the window shows the site the browser reported and the saved login", /^localhost \|.*Fill a login for localhost.*Local site.*alice/.test(list), list);
  check("the window never carries the password", !list.includes("alice-secret"));
  check("one window is open", (await h.find("decide.html")).length === 1);
  await h.trustedClick(login, ready);
  await sleep(1500);
  const again = await h.find("decide.html");
  check("a second click replaces the window (one per tab)", again.length === 1 && again[0]!.context !== win, `${again.length} open`);
  win = again[0]!.context;
  await h.waitWindowText(win, /Fill a login/);

  // The window holds its acting controls for a moment after it shows or regains focus (8.3c: marked, not greyed out).
  const heldState = (selector: string) => `(() => { const b = document.querySelector(${JSON.stringify(selector)}); return b ? (b.disabled ? 'disabled' : String(b.getAttribute('aria-disabled') === 'true')) : 'none'; })()`;
  const held = await h.ev<string>(win, `(() => { window.dispatchEvent(new Event('focus')); return ${heldState("#main button.row")}; })()`);
  check("a login row is held (not greyed out) right after the window gets focus", held === "true", held);
  const cancelEnabled = await h.ev<string>(win, `(() => { const b = [...document.querySelectorAll('#main button')].find((b) => b.textContent === 'Cancel'); return String(b.disabled || b.hasAttribute('aria-disabled')); })()`);
  check("Cancel works at once", cancelEnabled === "false", cancelEnabled);
  await sleep(900);
  check("the row is free again after the hold", (await h.ev<string>(win, heldState("#main button.row"))) === "false");
  // Esc closes the chooser and fills nothing; the badge then opens a fresh one.
  await h.pressKey(win, "Escape");
  check("Esc closes the chooser", await h.windowGone(win));
  check("Esc filled nothing", (await h.ev<string>(login, "document.getElementById('u').value + document.getElementById('p').value")) === "");
  await h.trustedClick(login, ready);
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Fill a login/);
  // A page cannot see or press anything in the window; a click inside it chooses.
  check("nothing is filled before a row is clicked in the window", (await h.ev<string>(login, "document.getElementById('u').value + document.getElementById('p').value")) === "");
  await h.clickWindow(win, /^Local site/);
  await h.waitFor("the fill", async () => (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret");
  check("the username is filled", (await h.ev<string>(login, "document.getElementById('u').value")) === "alice");
  check("the password is filled", (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret");
  check("the window closes after a fill", await h.windowGone(win));

  // 8.3c: the same pick with the keyboard. The badge's panel is the page's doing: Enter waits for the hold; then Down and Enter fill.
  await h.ev(login, "document.getElementById('u').value = '', document.getElementById('p').value = ''");
  await h.trustedClick(login, ready);
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Local site/);
  await h.pressKey(win, "ArrowDown");
  const focusedRow = await h.ev<string>(win, "String(document.activeElement?.classList.contains('row'))");
  await h.pressKey(win, "Enter");
  const enterHeld = await h.ev<string>(win, heldState("#main button.row"));
  await sleep(300);
  if (enterHeld === "true") check("Enter on a row during the hold fills nothing", (await h.ev<string>(login, "document.getElementById('p').value")) === "");
  else console.log("note: the hold had ended before Enter reached the panel; the Enter-during-hold check is skipped this run");
  check("Down moves the focus to the first row", focusedRow === "true", focusedRow);
  await h.waitFor("the hold to end", async () => (await h.ev<string>(win, heldState("#main button.row"))) === "false", 3000);
  await h.pressKey(win, "Enter");
  await h.waitFor("the keyboard fill", async () => (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret").catch(() => {});
  check("Down then Enter in the panel fills the login", (await h.ev<string>(login, "document.getElementById('u').value + '/' + document.getElementById('p').value")) === "alice/alice-secret");
  check("the panel closes after the keyboard pick", await h.windowGone(win));
  check("the original tab's normal toolbar popup is restored", await h.ev<boolean>(popup, `chrome.tabs.query({active:true,currentWindow:true}).then(async tabs => (await chrome.action.getPopup({tabId:tabs[0].id})) === chrome.runtime.getURL('popup.html'))`));

  // 8.3c part 2: the normal toolbar popup opened by the user (a real click on the toolbar button; the shortcut opens the
  // same popup) focuses the first Fill for the page, and Enter fills that login.
  const shortcut = await h.ev<string>(popup, "chrome.commands.getAll().then((all) => all.find((c) => c.name === '_execute_action')?.shortcut ?? 'none')");
  check("the browser assigns the shortcut Alt+Shift+L", shortcut === "Alt+Shift+L", shortcut);

  // 8.3c follow-up: a panel whose request is gone turns into the normal popup marked `from=panel` (`toPopup`), and that
  // popup is held and focuses no Fill even where the background would tell an unmarked popup that the user opened it.
  const setTabPopup = (page: string) =>
    h.ev(popup, `chrome.tabs.query({ active: true, currentWindow: true }).then(([t]) => chrome.action.setPopup({ tabId: t.id, popup: ${JSON.stringify(page)} })).then(() => "ok")`);
  await h.ev(login, "document.getElementById('u').value = '', document.getElementById('p').value = ''");
  // (a) Information only: a panel for a request that does not exist. Its port is refused at once, and Firefox has so far
  // always closed the panel (onDisconnect) before `toPopup` could run, so this cannot be a check.
  await setTabPopup("decide.html?id=gone&presentation=panel");
  await h.clickToolbarButton();
  const stale = await h.waitFor("a stale panel turned into the popup", async () => (await h.toolbarPopup()) ?? false, 3000).catch(() => undefined);
  console.log(`info a stale panel ${stale ? `turned into the popup (${await h.ev<string>(stale, "location.search")})` : "closed before it could turn into the popup"}`);
  if (stale) await h.closeContext(stale);
  await h.waitFor("no toolbar popup", async () => !(await h.toolbarPopup()), 5000).catch(() => {});
  // (b) The popup a panel turns into, opened in a real toolbar panel with a real click: the background has no count of
  // its own for this window, so it would tell an unmarked popup that the user opened it (as in the next step).
  await setTabPopup("popup.html?from=panel");
  check("the toolbar button can be clicked for the marked popup", (await h.clickToolbarButton()) === "clicked");
  const turned = await h.waitFor("the marked popup", async () => (await h.toolbarPopup()) ?? false, 8000).catch(() => undefined);
  check("the popup a panel turns into opens in the toolbar panel", !!turned);
  if (turned) {
    await h.waitFor("the popup's logins", async () => (await h.ev<number>(turned, "document.querySelectorAll('ul.list button').length")) > 0, 8000).catch(() => {});
    const view = await h.ev<string>(
      turned,
      `JSON.stringify({ search: location.search, fills: document.querySelectorAll('ul.list button').length, held: document.querySelectorAll('ul.list button[aria-disabled="true"]').length })`,
    );
    const v = JSON.parse(view) as { search: string; fills: number; held: number };
    check("the popup a panel turned into carries the marker and is held (its Fill buttons aria-disabled)", v.search === "?from=panel" && v.fills > 0 && v.held === v.fills, view);
    await sleep(800); // past the hold: a user-opened note would have moved the focus to the first Fill by now
    const later = await h.ev<string>(turned, `JSON.stringify({ active: document.activeElement?.textContent ?? '', inList: !!document.activeElement?.closest('ul.list') })`);
    check("the popup a panel turned into has no Fill focused", !JSON.parse(later).inList && JSON.parse(later).active !== "Fill", later);
    await h.pressKey(turned, "Enter");
    await sleep(300);
    check("Enter in that popup fills nothing", (await h.ev<string>(login, "document.getElementById('p').value")) === "");
    await h.closeContext(turned);
    await h.waitFor("the popup to close", async () => !(await h.toolbarPopup()), 5000).catch(() => {});
  }
  await setTabPopup("popup.html");
  await h.ev(login, "document.getElementById('u').value = '', document.getElementById('p').value = ''");
  check("the toolbar button can be clicked for the normal popup", (await h.clickToolbarButton()) === "clicked");
  const toolbarPopup = await h.waitFor("the toolbar popup", async () => (await h.toolbarPopup()) ?? false);
  await h.waitFor("the first Fill to have the focus", async () => (await h.ev<string>(toolbarPopup, "document.activeElement?.textContent ?? ''")) === "Fill", 8000).catch(() => {});
  const popupFocus = await h.ev<string>(
    toolbarPopup,
    `JSON.stringify({ active: document.activeElement?.textContent ?? '', own: !!document.activeElement?.closest('ul.list') && document.querySelector('h2')?.textContent === 'Logins for localhost', held: document.querySelectorAll('[aria-disabled]').length })`,
  );
  check("a popup the user opened focuses the first Fill for this site, and nothing is held", popupFocus === JSON.stringify({ active: "Fill", own: true, held: 0 }), popupFocus);
  await h.pressKey(toolbarPopup, "Enter");
  await h.waitFor("the popup's fill", async () => (await h.ev<string>(login, "document.getElementById('p').value")) === "alice-secret").catch(() => {});
  check("Enter in the popup fills that login", (await h.ev<string>(login, "document.getElementById('u').value + '/' + document.getElementById('p').value")) === "alice/alice-secret");
  check("the popup closes after filling", await h.waitFor("the popup to close", async () => !(await h.toolbarPopup()), 5000).then(() => true, () => false));

  // 3. A login that LocalPass filled is not offered for saving again.
  await h.ev(login, "document.getElementById('login').requestSubmit()");
  await noWindow("signing in with a filled login does not offer to save it", 3000);

  const openPicker = async (tab: string, fieldId = "u", shows = /Fill a login/) => {
    await h.send("browsingContext.activate", { context: tab });
    await h.ev(tab, `document.getElementById('${fieldId}').focus()`);
    const rect = await h.waitFor(`a picker badge on the #${fieldId} field`, async () => {
      const b = await h.badgeRect(tab);
      const field = JSON.parse(await h.ev<string>(tab, `JSON.stringify(document.getElementById('${fieldId}').getBoundingClientRect())`));
      return b && b.x >= field.left && b.x <= field.right && b.y >= field.top && b.y <= field.bottom ? b : false;
    });
    await h.trustedClick(tab, rect);
    const picker = await h.decisionWindow();
    await h.waitWindowText(picker, shows);
    return picker;
  };
  const target = await h.newTab(`${site}/login`);
  await h.ev(target, `document.body.insertAdjacentHTML('beforeend', '<form><input id="other-user" autocomplete="username" value="untouched"><input type="password" id="other-password" value="untouched"></form>')`);
  let picker = await openPicker(target);
  await h.clickWindow(picker, /^Local site/);
  await h.waitFor("targeted fill", async () => await h.ev<boolean>(target, "document.getElementById('p').value === 'alice-secret'"));
  check("only the form whose badge was clicked is filled", await h.ev<boolean>(target, "document.getElementById('other-user').value === 'untouched' && document.getElementById('other-password').value === 'untouched'"));
  await h.ev(target, "document.getElementById('u').value = '', document.getElementById('p').value = ''");
  picker = await openPicker(target);
  // BiDi routes page clicks directly to content and bypasses Firefox's native popup dismissal.
  // Use the native panel's hidePopup path (the same destruction as clicking outside).
  await h.closeContext(picker);
  await h.clickElement(target, "#p");
  check("native panel dismissal cancels the toolbar picker", await h.windowGone(picker));
  check("clicking outside fills nothing", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  picker = await openPicker(target);
  await h.newTab(`${site}/login`);
  check("changing tabs cancels the picker", await h.windowGone(picker));
  check("changing tabs fills nothing", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  picker = await openPicker(target);
  await h.send("browsingContext.navigate", { context: target, url: `${site}/login?new-document`, wait: "complete" });
  check("navigation cancels the picker", await h.windowGone(picker));
  check("the new document receives no stale fill", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  picker = await openPicker(target);
  const otherWindow = await h.send("browsingContext.create", { type: "window" });
  await h.send("browsingContext.navigate", { context: otherWindow.context, url: `${site}/login`, wait: "complete" });
  await h.send("browsingContext.activate", { context: otherWindow.context });
  check("switching browser windows dismisses the old picker", await h.windowGone(picker));
  const otherPicker = await openPicker(otherWindow.context);
  check("a second browser window uses its own toolbar panel", otherPicker.startsWith("panel:") && otherPicker.split(":")[1] !== picker.split(":")[1]);
  await h.clickWindow(otherPicker, /^Local site/);
  await h.waitFor("second window fill", async () => await h.ev<boolean>(otherWindow.context, "document.getElementById('p').value === 'alice-secret'"));
  check("filling in a second browser window leaves the first untouched", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  await h.closeContext(otherWindow.context);
  // "Generate a strong password" (8.3b): offered in the picker for a form that asks for a new password; the password goes to
  // the asking page's new password boxes, never into the panel.
  const signup = await h.newTab(`${site}/signup`);
  const generator = await openPicker(signup, "np", /Generate a strong password/);
  await h.clickWindow(generator, /^Generate a strong password/);
  await h.waitFor("generated password", async () => await h.ev<boolean>(signup, "document.getElementById('np').value !== ''"));
  const generated = await h.ev<string>(signup, "document.getElementById('np').value");
  check("Generate fills both new password boxes, not the user name", generated.length >= 12 && (await h.ev<boolean>(signup, `document.getElementById('np2').value === ${JSON.stringify(generated)} && document.getElementById('u').value === ''`)), `${generated.length} characters`);
  check("the picker closes after Generate", await h.windowGone(generator));
  await h.closeContext(signup);
  await h.send("browsingContext.activate", { context: target });
  // A locked vault: Unlock from the picker. The app's Unlock takes the focus, which closes the panel; the dev host holds
  // the Unlock until the test answers (`hold-unlock`), and the harness closes the panel through its native hide path.
  const lockedPicker = async () => {
    const lockedLine = nextHostLine((l) => l === "LOCKED");
    h.hostCommand("lock");
    await lockedLine();
    await h.ev(target, "document.getElementById('u').blur()");
    const panel = await openPicker(target, "u", /locked/);
    const text = await h.windowText(panel);
    const asked = nextHostLine((l) => l === "UNLOCK?");
    await h.clickWindow(panel, "Unlock");
    await asked();
    await h.closeContext(panel);
    return { panel, text };
  };
  h.hostCommand("hold-unlock");
  let lockedRun = await lockedPicker();
  check("the toolbar picker reports a locked vault", /LocalPass is locked/.test(lockedRun.text), lockedRun.text);
  check("the panel closed while the app's Unlock is on screen", await h.windowGone(lockedRun.panel));
  await sleep(800);
  const duringUnlock = await toolbar();
  check("closing the panel during the app's Unlock cancels nothing (the tab's popup is still the picker)", duringUnlock.popup.includes("/decide.html?"), JSON.stringify(duringUnlock));
  h.hostCommand("unlock yes");
  picker = await h.decisionWindow();
  const reopened = await h.waitWindowText(picker, /Fill a login/);
  check("after Unlock the panel reopens by itself with the logins", picker.startsWith("panel:") && /Fill a login for localhost.*Local site/.test(reopened), reopened);
  check("unlocking alone never fills a login", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  await h.clickWindow(picker, /^Local site/);
  await h.waitFor("the fill after Unlock", async () => await h.ev<boolean>(target, "document.getElementById('p').value === 'alice-secret'"));
  check("a login picked in the reopened panel fills the page", true);
  check("the tab has its normal popup again after the pick", normal(await toolbarIs(normal)));
  await h.ev(target, "document.getElementById('u').value = '', document.getElementById('p').value = ''");
  lockedRun = await lockedPicker();
  h.hostCommand("unlock no");
  const refusedUnlock = await toolbarIs(normal);
  check("a refused Unlock ends the pick: the tab has its normal popup again", normal(refusedUnlock), JSON.stringify(refusedUnlock));
  await noWindow("a refused Unlock does not reopen the panel", 1500);
  check("a refused Unlock fills nothing", await h.ev<boolean>(target, "document.getElementById('p').value === ''"));
  h.hostCommand("auto-unlock");
  // Unlocked again (through the popup page), so the steps below see the vault as before.
  await h.ev(popup, "chrome.runtime.sendMessage({ kind: 'unlock' }).then(() => 'ok')");
  await h.closeContext(target);

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

  // 4. Save a new login, in the toolbar panel: "Not now" drops it, closing the panel (a click elsewhere) leaves it waiting
  // with a badge, the toolbar icon shows it again, Save stores it.
  const typeAndSubmit = async (user: string, pass: string, action = "") => {
    const tab = await h.newTab(`${site}/login`);
    await h.ev(tab, `${action ? `document.getElementById('login').action = ${JSON.stringify(action)}, ` : ""}document.getElementById('u').value = ${JSON.stringify(user)}, document.getElementById('p').value = ${JSON.stringify(pass)}, document.getElementById('login').requestSubmit()`);
    return tab;
  };
  await typeAndSubmit("bob", "bob-password");
  win = await h.decisionWindow();
  const offer = await h.waitWindowText(win, /Save this login/);
  check("the save offer is shown in the toolbar panel", win.startsWith("panel:"), win);
  check("no extra browser window opens for a save offer", (await browserWindows()) === 1);
  check("a new login is offered for saving, with the site the browser reported", /^localhost \|.*Save this login to LocalPass\?.*bob on localhost/.test(offer), offer);
  check("the save window never shows the password", !offer.includes("bob-password"));
  check("while the offer is open the toolbar icon has a badge", (await toolbar()).badge === "1");
  await h.clickWindow(win, "Not now");
  check("Not now closes the window", await h.windowGone(win));
  check("Not now saved nothing", !h.vault().some((i) => i.username === "bob"));
  check("Not now gives the tab its normal popup back, without a badge", normal(await toolbarIs(normal)));
  await sleep(1500);
  check("the same sign-in is not offered twice", (await h.find("decide.html")).length === 0);

  const bobTab = await typeAndSubmit("bob", "bob-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  // A click elsewhere closes the panel. BiDi clicks bypass Firefox's own popup handling, so the harness uses the
  // panel's native hide path, which is what that click ends in.
  await h.closeContext(win);
  check("closing the save panel closes it", await h.windowGone(win));
  const left = await toolbarIs(waiting);
  check("a closed save offer is not Not now: it waits with a badge, and the tab's popup is the offer", waiting(left), JSON.stringify(left));
  check("nothing saved after closing the panel", !h.vault().some((i) => i.username === "bob"));
  await h.send("browsingContext.navigate", { context: bobTab, url: `${site}/dashboard`, wait: "complete" });
  await noWindow("a dismissed save offer does not open again by itself on the next page", 2500);
  const afterNext = await toolbar();
  check("the waiting offer keeps its badge on the next page of the tab", waiting(afterNext), JSON.stringify(afterNext));
  check("the toolbar button can be clicked", (await h.clickToolbarButton()) === "clicked");
  win = await h.decisionWindow();
  const shownAgain = await h.waitWindowText(win, /Save this login/);
  check("a click on the toolbar icon shows the offer again", win.startsWith("panel:") && /Save this login to LocalPass\?.*bob on localhost/.test(shownAgain), shownAgain);
  // 8.3c: the user opened this panel (the background did not call openPopup for it), so Save acts at once: it is not held,
  // not even by the panel's own first focus.
  const freeAfter = await (async () => {
    const start = Date.now();
    while (Date.now() - start < 2000) {
      if ((await h.ev<string>(win, heldState("#main button.primary"))) === "false") return Date.now() - start;
      await sleep(25);
    }
    return -1;
  })();
  check("shown again by the user from the toolbar icon, Save is not held", freeAfter >= 0 && freeAfter < 300, `${freeAfter} ms`);
  await h.clickWindow(win, "Save");
  const saved = await h.waitWindowText(win, /Saved/, 8000);
  check("the window says it was saved", /Saved/.test(saved), saved);
  const items = await h.waitForVault((all) => all.some((i) => i.username === "bob"));
  const bob = items.find((i) => i.username === "bob");
  check("the login is in the vault with its password", bob?.password === "bob-password" && bob.urls.some((u) => u.startsWith(site)), JSON.stringify(bob?.urls));
  const before = Date.now();
  check("the window closes by itself after about two seconds", await h.windowGone(win, 6000), `${Date.now() - before} ms`);
  check("after the save the tab has its normal popup again, without a badge", normal(await toolbarIs(normal)));

  // Esc is "Not now".
  await typeAndSubmit("erin", "erin-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.pressKey(win, "Escape");
  check("Esc closes the save panel", await h.windowGone(win));
  const afterEsc = await toolbarIs(normal);
  check("Esc dismisses the offer: normal popup, no badge", normal(afterEsc), JSON.stringify(afterEsc));
  check("Esc saved nothing", !h.vault().some((i) => i.username === "erin"));

  // The offer survives the redirects after a sign-in, here to another address (another origin) of the site, and
  // shows the site of the sign-in.
  const hopTab = await typeAndSubmit("hank", "hank-password", "/session-hop");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.waitFor("the redirect to the other address", async () => (await h.ev<string>(hopTab, "location.href")).startsWith(`http://127.0.0.1:${server.port}/dashboard`), 15000);
  await sleep(1500);
  const panelsAfterHop = await h.find("decide.html");
  const hopToolbar = await toolbar();
  if (!panelsAfterHop.length) {
    check("after the redirect the offer waits on the toolbar of the tab", waiting(hopToolbar), JSON.stringify(hopToolbar));
    await h.clickToolbarButton();
  }
  win = await h.decisionWindow();
  const hopOffer = await h.waitWindowText(win, /hank/);
  check(
    `after a redirect to another origin the offer still shows the sign-in's site (panel ${panelsAfterHop.length ? "stayed open" : "was closed by the navigation"})`,
    /^localhost \|.*Save this login to LocalPass\?.*hank on localhost/.test(hopOffer),
    hopOffer,
  );
  await h.clickWindow(win, "Save");
  const hank = (await h.waitForVault((all) => all.some((i) => i.username === "hank"))).find((i) => i.username === "hank");
  check("Save after the redirect stores the login for the site of the sign-in", hank?.password === "hank-password" && !!hank?.urls.some((u) => u.startsWith(site)), JSON.stringify(hank?.urls));
  await h.windowGone(win, 6000);

  // A waiting offer outlives a restart of the background: the toolbar icon still shows it.
  await typeAndSubmit("ivan", "ivan-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  await h.closeContext(win);
  await toolbarIs(waiting);
  check("the background was stopped while an offer waited", (await h.terminateBackground()) === "stopped");
  await h.clickToolbarButton();
  win = await h.decisionWindow();
  const restarted = await h.waitWindowText(win, /ivan|LocalPass/);
  check("after a restart of the background the toolbar icon still shows the waiting offer", /Save this login to LocalPass\?.*ivan on localhost/.test(restarted), restarted);
  await h.clickWindow(win, "Not now");
  check("after the restart Not now gives the tab its normal popup back", normal(await toolbarIs(normal)));

  // Saving with a locked vault: the app's Unlock takes the focus and the panel closes; the save goes on, and is done after Unlock.
  const lockedForSave = nextHostLine((l) => l === "LOCKED");
  h.hostCommand("lock");
  await lockedForSave();
  h.hostCommand("hold-unlock");
  await typeAndSubmit("gina", "gina-password");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login/);
  const askedForSave = nextHostLine((l) => l === "UNLOCK?");
  await h.clickWindow(win, "Save");
  await askedForSave();
  await h.closeContext(win);
  await sleep(800);
  const duringSave = await toolbar();
  check("closing the panel while the save waits for the app's Unlock cancels nothing", duringSave.popup.includes("/decide.html?"), JSON.stringify(duringSave));
  h.hostCommand("unlock yes");
  const gina = (await h.waitForVault((all) => all.some((i) => i.username === "gina"))).find((i) => i.username === "gina");
  check("the save is done after Unlock although its panel closed", gina?.password === "gina-password");
  check("then the tab has its normal popup again, without a badge", normal(await toolbarIs(normal)));
  h.hostCommand("auto-unlock");

  // 4b. Only a new login or a changed password is offered (8.2). The app is asked (checkLogin) before the offer, but only
  // for a sign-in the user sent (a trusted click or Enter key: the capture's `byUser`, 8.6 F1). A page's own submit
  // (`requestSubmit()`, whose submit event is trusted too) is offered as a plain save without asking the app, so a page
  // cannot use the answer to test passwords. Here the user's sign-ins go through WebDriver BiDi `input.performActions`.
  /** Types a login into a new login tab and sends it as the user: a real click on Sign in, or Enter in the password box. */
  const typeAndSend = async (user: string, pass: string, how: "click" | "enter") => {
    const tab = await h.newTab(`${site}/login`);
    await h.ev(tab, `document.getElementById('u').value = ${JSON.stringify(user)}, document.getElementById('p').value = ${JSON.stringify(pass)}`);
    if (how === "click") await h.clickElement(tab, "#go");
    else {
      await h.ev(tab, "document.getElementById('p').focus()");
      await h.pressKey(tab, "Enter");
    }
    return tab;
  };
  /** The capture the background keeps for the active tab (session storage `pending:<tab>`), without its password. */
  const capturedHere = async (): Promise<{ username?: string; byUser?: unknown; update?: unknown } | null> =>
    JSON.parse(
      await h.ev<string>(
        popup,
        `chrome.tabs.query({ active: true, currentWindow: true }).then(async ([t]) => { const p = (await chrome.storage.session.get("pending:" + t.id))["pending:" + t.id]; return JSON.stringify(p ? { username: p.username, byUser: p.byUser, update: p.update } : null); })`,
      ),
    );

  // Measured first: Firefox marks a real Enter key in the password box as sent by the user (a new login, so it is offered).
  await typeAndSend("mia", "mia-password", "enter");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Save this login.*mia on localhost/);
  const miaCapture = await capturedHere();
  check("a real Enter key in the password box is a sign-in the user sent (byUser)", miaCapture?.username === "mia" && miaCapture.byUser === true, JSON.stringify(miaCapture));
  await h.clickWindow(win, "Not now");
  await h.windowGone(win);

  await typeAndSend("alice", "alice-secret", "click");
  await noWindow("a saved login typed by hand is not offered", 3000);
  await typeAndSend(" Alice", "alice-secret", "enter");
  await noWindow("a saved login typed with other case and spaces is not offered", 3000);

  // F1: the same sign-ins sent by a page script are not checked with the app: offered as a plain save, never dropped, never
  // "Update".
  await typeAndSubmit("alice", "alice-secret");
  win = await h.decisionWindow();
  const pageSaved = await h.waitWindowText(win, /Save this login|Update the saved/);
  const pageSavedCapture = await capturedHere();
  check("a saved login that a page script submitted is offered as a plain save (not checked with the app)", /Save this login to LocalPass\?.*alice on localhost/.test(pageSaved) && !/Update/.test(pageSaved), pageSaved);
  check("a page script's submit is not a sign-in the user sent", pageSavedCapture?.byUser === false && pageSavedCapture.update === undefined, JSON.stringify(pageSavedCapture));
  await h.clickWindow(win, "Not now");
  check("Not now closes that offer", await h.windowGone(win));
  await typeAndSubmit("bob", "bob-changed");
  win = await h.decisionWindow();
  const pageChanged = await h.waitWindowText(win, /Save this login|Update the saved/);
  check("a changed password that a page script submitted is offered as \"Save this login\", not as an update", /Save this login to LocalPass\?.*bob on localhost/.test(pageChanged) && !/Update/.test(pageChanged), pageChanged);
  await h.clickWindow(win, "Not now");
  check("Not now closes that offer too", await h.windowGone(win));
  check("the page-submitted offers saved nothing", h.vault().filter((i) => i.username === "alice").length === 1 && h.vault().some((i) => i.username === "bob" && i.password === "bob-password"));

  await typeAndSend("bob", "bob-changed", "click");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Update the saved password|Save this login/);
  const changedCapture = await capturedHere();
  check("a real click on Sign in is a sign-in the user sent, and the app's answer is kept (byUser, update)", changedCapture?.byUser === true && changedCapture.update === true, JSON.stringify(changedCapture));
  const update = await h.waitWindowText(win, /Update the saved password/);
  check("a changed password is offered as an update", /^localhost \|.*Update the saved password in LocalPass\?.*bob on localhost.*\| Update \|/.test(update), update);
  check("the update window never shows the password", !update.includes("bob-changed"));
  await h.clickWindow(win, "Update");
  const updated = await h.waitWindowText(win, /updated/, 8000);
  check("the window says the password was updated", /Password updated in LocalPass/.test(updated), updated);
  const afterUpdate = await h.waitForVault((all) => all.some((i) => i.username === "bob" && i.password === "bob-changed"));
  check("the saved login has the new password and no second login was made", afterUpdate.filter((i) => i.username === "bob").length === 1 && afterUpdate.some((i) => i.username === "bob" && i.password === "bob-changed"));
  await h.windowGone(win, 6000);

  // After the background stopped (Firefox suspends an idle event page; the app may also have been restarted), the
  // connection is gone. A page's own submit (requestSubmit, which fires a trusted submit event) must not reconnect, so that
  // sign-in cannot be checked and is offered as before; a real click on Sign in reconnects and the saved login is not offered.
  const stopped = await h.terminateBackground();
  check("the background page was stopped", stopped === "stopped", stopped);
  await typeAndSubmit("alice", "alice-secret");
  win = await h.decisionWindow();
  const unchecked = await h.waitWindowText(win, /Save this login/);
  check("after a restart, a sign-in the page itself submitted is not checked (offered as before; it may not connect)", /Save this login to LocalPass\?.*alice on localhost/.test(unchecked), unchecked);
  await h.clickWindow(win, "Not now");
  await h.windowGone(win);
  await h.terminateBackground();
  const real = await h.newTab(`${site}/login`);
  await h.ev(real, `document.getElementById('u').value = 'alice', document.getElementById('p').value = 'alice-secret'`);
  await h.clickElement(real, "#go");
  await h.waitFor("the dashboard", async () => (await h.ev<string>(real, "location.pathname")) === "/dashboard");
  await noWindow("after a restart, a saved login the user sent with a real click is checked and not offered", 3500);

  // A two-step sign-in filled by LocalPass on both steps is not offered.
  const twoStep = await h.newTab(`${site}/two-step`);
  picker = await openPicker(twoStep);
  await h.clickWindow(picker, /^Local site/);
  await h.waitFor("the account step fill", async () => (await h.ev<string>(twoStep, "document.getElementById('u').value")) === "alice");
  check("the picker closes after filling the account step", await h.windowGone(picker));
  await h.ev(twoStep, "document.getElementById('step1').requestSubmit()");
  await h.waitFor("the password step", async () => (await h.ev<string>(twoStep, "location.pathname")) === "/two-step/password" && (await h.ev<boolean>(twoStep, "!!document.getElementById('p')")));
  await sleep(500);
  picker = await openPicker(twoStep, "p");
  await h.clickWindow(picker, /^Local site/);
  await h.waitFor("the password step fill", async () => (await h.ev<string>(twoStep, "document.getElementById('p').value")) === "alice-secret");
  await h.windowGone(picker);
  await h.ev(twoStep, "document.getElementById('login').requestSubmit()");
  await noWindow("a two-step sign-in filled by LocalPass is not offered", 3000);
  // Typed by hand with a new account: offered, with the account name from the first step.
  await h.send("browsingContext.navigate", { context: twoStep, url: `${site}/two-step`, wait: "complete" });
  await h.ev(twoStep, "document.getElementById('u').value = 'frank@localhost', document.getElementById('step1').requestSubmit()");
  await h.waitFor("the password step", async () => (await h.ev<string>(twoStep, "location.pathname")) === "/two-step/password" && (await h.ev<boolean>(twoStep, "!!document.getElementById('p')")));
  await h.ev(twoStep, "document.getElementById('p').value = 'frank-password', document.getElementById('login').requestSubmit()");
  win = await h.decisionWindow();
  const twoStepOffer = await h.waitWindowText(win, /Save this login/);
  check("a new two-step sign-in is offered with the account name from its first step", /Save this login to LocalPass\?.*frank@localhost on localhost/.test(twoStepOffer), twoStepOffer);
  await h.clickWindow(win, "Not now");
  check("Not now closes it", await h.windowGone(win));
  check("nothing was saved for the two-step checks", !h.vault().some((i) => i.username === "frank@localhost") && h.vault().filter((i) => i.username === "alice").length === 1);

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
  if (plainBadge) {
    await h.trustedClick(plain!.context, plainBadge);
    const framePicker = await h.decisionWindow();
    await h.waitWindowText(framePicker, /Fill a login/);
    await h.clickWindow(framePicker, /^Local site/);
    await h.waitFor("same-origin frame fill", async () => await h.ev<boolean>(plain!.context, "document.getElementById('p').value === 'alice-secret'"));
    check("the toolbar picker fills the requesting same-origin frame", true);
  }
  const cross = await h.newTab(`${site}/crossframe`);
  const crossChild = (await h.contexts()).find(c => c.context === cross)?.children?.[0];
  await h.ev(crossChild!.context, "document.getElementById('u').focus()");
  await sleep(1000);
  check("cross-origin frames receive no badge or credential", !(await h.badgeRect(crossChild!.context)) && await h.ev<boolean>(crossChild!.context, "document.getElementById('p').value === ''"));
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
  check("creating a passkey is asked in the toolbar panel, with no extra browser window", win.startsWith("panel:") && (await browserWindows()) === 1, win);
  const heldCreate = await h.ev<string>(win, `(() => { window.dispatchEvent(new Event('focus')); return ${heldState("#main button.primary")}; })()`);
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
  check("signing in with a passkey is asked in the toolbar panel, with no extra browser window", win.startsWith("panel:") && (await browserWindows()) === 1, win);
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
  // Closing the panel (a click elsewhere; the native hide path here) leaves the request waiting; the toolbar icon shows it again.
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.closeContext(win);
  check("closing the passkey panel closes it", await h.windowGone(win));
  await sleep(1500);
  const pendingKey = await toolbar();
  check("a closed passkey panel leaves the request waiting (the page has no answer, badge on the tab)", (await h.ev<string>(p, "document.getElementById('status').textContent")) === "idle" && waiting(pendingKey), JSON.stringify(pendingKey));
  await h.clickToolbarButton();
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  // 8.3c: opened by the user, so Down and Enter act at once (no wait for a hold).
  await h.pressKey(win, "ArrowDown");
  const passkeyRowHeld = await h.ev<string>(win, heldState("#main button.row"));
  await h.pressKey(win, "Enter");
  check("shown again from the toolbar icon, the passkey row is not held and Down, Enter sign in", passkeyRowHeld === "false" && (await status(p, "signed-in")) === "signed-in", passkeyRowHeld);
  check("then the tab has its normal popup again, without a badge", normal(await toolbarIs(normal)));
  // Esc refuses as Cancel does.
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.pressKey(win, "Escape");
  const escaped = await status(p, /^error:/);
  check("Esc refuses the request (NotAllowedError)", escaped.startsWith("error:NotAllowedError"), escaped);
  // The page withdrawing (aborting) a waiting request ends it.
  p = await passkeyTab();
  await h.ev(p, `(async () => { const o = await (await fetch('/api/auth/options', { method: 'POST' })).json(); window.__ac = new AbortController(); window.__r = navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(o), signal: window.__ac.signal }).then(() => 'ok', (e) => e.name); return 'started'; })()`);
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.closeContext(win);
  await toolbarIs(waiting);
  await h.ev(p, "window.__ac.abort(), 'aborted'");
  const aborted = await h.ev<string>(p, "window.__r");
  const afterAbort = await toolbarIs((t) => normal(t) && !t.mine);
  check("a page that aborts its waiting request gets AbortError, and the request is gone from the toolbar", aborted === "AbortError" && normal(afterAbort) && !afterAbort.mine, `${aborted} ${JSON.stringify(afterAbort)}`);
  // A navigation of the tab ends a waiting request.
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  await h.closeContext(win);
  await toolbarIs(waiting);
  await h.send("browsingContext.navigate", { context: p, url: `${rp.origin}/?next`, wait: "complete" });
  const afterNav = await toolbarIs((t) => normal(t) && !t.mine);
  check("navigating the tab ends its waiting passkey request", normal(afterNav) && !afterNav.mine, JSON.stringify(afterNav));

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
  const openBefore = (await toolbar()).open;
  check("no request is left open before the crowd", openBefore === 0, String(openBefore));
  const tabs: string[] = [];
  for (let i = 0; i < 9; i++) {
    const t = await passkeyTab();
    tabs.push(t);
    await h.clickElement(t, "#signin");
    await sleep(400);
  }
  await sleep(1500);
  // One panel shows at a time; the requests of the tabs left behind wait (badge), and the ninth is refused.
  const crowd = (await toolbar()).open;
  const ninth = await status(tabs[8]!, /^error:/, 5000);
  check("no more than eight LocalPass requests are open at once (the ninth is refused)", crowd <= 8 && ninth.startsWith("error:NotAllowedError"), `${crowd} open, ninth: ${ninth}`);
  for (const w of await h.find("decide.html")) await h.closeContext(w.context);
  for (const t of tabs) await h.closeContext(t);
  const emptied = await toolbarIs((t) => t.open === 0);
  check("closing their tabs ends the waiting requests", emptied.open === 0, JSON.stringify(emptied));

  // 8b. Firefox below 149 (forced: the background is told an older version) keeps the separate windows, as before.
  const forced = await h.ev<string>(popup, `(async () => { const bg = await browser.runtime.getBackgroundPage(); bg.__lpRealInfo = bg.chrome.runtime.getBrowserInfo; bg.chrome.runtime.getBrowserInfo = async () => ({ version: "140.0" }); return (await bg.chrome.runtime.getBrowserInfo()).version; })()`);
  check("the background was told it runs in Firefox 140", forced === "140.0", forced);
  const oldLogin = await h.newTab(`${site}/login`);
  picker = await openPicker(oldLogin);
  check("Firefox below 149: choosing a login uses a separate window", !picker.startsWith("panel:") && (await browserWindows()) === 2, picker);
  await h.clickWindow(picker, /^Local site/);
  await h.waitFor("the fill from the window", async () => await h.ev<boolean>(oldLogin, "document.getElementById('p').value === 'alice-secret'"));
  check("Firefox below 149: a login picked in the window fills the page", await h.windowGone(picker));
  // (localhost is on the never-save list since step 5: this sign-in is on 127.0.0.1.)
  const kimTab = await h.newTab(`http://127.0.0.1:${server.port}/login`);
  await h.ev(kimTab, "document.getElementById('u').value = 'kim', document.getElementById('p').value = 'kim-password', document.getElementById('login').requestSubmit()");
  win = await h.decisionWindow();
  const oldOffer = await h.waitWindowText(win, /Save this login/);
  check("Firefox below 149: the save offer uses a separate window", !win.startsWith("panel:") && /kim on 127\.0\.0\.1/.test(oldOffer), oldOffer);
  await h.closeContext(win);
  check("Firefox below 149: closing the save window is Not now (nothing saved, normal popup)", (await h.windowGone(win)) && !h.vault().some((i) => i.username === "kim") && normal(await toolbarIs(normal)));
  p = await passkeyTab();
  await h.clickElement(p, "#signin");
  win = await h.decisionWindow();
  await h.waitWindowText(win, /Choose a passkey/);
  check("Firefox below 149: a passkey sign-in uses a separate window", !win.startsWith("panel:"), win);
  await h.closeContext(win);
  const oldClosed = await status(p, /^error:/);
  check("Firefox below 149: closing the passkey window refuses the request (NotAllowedError)", oldClosed.startsWith("error:NotAllowedError"), oldClosed);
  await h.ev<string>(popup, `browser.runtime.getBackgroundPage().then((bg) => { bg.chrome.runtime.getBrowserInfo = bg.__lpRealInfo; return "ok"; })`);

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
