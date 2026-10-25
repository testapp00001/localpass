// Runs the uninstaller's cleanup script (src-tauri/windows/uninstall-cleanup.ps1) under Windows PowerShell
// 5.1, the same host the NSIS hook uses, against throwaway folders and a throwaway registry root. Get-AppxPackage
// and Remove-AppxPackage are shadowed by functions (functions win over cmdlets), so the real LocalPass package,
// the real native-messaging keys and the real provider data are never touched: every LP_* variable is set by
// the test and the script has no defaults. Windows only.
import { afterAll, describe, expect, setDefaultTimeout, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

// Every test starts several PowerShell processes (about a second each, much more on a busy PC); Bun's default
// of 5 s per test made the suite fail whenever the machine was busy.
setDefaultTimeout(120_000);

const windows = process.platform === "win32";
const POWERSHELL = "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe";
const SCRIPT = resolve(import.meta.dir, "../src-tauri/windows/uninstall-cleanup.ps1");

const work = windows ? mkdtempSync(join(tmpdir(), "lp-uninst-")) : "";
const regRoot = `HKCU:\\Software\\LocalPassTest\\uninstall-${process.pid}-${Date.now()}`;
let counter = 0;

afterAll(() => {
  if (!windows) return;
  rmSync(work, { recursive: true, force: true });
  ps(`Remove-Item -LiteralPath '${regRoot}' -Recurse -Force -ErrorAction SilentlyContinue`);
  // the shared parent goes too once nothing else of ours is in it
  ps(`$p = 'HKCU:\\Software\\LocalPassTest'; if ((Test-Path -LiteralPath $p) -and -not (Get-ChildItem -LiteralPath $p)) { Remove-Item -LiteralPath $p -Force }`);
});

// PowerShell occasionally fails to start for a moment on a busy PC; setup and read-back commands, whose result
// must be 0 and has a known shape, are retried. The script under test is never retried.
function retried<T extends { code: number; out: string }>(run: () => T, ok: (r: T) => boolean): T {
  let last = run();
  for (let i = 0; i < 3 && !ok(last); i++) last = run();
  return last;
}

function ps(command: string, env: Record<string, string> = {}) {
  const run = Bun.spawnSync(
    [POWERSHELL, "-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", command],
    { env: { ...process.env, ...env } },
  );
  return { code: run.exitCode, out: run.stdout.toString().trim(), err: run.stderr.toString().trim() };
}

// The script under test, run with the two Appx cmdlets replaced. T_PKG_DIR is where the fake package says it
// is installed (unset = no package); T_LOG gets one line per call, so the order can be checked.
function cleanup(env: Record<string, string>) {
  const shadow =
    "function Get-AppxPackage { param($Name) if ($env:T_PKG_DIR) { [pscustomobject]@{ InstallLocation = $env:T_PKG_DIR } } }; " +
    "function Remove-AppxPackage { param([Parameter(ValueFromPipeline)]$p) process { Add-Content -LiteralPath $env:T_LOG 'remove' } }; ";
  return ps(`${shadow}& '${SCRIPT}'; exit $LASTEXITCODE`, env);
}

// A fresh sandbox: install folder, a fake alias that logs "unregister" and exits with `aliasExit`, a log file.
function sandbox(aliasExit: number | null = 0) {
  const dir = join(work, `case${counter++}`);
  const install = join(dir, "Install");
  const passkeys = join(install, "windows-passkeys");
  mkdirSync(passkeys, { recursive: true });
  const log = join(dir, "log.txt");
  writeFileSync(log, "");
  const alias = join(dir, "alias.cmd");
  if (aliasExit !== null) {
    writeFileSync(alias, `@echo off\r\necho unregister>>"${log}"\r\nexit /b ${aliasExit}\r\n`);
  }
  const appPathKey = `${regRoot}\\case${counter}\\Provider`;
  retried(
    () => ps(`New-Item -Path '${appPathKey}' -Force | Out-Null; New-ItemProperty -Path '${appPathKey}' -Name AppPath -Value 'C:\\x\\localpass.exe' | Out-Null`),
    (r) => r.code === 0,
  );
  const env = {
    LP_DIR: passkeys,
    LP_ALIAS: alias,
    LP_EXE: join(install, "localpass.exe"),
    LP_NM_KEYS: "",
    LP_APPPATH_KEY: appPathKey,
    LP_PROVIDER_DATA: join(dir, "LocalPass", "provider"),
    LP_DELETE_DATA: "0",
    T_LOG: log,
  };
  const appPath = () =>
    retried(() => ps(`(Get-ItemProperty -LiteralPath '${appPathKey}' -ErrorAction SilentlyContinue).AppPath`), (r) => r.code === 0).out;
  const lines = () => readFileSync(log, "utf8").split(/\r?\n/).filter(Boolean);
  return { dir, install, passkeys, env, appPath, lines };
}

const hostKey = (name: string, manifest: string) => {
  const key = `${regRoot}\\${name}`;
  retried(() => ps(`New-Item -Path '${key}' -Value '${manifest}' -Force | Out-Null`), (r) => r.code === 0);
  return key;
};
const exists = (key: string) =>
  retried(() => ps(`Test-Path -LiteralPath '${key}'`), (r) => r.out === "True" || r.out === "False").out === "True";
const manifestFor = (dir: string, name: string, exe: string) => {
  const file = join(dir, `${name}.json`);
  writeFileSync(file, JSON.stringify({ name: "app.localpass.host", path: exe, type: "stdio" }));
  return file;
};

describe.skipIf(!windows)("uninstall cleanup script", () => {
  test("removes the browser keys that name this install's exe and nothing else", () => {
    const s = sandbox();
    const mine = hostKey(`nm-mine-${counter}`, manifestFor(s.dir, "mine", s.env.LP_EXE.toUpperCase()));
    const dev = hostKey(`nm-dev-${counter}`, manifestFor(s.dir, "dev", join(s.dir, "dev", "localpass.exe")));
    const noManifest = hostKey(`nm-gone-${counter}`, join(s.dir, "missing.json"));
    const broken = join(s.dir, "broken.json");
    writeFileSync(broken, "{ not json");
    const unreadable = hostKey(`nm-broken-${counter}`, broken);
    const run = cleanup({ ...s.env, LP_NM_KEYS: [mine, dev, noManifest, unreadable, `${regRoot}\\absent`].join(";") });
    expect(run.code).toBe(0);
    expect(exists(mine)).toBe(false);
    expect(exists(dev)).toBe(true);
    expect(exists(noManifest)).toBe(true);
    expect(exists(unreadable)).toBe(true);
  });

  test("leaves a package registered from another folder alone", () => {
    const s = sandbox();
    const run = cleanup({ ...s.env, T_PKG_DIR: join(s.dir, "Other", "windows-passkeys") });
    expect(run.code).toBe(0);
    expect(s.lines()).toEqual([]);
    expect(s.appPath()).toBe("C:\\x\\localpass.exe");
  });

  test("with no package there is nothing to do", () => {
    const s = sandbox();
    expect(cleanup(s.env).code).toBe(0);
    expect(s.lines()).toEqual([]);
  });

  test("unregisters first, then removes the package, then forgets the app path", () => {
    const s = sandbox(0);
    // the same folder spelled with another case and a trailing separator still counts
    const run = cleanup({ ...s.env, T_PKG_DIR: s.passkeys.toUpperCase() + "\\" });
    expect(run.code).toBe(0);
    expect(s.lines()).toEqual(["unregister", "remove"]);
    expect(s.appPath()).toBe("");
  });

  test("a failed unregister keeps the package and tells the installer (exit 3)", () => {
    const s = sandbox(1);
    const run = cleanup({ ...s.env, T_PKG_DIR: s.passkeys });
    expect(run.code).toBe(3);
    expect(s.lines()).toEqual(["unregister"]);
    expect(s.appPath()).toBe("C:\\x\\localpass.exe");
  });

  test("a missing alias keeps the package too", () => {
    const s = sandbox(null);
    const run = cleanup({ ...s.env, T_PKG_DIR: s.passkeys });
    expect(run.code).toBe(3);
    expect(s.lines()).toEqual([]);
  });

  test("stops provider processes of this folder only", async () => {
    const s = sandbox(0);
    const other = join(s.dir, "Other");
    mkdirSync(other, { recursive: true });
    const start = (folder: string) => {
      const exe = join(folder, "lp-winauth-provider.exe");
      writeFileSync(exe, readFileSync("C:\\Windows\\System32\\PING.EXE"));
      return Bun.spawn([exe, "-n", "60", "127.0.0.1"], { stdout: "ignore", stderr: "ignore" });
    };
    const mine = start(s.passkeys);
    const foreign = start(other);
    try {
      await Bun.sleep(300);
      expect(cleanup({ ...s.env, T_PKG_DIR: s.passkeys }).code).toBe(0);
      await Bun.sleep(300);
      expect(mine.killed || mine.exitCode !== null).toBe(true);
      expect(foreign.exitCode).toBeNull();
    } finally {
      mine.kill();
      foreign.kill();
    }
  });

  test("the provider's data goes only when asked and only a folder called provider", () => {
    const s = sandbox();
    const data = s.env.LP_PROVIDER_DATA;
    mkdirSync(data, { recursive: true });
    writeFileSync(join(data, "client.bin"), "secret");
    cleanup({ ...s.env, LP_DELETE_DATA: "0" });
    expect(existsSync(join(data, "client.bin"))).toBe(true);

    const wrong = join(s.dir, "NotTheProviderFolder");
    mkdirSync(wrong, { recursive: true });
    writeFileSync(join(wrong, "keep.txt"), "x");
    cleanup({ ...s.env, LP_DELETE_DATA: "1", LP_PROVIDER_DATA: wrong });
    expect(existsSync(join(wrong, "keep.txt"))).toBe(true);

    cleanup({ ...s.env, LP_DELETE_DATA: "1" });
    expect(existsSync(data)).toBe(false);
  });

  test("the data is removed even when the package could not be unregistered", () => {
    const s = sandbox(1);
    mkdirSync(s.env.LP_PROVIDER_DATA, { recursive: true });
    const run = cleanup({ ...s.env, T_PKG_DIR: s.passkeys, LP_DELETE_DATA: "1" });
    expect(run.code).toBe(3);
    expect(existsSync(s.env.LP_PROVIDER_DATA)).toBe(false);
  });
});
