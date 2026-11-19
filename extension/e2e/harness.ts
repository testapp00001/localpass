// Shared set-up for the browser end-to-end tests: the dev host (a broker with a throwaway vault),
// the native-messaging registration for this run, and Chrome or Edge with the extension.
//
// Windows: the browser finds the test helper through an HKCU registry entry that is set for the run
// and restored afterwards. Linux: Chrome and the Chromium browsers read
// `<user data dir>/NativeMessagingHosts/<host>.json` (chrome/common/chrome_paths.cc,
// DIR_USER_NATIVE_MESSAGING = DIR_USER_DATA + "NativeMessagingHosts"; `--user-data-dir` sets
// DIR_USER_DATA), so the manifest goes into the throwaway profile, and the browser and the dev host
// get a throwaway HOME, XDG folders and runtime folder (the broker's socket). Nothing is written to
// ~/.config/<browser>, and `--password-store=basic` keeps Chrome away from the login keyring.

import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { testProfile } from "./test-profile";
import puppeteer, { type Browser, type ElementHandle, type Page, type Target } from "puppeteer-core";

const root = resolve(import.meta.dir, "../..");
export const extensionDir = resolve(import.meta.dir, "../dist-e2e");
const windows = process.platform === "win32";
const exeSuffix = windows ? ".exe" : "";
/** Cargo's output folder: `$CARGO_TARGET_DIR` when set (the binaries just built there), else `<repo>/target`. */
const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(root, "target");
export const devHostExe = join(targetDir, `debug/examples/dev_host${exeSuffix}`);
export const helperExe = join(targetDir, `debug/examples/nmhost${exeSuffix}`);
export const EXTENSION_ID = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const HOST_NAME = "app.localpass.host";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

export const browserKind = (process.env.BROWSER ?? "chrome").toLowerCase();
const windowsBrowsers = {
  chrome: { exe: "C:/Program Files/Google/Chrome/Application/chrome.exe", registry: `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}` },
  edge: { exe: "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", registry: `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${HOST_NAME}` },
} as const;
/** Linux: where each browser's program usually is (first one found wins), and its real config folder (only looked at). */
const linuxBrowsers: Record<string, { exes: string[]; config: string }> = {
  chrome: { exes: ["/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/opt/google/chrome/chrome"], config: "google-chrome" },
  chromium: { exes: ["/usr/bin/chromium-browser", "/usr/bin/chromium"], config: "chromium" },
  edge: { exes: ["/usr/bin/microsoft-edge", "/usr/bin/microsoft-edge-stable"], config: "microsoft-edge" },
  brave: { exes: ["/usr/bin/brave-browser", "/usr/bin/brave"], config: "BraveSoftware/Brave-Browser" },
  vivaldi: { exes: ["/usr/bin/vivaldi", "/usr/bin/vivaldi-stable"], config: "vivaldi" },
};

/** Where Chrome and Edge are installed on Windows (the Windows provider test uses these). */
export const browserPaths = { chrome: windowsBrowsers.chrome.exe, edge: windowsBrowsers.edge.exe };

const windowsBrowser = windows ? windowsBrowsers[browserKind as keyof typeof windowsBrowsers] : undefined;
const linuxBrowser = windows ? undefined : linuxBrowsers[browserKind];
if (windows ? !windowsBrowser : !linuxBrowser) throw new Error(`unknown BROWSER ${browserKind}`);
/** The browser program the tests use (BROWSER=chrome|edge on Windows; chrome|chromium|edge|brave|vivaldi on Linux). */
export const browserExe = windowsBrowser?.exe ?? linuxBrowser!.exes.find((p) => existsSync(p)) ?? linuxBrowser!.exes[0]!;

/** Linux: the real config folder's native-messaging entries (names and times), to prove the run left them alone. */
function realConfigStamp(): string {
  if (!linuxBrowser) return "";
  const config = process.env.XDG_CONFIG_HOME?.startsWith("/") ? process.env.XDG_CONFIG_HOME : join(homedir(), ".config");
  const dir = join(config, linuxBrowser.config, "NativeMessagingHosts");
  if (!existsSync(dir)) return "(absent)";
  return readdirSync(dir)
    .map((name) => `${name}:${statSync(join(dir, name)).mtimeMs}`)
    .sort()
    .join("\n");
}

