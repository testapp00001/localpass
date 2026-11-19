// Helpers for driving an Android emulator from the tests: adb commands, and tapping or reading
// the screen through uiautomator (the system's credential sheet and biometric prompt are not web
// pages, so they can only be driven this way).

import { join } from "node:path";
import { androidTarget, assertEmulator } from "./android-target";

const target = androidTarget(process.env);
export const TEST_PACKAGE = target.package;
const sdk = process.env.ANDROID_HOME;
export const ADB = sdk ? join(sdk, "platform-tools", process.platform === "win32" ? "adb.exe" : "adb") : "adb";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

function runAdb(args: string[], allowFailure = false): string {
  const run = Bun.spawnSync([ADB, "-s", target.serial, ...args], { stdout: "pipe", stderr: "pipe" });
  const output = run.stdout.toString() + run.stderr.toString();
  const absentProcess = args[0] === "shell" && args[1] === "pidof" && run.exitCode === 1 && !output.trim();
  if (run.exitCode !== 0 && !allowFailure && !absentProcess) throw new Error(`ADB ${args.slice(0, 3).join(" ")} failed (${run.exitCode}): ${output.trim()}`);
  return output;
}

const savedSettings = new Map<string, string>();
const savedForwards = new Map<string, string | undefined>();
function restoreForward(mode: string, local: string, remote: string | undefined): void {
  if (remote) runAdb([mode, local, remote]);
  else {
    const exists = runAdb([mode, "--list"]).split(/\r?\n/).some(line => {
      const parts = line.trim().split(/\s+/);
      return parts[1] === local && (mode === "reverse" || parts[0] === target.serial);
    });
    if (exists) runAdb([mode, "--remove", local]);
  }
}

/** Restore exactly the settings and port mappings this run changed, even on an early failure. */
export function restoreAndroidState(): void {
  const failures: string[] = [];
  for (const [key, value] of savedSettings) {
    try {
      runAdb(value === "null" || !value ? ["shell", "settings", "delete", "secure", key] : ["shell", "settings", "put", "secure", key, value]);
      savedSettings.delete(key);
    } catch (error) { failures.push(String(error)); }
  }
  for (const [key, remote] of savedForwards) {
    const [mode, local] = key.split(" ") as [string, string];
    try {
      restoreForward(mode, local, remote);
      savedForwards.delete(key);
    } catch (error) { failures.push(String(error)); }
  }
  if (failures.length) throw new Error(`Android cleanup failed:\n${failures.join("\n")}`);
}
process.once("exit", () => {
  try { restoreAndroidState(); } catch (error) { console.error(String(error)); process.exitCode = 1; }
});
for (const signal of ["SIGINT", "SIGTERM"] as const) process.once(signal, () => process.exit(130));

export function requireEmulator(): void {
  assertEmulator(runAdb(["shell", "getprop", "ro.kernel.qemu"]));
}

function checkApk(apk: string): void {
  const analyzer = sdk ? join(sdk, "cmdline-tools", "latest", "bin", process.platform === "win32" ? "apkanalyzer.bat" : "apkanalyzer") : "apkanalyzer";
  const result = Bun.spawnSync([analyzer, "manifest", "application-id", apk], { stdout: "pipe", stderr: "pipe" });
  if (result.exitCode !== 0 || result.stdout.toString().trim() !== TEST_PACKAGE) {
    throw new Error(`Refusing APK: manifest must identify ${TEST_PACKAGE} (use -Plocalpass.audit=true)`);
  }
}

/** Commands always select the explicit serial and fail on unsuccessful execution. */
export function adb(...args: string[]): string {
  if (args[0] === "install") checkApk(args.at(-1)!);
  if (args[0] === "uninstall" || (args[0] === "shell" && args[1] === "pm" && args[2] === "clear")) {
    if (args.at(-1) !== TEST_PACKAGE) throw new Error("Refusing to remove or clear a non-audit app");
    requireEmulator();
  }
  if (args[0] === "shell" && /locksettings|fingerprint/.test(args[1] ?? "")) throw new Error("Test helpers must not change screen locks or enrolled fingerprints");
  if (args[0] === "emu") requireEmulator();
  if (args[0] === "shell" && args[1] === "settings" && ["put", "delete"].includes(args[2] ?? "") && args[3] === "secure") {
    const key = args[4]!;
    if (!savedSettings.has(key)) savedSettings.set(key, runAdb(["shell", "settings", "get", "secure", key]).trim());
  }
  if (args[0] === "forward" || args[0] === "reverse") {
    const mode = args[0];
    if (args[1] === "--remove-all") throw new Error("Removing unrelated port mappings is refused");
    if (args[1] === "--remove") {
      const key = `${mode} ${args[2]}`;
      if (!savedForwards.has(key)) throw new Error("Refusing to remove a port mapping this test did not create");
      restoreForward(mode, args[2]!, savedForwards.get(key));
      return "";
    }
    if (!args[1]?.startsWith("--")) {
      const key = `${mode} ${args[1]}`;
      if (!savedForwards.has(key)) {
        const existing = runAdb([mode, "--list"]).split(/\r?\n/).map(line => line.trim().split(/\s+/)).find(parts => parts[1] === args[1] && (mode === "reverse" || parts[0] === target.serial));
        savedForwards.set(key, existing?.[2]);
      }
    }
  }
  return runAdb(args);
}

