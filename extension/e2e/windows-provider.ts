// The Windows system passkey provider (crates/lp-winauth) checked by an independent relying party
// (SimpleWebAuthn, e2e/rp.ts). ASSISTED: a person has to do the parts Windows reserves for them.
//
//   cd extension && bun run e2e/windows-provider.ts [flags]
//
//   --cancel        also: cancel in the Windows Hello prompt (NotAllowedError, no second prompt)
//   --locked        also: sign in with the vault locked (exactly one Hello)
//   --app-closed    also: sign in with LocalPass closed (the provider starts it)
//   --all           the three above
//   --browser edge|chrome   (default: edge, or the BROWSER variable)
//   --port N        relying party port (default 8473, a free one if taken)
//   --timeout S     seconds to wait for each human step (default 240)
//   --user NAME     user name of the test passkey (default lp-test-<date>)
//   --virtual       self-test of this runner and the relying party with a CDP virtual authenticator
//                   (headless browser, no Windows, no provider, nobody needed)
//
// What it does: starts the relying party on 127.0.0.1 (rpId "localhost"), starts the browser with a
// THROWAWAY profile (no LocalPass extension, so Windows' own passkey UI is used), tells you what to
// click, and watches (a) the relying party's verdicts (/state), (b) the provider log
// (%LOCALAPPDATA%\LocalPass\provider\provider.log, only lines added during the run), (c) the app's
// prompts.log (Windows Hello events) and (d) `lp-winauth-provider status` (read-only) before and after.
// It ends with a PASS/FAIL table. It never touches the registration, the pairing or any process it
// did not start (LocalPass is never closed for you).
//
// The test passkey is created in your REAL vault (user name lp-test-...) and syncs like any item:
// delete it in the app afterwards (Items, kind Passkey, site "localhost").
//
// Test hooks (not needed in normal use): LP_VIRTUAL_NO_BACKUP=1 and LP_VIRTUAL_IDLE=1 make the
// --virtual self-test fail on purpose (flags without BE/BS; nobody answers), LP_BROWSER_ARGS adds
// browser arguments (for example --headless=new to check start, time-out and clean-up unattended).

