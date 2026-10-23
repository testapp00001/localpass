// Shared set-up for the browser end-to-end tests: the dev host (a broker with a throwaway vault),
// the native-messaging registration for this run, and Chrome or Edge with the extension.

import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import puppeteer, { type Browser } from "puppeteer-core";

const root = resolve(import.meta.dir, "../..");
export const extensionDir = resolve(import.meta.dir, "../dist-e2e");
const devHostExe = join(root, "target/debug/examples/dev_host.exe");
const helperExe = join(root, "target/debug/examples/nmhost.exe");
export const EXTENSION_ID = "khlgdnhadbjfghhonaohnhdmedgnenlc";
const HOST_NAME = "app.localpass.host";

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

const browserKind = (process.env.BROWSER ?? "chrome").toLowerCase();
const browsers = {
  chrome: { exe: "C:/Program Files/Google/Chrome/Application/chrome.exe", registry: `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}` },
  edge: { exe: "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe", registry: `HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\${HOST_NAME}` },
} as const;
const browser = browsers[browserKind as keyof typeof browsers];
if (!browser) throw new Error(`unknown BROWSER ${browserKind}`);
/** Where Chrome and Edge are installed, and which one the tests use (BROWSER=chrome|edge). */
export const browserPaths = { chrome: browsers.chrome.exe, edge: browsers.edge.exe };

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
  private workDir = mkdtempSync(join(tmpdir(), "lp-e2e-"));
  private previousRegistration: string | null = null;
  private profile = `e2e${process.pid}`;

  check(name: string, ok: boolean, detail = "") {
    console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`);
    if (!ok) this.failures++;
  }

  private reg(...args: string[]) {
    return Bun.spawnSync(["reg", ...args], { stdout: "pipe", stderr: "pipe" });
  }

  private restoreRegistry = () => {
    if (this.previousRegistration) this.reg("add", browser.registry, "/ve", "/d", this.previousRegistration, "/f");
    else this.reg("delete", browser.registry, "/f");
  };

  async start(logins: string[] = []) {
    this.host = Bun.spawn([devHostExe, ...logins.flatMap((l) => ["--login", l])], {
      env: { ...process.env, LOCALPASS_PROFILE: this.profile },
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

    // Point the browser at the test helper for the length of the run.
    const query = this.reg("query", browser.registry, "/ve");
    if (query.exitCode === 0) this.previousRegistration = /REG_SZ\s+(.*)\r?$/m.exec(query.stdout.toString())?.[1]?.trim() ?? null;
    const manifest = join(this.workDir, `${HOST_NAME}.json`);
    writeFileSync(manifest, JSON.stringify({ name: HOST_NAME, description: "LocalPass e2e", path: helperExe, type: "stdio", allowed_origins: [`chrome-extension://${EXTENSION_ID}/`] }));
    process.on("exit", this.restoreRegistry);
    this.reg("add", browser.registry, "/ve", "/d", manifest, "/f");

    this.chrome = await puppeteer.launch({
      executablePath: browser.exe,
      headless: true,
      userDataDir: join(this.workDir, "profile"),
      enableExtensions: [extensionDir],
      args: ["--no-first-run", "--no-default-browser-check"],
      env: { ...process.env, LOCALPASS_PROFILE: this.profile } as Record<string, string>,
    });
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
    await popup.evaluate(() => [...document.querySelectorAll("button")].find((b) => b.textContent === "Connect")!.click());
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
    this.restoreRegistry();
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
