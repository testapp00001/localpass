// What the two ASSISTED browser scripts share (`e2e/assisted.ts` for Chrome and the Chromium browsers,
// `e2e/firefox/assisted.ts` for Firefox; Linux, against the REAL app a person started): the start-up checks, the questions to
// the person (with a timeout), the new lines of the app's `prompts.log`, and the watcher of the Unlock round trip
// (decision panel -> Unlock -> app -> panel again -> fill) that reports what a script can see while the person acts.
//
// Nothing here starts the app, writes to the person's real browser folders or reads the vault.

import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { Readable } from "node:stream";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** How long the scripts wait for the person at each step. */
export const STEP_MS = Number(process.env.LP_ASSIST_STEP_S ?? 180) * 1000;

export const say = (text: string) => console.log(`\n>>> ${text}`);
const stamp = (ms: number) => new Date(ms).toLocaleString("sv-SE");
const seconds = (ms: number) => `${(ms / 1000).toFixed(1)} s`;

/** The test page both scripts serve on http://localhost:<port>: one login form. */
export const TEST_PAGE = `<!doctype html><title>LocalPass assisted</title><h1>LocalPass assisted check</h1>
<form><p><input id="u" autocomplete="username" placeholder="user name" style="width:300px;height:30px"></p>
<p><input id="p" type="password" placeholder="password" style="width:300px;height:30px"></p></form>`;

// ---- start-up ----

/** The broker's socket of a profile (as `lp-broker`'s Linux `pipe.rs` names it). */
export function appSocket(profile: string): string {
  const runtime = process.env.XDG_RUNTIME_DIR ?? "";
  return join(runtime, "localpass", profile === "default" ? "broker-v1.sock" : `broker-v1-${profile}.sock`);
}

/** Stops the script (before any browser opens) unless the app of this profile accepts a connection. */
export async function requireAppListening(profile: string): Promise<void> {
  const socket = appSocket(profile);
  // A socket file can outlive the app (a stale one): only a connection that is accepted proves it is running.
  const listening = await Bun.connect({ unix: socket, socket: { data() {}, open(s) { s.end(); }, error() {} } }).then(
    () => true,
    () => false,
  );
  if (!listening) {
    console.error(`\nthe app is not listening on ${socket}.\nStart it first: cd app && LOCALPASS_PROFILE=${profile} bun run tauri dev`);
    process.exit(2);
  }
  console.log(`app: listening on ${socket}`);
}

/** The app's data folder for a profile (`lib.rs` `data_dir`: Tauri's app data dir for identifier `app.localpass`). */
export function appDataDir(profile: string): string {
  const data = process.env.XDG_DATA_HOME?.startsWith("/") ? process.env.XDG_DATA_HOME : join(homedir(), ".local/share");
  const base = join(data, "app.localpass");
  return profile === "default" ? base : join(base, "profiles", profile);
}

function newestFile(dir: string): number {
  let newest = 0;
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const s = statSync(p);
      if (s.isDirectory()) walk(p);
      else newest = Math.max(newest, s.mtimeMs);
    }
  };
  if (existsSync(dir)) walk(dir);
  return newest;
}

/**
 * Prints which builds this run uses and the commands that make them, and warns when the extension build is older than its
 * source. Returns what is missing (the caller stops on it after checking that the app listens).
 */
