// Shared set-up for the Firefox end-to-end tests (Linux): the dev host (a broker with a throwaway vault),
// a throwaway HOME with the native-messaging registration for this run, and a headless Firefox driven over
// WebDriver BiDi (no geckodriver). Nothing here touches the real ~/.mozilla, keyring or runtime folder.
//
//   cargo build -p lp-broker --examples && cd extension && bun run build --firefox --e2e && bun run e2e/firefox/run.ts

import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dir, "../../..");
export const extensionDir = resolve(import.meta.dir, "../../dist-firefox-e2e");
export const shippedDir = resolve(import.meta.dir, "../../dist-firefox");
/** Cargo's output folder: `$CARGO_TARGET_DIR` when set (the binaries just built there), else `<repo>/target`. */
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "target");
const devHostExe = join(targetDir, "debug/examples/dev_host");
const helperExe = join(targetDir, "debug/examples/nmhost");
export const GECKO_ID = "localpass@localpass.local";
const HOST_NAME = "app.localpass.host";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The keys `pressKey` can send: their DOM key code, and WebDriver's code point for them. */
const KEYS = {
  Escape: { keyCode: 27, webdriver: "\uE00C" },
  Enter: { keyCode: 13, webdriver: "\uE007" },
  ArrowUp: { keyCode: 38, webdriver: "\uE013" },
  ArrowDown: { keyCode: 40, webdriver: "\uE015" },
} as const;

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
    const headed = !!this.assisted || process.env.LP_FF_HEADED === "1";
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
        // Allow only this run's test actor module through the content-process file sandbox.
        `user_pref("security.sandbox.content.read_path_whitelist", ${JSON.stringify(join(this.workDir, "PanelActor.sys.mjs"))});`,
      ].join("\n"),
    );
    const port = 9400 + Math.floor(Math.random() * 400);
    this.firefox = Bun.spawn(
      ["firefox", ...(headed ? [] : ["--headless"]), "--no-remote", "--profile", profile, "--remote-debugging-port", String(port), "--remote-allow-system-access", "about:blank"],
      {
        env: {
          ...process.env,
          HOME: this.home,
          XDG_CONFIG_HOME: join(this.home, ".config"),
          XDG_DATA_HOME: join(this.home, ".local/share"),
          XDG_CACHE_HOME: join(this.home, ".cache"),
          ...(this.assisted ? {} : { XDG_RUNTIME_DIR: this.runtime, ...(headed ? { MOZ_ENABLE_WAYLAND: "0" } : { MOZ_HEADLESS: "1" }) }),
          LOCALPASS_PROFILE: this.profile,
          // Firefox 159 refuses scripts in extension pages unless this variable is set (the flag alone worked in 158).
          MOZ_REMOTE_ALLOW_SYSTEM_ACCESS: "1",
        },
        stdout: "ignore",
        stderr: process.env.LP_FF_DEBUG ? "inherit" : "ignore",
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
    // Firefox does not list toolbar popups in BiDi's content tree. A test-only window actor
    // lets us inspect their real documents through the browser chrome scope instead.
    const actor = join(this.workDir, "PanelActor.sys.mjs");
    writeFileSync(actor, `export class LocalPassPanelTestChild extends JSWindowActorChild {
      async receiveMessage({data}) {
        const sandbox = Cu.Sandbox(Services.scriptSecurityManager.getSystemPrincipal(), {
          sandboxPrototype: this.contentWindow.wrappedJSObject, wantXrays: false
        });
        return await Cu.evalInSandbox(data, sandbox);
      }
    }`);
    const trees = await this.send("browsingContext.getTree", { "moz:scope": "chrome" });
    await this.ev(trees.contexts[0].context, `(() => {
      Services.io.getProtocolHandler("resource").QueryInterface(Ci.nsIResProtocolHandler)
        .setSubstitution("localpass-test", Services.io.newURI(${JSON.stringify(pathToFileURL(`${this.workDir}/`).href)}));
      ChromeUtils.registerWindowActor("LocalPassPanelTest", {
        child: { esModuleURI: "resource://localpass-test/PanelActor.sys.mjs" }, allFrames: true
      });
    })()`);
    // The module background connects to the helper and finishes its set-up; the page-side scripts need it.
    await sleep(1500);
  }

  send(method: string, params: object = {}): Promise<any> {
    const n = ++this.id;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(n);
        reject(new Error(`timed out: ${method} ${JSON.stringify(params).slice(0, 180)}`));
      }, 30000);
      this.pending.set(n, (m) => {
        clearTimeout(timer);
        if (m.type === "error") reject(new Error(`${method}: ${m.error} ${m.message}`));
        else resolve(m.result);
      });
      this.ws.send(JSON.stringify({ id: n, method, params }));
    });
  }

  /** Evaluates in a context (a page, or an extension page thanks to --remote-allow-system-access). */
  async ev<T = unknown>(context: string, expression: string): Promise<T> {
    if (context.startsWith("panel:")) {
      const [, owner, id] = context.split(":");
      return this.ev<T>(owner!, `BrowsingContext.get(${id}).currentWindowGlobal.getActor("LocalPassPanelTest").sendQuery("eval", ${JSON.stringify(expression)})`);
    }
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
    if (context.startsWith("panel:")) {
      const [, owner, id] = context.split(":");
      await this.ev(owner!, `[...document.querySelectorAll('browser')].find(b => b.browsingContext?.id === ${id})?.closest('panel')?.hidePopup()`);
      return;
    }
    await this.send("browsingContext.close", { context }).catch(() => {});
  }

  async contexts(): Promise<Ctx[]> {
    const contexts: Ctx[] = (await this.send("browsingContext.getTree", {})).contexts;
    const windows = (await this.send("browsingContext.getTree", { "moz:scope": "chrome" })).contexts;
    for (const win of windows) {
      const panels = JSON.parse(await this.ev<string>(win.context, `JSON.stringify([...document.querySelectorAll('browser')]
        .filter(b => b.currentURI?.spec.includes('decide.html?') && b.currentURI.spec.includes('presentation=panel'))
        .map(b => ({ context: 'panel:' + ${JSON.stringify(win.context)} + ':' + b.browsingContext.id, url: b.currentURI.spec })))`));
      contexts.push(...panels);
    }
    return contexts;
  }

  /**
   * The normal toolbar popup (`popup.html` in a toolbar panel, not the popup page in a tab; also `popup.html?from=panel`,
   * the popup a decision panel turned into), as a `panel:` context, if open.
   */
  async toolbarPopup(): Promise<string | undefined> {
    const windows = (await this.send("browsingContext.getTree", { "moz:scope": "chrome" })).contexts;
    for (const win of windows) {
      const found = await this.ev<string>(win.context, `(() => { const b = [...document.querySelectorAll('browser')]
        .find(b => /\\/popup\\.html(\\?from=panel)?$/.test(b.currentURI?.spec ?? '') && b.closest('panel')); return b ? String(b.browsingContext.id) : ''; })()`);
      if (found) return `panel:${win.context}:${found}`;
    }
    return undefined;
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

  /** Clicks the window's button (or row) with this label once it is not held (a window the page caused holds acting controls for 600 ms). */
  async clickWindow(ctx: string, label: string | RegExp, timeoutMs = 8000): Promise<void> {
    const source = label instanceof RegExp ? label.source : label.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    await this.waitFor(`button "${label}"`, async () => {
      const r = await this.ev<string>(
        ctx,
        `(() => { const b = [...document.querySelectorAll('#main button')].find((x) => new RegExp(${JSON.stringify(label instanceof RegExp ? source : `^${source}`)}).test(x.firstChild?.textContent ?? x.textContent ?? '')); if (!b) return 'missing'; if (b.disabled || b.getAttribute('aria-disabled') === 'true') return 'held'; b.click(); return 'clicked'; })()`,
      );
      return r === "clicked";
    }, timeoutMs);
  }

  /**
   * A key press the browser reports as a real one in this context. BiDi key input (WebDriver code points: Escape is
   * U+E00C) does not reach the page in a toolbar panel (measured: not even a plain letter arrives; Firefox's own popup
   * handling closes the panel on that Esc), so in a panel the key is sent through the panel page's own text input
   * processor, as Firefox's tests do: a trusted keydown and keyup in that page, which the page may handle first.
   */
  async pressKey(context: string, key: keyof typeof KEYS) {
    if (context.startsWith("panel:")) {
      const init = JSON.stringify(JSON.stringify({ key, code: key, keyCode: KEYS[key].keyCode }));
      const sent = await this.ev<string>(
        context,
        `(() => { const tip = Components.classes["@mozilla.org/text-input-processor;1"].createInstance(Components.interfaces.nsITextInputProcessor);
          if (!tip.beginInputTransactionForTests(window)) return 'no transaction';
          const event = new window.KeyboardEvent("", window.JSON.parse(${init}));
          tip.keydown(event, tip.KEY_NON_PRINTABLE_KEY); tip.keyup(event, tip.KEY_NON_PRINTABLE_KEY); return 'sent'; })()`,
      ).catch((e) => String(e)); // the panel may close while the key is handled
      if (sent !== "sent" && !/closed|destroyed|unknown error|no such/i.test(sent)) throw new Error(`could not press ${key} in the panel: ${sent}`);
      return;
    }
    const value = KEYS[key].webdriver;
    await this.send("input.performActions", {
      context: context.startsWith("panel:") ? context.split(":")[1] : context,
      actions: [{ type: "key", id: "k", actions: [{ type: "keyDown", value }, { type: "keyUp", value }] }],
    });
  }

  /** A click the browser reports as a real one (BiDi input), on the centre of an element found by `rectExpr`. */
  async trustedClick(context: string, rect: { x: number; y: number }) {
    // A real click cannot target a background tab. BiDi itself does not activate it.
    await this.send("browsingContext.activate", { context }).catch(() => {});
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

  /**
   * A click on the LocalPass toolbar button of the first browser window, as a person makes it: the button is pinned
   * to the toolbar (MV3 add-ons start in the Extensions menu) and gets a real mouse click (BiDi input in the browser
   * window's own chrome context), so Firefox opens the tab's toolbar popup.
   */
  async clickToolbarButton(): Promise<string> {
    const chromeContext = (await this.send("browsingContext.getTree", { "moz:scope": "chrome" })).contexts[0].context;
    const widget = JSON.stringify(`${GECKO_ID.toLowerCase().replace(/[^a-z0-9_-]/g, "_")}-browser-action`);
    const rect = await this.ev<string>(
      chromeContext,
      `(() => {
        if (!document.getElementById(${widget})?.closest('#nav-bar')) CustomizableUI.addWidgetToArea(${widget}, CustomizableUI.AREA_NAVBAR);
        const node = document.getElementById(${widget});
        if (!node) return '';
        const button = node.querySelector('.unified-extensions-item-action-button') ?? node.querySelector('toolbarbutton') ?? node;
        const r = button.getBoundingClientRect();
        return r.width ? JSON.stringify({ x: r.x + r.width / 2, y: r.y + r.height / 2 }) : '';
      })()`,
    );
    if (!rect) return "no button";
    const { x, y } = JSON.parse(rect);
    await this.send("input.performActions", {
      context: chromeContext,
      actions: [
        {
          type: "pointer",
          id: "toolbar",
          parameters: { pointerType: "mouse" },
          actions: [
            { type: "pointerMove", x: Math.round(x), y: Math.round(y), origin: "viewport" },
            { type: "pointerDown", button: 0 },
            { type: "pointerUp", button: 0 },
          ],
        },
      ],
    });
    return "clicked";
  }

  /**
   * Stops the add-on's background page, as Firefox does when it suspends an idle event page (and as about:debugging's
   * "Terminate background script" does). Its native-messaging port closes with it; the next event starts it again.
   */
  async terminateBackground(): Promise<string> {
    const chromeContext = (await this.send("browsingContext.getTree", { "moz:scope": "chrome" })).contexts[0].context;
    return this.ev<string>(
      chromeContext,
      `(async () => { const e = WebExtensionPolicy.getByID(${JSON.stringify(GECKO_ID)}).extension; await e.terminateBackground({ disableResetIdleForTest: true }); return String(e.backgroundState); })()`,
    );
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
