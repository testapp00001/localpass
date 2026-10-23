// V9 in the page's own world: a WebAuthn call over a size limit (or the wrong shape) is handed to
// the browser's own implementation and never reaches the extension.

import { beforeAll, beforeEach, describe, expect, test } from "bun:test";
import type { Json } from "./chrome";

const posted: Json[] = [];

beforeAll(async () => {
  (window as unknown as { happyDOM: { setURL(u: string): void } }).happyDOM.setURL("https://login.example.com/signin");
  class Container {
    create() {
      return Promise.resolve("native-create");
    }
    get() {
      return Promise.resolve("native-get");
    }
  }
  const g = globalThis as unknown as Record<string, unknown>;
  g.CredentialsContainer = Container;
  g.PublicKeyCredential = class {};
  g.AuthenticatorAttestationResponse = class {};
  g.AuthenticatorAssertionResponse = class {};
  (window as unknown as Record<string, unknown>).PublicKeyCredential = g.PublicKeyCredential;
  Object.defineProperty(navigator, "credentials", { value: new Container(), configurable: true });
  window.addEventListener("message", (e) => {
    if ((e.data as Json | undefined)?.__lp === "req") posted.push(e.data as Json);
  });
  await import("../src/content/inject");
});

beforeEach(() => {
  posted.length = 0;
});

const bytes = (n: number) => new Uint8Array(n).fill(1);
const creation = (extra: Json = {}, user: Json = {}) => ({
  publicKey: {
    rp: { id: "example.com", name: "Example" },
    user: { id: bytes(16), name: "alice", displayName: "Alice", ...user },
    challenge: bytes(32),
    pubKeyCredParams: [{ type: "public-key", alg: -7 }],
    ...extra,
  },
});
const request = (extra: Json = {}) => ({ publicKey: { rpId: "example.com", challenge: bytes(32), ...extra } });
const create = (options: unknown) => (navigator.credentials as unknown as { create(o: unknown): Promise<unknown> }).create(options);
const get = (options: unknown) => (navigator.credentials as unknown as { get(o: unknown): Promise<unknown> }).get(options);
const ids = (n: number, size = 8) => Array.from({ length: n }, () => ({ type: "public-key", id: bytes(size) }));

describe("create", () => {
  test("a call within the limits is sent to the extension", async () => {
    void create(creation({ excludeCredentials: ids(256, 1023) }, { name: "x".repeat(256) }));
    await new Promise((r) => setTimeout(r, 20));
    expect(posted).toHaveLength(1);
    expect(posted[0]).toMatchObject({ op: "create" });
  });

  test.each([
    ["257 excluded credentials", creation({ excludeCredentials: ids(257) })],
    ["a 1024-byte credential id", creation({ excludeCredentials: ids(1, 1024) })],
    ["a 257-character user name", creation({}, { name: "x".repeat(257) })],
    ["a 257-character display name", creation({}, { displayName: "x".repeat(257) })],
    ["a 257-character rp name", creation({ rp: { id: "example.com", name: "x".repeat(257) } })],
    ["a 65-byte user handle", creation({}, { id: bytes(65) })],
    ["a huge challenge", creation({ challenge: bytes(5000) })],
    ["no user", creation({ user: undefined })],
    ["an rp id for another site", creation({ rp: { id: "bank.com", name: "x" } })],
  ])("%s goes to the browser's own flow", async (_name, options) => {
    expect(await create(options)).toBe("native-create");
    expect(posted).toEqual([]);
  });
});

describe("get", () => {
  test("a call within the limits is sent to the extension", async () => {
    void get(request({ allowCredentials: ids(256, 1023) }));
    await new Promise((r) => setTimeout(r, 20));
    expect(posted).toHaveLength(1);
    expect(posted[0]).toMatchObject({ op: "get" });
  });

  test.each([
    ["257 allowed credentials", request({ allowCredentials: ids(257) })],
    ["a 1024-byte credential id", request({ allowCredentials: ids(1, 1024) })],
    ["a huge challenge", request({ challenge: bytes(5000) })],
    ["an rp id for another site", request({ rpId: "bank.com" })],
    ["an rp id that is an IP-address trick", request({ rpId: "ample.com" })],
  ])("%s goes to the browser's own flow", async (_name, options) => {
    expect(await get(options)).toBe("native-get");
    expect(posted).toEqual([]);
  });
});