/**
 * One run's throwaway places: the work folder (profile, manifest, Linux home and runtime folder),
 * the environment of the dev host and the browser, the helper's registration and its undo.
 */
export class BrowserSetup {
  readonly workDir = mkdtempSync(join(tmpdir(), "lp-e2e-"));
  readonly profile: string;
  readonly userDataDir = join(this.workDir, "profile");
  private readonly home = join(this.workDir, "home");
  private readonly runtime = join(this.workDir, "run");
  private previousRegistration: string | null = null;
  private registered = false;
  private readonly configBefore = realConfigStamp();

  /**
   * `assisted` (Linux, `e2e/assisted.ts`): the real app, started by the person with `LOCALPASS_PROFILE=<profile>`, instead
   * of the dev host. The helper is the app's own executable, the browser is headed and keeps the real runtime folder (the
   * app's socket is there) and display; HOME, the XDG folders and the user data dir stay throwaway.
   */
  constructor(private readonly assisted?: { appExe: string; profile: string }) {
    this.profile = testProfile(assisted?.profile ?? `e2e${process.pid}`);
    if (!windows) {
      mkdirSync(this.runtime, { mode: 0o700 });
      mkdirSync(this.home);
    }
  }

  /** The dev host's environment: on Linux its socket goes into the throwaway runtime folder. */
  hostEnv(): Record<string, string> {
    return { ...(process.env as Record<string, string>), LOCALPASS_PROFILE: this.profile, ...(windows ? {} : { XDG_RUNTIME_DIR: this.runtime }) };
  }

  /** The browser's environment (and so the helper's, which the browser starts). */
  browserEnv(): Record<string, string> {
    if (windows) return { ...(process.env as Record<string, string>), LOCALPASS_PROFILE: this.profile };
    const env: Record<string, string> = {
      ...(process.env as Record<string, string>),
      HOME: this.home,
      XDG_CONFIG_HOME: join(this.home, ".config"),
      XDG_DATA_HOME: join(this.home, ".local/share"),
      XDG_CACHE_HOME: join(this.home, ".cache"),
      XDG_RUNTIME_DIR: this.assisted ? (process.env.XDG_RUNTIME_DIR ?? this.runtime) : this.runtime,
      LOCALPASS_PROFILE: this.profile,
    };
    // A headed run (LP_CHROME_HEADED=1) still finds the Wayland display in the real runtime folder.
    const wayland = process.env.WAYLAND_DISPLAY;
    if (wayland && process.env.XDG_RUNTIME_DIR && !wayland.startsWith("/")) env.WAYLAND_DISPLAY = join(process.env.XDG_RUNTIME_DIR, wayland);
    return env;
  }

  private reg(...args: string[]) {
    return Bun.spawnSync(["reg", ...args], { stdout: "pipe", stderr: "pipe" });
  }

  /** Points the browser at the test helper for the length of the run. */
  registerHelper() {
    const manifest = JSON.stringify({ name: HOST_NAME, description: "LocalPass e2e", path: this.assisted?.appExe ?? helperExe, type: "stdio", allowed_origins: [`chrome-extension://${EXTENSION_ID}/`] });
    if (!windows) {
      const dir = join(this.userDataDir, "NativeMessagingHosts");
      mkdirSync(dir, { recursive: true });
      writeFileSync(join(dir, `${HOST_NAME}.json`), manifest);
      return;
    }
    const query = this.reg("query", windowsBrowser!.registry, "/ve");
    if (query.exitCode === 0) this.previousRegistration = /REG_SZ\s+(.*)\r?$/m.exec(query.stdout.toString())?.[1]?.trim() ?? null;
    const path = join(this.workDir, `${HOST_NAME}.json`);
    writeFileSync(path, manifest);
    process.on("exit", this.restoreRegistry);
    this.registered = true;
    this.reg("add", windowsBrowser!.registry, "/ve", "/d", path, "/f");
  }

