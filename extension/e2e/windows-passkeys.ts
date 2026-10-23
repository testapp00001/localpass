// The Settings > Windows passkeys section in the real app, against a THROWAWAY provider package, so the
// real registration (LocalPass.Passkeys) is never touched: Set up -> status -> Remove through the UI.
//
//   cargo build -p localpass --features custom-protocol                       (own CARGO_TARGET_DIR is fine)
//   cargo build --release -p lp-winauth --bin lp-winauth-provider --features test-identity   (own target dir)
//   set LP_APP_EXE=<...>\debug\localpass.exe  set LP_TEST_PROVIDER_EXE=<...>\release\lp-winauth-provider.exe
//   cd extension && bun run e2e/windows-passkeys.ts
//
// Needs Windows 11 with the plug-in API and Developer Mode. The app is copied to a temp folder with a test
// bundle next to it (the shipped manifest with the test identity, exe under another name, so the execution
// alias of the real provider is never shared); the app runs with its own profile, WebView2 folder and the
// debug-only LOCALPASS_PROVIDER_* variables. Clicking "Turn on in Windows settings" is not tested (it would
// open Windows Settings on your screen) and nothing can enable the provider without a Windows Hello gesture.
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../..");
const appExeSource = process.env.LP_APP_EXE ?? join(root, "target/debug/localpass.exe");
const testProviderExe = process.env.LP_TEST_PROVIDER_EXE;
if (!testProviderExe || !existsSync(testProviderExe)) throw new Error("set LP_TEST_PROVIDER_EXE to a provider built with --features test-identity");
if (!existsSync(appExeSource)) throw new Error(`no app at ${appExeSource}: build it with --features custom-protocol`);

const PACKAGE = "LocalPass.PasskeysTest";
const EXE = "lp-winauth-provider-test.exe";
const REGKEY = "Software\\LocalPass\\ProviderTest";
const DATA = "provider-test";
const alias = join(process.env.LOCALAPPDATA!, "Microsoft", "WindowsApps", EXE);

