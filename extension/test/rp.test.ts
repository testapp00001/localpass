// The local relying party (e2e/rp.ts) against a software authenticator: what it accepts, and that it
// notices each way an authenticator can get a WebAuthn response wrong (no UV, no backup flags, wrong
// origin, a counter that moves, a missing user handle, an unknown credential, a signature from
// another key). No browser, no Windows. The real thing is checked by e2e/windows-provider.ts.

import { GlobalRegistrator } from "@happy-dom/global-registrator";
import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createHash, generateKeyPairSync, randomBytes, sign } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { isoBase64URL, isoCBOR } from "@simplewebauthn/server/helpers";
import { DEFAULT_ASSISTED_PORT, startRelyingParty, type RelyingPartyOptions, type RpEvent } from "../e2e/rp";

// The DOM tests' preload replaces fetch and Response with happy-dom's, which the RP's server cannot use.
beforeAll(() => {
  if (GlobalRegistrator.isRegistered) GlobalRegistrator.unregister();
});
afterAll(() => {
  GlobalRegistrator.register();
});

const UP = 1;
const UV = 4;
const BE = 8;
const BS = 16;
const AT = 64;
const GOOD = UP | UV | BE | BS;

const sha256 = (data: Uint8Array | string) => createHash("sha256").update(data).digest();
const b64 = (data: Uint8Array) => isoBase64URL.fromBuffer(new Uint8Array(data));
const u32 = (n: number) => {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(n);
  return b;
};

/** An authenticator in software: one ES256 credential, with every flag and the counter under the test's control. */
function softwareAuthenticator(userHandle = "") {
  const { publicKey, privateKey } = generateKeyPairSync("ec", { namedCurve: "P-256" });
  const jwk = publicKey.export({ format: "jwk" });
  const cose = isoCBOR.encode(
    new Map<number, number | Uint8Array>([
      [1, 2],
      [3, -7],
      [-1, 1],
      [-2, isoBase64URL.toBuffer(jwk.x!)],
      [-3, isoBase64URL.toBuffer(jwk.y!)],
    ]),
  );
  const credentialId = randomBytes(16);
  const aaguid = Buffer.alloc(16, 0);
  const id = b64(credentialId);
  const base = { id, rawId: id, type: "public-key", clientExtensionResults: {} };
  return {
    id,
    userHandle,
    create(o: { challenge: string; origin: string; flags?: number; counter?: number; rpId?: string }) {
      const clientData = JSON.stringify({ type: "webauthn.create", challenge: o.challenge, origin: o.origin, crossOrigin: false });
      const idLength = Buffer.alloc(2);
      idLength.writeUInt16BE(credentialId.length);
      const authData = Buffer.concat([sha256(o.rpId ?? "localhost"), Buffer.from([(o.flags ?? GOOD) | AT]), u32(o.counter ?? 0), aaguid, idLength, credentialId, cose]);
      const attestationObject = isoCBOR.encode(new Map<string, unknown>([["fmt", "none"], ["attStmt", new Map()], ["authData", authData]]) as Parameters<typeof isoCBOR.encode>[0]);
      return { ...base, response: { clientDataJSON: b64(Buffer.from(clientData)), attestationObject: b64(attestationObject), transports: ["internal"] }, authenticatorAttachment: "platform" };
    },
    get(o: { challenge: string; origin: string; flags?: number; counter?: number; handle?: string | null; key?: typeof privateKey }) {
      const clientData = JSON.stringify({ type: "webauthn.get", challenge: o.challenge, origin: o.origin, crossOrigin: false });
      const authData = Buffer.concat([sha256("localhost"), Buffer.from([o.flags ?? GOOD]), u32(o.counter ?? 0)]);
      const signature = sign("sha256", Buffer.concat([authData, sha256(clientData)]), o.key ?? privateKey);
      const handle = o.handle === undefined ? userHandle : o.handle;
      return { ...base, response: { clientDataJSON: b64(Buffer.from(clientData)), authenticatorData: b64(authData), signature: b64(signature), ...(handle ? { userHandle: handle } : {}) } };
    },
  };
}

