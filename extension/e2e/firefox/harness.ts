// Shared set-up for the Firefox end-to-end tests (Linux): the dev host (a broker with a throwaway vault),
// a throwaway HOME with the native-messaging registration for this run, and a headless Firefox driven over
// WebDriver BiDi (no geckodriver). Nothing here touches the real ~/.mozilla, keyring or runtime folder.
//
//   cargo build -p lp-broker --examples && cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts

import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dir, "../../..");
export const extensionDir = resolve(import.meta.dir, "../../dist-firefox-e2e");
export const shippedDir = resolve(import.meta.dir, "../../dist-firefox");
const devHostExe = join(root, "target/debug/examples/dev_host");
const helperExe = join(root, "target/debug/examples/nmhost");
export const GECKO_ID = "localpass@localpass.local";
const HOST_NAME = "app.localpass.host";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export interface VaultItem {
  id: string;
  kind: string;
  title: string;
  username: string;
  password: string;
  urls: string[];
  passkey: { rpId: string; rpName: string; userName: string } | null;
}

/** A page-level context, as BiDi reports it. */
export interface Ctx {
  context: string;
  url: string;
  children?: Ctx[];
}

/** Modification times under the real ~/.mozilla, to prove a run never touched it. */
function realMozillaStamp(): string {
  const dir = join(homedir(), ".mozilla");
  if (!existsSync(dir)) return "(absent)";
  const out: string[] = [];
  const walk = (d: string) => {
    for (const name of readdirSync(d)) {
      const p = join(d, name);
      const s = statSync(p);
      out.push(`${p}:${s.mtimeMs}`);
      if (s.isDirectory()) walk(p);
    }
  };
  try {
    walk(dir);
  } catch {
    // a Firefox of the user's may be writing while we look: only the file list matters then
  }
  return out.filter((l) => /native-messaging-hosts/.test(l)).sort().join("\n");
}

export class FirefoxHarness {
  failures = 0;
  readonly workDir = mkdtempSync(join(tmpdir(), "lp-ff-"));
  private readonly home = join(this.workDir, "home");
  private readonly runtime = join(this.workDir, "run");
  private readonly profile: string;
  private lines: string[] = [];
  private host!: ReturnType<typeof Bun.spawn>;
  private firefox!: ReturnType<typeof Bun.spawn>;
  private ws!: WebSocket;
  private id = 0;
  private pending = new Map<number, (m: any) => void>();
  private mozillaBefore = realMozillaStamp();

  /**
   * `assisted`: the real app (started by the person, `LOCALPASS_PROFILE=<profile>`) instead of the dev host; Firefox is headed,
   * keeps the real runtime folder and display, and finds the app's own executable as the native-messaging helper.
   */
  constructor(private readonly assisted?: { appExe: string; profile: string }) {
    this.profile = assisted?.profile ?? `e2e${process.pid}`;
  }
  /** Events from BiDi, newest last (browsingContext.contextCreated / contextDestroyed). */
  readonly events: { method: string; params: any }[] = [];

  check(name: string, ok: boolean, detail = "") {
    console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
    if (!ok) this.failures++;
  }

  // ---- the dev host ----

  /** Firefox reads this file from $HOME (the throwaway one) and starts the helper with [exe, <this file>, <add-on id>]. */
  registerHelper(path: string) {
    mkdirSync(join(this.home, ".mozilla/native-messaging-hosts"), { recursive: true });
    writeFileSync(
      join(this.home, ".mozilla/native-messaging-hosts", `${HOST_NAME}.json`),
      JSON.stringify({ name: HOST_NAME, description: "LocalPass e2e", path, type: "stdio", allowed_extensions: [GECKO_ID] }),
    );
  }

  async startHost(logins: string[] = []) {
    mkdirSync(this.runtime, { mode: 0o700 });
    this.host = Bun.spawn([devHostExe, ...logins.flatMap((l) => ["--login", l])], {
      env: { ...process.env, LOCALPASS_PROFILE: this.profile, XDG_RUNTIME_DIR: this.runtime },
      stdin: "pipe",
      stdout: "pipe",
      stderr: "inherit",
    });
    void (async () => {
      const decoder = new TextDecoder();
      let buffer = "";
      for await (const chunk of this.host.stdout as ReadableStream<Uint8Array>) {
        buffer += decoder.decode(chunk);
        let index;
        while ((index = buffer.indexOf("\n")) >= 0) {
          this.lines.push(buffer.slice(0, index).trim());
          buffer = buffer.slice(index + 1);
        }
      }
    })();
    await this.waitForHost((l) => l.startsWith("READY"));
    this.registerHelper(helperExe);
  }

  async waitForHost(predicate: (line: string) => boolean, timeoutMs = 15000): Promise<string> {
    const start = Date.now();
    let seen = 0;
    while (Date.now() - start < timeoutMs) {
      for (; seen < this.lines.length; seen++) if (predicate(this.lines[seen]!)) return this.lines[seen]!;
      await sleep(100);
    }
    throw new Error(`the dev host never printed the expected line; saw: ${this.lines.slice(-5).join(" | ")}`);
  }