let failures = 0;
function check(name: string, ok: boolean, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
  if (!ok) failures++;
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const ps = (script: string) => Bun.spawnSync(["powershell", "-NoProfile", "-NonInteractive", "-Command", script], { stdout: "pipe", stderr: "pipe" }).stdout.toString().trim();
const packageLocation = () => ps(`(Get-AppxPackage -Name ${PACKAGE}).InstallLocation`);
const sid = Bun.spawnSync(["whoami", "/user", "/fo", "csv", "/nh"], { stdout: "pipe" }).stdout.toString().trim().split(",")[1].replace(/"/g, "");
const pluginNames = () => Bun.spawnSync(["reg", "query", `HKLM\\SOFTWARE\\Microsoft\\FIDO\\${sid}\\Plugins`, "/s", "/v", "Name"], { stdout: "pipe", stderr: "pipe" }).stdout.toString();
const appPathValue = () => Bun.spawnSync(["reg", "query", `HKCU\\${REGKEY}`, "/v", "AppPath"], { stdout: "pipe", stderr: "pipe" });

if (packageLocation()) throw new Error(`${PACKAGE} is already registered: remove it first`);

// ---- the app copy with a test bundle next to it ----

const workDir = mkdtempSync(join(tmpdir(), "lp-wp-e2e-"));
const appDir = join(workDir, "app");
const bundle = join(appDir, "windows-passkeys");
mkdirSync(join(bundle, "Assets"), { recursive: true });
copyFileSync(appExeSource, join(appDir, "localpass.exe"));
copyFileSync(testProviderExe, join(bundle, EXE));
for (const logo of ["StoreLogo", "Square44x44Logo", "Square150x150Logo"]) copyFileSync(join(root, "app/src-tauri/icons", `${logo}.png`), join(bundle, "Assets", `${logo}.png`));
const clsid = Bun.spawnSync([testProviderExe, "clsid"], { stdout: "pipe" }).stdout.toString().trim().replace(/[{}]/g, "");
if (clsid.toUpperCase() === "91E9966B-58AE-455A-AB60-4B2FDB66A730") throw new Error("that provider has the real class id");
const manifest = readFileSync(join(root, "crates/lp-winauth/package/AppxManifest.xml"), "utf8")
  .replaceAll("91E9966B-58AE-455A-AB60-4B2FDB66A730", clsid)
  .replace('Name="LocalPass.Passkeys"', `Name="${PACKAGE}"`)
  .replaceAll("lp-winauth-provider.exe", EXE)
  .replace('DisplayName="LocalPass"', 'DisplayName="LocalPass (test)"');
writeFileSync(join(bundle, "AppxManifest.xml"), manifest);

const profile = `e2ewp${process.pid}`;
const debugPort = 9400 + (process.pid % 60);
const app = Bun.spawn([join(appDir, "localpass.exe"), "--minimized"], {
  env: {
    ...process.env,
    LOCALPASS_PROFILE: profile,
    LOCALPASS_PROVIDER_PACKAGE: PACKAGE,
    LOCALPASS_PROVIDER_EXE: EXE,
    LOCALPASS_PROVIDER_REGKEY: REGKEY,
    LOCALPASS_PROVIDER_DATA: DATA,
    WEBVIEW2_USER_DATA_FOLDER: join(workDir, "webview"),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${debugPort}`,
  },
  stdout: "ignore",
  stderr: "ignore",
});

async function connectToApp() {
  let target: any;
  for (let i = 0; i < 100 && !target; i++) {
    try {
      const targets: any[] = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
      target = targets.find((t) => t.type === "page");
    } catch {}
    if (!target) await sleep(200);
  }
  if (!target) throw new Error("the app's web view never appeared");
  const ws = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise<void>((res, rej) => {
    ws.onopen = () => res();
    ws.onerror = rej;
  });
  let id = 0;
  const pending = new Map<number, (m: any) => void>();
  ws.onmessage = (ev) => {
    const m = JSON.parse(String(ev.data));
    pending.get(m.id)?.(m);
  };
  return {
    async eval<T>(expression: string): Promise<T> {
      const my = ++id;
      const reply = await new Promise<any>((resolve) => {
        pending.set(my, resolve);
        ws.send(JSON.stringify({ id: my, method: "Runtime.evaluate", params: { expression, awaitPromise: true, returnByValue: true } }));
      });
      if (reply.result?.exceptionDetails) throw new Error(JSON.stringify(reply.result.exceptionDetails.exception?.description ?? reply.result.exceptionDetails));
      return reply.result.result.value as T;
    },
    close: () => ws.close(),
  };
}
type Ui = Awaited<ReturnType<typeof connectToApp>>;

/** The text of the Windows passkeys section (empty when it is not drawn). */
const section = (ui: Ui) =>
  ui.eval<string>(`(() => { const s = [...document.querySelectorAll('section')].find(x => x.querySelector('h2')?.textContent.trim().toLowerCase() === 'windows passkeys'); return s ? s.innerText.replace(/\\s+/g, ' ') : ''; })()`);
async function waitForSection(ui: Ui, needle: string, timeoutMs = 60000) {
  const start = Date.now();
  let last = "";
  while (Date.now() - start < timeoutMs) {
    last = await section(ui);
    if (last.includes(needle)) return last;
    await sleep(300);
  }
  throw new Error(`the section never showed "${needle}"; it shows: ${last.slice(0, 400)}`);
}
/** Clicks a button inside the Windows passkeys section, or inside the open dialog when `dialog` is set. */
const click = (ui: Ui, label: string, dialog = false) =>
  ui.eval(`(() => { const scope = ${dialog} ? document.querySelector('[role=dialog]') : [...document.querySelectorAll('section')].find(x => x.querySelector('h2')?.textContent.trim().toLowerCase() === 'windows passkeys'); if (!scope) throw new Error('no scope'); const b = [...scope.querySelectorAll('button')].find(x => x.textContent.trim() === ${JSON.stringify(label)}); if (!b) throw new Error('no button ${label}'); b.click(); return true; })()`);
const buttons = (ui: Ui) =>
  ui.eval<string[]>(`(() => { const s = [...document.querySelectorAll('section')].find(x => x.querySelector('h2')?.textContent.trim().toLowerCase() === 'windows passkeys'); return s ? [...s.querySelectorAll('button')].map(b => b.textContent.trim() + (b.disabled ? ' (disabled)' : '')) : []; })()`);

let ui: Ui | undefined;
try {
  ui = await connectToApp();
  const invoke = (command: string, args: object) => ui!.eval(`window.__TAURI_INTERNALS__.invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})`);
  for (let i = 0; i < 100 && !(await ui.eval<string>("document.body.innerText")).includes("Create a new vault"); i++) await sleep(200);
  await invoke("create_vault", { password: "end to end master password", deviceName: "E2E PC" });
  await sleep(500);
  await ui.eval(`(() => { const b = [...document.querySelectorAll('button')].filter(x => x.offsetParent !== null).find(x => x.textContent.trim().includes('Settings')); b.click(); return true; })()`);

  const first = await waitForSection(ui, "Added to Windows");
  check("Settings shows the Windows passkeys section", true);
  check("nothing is set up yet", first.includes("Not set up yet"), first.slice(0, 200));
  check("the next step is Set up", (await buttons(ui)).includes("Set up"), JSON.stringify(await buttons(ui)));
  check("the explanation mentions the extension switch", first.includes("passkey switch"));

  await click(ui, "Set up");
  const after = await waitForSection(ui, "Turn on in Windows settings");
  check("Set up added the package from the app's own folder", packageLocation().toLowerCase() === bundle.toLowerCase(), packageLocation());
  check("LocalPass (test) is in Windows' list of passkey providers", pluginNames().includes("LocalPass (test)"));
  check("the app path was written for the provider", appPathValue().exitCode === 0 && appPathValue().stdout.toString().toLowerCase().includes("localpass.exe"));
  check("the section says it is added but not turned on", after.includes("LocalPass is in Windows' list of passkey providers") && after.includes("Windows keeps it off until you turn it on yourself"), after.slice(0, 300));
  const afterButtons = await buttons(ui);
  check("the next step is Turn on in Windows settings, with Remove offered", afterButtons.some((b) => b.includes("Turn on in Windows settings")) && afterButtons.includes("Remove"), JSON.stringify(afterButtons));
  check("the provider alias exists", existsSync(alias) || ps(`Test-Path '${alias}'`) === "True");

  // Remove through the dialog.
  await click(ui, "Remove");
  await sleep(300);
  const dialog = await ui.eval<string>("document.querySelector('[role=dialog]')?.innerText ?? ''");
  check("Remove asks first and explains", dialog.includes("Remove Windows passkeys?") && dialog.includes("stay in your vault"), dialog.slice(0, 120));
  await click(ui, "Remove", true);
  const removed = await waitForSection(ui, "Not set up yet");
  check("after Remove nothing is set up", removed.includes("Not set up yet") && (await buttons(ui)).includes("Set up") && !(await buttons(ui)).includes("Remove"));
  check("the package is gone", packageLocation() === "");
  check("LocalPass (test) left Windows' list", !pluginNames().includes("LocalPass (test)"));
  check("the app path value is gone", appPathValue().exitCode !== 0);
  check("the alias is gone", ps(`Test-Path '${alias}'`) === "False");
} catch (e) {
  failures++;
  console.log(`FAIL ${(e as Error).message}`);
} finally {
  ui?.close();
  app.kill();
  await sleep(500);
  // Whatever happened, the throwaway package must not stay.
  if (packageLocation()) {
    console.log("cleaning up the throwaway package");
    if (existsSync(alias)) Bun.spawnSync([alias, "unregister"], { stdout: "pipe", stderr: "pipe" });
    ps(`Get-Process -Name ${EXE.replace(".exe", "")} -ErrorAction SilentlyContinue | Stop-Process -Force; Get-AppxPackage -Name ${PACKAGE} | Remove-AppxPackage`);
  }
  Bun.spawnSync(["reg", "delete", `HKCU\\${REGKEY}`, "/f"], { stdout: "pipe", stderr: "pipe" });
  rmSync(join(process.env.LOCALAPPDATA!, "LocalPass", DATA), { recursive: true, force: true });
  rmSync(join(process.env.APPDATA!, "app.localpass", "profiles", profile), { recursive: true, force: true });
  rmSync(workDir, { recursive: true, force: true });
}
console.log(failures ? `${failures} check(s) failed` : "all checks passed");
process.exit(failures ? 1 : 0);
