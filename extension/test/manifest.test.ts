// The manifest: no permission the code does not use (V16), and the extension id (which the
// native-messaging registration and the app's EXTENSION_ID depend on) cannot change by accident.

import { describe, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { FIREFOX_MIN_VERSION, firefoxManifest } from "../manifest-firefox";

const root = join(import.meta.dir, "..");
const manifest = JSON.parse(readFileSync(join(root, "manifest.json"), "utf8")) as Record<string, unknown>;

/** Chrome's extension id: the first 16 bytes of SHA-256 of the DER public key, as letters a-p. */
function extensionId(key: string): string {
  const digest = createHash("sha256").update(Buffer.from(key, "base64")).digest().subarray(0, 16);
  return [...digest].map((b) => String.fromCharCode(97 + (b >> 4)) + String.fromCharCode(97 + (b & 15))).join("");
}

function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? sources(join(dir, e.name)) : e.name.endsWith(".ts") ? [join(dir, e.name)] : []));
}

describe("permissions", () => {
  test("exactly the permissions the code uses", () => {
    expect([...(manifest.permissions as string[])].sort()).toEqual(["activeTab", "nativeMessaging", "privacy", "storage"]);
  });

  test("scripting is not declared and nothing calls chrome.scripting", () => {
    expect(manifest.permissions).not.toContain("scripting");
    for (const file of sources(join(root, "src"))) expect(readFileSync(file, "utf8"), file).not.toMatch(/chrome\.scripting|\bscripting\./);
  });

  test("no host permissions, no externally_connectable, no web_accessible_resources", () => {
    for (const key of ["host_permissions", "optional_host_permissions", "externally_connectable", "web_accessible_resources"]) expect(manifest[key]).toBeUndefined();
  });
});

describe("identity", () => {
  test("the key still gives the id the app and the browser registration expect", () => {
    expect(extensionId(manifest.key as string)).toBe("khlgdnhadbjfghhonaohnhdmedgnenlc");
  });

  test("the version equals the package version", () => {
    expect(manifest.version).toBe((JSON.parse(readFileSync(join(root, "package.json"), "utf8")) as { version: string }).version);
  });
});

describe("the Firefox manifest", () => {
  // The id is read from the Rust constant the app's native-messaging registration allows, so the two cannot drift.
  const rust = readFileSync(join(root, "../crates/lp-broker/src/nmhost.rs"), "utf8");
  const geckoId = /FIREFOX_EXTENSION_ID: &str = "([^"]+)"/.exec(rust)?.[1] ?? "";
  const ff = firefoxManifest(manifest, geckoId) as Record<string, any>;

  test("its add-on id is the app's FIREFOX_EXTENSION_ID", () => {
    expect(geckoId).toMatch(/^[a-z0-9._-]+@[a-z0-9.-]+$/);
    expect(ff.browser_specific_settings.gecko.id).toBe(geckoId);
    expect(ff.browser_specific_settings.gecko.strict_min_version).toBe(FIREFOX_MIN_VERSION);
  });

  test("an event page instead of a service worker, no Chrome key or minimum version", () => {
    expect(ff.background).toEqual({ scripts: ["background.js"], type: "module" });
    for (const key of ["key", "minimum_chrome_version"]) expect(ff[key]).toBeUndefined();
  });

  test("the same permissions, scripts and (absent) host permissions as the Chrome manifest", () => {
    expect(ff.permissions).toEqual(manifest.permissions);
    expect(ff.content_scripts).toEqual(manifest.content_scripts);
    for (const key of ["host_permissions", "optional_host_permissions", "externally_connectable", "web_accessible_resources"]) expect(ff[key]).toBeUndefined();
  });

  test("it says LocalPass collects no data", () => {
    expect(ff.browser_specific_settings.gecko.data_collection_permissions).toEqual({ required: ["none"] });
  });

  test("the Chrome manifest is untouched by the derivation", () => {
    expect((manifest.background as { service_worker: string }).service_worker).toBe("background.js");
    expect(manifest.key).toBeTruthy();
  });
});