export function describeBuilds(opts: { browser: string; extensionDir: string; buildCommand: string; appExe: string; profile: string }): string[] {
  const missing: string[] = [];
  const src = join(import.meta.dir, "../src");
  console.log(`LocalPass assisted check: ${opts.browser}, ${stamp(Date.now())}`);
  console.log("Commands the person must have run, in this order:");
  console.log(`  terminal 1: cd app && LOCALPASS_PROFILE=${opts.profile} bun run tauri dev   (optionally with LOCALPASS_RAISE=present|remap)`);
  console.log(`  terminal 2: cd extension && ${opts.buildCommand}`);
  if (existsSync(join(opts.extensionDir, "manifest.json"))) {
    const manifest = JSON.parse(readFileSync(join(opts.extensionDir, "manifest.json"), "utf8")) as { version: string };
    const built = newestFile(opts.extensionDir);
    const source = newestFile(src);
    console.log(`extension: ${opts.extensionDir} (version ${manifest.version}, built ${stamp(built)})`);
    if (source > built) console.log(`  WARNING: extension/src changed at ${stamp(source)}, after this build: build it again`);
  } else missing.push(`no extension build in ${opts.extensionDir}: run \`${opts.buildCommand.split(" && bun run e2e")[0]}\` first`);
  if (existsSync(opts.appExe)) {
    console.log(`app binary (native-messaging helper): ${opts.appExe} (built ${stamp(statSync(opts.appExe).mtimeMs)})`);
    console.log("  it must be the binary of the running app: `tauri dev` builds into target/debug unless CARGO_TARGET_DIR is set");
  } else {
    missing.push(`no app binary at ${opts.appExe}: \`bun run tauri dev\` builds target/debug/localpass (or $CARGO_TARGET_DIR/debug/localpass); pass the path as the second argument`);
  }
  return missing;
}

/** Stops on what `describeBuilds` found missing. */
export function requireBuilds(missing: string[]) {
  if (!missing.length) return;
  for (const m of missing) console.error(m);
  process.exit(2);
}

// ---- the person ----

/** Lines typed in this terminal, and the answers to put into the summary. */
export class Person {
  private lines: string[] = [];
  /**
   * The one wait for a line, if any. Each wait owns its timer: ending it (a line, its own timeout, `cancelLine` or a newer
   * `line()`) clears that timer, and a timer can only end the wait it belongs to.
   */
  private wait: { end: (line: string | null) => void } | undefined;
  readonly answers: { step: string; question: string; answer: string }[] = [];
  readonly notes: { step: string; text: string }[] = [];

  /** `input` is this terminal; tests pass a stream of their own. */
  constructor(input: Readable = process.stdin) {
    const rl = createInterface({ input });
    rl.on("line", (line) => {
      if (this.wait) this.wait.end(line.trim());
      else this.lines.push(line.trim());
    });
  }

  /**
   * The next line typed, or null after `timeoutMs` (or when the wait is ended by `cancelLine` or a newer `line()`). Lines typed
   * earlier (while nothing asked) are dropped.
   */
  line(timeoutMs = STEP_MS): Promise<string | null> {
    this.lines = [];
    this.cancelLine();
    return new Promise((resolve) => {
      let timer: ReturnType<typeof setTimeout> | undefined;
      const wait = {
        end: (line: string | null) => {
          if (this.wait !== wait) return;
          this.wait = undefined;
          clearTimeout(timer);
          resolve(line);
        },
      };
      this.wait = wait;
      timer = setTimeout(() => wait.end(null), timeoutMs);
    });
  }

  /** Waits for Enter (true) or the time to run out (false). */
  async enter(prompt: string, timeoutMs = STEP_MS): Promise<boolean> {
    console.log(`${prompt}\n    [press Enter here when done; ${Math.round(timeoutMs / 1000)} s]`);
    const got = (await this.line(timeoutMs)) !== null;
    if (!got) console.log("    (no answer in time: going on)");
    return got;
  }

  /** Waits until `done` is true (checked every 500 ms), Enter is pressed (false) or the time runs out (false). */
  async until(prompt: string, done: () => Promise<boolean>, timeoutMs = STEP_MS): Promise<boolean> {
    console.log(`${prompt}\n    [the script goes on by itself once it sees it; Enter skips; ${Math.round(timeoutMs / 1000)} s]`);
    let skipped = false;
    const typed = this.line(timeoutMs).then(() => (skipped = true));
    const start = Date.now();
    while (!skipped && Date.now() - start < timeoutMs) {
      if (await done().catch(() => false)) {
        this.cancelLine();
        return true;
      }
      await Promise.race([sleep(500), typed]);
    }
    this.cancelLine();
    return false;
  }

  /** Ends the current wait for a line, if any (its promise gets null, its timer is cleared). */
  cancelLine() {
    this.wait?.end(null);
  }

