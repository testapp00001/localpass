// Helpers for driving an Android emulator from the tests: adb commands, and tapping or reading
// the screen through uiautomator (the system's credential sheet and biometric prompt are not web
// pages, so they can only be driven this way).

import { join } from "node:path";

const sdk = process.env.ANDROID_HOME ?? join(process.env.LOCALAPPDATA ?? "", "Android", "Sdk");
export const ADB = join(sdk, "platform-tools", "adb.exe");

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Runs adb and returns its output (stderr is folded in so errors are visible). */
export function adb(...args: string[]): string {
  const run = Bun.spawnSync([ADB, ...args], { stdout: "pipe", stderr: "pipe" });
  return run.stdout.toString() + run.stderr.toString();
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
  const run = Bun.spawnSync([ADB, "exec-out", "screencap", "-p"], { stdout: "pipe" });
  Bun.write(path, run.stdout);
}