async function withRp<T>(options: RelyingPartyOptions, run: (rp: ReturnType<typeof startRelyingParty>, call: (path: string, body?: unknown) => Promise<any>) => Promise<T>): Promise<T> {
  const rp = startRelyingParty({ port: 0, userVerification: "required", assisted: true, counterZero: true, quiet: true, ...options });
  const call = async (path: string, body?: unknown) =>
    (await fetch(rp.origin + path, body === undefined && !path.startsWith("/api") ? undefined : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body ?? {}) })).json();
  try {
    return await run(rp, call);
  } finally {
    rp.stop();
  }
}

type Call = (path: string, body?: unknown) => Promise<any>;
type Authenticator = ReturnType<typeof softwareAuthenticator>;

/** Options request, authenticator response, verify request: the event the RP recorded. */
async function register(rp: { origin: string }, call: Call, a: Authenticator, tweak: Partial<Parameters<Authenticator["create"]>[0]> = {}, username = "alice"): Promise<{ verified: boolean; event?: RpEvent }> {
  const options = await call("/api/register/options", { username });
  return call("/api/register/verify", a.create({ challenge: options.challenge, origin: rp.origin, ...tweak }));
}

async function signIn(rp: { origin: string }, call: Call, a: Authenticator, tweak: Partial<Parameters<Authenticator["get"]>[0]> = {}): Promise<{ verified: boolean; event?: RpEvent }> {
  const options = await call("/api/auth/options", {});
  return call("/api/auth/verify", a.get({ challenge: options.challenge, origin: rp.origin, ...tweak }));
}

const failed = (event: RpEvent | undefined) => (event?.checks ?? []).filter((c) => !c.ok).map((c) => c.name);