  /** A promise that resolves when the person presses Enter (for "stop waiting"); `cancelLine` ends it. */
  pressed(): Promise<null | string> {
    return this.line(24 * 3600_000);
  }

  /** A y/n question; the answer (or "no answer") goes into the summary. */
  async yesNo(step: string, question: string, timeoutMs = STEP_MS): Promise<string> {
    for (;;) {
      console.log(`  ? ${question} [y/n, ? = could not tell]`);
      const line = await this.line(timeoutMs);
      if (line === null) return this.record(step, question, "no answer");
      const a = line.toLowerCase();
      if (["y", "yes", "n", "no", "?"].includes(a)) return this.record(step, question, a.startsWith("y") ? "yes" : a.startsWith("n") ? "no" : "could not tell");
      console.log("    please type y, n or ?");
    }
  }

  /** A free answer (may be empty). */
  async text(step: string, question: string, timeoutMs = STEP_MS): Promise<string> {
    console.log(`  ? ${question} [type it and press Enter; Enter alone = none]`);
    const line = await this.line(timeoutMs);
    return this.record(step, question, line === null ? "no answer" : line || "none");
  }

  private record(step: string, question: string, answer: string): string {
    this.answers.push({ step, question, answer });
    return answer;
  }

  note(step: string, text: string) {
    this.notes.push({ step, text });
  }

  /** The summary to paste into the handoff. */
  summary(header: string): string {
    const out = [`- ${header}`];
    const steps = [...new Set([...this.notes.map((n) => n.step), ...this.answers.map((a) => a.step)])];
    for (const step of steps) {
      out.push(`  - ${step}:`);
      for (const n of this.notes.filter((x) => x.step === step)) out.push(`    - seen by the script: ${n.text}`);
      for (const a of this.answers.filter((x) => x.step === step)) out.push(`    - ${a.question} **${a.answer}**`);
    }
    return out.join("\n");
  }
}

/** The questions only the person can answer, after the app raised a prompt for the browser. */
export async function askAboutFocus(person: Person, step: string, opts: { hidden: boolean; unlock: boolean }) {
  const what = opts.unlock ? "the unlock screen" : "\"Confirm it is you\"";
  await person.yesNo(step, `Did the LocalPass window come to the front with ${what}?`);
  await person.yesNo(step, "Was the cursor already in the master password field, so you could type at once without clicking?");
  await person.yesNo(step, "Did GNOME show a \"LocalPass is ready\" notification (instead of, or before, the window)?");
  if (opts.hidden) await person.yesNo(step, "After you finished in the app, did the LocalPass window go away again (back to the tray)?");
  else await person.yesNo(step, "After you finished in the app, did the LocalPass window stay open?");
  await person.yesNo(step, "Did the browser come back to the front by itself afterwards (no click needed)?");
  if (opts.unlock) await person.yesNo(step, "Did the LocalPass panel come back in the browser by itself, showing the login?");
  await person.text(step, "Anything else you saw (wrong window raised, a second prompt, a delay)?");
}

// ---- the app's prompts.log ----

/** The app's `prompts.log` of a profile: what was added since `mark()` (also across a rotation to `prompts.log.1`). */
export class PromptsLog {
  readonly path: string;
  private offset = 0;
  constructor(profile: string) {
    this.path = process.env.LP_PROMPTS_LOG ?? join(appDataDir(profile), "prompts.log");
  }
  private size(p: string) {
    return existsSync(p) ? statSync(p).size : 0;
  }
  mark() {
    this.offset = this.size(this.path);
  }
  /** The lines added since `mark()`, and moves the mark to the end. */
  since(): string[] {
    const now = this.size(this.path);
    let text = "";
    if (now < this.offset) {
      // Rotated: the rest of the old file is prompts.log.1 beyond the mark.
      const old = `${this.path}.1`;
      if (existsSync(old)) text += readFileSync(old).subarray(this.offset).toString("utf8");
      text += existsSync(this.path) ? readFileSync(this.path, "utf8") : "";
    } else if (now > this.offset) {
      text = readFileSync(this.path).subarray(this.offset).toString("utf8");
    }
    this.offset = now;
    return text.split("\n").filter((l) => l.trim() !== "");
  }
  /** Prints `lines` (by default what was added since the mark, which moves the mark; pass what `since()` already returned). */
  print(step: string, lines: string[] = this.since()) {
    console.log(`  prompts.log, new lines during ${step} (${this.path}): ${lines.length ? "" : existsSync(this.path) ? "none" : "the file does not exist"}`);
    for (const l of lines) console.log(`    ${l}`);
  }
}