  private restoreRegistry = () => {
    if (!windows || !this.registered) return;
    if (this.previousRegistration) this.reg("add", windowsBrowser!.registry, "/ve", "/d", this.previousRegistration, "/f");
    else this.reg("delete", windowsBrowser!.registry, "/f");
  };

  /** Starts the browser with the extension in the throwaway profile. */
  launch(): Promise<Browser> {
    const headed = !!this.assisted || process.env.LP_CHROME_HEADED === "1";
    return puppeteer.launch({
      executablePath: browserExe,
      headless: !headed,
      // A person uses an assisted window: it keeps its own size instead of puppeteer's fixed 800x600 page.
      ...(this.assisted ? { defaultViewport: null } : {}),
      userDataDir: this.userDataDir,
      // Recent Chrome ignores --load-extension; puppeteer installs unpacked extensions over CDP.
      enableExtensions: [extensionDir],
      args: ["--no-first-run", "--no-default-browser-check", ...(windows ? [] : ["--password-store=basic"])],
      env: this.browserEnv(),
    });
  }

  /** Undoes the registration and deletes the work folder. Returns a failure text when the real config was touched (Linux). */
  cleanup(): string | null {
    this.restoreRegistry();
    try {
      rmSync(this.workDir, { recursive: true, force: true });
    } catch {}
    const after = realConfigStamp();
    return after === this.configBefore ? null : `the real ${linuxBrowser?.config} NativeMessagingHosts folder changed: ${this.configBefore} -> ${after}`;
  }
}

// ---- LocalPass's decision panel (Chrome's toolbar popup) ----
//
// Every pick, save and passkey confirmation is made in the extension's decision page in the toolbar panel
// (8.3b). Puppeteer reaches an open action popup as a page target, also in headless Chrome, and its
// clicks there are trusted (measured with Chrome 157, 8.3b). A headed window launched by the harness
// never gets the focus on GNOME, and Chrome then refuses `action.openPopup` ("Cannot show popup for an
// inactive window"), so these tests run headless.

const isPanel = (t: Target) => t.url().startsWith(`chrome-extension://${EXTENSION_ID}/decide.html?`) && t.url().includes("presentation=panel");

/** Finds the panels that open, one after the other. */
export class Panels {
  private seen = new Set<Target>();
  constructor(private browser: Browser) {
    for (const t of browser.targets()) if (isPanel(t)) this.seen.add(t);
  }

  /** The next panel that opens (by itself, or by the badge), once it shows more than its loading lines. */
  async next(timeoutMs = 10000): Promise<Page> {
    const target = await this.browser.waitForTarget((t) => isPanel(t) && !this.seen.has(t), { timeout: timeoutMs });
    this.seen.add(target);
    const page = (await target.asPage())!;
    await page.waitForFunction(() => {
      const text = document.querySelector("main")?.textContent ?? "";
      return text !== "" && !/^(Connecting…|Looking for your)/.test(text);
    }, { timeout: timeoutMs });
    return page;
  }

  /** Whether a new panel opens within `ms` (it should not). */
  async opens(ms: number): Promise<boolean> {
    return this.next(ms).then(
      () => true,
      () => false,
    );
  }

  /** How many panels are open now. */
  open(): number {
    return this.browser.targets().filter(isPanel).length;
  }
}

/** The text the panel shows (its header names the site). */
export async function panelText(panel: Page): Promise<string> {
  return (await panel.evaluate(() => document.body.innerText)).replace(/\s+/g, " ").trim();
}

/** Waits for the panel's text to match. */
export async function panelShows(panel: Page, pattern: RegExp, timeoutMs = 10000): Promise<string> {
  const start = Date.now();
  let text = "";
  while (Date.now() - start < timeoutMs) {
    text = await panelText(panel).catch(() => text);
    if (pattern.test(text)) return text;
    await sleep(100);
  }
  return text;
}

/**
 * Clicks the panel's button or row whose text starts with `label`, once it is not held (acting controls are
 * held for 0.6 s after each view of a panel the page caused, as a person would wait; `aria-disabled` says so).
 * The click is a real (trusted) one.
 */