import { spawn, spawnSync } from "node:child_process";
import { closeSync, existsSync, fstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import puppeteer, { type Browser, type CDPSession, type Page } from "puppeteer-core";
import { browserPaths, sleep } from "./harness";
import { analysePrompts, analyseProvider, failureCount, formatTable, judgePhase, parseProviderStatus, seconds, type PhaseSpec, type ProviderStatus, type Row, type Status, type WaitResult } from "./provider-analysis";
import { DEFAULT_ASSISTED_PORT, startRelyingParty, type RpEvent } from "./rp";

// ---------------------------------------------------------------------------------------------
// Arguments

const startedAt = Date.now();
const argv = process.argv.slice(2);
const flag = (name: string) => argv.includes(name);
const value = (name: string) => {
  const i = argv.indexOf(name);
  return i >= 0 ? argv[i + 1] : undefined;
};
if (flag("--help") || flag("-h")) {
  const text = readFileSync(import.meta.path, "utf8").split("\n");
  console.log(text.slice(0, text.findIndex((l, i) => i > 0 && !l.startsWith("//"))).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(0);
}

const browserName = ((value("--browser") ?? process.env.BROWSER ?? "edge").toLowerCase() === "chrome" ? "chrome" : "edge") as "chrome" | "edge";
const browserExe = browserPaths[browserName];
const virtual = flag("--virtual");
const all = flag("--all");
const want = { cancel: all || flag("--cancel"), locked: all || flag("--locked"), closed: all || flag("--app-closed") };
const timeoutS = Number(value("--timeout") ?? 240);
const stamp = new Date().toISOString().replace(/[-:T]/g, "").slice(0, 12);
const userName = value("--user") ?? `lp-test-${stamp}`;
const root = resolve(import.meta.dir, "../..");
const outDir = join(root, "target", "windows-provider-test", new Date().toISOString().replace(/[:.]/g, "-"));
mkdirSync(outDir, { recursive: true });

const localAppData = process.env.LOCALAPPDATA ?? "";
const appData = process.env.APPDATA ?? "";
const alias = join(localAppData, "Microsoft", "WindowsApps", "lp-winauth-provider.exe");
const providerLogPath = join(localAppData, "LocalPass", "provider", "provider.log");
const sheetScript = join(root, "tools", "windows-provider", "passkey-sheet.ps1");

// ---------------------------------------------------------------------------------------------
// Results

const rows: Row[] = [];
function pushRow(row: Row) {
  rows.push(row);
  console.log(`   ${row.status.padEnd(4)} ${row.name}${row.detail ? ` -- ${row.detail}` : ""}`);
}
const add = (status: Status, name: string, detail = "") => pushRow({ status, name, detail });

// ---------------------------------------------------------------------------------------------
// Logs: only what is appended after the start

class LogTail {
  private offset: number;
  constructor(readonly path: string) {
    this.offset = this.size();
  }
  private size(): number {
    try {
      const fd = openSync(this.path, "r");
      try {
        return fstatSync(fd).size;
      } finally {
        closeSync(fd);
      }
    } catch {
      return 0;
    }
  }
  /** The text added since the last call (the file may have been rotated: start again from 0). */
  take(): string {
    let fd: number;
    try {
      fd = openSync(this.path, "r");
    } catch {
      return "";
    }
    try {
      const size = fstatSync(fd).size;
      if (size < this.offset) this.offset = 0;
      const buffer = Buffer.alloc(size - this.offset);
      readSync(fd, buffer, 0, buffer.length, this.offset);
      this.offset = size;
      return buffer.toString("utf8");
    } finally {
      closeSync(fd);
    }
  }
}

// ---------------------------------------------------------------------------------------------
// The provider (read-only) and the processes around it

function providerStatus(): ProviderStatus | undefined {
  const r = spawnSync(alias, ["status"], { encoding: "utf8", timeout: 30000, windowsHide: true });
  if (r.error || r.status === null) return undefined;
  return parseProviderStatus(`${r.stdout ?? ""}${r.stderr ?? ""}`);
}

function runningProcesses(imageName: string): number[] {
  const r = spawnSync("tasklist", ["/FI", `IMAGENAME eq ${imageName}`, "/NH", "/FO", "CSV"], { encoding: "utf8", windowsHide: true });
  return [...(r.stdout ?? "").matchAll(/^"[^"]+","(\d+)"/gm)].map((m) => Number(m[1]));
}

/** Stops the processes whose command line mentions our throwaway profile folder, and nothing else. */
function killProfileProcesses(dir: string) {
  const script = `$d = $env:LP_PROFILE_DIR; Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -and $_.CommandLine.Contains($d) } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }`;
  spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { env: { ...process.env, LP_PROFILE_DIR: dir }, windowsHide: true, timeout: 30000 });
}

/**
 * A browser that is killed while Windows' passkey sheet is open leaves the sheet on screen. Close
 * the ones for "localhost" (only those: passkey-sheet.ps1 -Mode cleanup checks the text) and report how many.
 */
function closeLeftoverSheets(): number {
  if (!existsSync(sheetScript)) return 0;
  const r = spawnSync("powershell", ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", sheetScript, "-Mode", "cleanup"], { encoding: "utf8", timeout: 60000, windowsHide: true });
  try {
    return (JSON.parse((r.stdout ?? "").trim()) as { cancelled?: number }).cancelled ?? 0;
  } catch {
    return 0;
  }
}

// ---------------------------------------------------------------------------------------------
// Set-up

const rp = startRelyingParty({ port: value("--port") ? Number(value("--port")) : DEFAULT_ASSISTED_PORT, userVerification: "required", assisted: true, userName, counterZero: !virtual, logFile: join(outDir, "rp.jsonl") });
const site = rp.origin + "/";

async function rpState(): Promise<{ events: RpEvent[]; credentials: { id: string; userName: string; counter: number; aaguid: string }[] }> {
  return (await fetch(`${rp.origin}/state`)).json();
}

const profileDir = mkdtempSync(join(tmpdir(), "lp-winprov-"));
let browserProcess: ReturnType<typeof spawn> | undefined;
let puppeteerBrowser: Browser | undefined; // --virtual only
let page: Page | undefined;
let cdp: CDPSession | undefined;
let virtualId = "";
let cleaned = false;
let leftSheets = 0;

async function cleanup() {
  if (cleaned) return;
  cleaned = true;
  try {
    if (puppeteerBrowser) await Promise.race([puppeteerBrowser.close().catch(() => {}), sleep(8000)]);
  } catch {}
  if (browserProcess?.pid) spawnSync("taskkill", ["/PID", String(browserProcess.pid), "/T", "/F"], { windowsHide: true });
  killProfileProcesses(profileDir);
  if (!virtual) leftSheets = closeLeftoverSheets();
  rp.stop();
  for (let i = 0; i < 8; i++) {
    try {
      rmSync(profileDir, { recursive: true, force: true });
      break;
    } catch {
      await sleep(500);
    }
  }
}
process.on("SIGINT", () => void cleanup().then(() => process.exit(130)));
process.on("exit", () => {
  rp.stop();
});

async function startBrowser() {
  if (virtual) {
    puppeteerBrowser = await puppeteer.launch({ executablePath: browserExe, headless: true, userDataDir: profileDir, args: ["--no-first-run", "--no-default-browser-check"] });
    page = await puppeteerBrowser.newPage();
    cdp = await page.createCDPSession();
    await cdp.send("WebAuthn.enable");
    const noBackup = process.env.LP_VIRTUAL_NO_BACKUP === "1";
    virtualId = (
      await cdp.send("WebAuthn.addVirtualAuthenticator", {
        options: { protocol: "ctap2", transport: "internal", hasResidentKey: true, hasUserVerification: true, isUserVerified: true, automaticPresenceSimulation: true, defaultBackupEligibility: !noBackup, defaultBackupState: !noBackup },
      })
    ).authenticatorId;
    await page.goto(site);
    return;
  }
  const extra = (process.env.LP_BROWSER_ARGS ?? "").split(/\s+/).filter(Boolean);
  const args = [`--user-data-dir=${profileDir}`, "--no-first-run", "--no-default-browser-check", "--disable-sync", "--new-window", ...extra, site];
  browserProcess = spawn(browserExe, args, { stdio: "ignore", detached: false, windowsHide: false });
  browserProcess.on("error", (e) => console.error(`could not start ${browserExe}: ${e.message}`));
}

// ---------------------------------------------------------------------------------------------
// Phases

interface Phase extends PhaseSpec {
  title: string;
  steps: string[];
  /** Waits for / checks the precondition: true to go on, or the reason to skip this phase ("FAIL: ..." counts as a failure). */
  before?: () => Promise<true | string>;
}

// Everything the two logs gained during the run (written to the output folder) and during the
// current phase (analysed).
const archive = { provider: "", prompts: "" };
const phaseLogs = { provider: "", prompts: "" };
let providerTail: LogTail;
let promptsTail: LogTail;
function pump() {
  const p = providerTail.take();
  const q = promptsTail.take();
  archive.provider += p;
  archive.prompts += q;
  phaseLogs.provider += p;
  phaseLogs.prompts += q;
}

/** Only with --virtual does the runner click the page itself; otherwise a person does. */
async function actFor(phase: Phase) {
  if (!virtual || !page || process.env.LP_VIRTUAL_IDLE === "1") return;
  const id = phase.action === "register" ? "#register" : "#signin";
  if (phase.outcome === "cancelled") {
    // A user who refuses the verification: the browser reports NotAllowedError.
    await cdp!.send("WebAuthn.setUserVerified", { authenticatorId: virtualId, isUserVerified: false });
  }
  await page.click(id);
}

async function waitForOutcome(phase: Phase, sinceSeq: number): Promise<WaitResult> {
  const start = Date.now();
  let lastBeat = start;
  let announced = false;
  while (Date.now() - start < timeoutS * 1000) {
    const state = await rpState().catch(() => undefined);
    if (state) {
      const fresh = state.events.filter((e) => e.seq > sinceSeq);
      const ceremony = fresh.find((e) => e.kind === phase.action);
      const pageError = fresh.find((e) => e.kind === "client" && e.outcome === "error");
      if (phase.outcome === "success" && ceremony) return { kind: "event", event: ceremony };
      if (pageError) return { kind: "page-error", pageError };
      if (phase.outcome === "cancelled" && ceremony) return { kind: "event", event: ceremony };
    }
    if (!announced && !virtual) {
      pump();
      if (analyseProvider(phaseLogs.provider).txs.length) {
        announced = true;
        console.log("   ... the provider received the request from Windows");
      }
    }
    if (Date.now() - lastBeat > 20000) {
      lastBeat = Date.now();
      console.log(`   ... still waiting (${Math.round((Date.now() - start) / 1000)} s of ${timeoutS} s)`);
    }
    await sleep(250);
  }
  const events = (await rpState().catch(() => ({ events: [] as RpEvent[] }))).events.filter((e) => e.seq > sinceSeq && e.kind === "client");
  return { kind: "timeout", timeoutS, pageEvents: events.map((e) => `${e.outcome}:${e.errorName ?? ""}`) };
}

const reports: unknown[] = [];
let registered = false;

async function runPhase(phase: Phase) {
  console.log(`\n=== ${phase.id}: ${phase.title}`);
  const skip = phase.before ? await phase.before() : true;
  if (skip !== true) {
    add(skip.startsWith("FAIL: ") ? "FAIL" : "SKIP", phase.id, skip.replace(/^FAIL: /, ""));
    return;
  }
  pump();
  phaseLogs.provider = phaseLogs.prompts = "";
  const sinceSeq = (await rpState()).events.length;
  const begin = Date.now();
  if (!virtual) for (const s of phase.steps) console.log(`   ${s}`);
  await actFor(phase);
  const result = await waitForOutcome(phase, sinceSeq);
  // Let the logs catch up, and give a second Hello prompt time to (not) appear.
  await sleep(phase.outcome === "cancelled" ? 4000 : 2000);
  pump();
  if (virtual && phase.outcome === "cancelled") await cdp!.send("WebAuthn.setUserVerified", { authenticatorId: virtualId, isUserVerified: true });
  const provider = analyseProvider(phaseLogs.provider);
  const hello = analysePrompts(phaseLogs.prompts);
  const pageMs = (await rpState()).events.filter((e) => e.seq > sinceSeq && e.kind === "client").map((e) => e.pageMs)[0];

  const judged = judgePhase({ phase, result, provider, hello, virtual, pageMs, phaseMs: Date.now() - begin, appRunningAfter: phase.id === "closed" ? runningProcesses("localpass.exe").length > 0 : undefined });
  for (const row of judged.rows) pushRow(row);
  for (const hint of judged.hints) console.log(`      ${hint}`);
  if (judged.registered) registered = true;
  reports.push({ phase: phase.id, result: result.kind, provider: { txs: provider.txs, nonZero: provider.nonZero }, hello: { starts: hello.starts, dialogs: hello.dialogs } });
}

// ---------------------------------------------------------------------------------------------
// Main

try {
  console.log(`LocalPass Windows passkey provider test${virtual ? " (--virtual self-test: no Windows provider involved)" : ""}`);
  console.log(`output: ${outDir}`);
  providerTail = new LogTail(providerLogPath);
  const hostProfile = virtual ? undefined : providerStatus()?.profile;
  const promptsPath = hostProfile && !/^none/.test(hostProfile) ? join(appData, "app.localpass", "profiles", hostProfile, "prompts.log") : join(appData, "app.localpass", "prompts.log");
  promptsTail = new LogTail(promptsPath);

  let before: ProviderStatus | undefined;
  if (!virtual) {
    console.log("\n=== preflight (read-only `lp-winauth-provider status`)");
    before = providerStatus();
    if (!existsSync(browserExe)) add("FAIL", `${browserName} is installed`, browserExe);
    if (!before) {
      add("FAIL", "provider status", `could not run ${alias} (package not registered?)`);
    } else {
      add(before.registered && before.enabled ? "PASS" : "FAIL", "provider registered and turned on in Windows", before.registered ? (before.enabled ? "enabled" : "registered but disabled: Settings > Accounts > Passkeys > Advanced options > LocalPass") : "not registered (tools\\windows-provider\\register.ps1 or the app's Settings)");
      add(before.paired ? "PASS" : "FAIL", "provider paired with the app", before.paired ? "yes" : "run: lp-winauth-provider pair (approve the code in the app)");
      add("INFO", "app", `${before.appReachable ? "running" : "not running (the provider will start it)"}${before.vault ? `, vault ${before.vault}` : ""}; credential cache ${before.cacheRows ?? "?"} row(s)`);
    }
    if (rows.some((r) => r.status === "FAIL")) throw new Error("preflight failed");
  }

  await startBrowser();
  console.log(virtual ? `\nheadless browser with a virtual authenticator at ${site}` : `\n${browserName} started with a throwaway profile at ${site}  (LocalPass extension NOT installed; Windows' own passkey UI is used)`);
  console.log(`test user name: ${userName}`);
  if (!virtual) console.log(`If the Windows sheet offers another provider, use its "Change" / "Other options" link and choose LocalPass. Each step waits up to ${timeoutS} s.`);

  const phases: Phase[] = [
    {
      id: "register",
      title: "create a passkey",
      action: "register",
      outcome: "success",
      steps: [`1. In the browser window click "Register" (user name ${userName}).`, "2. In the Windows sheet choose LocalPass and continue.", "3. Accept the Windows Hello prompt of the LocalPass app (it may only blink in the taskbar)."],
    },
    {
      id: "signin",
      title: "sign in with it",
      action: "signin",
      outcome: "success",
      before: async () => registered || "no registration was verified",
      steps: [`1. Click "Sign in".`, "2. In the Windows sheet choose the LocalPass passkey.", "3. Accept the Windows Hello prompt of the app."],
    },
  ];
  if (want.cancel)
    phases.push({
      id: "cancel",
      title: "cancel in Windows Hello",
      action: "signin",
      outcome: "cancelled",
      before: async () => registered || "no registration was verified",
      steps: [`1. Click "Sign in", choose the LocalPass passkey.`, "2. When the app shows Windows Hello, CANCEL it (Esc or the Cancel button). Expect: the page shows an error and no second prompt."],
    });
  if (want.locked)
    phases.push({
      id: "locked",
      title: "sign in with the vault locked",
      action: "signin",
      outcome: "success",
      before: async () => {
        if (!registered) return "no registration was verified";
        if (virtual) return "needs the real app";
        console.log('   Lock the vault now: LocalPass window > the lock button (or wait for the auto-lock). Waiting until `status` says "vault: Locked"...');
        const start = Date.now();
        while (Date.now() - start < timeoutS * 1000) {
          if (providerStatus()?.vault === "Locked") return true;
          await sleep(2000);
        }
        return "FAIL: the vault never showed as Locked within the timeout (precondition)";
      },
      steps: [`1. Click "Sign in", choose the LocalPass passkey.`, "2. The app asks to unlock (Windows Hello): accept it. That must be the ONLY Hello prompt."],
    });
  if (want.closed)
    phases.push({
      id: "closed",
      title: "sign in with LocalPass closed",
      action: "signin",
      outcome: "success",
      before: async () => {
        if (!registered) return "no registration was verified";
        if (virtual) return "needs the real app";
        if (runningProcesses("localpass.exe").length === 0) return true;
        console.log("   Quit LocalPass completely now (tray icon > Quit). The runner never closes it for you. Waiting...");
        const start = Date.now();
        while (Date.now() - start < timeoutS * 1000) {
          if (runningProcesses("localpass.exe").length === 0) return true;
          await sleep(2000);
        }
        return "FAIL: localpass.exe is still running after the timeout (precondition)";
      },
      steps: [`1. Click "Sign in", choose the LocalPass passkey.`, "2. The provider should start LocalPass (minimised); accept its single Windows Hello prompt."],
    });

  for (const phase of phases) await runPhase(phase);

  // After.
  pump();
  if (!virtual) {
    console.log("\n=== after (read-only `status`)");
    const after = providerStatus();
    if (after) {
      const delta = (after.cacheRows ?? 0) - (before?.cacheRows ?? 0);
      add(registered ? (delta === 1 ? "PASS" : "WARN") : "INFO", "credential cache grew by one row", `${before?.cacheRows ?? "?"} -> ${after.cacheRows ?? "?"}`);
      add(after.registered && after.enabled && after.paired ? "PASS" : "FAIL", "provider still registered, enabled and paired", after.appReachable ? `app running, vault ${after.vault ?? "?"}` : "app not running");
    } else add("FAIL", "provider status after", "could not run");
  }
  const state = await rpState();
  add("INFO", "relying party", `${state.credentials.length} credential(s) registered, ${state.events.filter((e) => e.kind === "register" || e.kind === "signin").length} ceremony verdict(s), log ${join(outDir, "rp.jsonl")}`);
} catch (e) {
  console.error(e instanceof Error ? e.message : e);
  if (!rows.some((r) => r.status === "FAIL")) add("FAIL", "runner", String(e));
} finally {
  try {
    pump();
  } catch {}
  writeFileSync(join(outDir, "provider-log-new-lines.txt"), archive.provider);
  writeFileSync(join(outDir, "prompts-log-new-lines.txt"), archive.prompts);
  await cleanup();
  if (leftSheets) rows.push({ status: "INFO", name: "closed leftover passkey sheet(s) for localhost", detail: String(leftSheets) });
  writeFileSync(join(outDir, "report.json"), JSON.stringify({ rows, reports, userName, browser: browserName, virtual }, null, 2));
}

console.log("\n==== RESULT");
for (const line of formatTable(rows)) console.log(line);
if (!virtual && registered) console.log(`\nDelete the test passkey "${userName}" (site localhost) in the LocalPass app: it is in your real vault.`);
console.log(`(${seconds(Date.now() - startedAt)} in total; logs and report: ${outDir})`);
process.exit(failureCount(rows) ? 1 : 0);