// ---- the Unlock round trip ----

/** One page of LocalPass's decision UI (a toolbar panel, or the separate window when the panel could not open). */
export interface DecisionView {
  id: string;
  kind: "panel" | "window";
}

/** What a browser script lets the watcher see. Every call may fail while windows come and go; the watcher tolerates that. */
export interface RoundTripProbe {
  /** The decision pages open now. */
  views(): Promise<DecisionView[]>;
  /** Their text (site, lines and buttons). */
  text(view: DecisionView): Promise<string>;
  /**
   * Makes the page record the person's press on its Unlock button: `localStorage["lp-assist-unlock"] = Date.now()`
   * (extension pages share one origin, so the control tab reads it after the panel closed).
   */
  arm(view: DecisionView): Promise<void>;
  /** The time of the last recorded Unlock press (from the control tab), or 0. */
  unlockPressedAt(): Promise<number>;
  clearUnlockPressed(): Promise<void>;
  /** The vault status the app reports (the popup's `state`, asked from the control tab). */
  vault(): Promise<string>;
  /** What the test page's fields hold now (the password only as its length). */
  fields(): Promise<{ username: string; password: number }>;
  /** The toolbar badge text of the test tab ("1": a save or passkey waits). */
  badge(): Promise<string>;
}

/** The script's own code for the Unlock press, evaluated in a decision page. */
export const ARM_UNLOCK = `(() => {
  const root = document.documentElement;
  if (root.dataset.lpAssistArmed) return 'armed';
  root.dataset.lpAssistArmed = '1';
  const mark = (target) => {
    const b = target && target.closest ? target.closest('button') : null;
    if (!b || !/^Unlock/.test(b.textContent || '') || b.disabled || b.getAttribute('aria-disabled') === 'true') return;
    try { localStorage.setItem('lp-assist-unlock', String(Date.now())); } catch (e) {}
  };
  document.addEventListener('click', (e) => mark(e.target), true);
  document.addEventListener('keydown', (e) => { if ((e.key === 'Enter' || e.key === ' ') && !e.repeat) mark(e.target); }, true);
  return 'armed';
})()`;

export interface RoundTripResult {
  filled: boolean;
  events: { at: number; what: string }[];
  /** Times (ms since the epoch) of the milestones, 0 when not seen. */
  firstPanel: number;
  unlockPressed: number;
  firstPanelClosed: number;
  unlocked: number;
  reopened: number;
  filledAt: number;
  reopenKind: string;
}

/**
 * Watches one Unlock round trip until the page is filled, the time runs out or `stop` resolves (Enter). Prints each
 * change as it happens: decision pages opening and closing (and their text), the Unlock press, the vault status, the fill.
 */
