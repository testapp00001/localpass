// Cuts a release. Run it from anywhere inside the repository.
//
//   bun scripts/release.ts 0.3.0          bump every version, regenerate CHANGELOG.md, commit, tag v0.3.0
//   bun scripts/release.ts --check 0.3.0  (CI) fail unless every version file says 0.3.0
//
// It never pushes. Pushing the tag is what starts .github/workflows/release.yml, so that step stays yours:
//   git push origin main v0.3.0
//
// Plain X.Y.Z only: the browser extension's manifest does not allow a pre-release suffix, and the Android
// versionCode (major*1000000 + minor*1000 + patch) is derived from it.
import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const repo = resolve(import.meta.dir, "..");
const VERSION = /^(\d+)\.(\d+)\.(\d+)$/;

// Each file holds its version in one line; `pattern` captures the text before the number.
const files = [
  { path: "Cargo.toml", pattern: /(\[workspace\.package\][^[]*?\nversion = ")[^"]+(")/ },
  { path: "app/package.json", pattern: /(\n  "version": ")[^"]+(")/ },
  { path: "app/src-tauri/tauri.conf.json", pattern: /(\n  "version": ")[^"]+(")/ },
  { path: "extension/package.json", pattern: /(\n  "version": ")[^"]+(")/ },
  { path: "extension/manifest.json", pattern: /(\n  "version": ")[^"]+(")/ },
];

function read(path: string): string {
  return readFileSync(join(repo, path), "utf8");
}

function currentVersion(file: { path: string; pattern: RegExp }): string | undefined {
  const m = read(file.path).match(file.pattern);
  return m ? read(file.path).slice(m.index! + m[1].length).match(/^[^"]+/)![0] : undefined;
}

function run(cmd: string, args: string[], capture = false): string {
  const r = spawnSync(cmd, args, { cwd: repo, encoding: "utf8", stdio: capture ? ["ignore", "pipe", "inherit"] : "inherit" });
  if (r.status !== 0) {
    console.error(`release: ${cmd} ${args.join(" ")} failed (${r.status ?? r.error})`);
    process.exit(1);
  }
  return (r.stdout ?? "").trim();
}

function fail(message: string): never {
  console.error(`release: ${message}`);
  process.exit(1);
}

const args = process.argv.slice(2);
const check = args[0] === "--check";
const version = check ? args[1] : args[0];
if (!version || !VERSION.test(version)) fail("usage: bun scripts/release.ts [--check] <X.Y.Z>");

if (check) {
  const wrong = files.filter((f) => currentVersion(f) !== version).map((f) => `${f.path} says ${currentVersion(f) ?? "nothing"}`);
  if (wrong.length) fail(`tag ${version} does not match: ${wrong.join("; ")}`);
  console.log(`release: all ${files.length} version files say ${version}`);
  process.exit(0);
}

if (run("git", ["status", "--porcelain"], true)) fail("the working tree is not clean; commit or stash first");
const branch = run("git", ["rev-parse", "--abbrev-ref", "HEAD"], true);
if (branch !== "main") fail(`releases are cut from main, you are on ${branch}`);
const tag = `v${version}`;
if (run("git", ["tag", "--list", tag], true)) fail(`${tag} already exists`);

// Going backwards is almost always a typo. The current version is allowed (first release of it).
const parts = (v: string) => v.split(".").map(Number);
const current = currentVersion(files[0])!;
const firstDifference = parts(version).map((n, i) => n - parts(current)[i]).find((d) => d !== 0) ?? 0;
if (firstDifference < 0) fail(`${version} is older than ${current}`);

for (const f of files) {
  const text = read(f.path);
  if (!f.pattern.test(text)) fail(`could not find the version in ${f.path}`);
  writeFileSync(join(repo, f.path), text.replace(f.pattern, `$1${version}$2`));
}
// Cargo.lock lists every workspace crate with the version. Only workspace members are touched.
run("cargo", ["update", "--workspace"]);
run("bun", ["scripts/release.ts", "--check", version]);

run("bunx", ["git-cliff", "--config", "cliff.toml", "--tag", tag, "--output", "CHANGELOG.md"]);

run("git", ["add", "-A", ...files.map((f) => f.path), "Cargo.lock", "CHANGELOG.md"]);
run("git", ["commit", "-m", `chore(release): ${tag}`]);
run("git", ["tag", "-a", tag, "-m", `LocalPass ${version}`]);

console.log(`\nrelease: ${tag} is committed and tagged. Read CHANGELOG.md, then publish with:\n  git push origin main ${tag}`);
