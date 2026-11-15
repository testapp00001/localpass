// End-to-end test in a real browser:
//   cargo build -p lp-broker --examples && cd extension && bun run build --e2e && bun run e2e/run.ts
//
// Starts the dev host (a broker with a throwaway vault) and two local web sites, launches Chrome (or
// Edge: `BROWSER=edge`) with the extension in a fresh profile, and drives the real flow: connect,
// fill, save a new login, refuse other sites, locked vault. Every choice is made in LocalPass's decision
// panel (the toolbar popup, 8.3b); the only thing in a page is the field's badge, which opens the panel.
// Runs on Windows and Linux (headless: see `Panels` in harness.ts): how the browser finds the
// native-messaging helper for the run (a registry entry on Windows, a file in the throwaway profile on
// Linux) is in `harness.ts`, `BrowserSetup`.

import { type Browser, type Page } from "puppeteer-core";
import { BrowserSetup, EXTENSION_ID, Panels, devHostExe, inPageControls, panelClick, panelClosed, panelShows, panelText, toolbarBadge } from "./harness";

const setup = new BrowserSetup();

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

// ---- the web sites under test ----

const loginPage = (title: string) => `<!doctype html><title>${title}</title>
<form method="post" action="/session" id="login">
  <input name="username" autocomplete="username" placeholder="Username">
  <input name="password" type="password" placeholder="Password">
  <button type="submit">Sign in</button>
</form>`;

/** A two-step sign-in: the account name on one page, the password on the next (with no user name box). */
const accountStep = `<!doctype html><title>Account</title>
<form method="post" action="/two-step/password" id="step1">
  <input name="username" type="text" autocomplete="username" placeholder="Account">
  <button type="submit">Next</button>
</form>`;
const passwordStep = `<!doctype html><title>Password</title>
<form method="post" action="/session" id="login">
  <input name="password" type="password" autocomplete="current-password" placeholder="Password">
  <button type="submit">Sign in</button>
</form>`;
/** A sign-up form: a new password and its confirmation. */
const signupPage = `<!doctype html><title>Sign up</title>
<form method="post" action="/session" id="signup">
  <input name="username" autocomplete="username" placeholder="Username">
  <input name="password" type="password" autocomplete="new-password" placeholder="New password">
  <input name="confirm" type="password" autocomplete="new-password" placeholder="Repeat it">
  <button type="submit">Create account</button>
</form>`;

const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/login") return new Response(loginPage("Login"), { headers: { "content-type": "text/html" } });
    // Pages that try to collect clicks on the extension's interface (RA-1): one hides everything it can
    // reach with !important CSS, one paints a see-through decoy over it that ignores the mouse.
    if (url.pathname === "/attack-style") {
      const css = "<style>html > div { opacity: 0 !important; display: none !important; visibility: hidden !important; transform: scale(0) !important; }</style>";
      return new Response(loginPage("Hidden") + css, { headers: { "content-type": "text/html" } });
    }
    if (url.pathname === "/attack-cover") {
      // The decoy always sits after the extension's host element in the document, so it paints above it.
      const background = url.searchParams.get("bg") ?? "#fff";
      const script = `<script>const place = () => { if (window.stopDecoy) return;
        let d = document.getElementById("decoy");
        if (!d) { d = document.createElement("div"); d.id = "decoy";
          d.style.cssText = "position:fixed;inset:0;z-index:2147483647;pointer-events:none;background:${background}"; }
        if (document.documentElement.lastElementChild !== d) document.documentElement.append(d); };
        new MutationObserver(place).observe(document.documentElement, { childList: true });</script>`;
      return new Response(loginPage("Covered") + script, { headers: { "content-type": "text/html" } });
    }
    if (url.pathname === "/two-step") return new Response(accountStep, { headers: { "content-type": "text/html" } });
    if (url.pathname === "/two-step/password") return new Response(passwordStep, { headers: { "content-type": "text/html" } });
    if (url.pathname === "/signup") return new Response(signupPage, { headers: { "content-type": "text/html" } });
    if (url.pathname === "/session") return new Response(null, { status: 303, headers: { location: "/dashboard" } });
    if (url.pathname === "/dashboard") return new Response("<!doctype html><title>Dashboard</title><h1>Welcome</h1>", { headers: { "content-type": "text/html" } });
    return new Response("not found", { status: 404 });
  },
});
const port = server.port;