export async function watchRoundTrip(probe: RoundTripProbe, stop: Promise<unknown>, timeoutMs = STEP_MS): Promise<RoundTripResult> {
  const start = Date.now();
  const r: RoundTripResult = { filled: false, events: [], firstPanel: 0, unlockPressed: 0, firstPanelClosed: 0, unlocked: 0, reopened: 0, filledAt: 0, reopenKind: "" };
  const event = (what: string, at = Date.now()) => {
    r.events.push({ at, what });
    console.log(`    +${seconds(at - start).padStart(7)}  ${what}`);
  };
  let stopped = false;
  void stop.then(() => (stopped = true));
  await probe.clearUnlockPressed().catch(() => {});
  const open = new Map<string, { view: DecisionView; text: string; n: number }>();
  let count = 0;
  let vault = await probe.vault().catch(() => "?");
  event(`watching (vault ${vault})`);
  let tick = 0;
  let seenEvents = r.events.length;
  while (!stopped && Date.now() - start < timeoutMs) {
    tick++;
    const views = await probe.views().catch(() => null);
    if (views) {
      for (const v of views) {
        if (open.has(v.id)) continue;
        count++;
        open.set(v.id, { view: v, text: "", n: count });
        await probe.arm(v).catch(() => {});
        event(`decision ${v.kind} #${count} opened`);
        if (count === 1) r.firstPanel = Date.now();
        else if (!r.reopened) {
          r.reopened = Date.now();
          r.reopenKind = v.kind;
        }
      }
      for (const [id, o] of [...open]) {
        if (views.some((v) => v.id === id)) continue;
        open.delete(id);
        event(`decision ${o.view.kind} #${o.n} closed`);
        if (o.n === 1) r.firstPanelClosed = Date.now();
      }
    }
    for (const o of open.values()) {
      const text = (await probe.text(o.view).catch(() => o.text)).replace(/\s+/g, " ").trim();
      if (text && text !== o.text) {
        o.text = text;
        event(`#${o.n} shows: ${text.slice(0, 160)}`);
      }
    }
    if (!r.unlockPressed) {
      const at = await probe.unlockPressedAt().catch(() => 0);
      if (at) {
        r.unlockPressed = at;
        event("Unlock pressed in the panel", at);
      }
    }
    // The vault every third look, and at once when a decision page came or went (so the order of the events is right).
    const changed = r.events.length !== seenEvents;
    seenEvents = r.events.length;
    if (tick % 3 === 0 || changed) {
      const now = await probe.vault().catch(() => vault);
      if (now !== vault) {
        event(`vault: ${vault} -> ${now}`);
        if (now === "unlocked" && !r.unlocked) r.unlocked = Date.now();
        vault = now;
      }
    }
    const f = await probe.fields().catch(() => ({ username: "", password: 0 }));
    if (f.password > 0) {
      r.filled = true;
      r.filledAt = Date.now();
      event(`page filled (user name "${f.username}", password of ${f.password} characters)`);
      break;
    }
    await sleep(150);
  }
  if (!r.filled) {
    event(stopped ? "stopped by Enter" : "timed out");
    const views = await probe.views().catch(() => []);
    const texts = await Promise.all(views.map(async (v) => `${v.kind}: ${(await probe.text(v).catch(() => "?")).replace(/\s+/g, " ").slice(0, 120)}`));
    console.log(`    state now: vault ${await probe.vault().catch(() => "?")}; decision pages open: ${texts.length ? texts.join(" || ") : "none"}; toolbar badge "${await probe.badge().catch(() => "?")}"`);
  }
  return r;
}

/** One line for the summary from a round trip's times. */
export function roundTripLine(r: RoundTripResult): string {
  // The watcher looks about every 150 ms (the vault less often), so very short spans are "at once".
  const span = (from: number, to: number) => (from && to ? (to - from < 400 ? "at once (< 0.4 s)" : seconds(to - from)) : "not seen");
  // Unlock pressed is recorded by the panel page; when it was not (closed too fast), the first panel's closing stands in.
  const base = r.unlockPressed || r.firstPanelClosed;
  const from = r.unlockPressed ? "Unlock" : "first panel closed (Unlock press not recorded)";
  const parts = [
    r.filled ? "filled" : "NOT filled",
    ...(r.unlockPressed ? [`Unlock -> first panel closed ${span(r.unlockPressed, r.firstPanelClosed)}`] : []),
    `${from} -> vault unlocked ${span(base, r.unlocked)}`,
    `vault unlocked -> panel back ${r.reopened ? `${span(r.unlocked || base, r.reopened)} (${r.reopenKind})` : "not seen"}`,
    `${from} -> filled ${span(base, r.filledAt)} (includes your pick in the panel)`,
  ];
  if (!r.reopened && r.filled) parts.push("no second panel: the first one stayed open or the fill came another way");
  return parts.join("; ");
}

// ---- the steps both scripts run ----

