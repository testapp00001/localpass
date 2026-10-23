// Stages the Windows passkey provider for the installer: builds lp-winauth-provider (release) and copies
// the exe, the package manifest and the logos into app/windows-passkeys/, which the Windows-only Tauri
// config (src-tauri/tauri.windows.conf.json) ships as the "windows-passkeys" resource folder.
//
//   bun scripts/stage-provider.ts        (also the last step of `bun run build:all`, i.e. `tauri build`)
//
// It does nothing for other platforms, so an Android build started from Windows (TAURI_ENV_PLATFORM is
// "android") neither compiles nor ships it. The cargo target directory is the one cargo would use
// (CARGO_TARGET_DIR is inherited), so build with a separate one if the app is running from target\release.
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const repo = resolve(import.meta.dir, "..", "..");
const stage = resolve(import.meta.dir, "..", "windows-passkeys");
const EXE = "lp-winauth-provider.exe";
const KEEP = new Set(["README.txt"]);

const platform = process.env.TAURI_ENV_PLATFORM;
if (platform ? platform !== "windows" : process.platform !== "win32") {
  console.log(`stage-provider: skipped (${platform ?? process.platform}); the Windows passkey provider is Windows only`);
  process.exit(0);
}
const triple = process.env.TAURI_ENV_TARGET_TRIPLE;
if (triple && triple !== "x86_64-pc-windows-msvc") {
  console.warn(`stage-provider: skipped (${triple}); the provider package is x64 only`);
  process.exit(0);
}

function run(cmd: string, args: string[], capture = false): string {
  const r = spawnSync(cmd, args, { cwd: repo, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit" });
  if (r.status !== 0) {
    console.error(`stage-provider: ${cmd} ${args.join(" ")} failed (${r.status ?? r.error})`);
    process.exit(1);
  }
  return r.stdout ?? "";
}

run("cargo", ["build", "--release", "-p", "lp-winauth", "--bin", "lp-winauth-provider", ...(triple ? ["--target", triple] : [])]);
const targetDir = JSON.parse(run("cargo", ["metadata", "--no-deps", "--format-version", "1"], true)).target_directory as string;
const exe = join(targetDir, ...(triple ? [triple] : []), "release", EXE);
if (!existsSync(exe)) {
  console.error(`stage-provider: ${exe} was not built`);
  process.exit(1);
}

// The manifest is shipped as it is; its class id must be the one compiled into the exe.
const manifestSource = join(repo, "crates", "lp-winauth", "package", "AppxManifest.xml");
const manifest = readFileSync(manifestSource, "utf8");
const clsid = run(exe, ["clsid"], true).trim().replace(/^\{|\}$/g, "");
if (!/^[0-9A-Fa-f]{8}(-[0-9A-Fa-f]{4}){3}-[0-9A-Fa-f]{12}$/.test(clsid) || !manifest.includes(`<com:Class Id="${clsid}"`)) {
  console.error(`stage-provider: the manifest does not name the exe's class id ${clsid}`);
  process.exit(1);
}

mkdirSync(stage, { recursive: true });
for (const name of readdirSync(stage)) if (!KEEP.has(name)) rmSync(join(stage, name), { recursive: true, force: true });
const assets = join(stage, "Assets");
mkdirSync(assets);
copyFileSync(exe, join(stage, EXE));
copyFileSync(manifestSource, join(stage, "AppxManifest.xml"));
for (const logo of ["StoreLogo", "Square44x44Logo", "Square150x150Logo"]) {
  copyFileSync(join(repo, "app", "src-tauri", "icons", `${logo}.png`), join(assets, `${logo}.png`));
}
console.log(`stage-provider: ${stage} (${EXE} ${(statSync(join(stage, EXE)).size / 1048576).toFixed(1)} MB, class {${clsid}})`);