  /** Lines the host printed so far that match. */
  hostLines(predicate: (line: string) => boolean): string[] {
    return this.lines.filter(predicate);
  }

  hostCommand(command: string) {
    (this.host.stdin as { write(s: string): void; flush(): void }).write(`${command}\n`);
    (this.host.stdin as { flush(): void }).flush();
  }

  vault(): VaultItem[] {
    const last = [...this.lines].reverse().find((l) => l.startsWith("VAULT "));
    return last ? JSON.parse(last.slice(6)) : [];
  }

  async waitForVault(ok: (items: VaultItem[]) => boolean, timeoutMs = 5000): Promise<VaultItem[]> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const items = this.vault();
      if (ok(items)) return items;
      await sleep(100);
    }
    return this.vault();
  }

  // ---- Firefox ----

  async startFirefox(extDir: string) {
    const profile = join(this.workDir, "profile");
    mkdirSync(profile);
    writeFileSync(
      join(profile, "user.js"),
      [
        'user_pref("browser.shell.checkDefaultBrowser", false);',
        'user_pref("datareporting.policy.dataSubmissionEnabled", false);',
        'user_pref("app.update.enabled", false);',
        'user_pref("extensions.autoDisableScopes", 0);',
        'user_pref("browser.startup.homepage_override.mstone", "ignore");',
      ].join("\n"),
    );
    const port = 9400 + Math.floor(Math.random() * 400);
    this.firefox = Bun.spawn(
      ["firefox", ...(this.assisted ? [] : ["--headless"]), "--no-remote", "--profile", profile, "--remote-debugging-port", String(port), "--remote-allow-system-access", "about:blank"],
      {
        env: {
          ...process.env,
          HOME: this.home,
          XDG_CONFIG_HOME: join(this.home, ".config"),
          XDG_DATA_HOME: join(this.home, ".local/share"),
          XDG_CACHE_HOME: join(this.home, ".cache"),
          ...(this.assisted ? {} : { XDG_RUNTIME_DIR: this.runtime, MOZ_HEADLESS: "1" }),
          LOCALPASS_PROFILE: this.profile,
        },
        stdout: "ignore",
        stderr: "ignore",
      },
    );
    for (let i = 0; i < 60 && !this.ws; i++) {
      try {
        const s = new WebSocket(`ws://127.0.0.1:${port}/session`);
        await new Promise<void>((res, rej) => ((s.onopen = () => res()), (s.onerror = () => rej(new Error("x")))));
        this.ws = s;
      } catch {
        await sleep(500);
      }
    }
    if (!this.ws) throw new Error("no BiDi endpoint");
    this.ws.onmessage = (e) => {
      const m = JSON.parse(String(e.data));
      if (m.id && this.pending.has(m.id)) {
        this.pending.get(m.id)!(m);
        this.pending.delete(m.id);
      } else if (m.type === "event") this.events.push({ method: m.method, params: m.params });
    };
    await this.send("session.new", { capabilities: {} });
    await this.send("session.subscribe", { events: ["browsingContext.contextCreated", "browsingContext.contextDestroyed"] });
    await this.send("webExtension.install", { extensionData: { type: "path", path: extDir } });
    // The module background connects to the helper and finishes its set-up; the page-side scripts need it.
    await sleep(1500);
  }

  send(method: string, params: object = {}): Promise<any> {
    const n = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(n, (m) => (m.type === "error" ? reject(new Error(`${method}: ${m.error} ${m.message}`)) : resolve(m.result)));
      this.ws.send(JSON.stringify({ id: n, method, params }));
    });
  }

  /** Evaluates in a context (a page, or an extension page thanks to --remote-allow-system-access). */
  async ev<T = unknown>(context: string, expression: string): Promise<T> {
    const r = await this.send("script.evaluate", { expression, target: { context }, awaitPromise: true });
    if (r.type !== "success") throw new Error(`evaluate failed: ${JSON.stringify(r.exceptionDetails?.text)} in ${expression.slice(0, 80)}`);
    return (r.result.type === "undefined" || r.result.type === "null" ? undefined : r.result.value) as T;
  }

  /** The first tab: later tabs open in its window, because a focused popup (a decision window) has no tab strip. */
  private mainTab: string | undefined;

  async newTab(url: string): Promise<string> {
    const { context } = await this.send("browsingContext.create", { type: "tab", ...(this.mainTab ? { referenceContext: this.mainTab } : {}) });
    this.mainTab ??= context;
    await this.send("browsingContext.navigate", { context, url, wait: "complete" });
    await this.send("browsingContext.activate", { context }).catch(() => {});
    return context;
  }

  async closeContext(context: string) {
    await this.send("browsingContext.close", { context }).catch(() => {});
  }

  async contexts(): Promise<Ctx[]> {
    return (await this.send("browsingContext.getTree", {})).contexts;
  }

  /** Top-level contexts whose address contains `part` (the decision window is `decide.html`). */
  async find(part: string): Promise<Ctx[]> {
    return (await this.contexts()).filter((c) => c.url.includes(part));
  }

  async waitFor<T>(what: string, fn: () => Promise<T | undefined | false>, timeoutMs = 10000): Promise<T> {
    const start = Date.now();
    let last: unknown;
    while (Date.now() - start < timeoutMs) {
      try {
        const r = await fn();
        if (r) return r;
      } catch (e) {
        last = e;
      }
      await sleep(100);
    }
    throw new Error(`timed out waiting for ${what}${last ? `: ${String(last)}` : ""}`);
  }

  /** The next decision window (not one of `except`). */
  async decisionWindow(except: string[] = [], timeoutMs = 10000): Promise<string> {
    const c = await this.waitFor("a decision window", async () => (await this.find("decide.html")).find((w) => !except.includes(w.context)), timeoutMs);
    return c.context;
  }

  async windowGone(context: string, timeoutMs = 6000): Promise<boolean> {
    try {
      await this.waitFor("the window to close", async () => !(await this.contexts()).some((c) => c.context === context), timeoutMs);
      return true;
    } catch {
      return false;
    }
  }

  /** What a decision window shows now (header site, then the main text, rows and buttons separated by " | "). */
  async windowText(ctx: string): Promise<string> {
    return this.ev<string>(ctx, `(document.getElementById('site')?.textContent ?? '') + ' | ' + [...document.querySelectorAll('#main p, #main button')].map((e) => e.textContent).join(' | ')`);
  }

  async waitWindowText(ctx: string, pattern: RegExp, timeoutMs = 10000): Promise<string> {
    let text = "";
    try {
      await this.waitFor(`window text ${pattern}`, async () => ((text = await this.windowText(ctx)), pattern.test(text)), timeoutMs);
    } catch {
      // the caller's check reports the text
    }
    return text;
  }

  /** Clicks the window's button (or row) with this label once it is enabled (the window holds acting controls for 600 ms). */
  async clickWindow(ctx: string, label: string | RegExp, timeoutMs = 8000): Promise<void> {
    const source = label instanceof RegExp ? label.source : label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    await this.waitFor(`button "${label}"`, async () => {
      const r = await this.ev<string>(
        ctx,
        `(() => { const b = [...document.querySelectorAll('#main button')].find((x) => new RegExp(${JSON.stringify(label instanceof RegExp ? source : `^${source}`)}).test(x.firstChild?.textContent ?? x.textContent ?? '')); if (!b) return 'missing'; if (b.disabled) return 'disabled'; b.click(); return 'clicked'; })()`,
      );
      return r === "clicked";
    }, timeoutMs);
  }

  /** A click the browser reports as a real one (BiDi input), on the centre of an element found by `rectExpr`. */
  async trustedClick(context: string, rect: { x: number; y: number }) {
    await this.send("input.performActions", {
      context,
      actions: [
        {
          type: "pointer",
          id: "m",
          parameters: { pointerType: "mouse" },
          actions: [
            { type: "pointerMove", x: Math.round(rect.x), y: Math.round(rect.y), origin: "viewport" },
            { type: "pointerDown", button: 0 },
            { type: "pointerUp", button: 0 },
          ],
        },
      ],
    });
  }

  /** Trusted click on the page element matching a CSS selector (scrolled into view first). */
  async clickElement(context: string, selector: string) {
    const rect = await this.ev<string>(
      context,
      `(() => { const e = document.querySelector(${JSON.stringify(selector)}); if (!e) return ''; e.scrollIntoView({ block: 'center' }); const r = e.getBoundingClientRect(); return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2 }); })()`,
    );
    if (!rect) throw new Error(`no element ${selector}`);
    await this.trustedClick(context, JSON.parse(rect));
  }

  /** The LocalPass badge of the focused field (the e2e build keeps its shadow root open). */
  async badgeRect(context: string): Promise<{ x: number; y: number; ready: boolean } | undefined> {
    const r = await this.ev<string>(
      context,
      `(() => { const h = [...document.documentElement.children].find((e) => e.shadowRoot); const b = h && h.shadowRoot.querySelector('.badge'); if (!b) return ''; const r = b.getBoundingClientRect(); return JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2, ready: b.hasAttribute('data-ready') }); })()`,
    );
    return r ? JSON.parse(r) : undefined;
  }

  async stop() {
    try {
      this.ws?.close();
    } catch {}
    try {
      this.firefox?.kill();
      await Promise.race([this.firefox?.exited, sleep(5000)]);
    } catch {}
    try {
      if (this.host) {
        this.hostCommand("quit");
        this.host.kill();
      }
    } catch {}
    const after = realMozillaStamp();
    this.check("the real ~/.mozilla native-messaging-hosts folder was not touched", after === this.mozillaBefore);
    try {
      rmSync(this.workDir, { recursive: true, force: true });
    } catch {}
  }

  finish(successMessage: string): never {
    if (this.failures) {
      console.error(`${this.failures} check(s) failed`);
      process.exit(1);
    }
    console.log(successMessage);
    process.exit(0);
  }
}