/** What the scripts need of a browser for the steps below. */
export interface StepDriver {
  probe: RoundTripProbe;
  /** Brings the test tab to the front of its window, empties its fields and closes leftover decision pages. */
  prepare(): Promise<void>;
  /** Asks the background (from the control tab) for every login, the request "Show all my logins" makes. */
  loadLogins(): Promise<string>;
  /** The vault status and the number of logins for the test page (the popup's `state`). */
  state(): Promise<{ vault: string; logins: number }>;
}

/** Waits until the vault is unlocked and has a login for the test page (the person adds one in the app if needed). */
export async function ensureLogin(person: Person, driver: StepDriver, site: string): Promise<boolean> {
  await driver.prepare();
  const ok = async () => {
    const s = await driver.state();
    return s.vault === "unlocked" && s.logins > 0;
  };
  if (await ok().catch(() => false)) {
    console.log(`the vault is unlocked and has a login for ${site}`);
    return true;
  }
  say(`In the app: unlock the vault if it is locked, and make sure it has a login whose website is http://localhost (any user name and password; ${site} is the test page).`);
  const done = await person.until("waiting for the app to report a login for the test page...", ok);
  if (!done) console.log(`  still no login for ${site} (state: ${JSON.stringify(await driver.state().catch(() => "?"))}); the round trips need one`);
  return done;
}

/** "Show all my logins" (the app must confirm that it is you), with the window hidden or open. */
export async function confirmTrial(person: Person, log: PromptsLog, driver: StepDriver, step: string, hidden: boolean) {
  say(
    hidden
      ? `${step}: "Show all my logins" with the LocalPass window HIDDEN. Close the LocalPass window to the tray (x) now.`
      : `${step}: "Show all my logins" with the LocalPass window OPEN. Open the LocalPass window now (the vault may stay unlocked).`,
  );
  await person.enter("Then come back here.");
  console.log("In 5 s the script asks the app for all logins, which needs \"Confirm it is you\": enter the master password in the app (or Cancel). Look at where the prompt appears.");
  await sleep(5000);
  log.mark();
  const started = Date.now();
  const reply = await driver.loadLogins().catch((e) => `failed: ${String(e)}`);
  const line = `reply after ${seconds(Date.now() - started)}: ${reply}`;
  console.log(`  ${line}   (ok = confirmed, code "denied" = refused or cancelled)`);
  person.note(step, line);
  const lines = log.since();
  log.print(step, lines);
  person.note(step, `prompts.log: ${lines.length ? lines.join(" / ") : "no new lines"}`);
  await askAboutFocus(person, step, { hidden, unlock: false });
}

/** The Unlock round trip from the decision panel, with the app hidden in the tray or open behind the browser. */
export async function unlockRoundTrip(person: Person, log: PromptsLog, driver: StepDriver, step: string, hidden: boolean) {
  say(
    hidden
      ? `${step}: Unlock round trip, app HIDDEN in the tray. In the app: lock the vault (Lock), then close the window to the tray (x).`
      : `${step}: Unlock round trip, app OPEN BEHIND the browser. In the app: lock the vault (Lock) and leave its window open; then click the browser window so it is in front of LocalPass.`,
  );
  const locked = await person.until("waiting for the app to report a locked vault...", async () => (await driver.state()).vault === "locked");
  if (!locked) console.log("  the vault does not look locked: the round trip below may not ask for Unlock");
  await person.enter(hidden ? "Is the LocalPass window closed to the tray?" : "Is the LocalPass window open, and the browser in front of it?");
  await driver.prepare();
  log.mark();
  say(
    "Now in the browser: click the user name field of the test page, click the LocalPass badge in it, press Unlock in the panel, and type the master password in the app. " +
      "When the LocalPass panel shows the login again, pick it. (Enter here stops waiting.)",
  );
  const r = await watchRoundTrip(driver.probe, person.pressed());
  person.cancelLine();
  const line = roundTripLine(r);
  console.log(`  result: ${line}`);
  person.note(step, line);
  const lines = log.since();
  log.print(step, lines);
  person.note(step, `prompts.log: ${lines.length ? lines.join(" / ") : "no new lines"}`);
  await askAboutFocus(person, step, { hidden, unlock: true });
}