// ---- the dev host (a broker with a throwaway vault) ----

const host = Bun.spawn([devHostExe, "--login", `Local site|http://localhost:${port}|alice|alice-secret`], {
  env: setup.hostEnv(),
  stdin: "pipe",
  stdout: "pipe",
  stderr: "inherit",
});
const hostLines: string[] = [];
(async () => {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of host.stdout) {
    buffer += decoder.decode(chunk);
    let index;
    while ((index = buffer.indexOf("\n")) >= 0) {
      hostLines.push(buffer.slice(0, index).trim());
      buffer = buffer.slice(index + 1);
    }
  }
})();
async function waitForHost(predicate: (line: string) => boolean, timeoutMs = 15000): Promise<string> {
  const start = Date.now();
  let seen = 0;
  while (Date.now() - start < timeoutMs) {
    for (; seen < hostLines.length; seen++) if (predicate(hostLines[seen]!)) return hostLines[seen]!;
    await sleep(100);
  }
  throw new Error(`the dev host never printed the expected line; saw: ${hostLines.slice(-5).join(" | ")}`);
}
function hostCommand(command: string) {
  host.stdin.write(`${command}\n`);
  host.stdin.flush();
}
function vault(): { title: string; username: string; password: string; urls: string[] }[] {
  const last = [...hostLines].reverse().find((l) => l.startsWith("VAULT "));
  return last ? JSON.parse(last.slice(6)) : [];
}

// ---- the native-messaging registration, set for this run only ----

setup.registerHelper();

// ---- helpers for the pages ----

let panels!: Panels;

/** Focuses the field and clicks LocalPass's badge in it (a real click): that only asks for the panel. */
async function clickBadge(page: Page, field = 'input[name="username"]') {
  // Focus events only fire in the tab that is in front, and the panel opens only for the tab in front.
  await page.bringToFront();
  await page.focus(field);
  const badge = await page.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 });
  await badge!.click();
}

/** The badge's click opens the picker panel. */
async function openPicker(page: Page, field?: string): Promise<Page> {
  const next = panels.next();
  await clickBadge(page, field);
  return next;
}

async function value(page: Page, name: string): Promise<string> {
  return page.$eval(`input[name="${name}"]`, (el) => (el as HTMLInputElement).value);
}

