// Builds the extension into dist/: bundles each script and copies the static files.
// Content scripts must be classic scripts, so they are bundled as IIFEs; the service worker and
// the popup are modules.
import { cp, mkdir, rm } from "node:fs/promises";

// `--e2e` builds into dist-e2e with the on-page interface inspectable by tests. Never ship that.
const e2e = process.argv.includes("--e2e");
const out = e2e ? "dist-e2e" : "dist";
await rm(out, { recursive: true, force: true });
await mkdir(out, { recursive: true });

const bundles: { entry: string; name: string; format: "esm" | "iife" }[] = [
  { entry: "src/background/index.ts", name: "background", format: "esm" },
  { entry: "src/content/index.ts", name: "content", format: "iife" },
  { entry: "src/content/inject.ts", name: "inject", format: "iife" },
  { entry: "src/popup/index.ts", name: "popup", format: "esm" },
];

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
    define: { __E2E__: String(e2e) },
  });
  if (!result.success) {
    failed = true;
    for (const log of result.logs) console.error(log);
  }
}
if (failed) process.exit(1);

await cp("manifest.json", `${out}/manifest.json`);
await cp("public", out, { recursive: true });
console.log(`built ${out}/`);