/** For assertions whose expected outcome is a failing command (e.g. release run-as). */
export function adbExpectedFailure(...args: string[]): string {
  if (args[0] !== "shell" || args[1] !== "run-as" || args[2] !== TEST_PACKAGE) throw new Error("Unexpected negative-test command");
  return runAdb(args, true);
}

export const shell = (...args: string[]) => adb("shell", ...args).replace(/\r/g, "").trim();

export interface UiNode {
  text: string;
  desc: string;
  id: string;
  cls: string;
  clickable: boolean;
  bounds: [number, number, number, number];
}

const attr = (node: string, name: string) => new RegExp(`\\s${name}="([^"]*)"`).exec(node)?.[1] ?? "";
const unescape = (s: string) => s.replace(/&quot;/g, '"').replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&#10;/g, "\n").replace(/&apos;/g, "'");

/** Everything on screen right now. */
export function screen(): UiNode[] {
  shell("uiautomator", "dump", "/sdcard/lp-ui.xml");
  const xml = adb("exec-out", "cat", "/sdcard/lp-ui.xml");
  const nodes: UiNode[] = [];
  for (const m of xml.matchAll(/<node [^>]*>/g)) {
    const node = m[0];
    const b = /bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]"/.exec(node);
    if (!b) continue;
    nodes.push({
      text: unescape(attr(node, "text")),
      desc: unescape(attr(node, "content-desc")),
      id: attr(node, "resource-id"),
      cls: attr(node, "class"),
      clickable: attr(node, "clickable") === "true",
      bounds: [Number(b[1]), Number(b[2]), Number(b[3]), Number(b[4])],
    });
  }
  return nodes;
}

export const labels = (nodes = screen()) => nodes.flatMap((n) => [n.text, n.desc]).filter(Boolean);

/** The node whose text, description or id matches (a tappable one first). */
export function find(match: string | RegExp, nodes = screen()): UiNode | undefined {
  const test = (s: string) => (typeof match === "string" ? s === match : match.test(s));
  const found = nodes.filter((n) => test(n.text) || test(n.desc) || test(n.id));
  // A label can appear twice (a heading and a button): prefer the one that can be tapped.
  return found.find((n) => n.clickable) ?? found[0];
}

export function tapNode(node: UiNode) {
  const [l, t, r, b] = node.bounds;
  shell("input", "tap", String(Math.round((l + r) / 2)), String(Math.round((t + b) / 2)));
}

/** Waits for something on screen to match, returning it. */
export async function waitFor(match: string | RegExp, timeoutMs = 20000): Promise<UiNode> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const nodes = screen();
    const node = find(match, nodes);
    if (node) return node;
    // A slow emulator can make the system say an app isn't responding; carry on waiting for it.
    const wait = find(/isn't responding/, nodes) && find(/^Wait$/, nodes);
    if (wait) tapNode(wait);
    await sleep(700);
  }
  throw new Error(`"${match}" never appeared; the screen shows: ${labels().slice(0, 25).join(" | ")}`);
}

/** Scrolls a list up in small steps until the match is on screen (a long settings page), then returns it. */
export async function scrollTo(match: string | RegExp, steps = 10): Promise<UiNode> {
  for (let i = 0; i < steps; i++) {
    const node = find(match);
    if (node) return node;
    shell("input", "swipe", "800", "1800", "800", "1000", "300");
    await sleep(700);
  }
  return waitFor(match, 5000);
}

export async function tap(match: string | RegExp, timeoutMs = 20000) {
  tapNode(await waitFor(match, timeoutMs));
}

/** Types into the focused field (spaces become %s for `input text`). */
export function type(text: string) {
  shell("input", "text", text.replace(/ /g, "%s"));
}

export const key = (code: string) => shell("input", "keyevent", code);

export function screenshot(path: string) {
  const run = Bun.spawnSync([ADB, "-s", target.serial, "exec-out", "screencap", "-p"], { stdout: "pipe", stderr: "pipe" });
  if (run.exitCode !== 0) throw new Error(`ADB screenshot failed: ${run.stderr.toString()}`);
  Bun.write(path, run.stdout);
}
