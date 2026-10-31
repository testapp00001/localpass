// ASSISTED Firefox check against the REAL desktop app (Linux). A person has to be at the screen.
//
//   1. cd app && LOCALPASS_PROFILE=dev2 bun run tauri dev        (any throwaway profile; its vault must be unlocked or lockable
//      by you; the app uses your real keyring)
//   2. cd extension && bun run build --firefox --e2e && bun run e2e/firefox/assisted.ts [profile] [path to the localpass binary]
//
// A visible Firefox (throwaway profile and HOME; your real ~/.mozilla is not touched) gets the extension, and its native-messaging
// helper is the app's own binary. The script walks you through what only you can judge and prints what the app did:
//   A. pairing: approve the 6-digit code in the app;
//   B. "Show all my logins" with the LocalPass window HIDDEN (closed to the tray): the app must raise "Confirm it is you" (or, on
//      Wayland, GNOME may only show a "LocalPass is ready" notification: note which), and the reply says accepted or refused;
//   C. the same with the window open.
// Every step waits for you (up to 3 minutes). Nothing here is checked automatically; read the printed verdicts.

import { join, resolve } from "node:path";
import { FirefoxHarness, extensionDir, sleep } from "./harness";

const profile = process.argv[2] ?? "dev2";
const appExe = resolve(process.argv[3] ?? join(import.meta.dir, "../../../target/debug/localpass"));
const runtime = process.env.XDG_RUNTIME_DIR ?? "";
const socket = join(runtime, "localpass", profile === "default" ? "broker-v1.sock" : `broker-v1-${profile}.sock`);

const say = (text: string) => console.log(`\n>>> ${text}`);

if (!(await Bun.file(appExe).exists())) throw new Error(`no app binary at ${appExe} (cargo build -p localpass, or pass the path)`);
// A socket file can outlive the app (a stale one): only a connection that is accepted proves it is running.
const listening = await Bun.connect({ unix: socket, socket: { data() {}, open(s) { s.end(); }, error() {} } }).then(() => true, () => false);
if (!listening) throw new Error(`the app is not listening on ${socket}; start it with LOCALPASS_PROFILE=${profile} first`);

const h = new FirefoxHarness({ appExe, profile });
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: () => new Response(`<!doctype html><title>Assisted</title><form><input id="u" autocomplete="username" style="width:300px;height:30px"><input type="password"></form>`, { headers: { "content-type": "text/html" } }),
});

try {
  h.registerHelper(appExe);
  await h.startFirefox(extensionDir);
  const tab = await h.newTab(`http://localhost:${server.port}/`);
  await h.ev(tab, "document.getElementById('u').focus()");
  const badge = await h.waitFor("the badge", async () => (await h.badgeRect(tab)) ?? false);
  await h.trustedClick(tab, badge);
  const win = await h.decisionWindow();
  await h.ev(win, `chrome.tabs.create({ url: chrome.runtime.getURL("popup.html") }).then(() => "ok")`);
  const popup = (await h.waitFor("the popup tab", async () => (await h.find("popup.html"))[0])).context;
  const popupText = () => h.ev<string>(popup, "document.body.innerText");

  if ((await popupText()).includes("Connect to LocalPass")) {
    say("A. Click-through pairing: approve the browser in the LocalPass app when it asks (compare the 6-digit code with the one the app shows).");
    await h.ev(popup, `[...document.querySelectorAll("button")].find((b) => b.textContent === "Connect").click()`);
    await h.waitFor("pairing", async () => !(await popupText()).includes("Connect to LocalPass"), 180_000);
    console.log("paired.");
  } else console.log("(already paired in this throwaway profile)");

  // Each trial asks the background for the whole login list, the request "Show all my logins" makes (the app must confirm that it
  // is you), and judges the reply, not the page (the button is replaced by the list after the first answer).
  const trial = async (label: string) => {
    say(label);
    await sleep(8000);
    const started = Date.now();
    console.log("requested: look at the screen now. Where did the confirmation appear, and did it take the focus?");
    const reply = await h.ev<string>(popup, `chrome.runtime.sendMessage({ kind: "loadLogins" }).then((r) => JSON.stringify(r.ok ? { ok: true, logins: r.data.length } : { ok: false, code: r.code }))`);
    console.log(`result after ${Math.round((Date.now() - started) / 1000)} s: ${reply}   (ok = confirmed, code "denied" = refused or cancelled)`);
  };
  await trial("B. CLOSE the LocalPass window to the tray now (x). In 8 s the script asks the app to list all logins, which needs 'Confirm it is you'. Enter the master password (or Cancel).");
  await trial("C. OPEN the LocalPass window now. In 8 s the same request runs with the window open. Enter the master password (or Cancel).");
  say("Done. This run leaves a paired client (\"Firefox extension\") in the app's profile: forget it in Settings > Browser extension. Write what you saw (dialog raised or only a notification, focus, wrong password refused, Cancel) into docs/handoff-phase6-linux.md.");
} catch (e) {
  console.error(e);
  h.failures++;
} finally {
  server.stop(true);
  await h.stop();
}

h.finish("assisted run finished");