export async function panelClick(panel: Page, label: string, timeoutMs = 8000): Promise<void> {
  const handle = await panel.waitForFunction(
    (text) => [...document.querySelectorAll("button")].find((b) => (b.textContent ?? "").startsWith(text) && !b.disabled && b.getAttribute("aria-disabled") !== "true") ?? null,
    { timeout: timeoutMs },
    label,
  );
  await (handle.asElement() as ElementHandle<HTMLButtonElement>).click();
}

/** Whether the panel page has closed. */
export async function panelClosed(panel: Page, timeoutMs = 5000): Promise<boolean> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    if (panel.isClosed()) return true;
    const alive = await panel.evaluate(() => true).catch(() => false);
    if (!alive) return true;
    await sleep(100);
  }
  return false;
}

/** The toolbar badge of the active tab ("1" while a save or passkey request waits there). */
export async function toolbarBadge(browser: Browser): Promise<string> {
  const target = await browser.waitForTarget((t) => t.type() === "service_worker" && t.url().includes(EXTENSION_ID), { timeout: 10000 });
  const worker = (await target.worker())!;
  return worker.evaluate(async () => {
    const [tab] = await chrome.tabs.query({ active: true, lastFocusedWindow: true });
    return tab?.id === undefined ? "" : chrome.action.getBadgeText({ tabId: tab.id });
  });
}

/** What LocalPass drew into the page: the elements in its shadow root (open in the e2e build), as tag.class. */
export async function inPageControls(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    [...document.documentElement.children]
      .filter((e) => e.shadowRoot)
      .flatMap((host) => [...host.shadowRoot!.children].map((e) => `${e.tagName.toLowerCase()}${e.className ? `.${String(e.className).replace(/ /g, ".")}` : ""}`))
      .filter((name) => name !== "style"),
  );
}

export interface VaultItem {
  id: string;
  kind: string;
  title: string;
  username: string;
  password: string;
  urls: string[];
  passkey: { rpId: string; rpName: string; userName: string } | null;
}

export class Harness {
  failures = 0;
  chrome!: Browser;
  private lines: string[] = [];
  private host!: ReturnType<typeof Bun.spawn>;
  private setup = new BrowserSetup();

  check(name: string, ok: boolean, detail = "") {
    console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
    if (!ok) this.failures++;
  }

  async start(logins: string[] = []) {
    this.host = Bun.spawn([devHostExe, ...logins.flatMap((l) => ["--login", l])], {
      env: this.setup.hostEnv(),
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
    this.setup.registerHelper();
    this.chrome = await this.setup.launch();
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

  hostCommand(command: string) {
    (this.host.stdin as { write(s: string): void; flush(): void }).write(`${command}\n`);
    (this.host.stdin as { flush(): void }).flush();
  }

  vault(): VaultItem[] {
    const last = [...this.lines].reverse().find((l) => l.startsWith("VAULT "));
    return last ? JSON.parse(last.slice(6)) : [];
  }

  /** Waits (the dev host prints the vault a moment after a change) for the vault to satisfy `ok`. */
  async waitForVault(ok: (items: VaultItem[]) => boolean, timeoutMs = 5000): Promise<VaultItem[]> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
      const items = this.vault();
      if (ok(items)) return items;
      await sleep(100);
    }
    return this.vault();
  }

  /** Connects the browser to the app through the extension's popup. */
  async pair() {
    const popup = await this.chrome.newPage();
    await popup.goto(`chrome-extension://${EXTENSION_ID}/popup.html`);
    await popup.waitForFunction(() => document.body.innerText.includes("Connect to LocalPass"), { timeout: 20000 });
    await panelClick(popup, "Connect"); // the popup in a tab is held for 0.6 s, like a panel the page caused (8.3c)
    await this.waitForHost((l) => l.startsWith("PAIRED"));
    await popup.waitForFunction(() => !document.body.innerText.includes("Connect to LocalPass"), { timeout: 10000 });
    await popup.close();
  }

  async stop() {
    // Closing the browser can hang (seen after the service-worker test); do not wait for it forever.
    await Promise.race([this.chrome?.close().catch(() => {}), sleep(8000)]);
    this.chrome?.process()?.kill();
    this.hostCommand("quit");
    this.host.kill();
    const touched = this.setup.cleanup();
    this.check("the real browser config was not touched", touched === null, touched ?? "");
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
