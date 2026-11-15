// The Firefox manifest is derived from manifest.json, so the two cannot drift: Firefox MV3 has no
// service worker (an event page is declared with `background.scripts`), takes its add-on id from
// `browser_specific_settings.gecko` instead of a `key`, and does not know `minimum_chrome_version`.
// Chrome needs `tabs` to read the address of the tab a decision is for (its content-script matches do
// not grant that; measured in Chrome 157, where it adds no install warning); Firefox already reads it
// through the content scripts' host permissions, and `tabs` would add a prompt there, so it is left out.

/** First Firefox with MAIN-world content scripts (inject.js needs them). */
export const FIREFOX_MIN_VERSION = "128.0";

type Manifest = Record<string, unknown>;

export function firefoxManifest(chrome: Manifest, geckoId: string): Manifest {
  const { key: _key, minimum_chrome_version: _min, background, permissions, ...rest } = chrome;
  const bg = background as { service_worker: string; type?: string };
  return {
    ...rest,
    permissions: (permissions as string[]).filter((p) => p !== "tabs"),
    background: { scripts: [bg.service_worker], type: bg.type },
    browser_specific_settings: {
      // LocalPass collects nothing: no server, no telemetry (AMO asks add-ons to say so).
      gecko: { id: geckoId, strict_min_version: FIREFOX_MIN_VERSION, data_collection_permissions: { required: ["none"] } },
    },
  };
}
