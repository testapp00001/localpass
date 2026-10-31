// Builds the extension into dist/: bundles each script and copies the static files.
// Content scripts must be classic scripts, so they are bundled as IIFEs; the service worker and
// the popup are modules.
import { cp, mkdir, rm } from "node:fs/promises";
import { firefoxManifest } from "./manifest-firefox";

// `--e2e` builds into dist-e2e with the on-page interface inspectable by tests. Never ship that.
// `--firefox` builds the Firefox add-on into dist-firefox (dist-firefox-e2e together with `--e2e`).
const e2e = process.argv.includes("--e2e");
const firefox = process.argv.includes("--firefox");
const out = `dist${firefox ? "-firefox" : ""}${e2e ? "-e2e" : ""}`;
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const bundles: { entry: string; name: string; format: "esm" | "iife" }[] = [
  { entry: "src/background/index.ts", name: "background", format: "esm" },
  { entry: "src/content/index.ts", name: "content", format: "iife" },
  { entry: "src/content/inject.ts", name: "inject", format: "iife" },
  { entry: "src/popup/index.ts", name: "popup", format: "esm" },
];

// The decision window exists only in the Firefox add-on (the Chrome extension decides in the page).
if (firefox) bundles.push({ entry: "src/decide/index.ts", name: "decide", format: "esm" });

let failed = false;
for (const { entry, name, format } of bundles) {
  const result = await Bun.build({
    entrypoints: [entry],
    outdir: out,
    naming: `${name}.js`,
    target: "browser",
    format,
    minify: false,
    sourcemap: "none",
    define: { __E2E__: String(e2e), __FIREFOX__: String(firefox) },
  });
  if (!result.success) {
    failed = true;
    for (const log of result.logs) console.error(log);
  }
}
if (failed) process.exit(1);

if (firefox) {
  // The add-on id is the one the app's native-messaging registration allows (permanent once signed).
  const rust = await Bun.file("../crates/lp-broker/src/nmhost.rs").text();
  const id = /FIREFOX_EXTENSION_ID: &str = "([^"]+)"/.exec(rust)?.[1];
  if (!id) throw new Error("FIREFOX_EXTENSION_ID not found in crates/lp-broker/src/nmhost.rs");
  const manifest = firefoxManifest(await Bun.file("manifest.json").json(), id);
  await Bun.write(`${out}/manifest.json`, `${JSON.stringify(manifest, null, 2)}\n`);
} else {
  await cp("manifest.json", `${out}/manifest.json`);
}
await cp("public", out, { recursive: true });
if (firefox) await cp("public-firefox", out, { recursive: true });
console.log(`built ${out}/`);