let chrome: Browser | undefined;
try {
  await waitForHost((l) => l.startsWith("READY"));

  chrome = await setup.launch();
  panels = new Panels(chrome);

  // 1. Connect the browser to the app.
  const popup = await chrome.newPage();
  await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
  await popup.waitForFunction(() => document.body.innerText.includes("Connect to LocalPass"), { timeout: 20000 });
  check("the popup offers to connect an unknown browser", true);
  await panelClick(popup, "Connect"); // the popup in a tab is held for 0.6 s, like a panel the page caused (8.3c)
  const paired = await waitForHost((l) => l.startsWith("PAIRED"));
  check("the app was asked to approve the browser", /PAIRED .*extension code=\d{6}/.test(paired), paired);
  await popup.waitForFunction(() => !document.body.innerText.includes("Connect to LocalPass"), { timeout: 10000 });

  // 2. Fill a saved login on the site it belongs to: the badge opens the panel, the panel's row fills.
  const page = await chrome.newPage();
  await page.goto(`http://localhost:${port}/login`);
  await sleep(500);
  const picker = await openPicker(page);
  const pickerText = await panelShows(picker, /alice/);
  check("the panel lists the saved login for this site", pickerText.includes("Local site") && pickerText.includes("alice") && pickerText.includes(`Fill a login for localhost`), pickerText);
  check("the only LocalPass control in the page is the badge", JSON.stringify(await inPageControls(page)) === JSON.stringify(["button.badge"]), JSON.stringify(await inPageControls(page)));
  const held = await picker.evaluate(
    () =>
      [...document.querySelectorAll<HTMLButtonElement>("button.row")].every((b) => b.getAttribute("aria-disabled") === "true" && !b.disabled) &&
      !document.activeElement?.classList.contains("row"),
  );
  check("the panel's rows are held when it opens (not greyed out), and none has the focus", held);
  await panelClick(picker, "Local site");
  await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  check("the username is filled", (await value(page, "username")) === "alice");
  check("the password is filled", (await value(page, "password")) === "alice-secret");
  check("the panel closes after the pick", await panelClosed(picker));

  // 8.3c: the same pick with the keyboard. The badge's panel is the page's doing, so Enter waits for the hold; then
  // Down moves to the row and Enter fills.
  await page.$$eval("input", (inputs) => inputs.forEach((i) => ((i as HTMLInputElement).value = "")));
  const keys = await openPicker(page);
  await panelShows(keys, /alice/);
  await keys.keyboard.press("ArrowDown");
  const onRow = await keys.evaluate(() => document.activeElement?.classList.contains("row") === true);
  await keys.keyboard.press("Enter");
  const stillHeld = await keys.evaluate(() => document.querySelector("button.row")?.getAttribute("aria-disabled") === "true");
  await sleep(300);
  if (stillHeld) check("Enter on a row during the hold fills nothing", (await value(page, "password")) === "" && !keys.isClosed(), await value(page, "password"));
  else console.log("note: the hold had ended before Enter reached the panel; the Enter-during-hold check is skipped this run");
  check("Down moves the focus to the first row", onRow);
  await keys.waitForFunction(() => document.querySelector("button.row")?.getAttribute("aria-disabled") !== "true", { timeout: 3000 });
  await keys.keyboard.press("Enter");
  await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  check("Down then Enter in the panel fills the login", (await value(page, "username")) === "alice" && (await value(page, "password")) === "alice-secret");
  check("the panel closes after the keyboard pick", await panelClosed(keys));

  // 8.3c part 2: the normal toolbar popup.
  {
    const shortcut = await popup.evaluate(() => chrome.commands.getAll().then((all) => all.find((c) => c.name === "_execute_action")?.shortcut ?? "none"));
    check("the browser assigns the shortcut Alt+Shift+L", shortcut === "Alt+Shift+L", shortcut);
    const popupUrl = `chrome-extension://${EXTENSION_ID}/popup.html`;
    // The popup page in a tab is not a toolbar popup: the background never tells it that the user opened it, so it
    // focuses no Fill and holds for a moment; an error shows in the row, as text, never as an alert.
    let dialogs = 0;
    popup.on("dialog", (d) => void ((dialogs += 1), d.dismiss()));
    await popup.reload();
    // (Polling by time: a tab in the background runs no animation frames, puppeteer's default.)
    await popup.waitForFunction(() => [...document.querySelectorAll("button")].some((b) => b.textContent === "Fill"), { timeout: 8000, polling: 100 });
    const tabView = await popup.evaluate(() => ({ active: document.activeElement?.tagName ?? "", held: document.querySelector("ul.list button")?.getAttribute("aria-disabled") ?? "" }));
    check("the popup page in a tab focuses no Fill and is held at first", tabView.active !== "BUTTON" && tabView.held === "true", JSON.stringify(tabView));
    await page.goto("about:blank");
    await popup.waitForFunction(() => document.querySelector("ul.list button")?.getAttribute("aria-disabled") !== "true", { timeout: 5000, polling: 100 });
    await popup.evaluate(() => (document.querySelector("ul.list button") as HTMLButtonElement).click());
    const error = await popup
      .waitForFunction(() => document.querySelector("ul.list li .who .error")?.textContent ?? "", { timeout: 8000, polling: 100 })
      .then((h) => h.jsonValue() as Promise<string>, () => "");
    check("a failed fill is shown in its row, not as an alert", error !== "" && dialogs === 0, `${error} dialogs=${dialogs}`);
    await page.goto(`http://localhost:${port}/login`);
    await sleep(500);

    // Opened by the user (here `action.openPopup` called from the extension's own popup tab, which the background did
    // not call: what the background sees for a toolbar click or the shortcut, which headless Chrome cannot do), the
    // popup focuses the first Fill for the page, and Enter fills.
    await page.bringToFront();
    const own = popup.target();
    const next = chrome.waitForTarget((t) => t.url() === popupUrl && t !== own, { timeout: 10000 });
    const asked = await popup.evaluate(() => chrome.action.openPopup().then(() => "ok", (e: Error) => e.message));
    check("the normal popup opens in the toolbar (no decision is open)", asked === "ok", asked);
    const toolbar = (await (await next).asPage())!;
    await toolbar.waitForFunction(() => document.activeElement?.textContent === "Fill", { timeout: 8000 }).catch(() => {});
    const focus = await toolbar.evaluate(() => ({
      active: document.activeElement?.textContent ?? "",
      inOwnList: !!document.activeElement?.closest("ul.list") && document.querySelector("h2")?.textContent === "Logins for localhost",
      held: document.querySelectorAll("[aria-disabled]").length,
    }));
    check("a popup the user opened focuses the first Fill for this site, and nothing is held", focus.active === "Fill" && focus.inOwnList && focus.held === 0, JSON.stringify(focus));
    await toolbar.keyboard.press("Enter");
    await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
    check("Enter in the popup fills that login", (await value(page, "username")) === "alice" && (await value(page, "password")) === "alice-secret");
    check("the popup closes after filling", await panelClosed(toolbar));
  }

  // The stored broker secret is out of reach of content scripts (V26): ask from the content
  // script's own context (an isolated world named after the extension), and from the extension's
  // popup page as the control. Only key names are printed, never values.
  {
    const cdp = await page.createCDPSession();
    const contexts: { id: number; origin: string }[] = [];
    cdp.on("Runtime.executionContextCreated", (e) => contexts.push({ id: e.context.id, origin: e.context.origin }));
    await cdp.send("Runtime.enable");
    await sleep(300);
    const script = contexts.find((c) => c.origin === `chrome-extension://${EXTENSION_ID}`);
    check("the page's content-script context was found", !!script);
    const evaluate = async (expression: string) =>
      script ? String((await cdp.send("Runtime.evaluate", { contextId: script.id, awaitPromise: true, returnByValue: true, expression })).result.value) : "no context";
    const local = await evaluate(`chrome.storage.local.get(null).then((r) => "READ " + Object.keys(r).join(","), (e) => "DENIED " + e.message)`);
    check("a content script cannot read storage.local", local.startsWith("DENIED"), local);
    const own = await popup.evaluate(() => chrome.storage.local.get(null).then((r) => Object.keys(r).sort().join(",")));
    check("the extension's own page can (so the check above means something)", own.includes("secret") && own.includes("clientId"), own);
    // 8.3b: the content script's own messages give no password and no login names.
    const asked = await evaluate(`Promise.all([
        chrome.runtime.sendMessage({ kind: "fill", itemId: "x" }),
        chrome.runtime.sendMessage({ kind: "generate" }),
        chrome.runtime.sendMessage({ kind: "findLogins", interactive: true }),
      ]).then((r) => JSON.stringify(r))`);
    check("a content script's own fill or generate is refused, and findLogins names no login", /"code":"forbidden".*"code":"forbidden".*"hasLogins":true/.test(asked) && !asked.includes("alice") && !asked.includes("Local site"), asked);
    await cdp.detach();
  }

  // Signing in with the filled login does not offer to save it again.
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  check("no save offer after signing in with a filled login", !(await panels.opens(2500)) && (await toolbarBadge(chrome)) === "");

  // 3. A login typed by hand is offered for saving in the panel, and saved on request.
  await page.goto(`http://localhost:${port}/login`);
  await page.type('input[name="username"]', "bob");
  await page.type('input[name="password"]', "bob-secret");
  let offer = panels.next();
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  let savePanel = await offer;
  const saveText = await panelShows(savePanel, /Save this login/);
  check("a new login is offered for saving, in the panel", saveText.includes("Save this login to LocalPass") && saveText.includes("bob on localhost"), saveText);
  check("the save offer draws nothing into the page", (await inPageControls(page)).length === 0, JSON.stringify(await inPageControls(page)));
  check("a waiting save shows the badge on the toolbar icon", (await toolbarBadge(chrome)) === "1");
  await panelClick(savePanel, "Save");
  await waitForHost((l) => l.startsWith("VAULT ") && l.includes('"bob"'), 10000);
  const bob = vault().find((i) => i.username === "bob");
  check("the new login reached the vault", bob?.password === "bob-secret" && bob.urls.some((u) => u.startsWith(`http://localhost:${port}`)), JSON.stringify(bob));
  check("the panel says it was saved, then closes", /Saved to LocalPass/.test(await panelShows(savePanel, /Saved to LocalPass/)) && (await panelClosed(savePanel)));

  // 3b. Only a new login or a changed password is offered (8.2): the app is asked (checkLogin) before the panel opens.
  const typeAndSubmit = async (user: string, pass: string) => {
    await page.goto(`http://localhost:${port}/login`);
    await page.type('input[name="username"]', user);
    await page.type('input[name="password"]', pass);
    await page.click("button[type=submit]");
    await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  };
  await typeAndSubmit("alice", "alice-secret");
  check("a saved login typed by hand is not offered", !(await panels.opens(2500)));
  await typeAndSubmit(" Alice", "alice-secret");
  check("a saved login typed with other case and spaces is not offered", !(await panels.opens(2500)));
  offer = panels.next();
  await typeAndSubmit("bob", "bob-changed");
  savePanel = await offer;
  const updateText = await panelShows(savePanel, /Update the saved password/);
  check("a changed password is offered as an update", updateText.includes("Update the saved password in LocalPass?") && updateText.includes("bob") && !updateText.includes("Save this login"), updateText);
  await panelClick(savePanel, "Update");
  await waitForHost((l) => l.startsWith("VAULT ") && l.includes('"bob-changed"'), 10000);
  check("the saved login got the new password, no second login was made", vault().filter((i) => i.username === "bob").length === 1 && vault().some((i) => i.username === "bob" && i.password === "bob-changed"));
  await panelClosed(savePanel);

  // A two-step sign-in filled by LocalPass on both steps is not offered.
  await page.goto(`http://localhost:${port}/two-step`);
  await sleep(500);
  let step = await openPicker(page);
  await panelClick(step, "Local site");
  await page.waitForFunction(() => (document.querySelector('input[name="username"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  check("the account step is filled", (await value(page, "username")) === "alice");
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/two-step/password", { timeout: 8000 });
  await sleep(500);
  step = await openPicker(page, 'input[name="password"]');
  await panelClick(step, "Local site");
  await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  check("the password step is filled", (await value(page, "password")) === "alice-secret");
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  check("a two-step sign-in filled by LocalPass is not offered", !(await panels.opens(2500)));
  // Typed by hand with a new account: offered, with the account name from the first step.
  await page.goto(`http://localhost:${port}/two-step`);
  await page.type('input[name="username"]', "frank@localhost");
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/two-step/password", { timeout: 8000 });
  await page.type('input[name="password"]', "frank-password");
  offer = panels.next();
  await page.click("button[type=submit]");
  await page.waitForFunction(() => location.pathname === "/dashboard", { timeout: 8000 });
  savePanel = await offer;
  const frankText = await panelShows(savePanel, /frank@localhost/);
  check("a new two-step sign-in is offered with the account name from its first step", frankText.includes("Save this login to LocalPass?") && frankText.includes("frank@localhost"), frankText);
  await panelClick(savePanel, "Not now");
  check("Not now closes the panel and clears the toolbar badge", (await panelClosed(savePanel)) && (await sleep(300), (await toolbarBadge(chrome)) === ""));
  check("nothing was saved for the two-step checks", !vault().some((i) => i.username === "frank@localhost") && vault().filter((i) => i.username === "alice").length === 1);

  // "Generate a strong password" in the picker, for a form that asks for a new password (8.3b).
  await page.goto(`http://localhost:${port}/signup`);
  await sleep(500);
  const generator = await openPicker(page, 'input[name="password"]');
  const generatorText = await panelShows(generator, /Generate a strong password/);
  check("the picker offers to generate a password for a new-password form", generatorText.includes("Generate a strong password") && generatorText.includes("Local site"), generatorText);
  await panelClick(generator, "Generate a strong password");
  await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  const made = await value(page, "password");
  check("the generated password fills both new password boxes, and not the user name", made.length >= 12 && (await value(page, "confirm")) === made && (await value(page, "username")) === "", `${made.length} characters`);
  check("the generated password never reached the panel", !(await panelText(generator).catch(() => "")).includes(made));
  await page.goto(`http://localhost:${port}/login`);
  await sleep(500);
  const loginPicker = await openPicker(page);
  check("a sign-in form gets no Generate row", !(await panelShows(loginPicker, /Local site/)).includes("Generate"));
  await panelClick(loginPicker, "Cancel");
  await panelClosed(loginPicker);

  // 4. Another site never gets these logins.
  const other = await chrome.newPage();
  await other.goto(`http://127.0.0.1:${port}/login`);
  await sleep(500);
  const otherPanel = await openPicker(other);
  const otherText = await panelShows(otherPanel, /No saved logins/);
  check("another site sees no saved logins", otherText.includes("No saved logins for 127.0.0.1") && !otherText.includes("alice"), otherText);
  check("nothing was filled on the other site", (await value(other, "username")) === "");
  await panelClick(otherPanel, "Close");
  await panelClosed(otherPanel);

  // 5. A locked vault is unlocked from the panel.
  hostCommand("lock");
  await waitForHost((l) => l === "LOCKED");
  await page.goto(`http://localhost:${port}/login`);
  await sleep(500);
  const lockedPanel = await openPicker(page);
  check("a locked vault is reported", (await panelShows(lockedPanel, /locked/)).includes("LocalPass is locked."));
  await panelClick(lockedPanel, "Unlock");
  const afterUnlock = await panelShows(lockedPanel, /Local site/);
  check("after unlocking the logins appear", afterUnlock.includes("Local site"), afterUnlock);
  await panelClick(lockedPanel, "Local site");
  await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
  check("and the pick fills", (await value(page, "password")) === "alice-secret");

  // 5b. The panel is still open when the Unlock worked and closes only afterwards (Chrome on Wayland keeps it open behind
  // the app and closes it when its window is in front again, 8.4): it opens again by itself, held, and the pick fills.
  // Closing the panel's page here stands in for Chrome closing it.
  for (const how of ["closed by the browser", "Esc"] as const) {
    hostCommand("lock");
    await waitForHost((l) => l === "LOCKED");
    await page.goto(`http://localhost:${port}/login`);
    await sleep(500);
    const first = await openPicker(page);
    await panelShows(first, /locked/);
    await panelClick(first, "Unlock");
    await panelShows(first, /Local site/);
    const next = panels.next();
    next.catch(() => {});
    if (how === "Esc") {
      await first.keyboard.press("Escape");
      check("Esc after the Unlock closes the panel", await panelClosed(first));
      check("and it does not open again", !(await panels.opens(2500)));
      continue;
    }
    await first.close();
    const again = await next.catch(() => null);
    check("a panel closed after the Unlock worked opens again by itself", again !== null);
    if (!again) continue;
    const shownAgain = await panelShows(again, /Local site/);
    const held = await again.evaluate(() => [...document.querySelectorAll("button.row")].every((b) => b.getAttribute("aria-disabled") === "true"));
    check("it shows the logins, its rows held at first (the background opened it)", shownAgain.includes("Local site") && held, shownAgain);
    await panelClick(again, "Local site");
    await page.waitForFunction(() => (document.querySelector('input[name="password"]') as HTMLInputElement).value !== "", { timeout: 8000 }).catch(() => {});
    check("and its pick fills", (await value(page, "password")) === "alice-secret");
    await panelClosed(again);
  }

  // 6. A page that hides, covers or restyles the badge can at most get the panel opened (RA-1, 8.3b).
  {
    const styled = await chrome.newPage();
    await styled.goto(`http://localhost:${port}/attack-style`);
    await sleep(500);
    await styled.bringToFront();
    await styled.focus('input[name="username"]');
    const badge = await styled.waitForSelector("pierce/.badge[data-ready]", { timeout: 8000 }).catch(() => null);
    check("page CSS with !important cannot hide the badge", badge !== null);
    const intact = await badge?.evaluate((el) => {
      const s = getComputedStyle((el.getRootNode() as ShadowRoot).host as HTMLElement);
      return s.display !== "none" && s.visibility === "visible" && s.opacity === "1" && s.transform === "none";
    });
    check("the badge keeps its own display, visibility, opacity and transform", intact === true);
    const next = panels.next();
    await badge!.click();
    const styledPanel = await next;
    check("its click only opens the panel, which page CSS cannot reach, and nothing is filled", (await panelShows(styledPanel, /Local site/)).includes("Local site") && (await value(styled, "password")) === "");
    await panelClick(styledPanel, "Cancel");
    await panelClosed(styledPanel);
    await styled.close();

    for (const [name, background] of [
      ["an opaque decoy", "#fff"],
      ["a 99% opaque decoy", "rgba(255,255,255,.99)"],
      ["a 95% opaque decoy", "rgba(255,255,255,.95)"],
    ] as const) {
      const covered = await chrome.newPage();
      await covered.goto(`http://localhost:${port}/attack-cover?bg=${encodeURIComponent(background)}`);
      await sleep(500);
      await covered.bringToFront();
      await covered.focus('input[name="username"]');
      const shown = await covered.waitForSelector("pierce/.badge", { timeout: 8000 });
      const next = panels.next();
      await shown!.click(); // the decoy ignores the mouse, so this reaches the badge underneath
      const coveredPanel = await next.catch(() => null);
      await sleep(800); // longer than the panel's hold on its rows
      check(`a click through ${name} at most opens the panel, and fills nothing`, (await value(covered, "username")) === "" && (await value(covered, "password")) === "", coveredPanel ? "panel opened" : "no panel");
      check(`with ${name}, the page still has no LocalPass control but the badge`, JSON.stringify(await inPageControls(covered)) === JSON.stringify(["button.badge"]));
      if (coveredPanel) {
        await panelClick(coveredPanel, "Cancel");
        await panelClosed(coveredPanel);
      }
      await covered.close();
    }
  }
} catch (e) {
  console.error(e);
  failures++;
} finally {
  await Promise.race([chrome?.close().catch(() => {}), sleep(8000)]);
  chrome?.process()?.kill();
  hostCommand("quit");
  host.kill();
  server.stop(true);
  const touched = setup.cleanup();
  check("the real browser config was not touched", touched === null, touched ?? "");
}

if (failures) {
  console.error(`${failures} check(s) failed`);
  process.exit(1);
}
console.log("all end-to-end checks passed");