describe("a correct authenticator", () => {
  test("registers, signs in twice, and everything is recorded", async () => {
    const dir = mkdtempSync(join(tmpdir(), "lp-rp-test-"));
    const logFile = join(dir, "rp.jsonl");
    try {
      await withRp({ logFile }, async (rp, call) => {
        const a = softwareAuthenticator(b64(Buffer.from("alice-1")));
        const registered = await register(rp, call, a);
        expect(registered.verified).toBe(true);
        expect(registered.event).toMatchObject({ kind: "register", userName: "alice", fmt: "none", counter: 0, flags: { up: true, uv: true, be: true, bs: true } });
        expect(registered.event!.aaguid).toBe("00000000-0000-0000-0000-000000000000");
        expect(failed(registered.event)).toEqual([]);

        for (let i = 0; i < 2; i++) {
          const result = await signIn(rp, call, a);
          expect(result.verified).toBe(true);
          expect(result.event).toMatchObject({ kind: "signin", counter: 0, userHandleMatches: true });
          expect(failed(result.event)).toEqual([]);
        }

        const state = await call("/state");
        expect(state.rpId).toBe("localhost");
        expect(state.userVerification).toBe("required");
        expect(state.credentials).toHaveLength(1);
        expect(state.events.map((e: RpEvent) => e.kind)).toEqual(["register", "signin", "signin"]);

        const logged = readFileSync(logFile, "utf8").trim().split("\n").map((l) => JSON.parse(l) as RpEvent);
        expect(logged.map((e) => e.seq)).toEqual([1, 2, 3]);
        expect(logged[0]!.credentialId).toBe(a.id);
      });
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  test("the registration options ask for what LocalPass design promises to answer", async () => {
    await withRp({}, async (_rp, call) => {
      const options = await call("/api/register/options", { username: "bob" });
      expect(options.rp.id).toBe("localhost");
      expect(options.attestation).toBe("none");
      expect(options.authenticatorSelection).toMatchObject({ residentKey: "required", userVerification: "required" });
      expect(options.pubKeyCredParams.map((p: { alg: number }) => p.alg)).toEqual([-7]);
      expect(options.user.name).toBe("bob");
      expect(options.excludeCredentials ?? []).toEqual([]);
    });
  });

  test("a second registration carries the first one in the exclude list (unless switched off)", async () => {
    await withRp({}, async (rp, call) => {
      const a = softwareAuthenticator();
      expect((await register(rp, call, a)).verified).toBe(true);
      const again = await call("/api/register/options", { username: "alice" });
      expect(again.excludeCredentials.map((c: { id: string }) => c.id)).toEqual([a.id]);
      const off = await call("/api/register/options", { username: "alice", exclude: false });
      expect(off.excludeCredentials ?? []).toEqual([]);
    });
  });

  test("the page's own report is recorded as a client event", async () => {
    await withRp({}, async (_rp, call) => {
      await call("/api/client", { action: "get", outcome: "error", errorName: "NotAllowedError", errorMessage: "cancelled", pageMs: 1234 });
      const state = await call("/state");
      expect(state.events).toHaveLength(1);
      expect(state.events[0]).toMatchObject({ kind: "client", action: "get", outcome: "error", errorName: "NotAllowedError", pageMs: 1234 });
    });
  });

  test("the default port is outside Windows' reserved ranges and not the Vite port", () => {
    expect(DEFAULT_ASSISTED_PORT).not.toBe(1420);
    expect(DEFAULT_ASSISTED_PORT >= 6885 && DEFAULT_ASSISTED_PORT <= 6984).toBe(false);
  });
});

describe("an authenticator that gets something wrong", () => {
  test("no user verification: the registration is refused", async () => {
    await withRp({}, async (rp, call) => {
      const result = await register(rp, call, softwareAuthenticator(), { flags: UP | BE | BS });
      expect(result.verified).toBe(false);
      const state = await call("/state");
      expect(state.credentials).toHaveLength(0);
      expect(state.events[0]).toMatchObject({ kind: "register", verified: false });
      expect(state.events[0].error).toContain("verif");
    });
  });

  test("no backup flags: the site accepts it but the check that LocalPass sets them fails", async () => {
    await withRp({}, async (rp, call) => {
      const result = await register(rp, call, softwareAuthenticator(), { flags: UP | UV });
      expect(result.verified).toBe(true);
      expect(failed(result.event)).toEqual(["backup flags BE=1 BS=1 (LocalPass design)"]);
    });
  });

  test("wrong origin in the client data: refused", async () => {
    await withRp({}, async (_rp, call) => {
      const result = await register({ origin: "http://localhost:1" }, call, softwareAuthenticator());
      expect(result.verified).toBe(false);
      expect((await call("/state")).events[0].error).toMatch(/origin/i);
    });
  });

  test("wrong RP id hash: refused", async () => {
    await withRp({}, async (rp, call) => {
      const result = await register(rp, call, softwareAuthenticator(), { rpId: "example.com" });
      expect(result.verified).toBe(false);
      expect((await call("/state")).events[0].error).toMatch(/RP ID/i);
    });
  });

  test("a registration counter other than 0 is flagged (and fine when the site does not insist)", async () => {
    await withRp({}, async (rp, call) => {
      expect(failed((await register(rp, call, softwareAuthenticator(), { counter: 1 })).event)).toEqual(["signature counter 0 at registration (LocalPass design)"]);
    });
    await withRp({ counterZero: false }, async (rp, call) => {
      expect(failed((await register(rp, call, softwareAuthenticator(), { counter: 1 })).event)).toEqual([]);
    });
  });

  test("sign-in: a missing user handle, no UV, an unknown credential, another key, a counter that moved", async () => {
    await withRp({}, async (rp, call) => {
      const a = softwareAuthenticator(b64(Buffer.from("alice-1")));
      expect((await register(rp, call, a)).verified).toBe(true);

      const noHandle = await signIn(rp, call, a, { handle: null });
      expect(noHandle.verified).toBe(true);
      expect(failed(noHandle.event)).toEqual(["user handle returned and equal to the registered user id"]);

      const wrongHandle = await signIn(rp, call, a, { handle: b64(Buffer.from("mallory-1")) });
      expect(wrongHandle.event!.userHandleMatches).toBe(false);

      expect((await signIn(rp, call, a, { flags: UP | BE | BS })).verified).toBe(false);

      expect((await signIn(rp, call, softwareAuthenticator())).verified).toBe(false);
      const events: RpEvent[] = (await call("/state")).events;
      expect(failed(events.at(-1))).toEqual(["the credential was registered with this site"]);

      const otherKey = generateKeyPairSync("ec", { namedCurve: "P-256" }).privateKey;
      expect((await signIn(rp, call, a, { key: otherKey })).verified).toBe(false);

      // Last: a verified counter above 0 is remembered, which would make a later sign-in fail for another reason.
      const moved = await signIn(rp, call, a, { counter: 5 });
      expect(failed(moved.event)).toEqual(["signature counter stays 0 (LocalPass design)"]);
    });
  });
});
