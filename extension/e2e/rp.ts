// A small relying party for the browser tests: a page with Register / Sign in buttons that use
// navigator.credentials, and a server that verifies everything with SimpleWebAuthn, an
// implementation independent of LocalPass. The site is reached as http://localhost:PORT (rpID
// "localhost").
//
// Two users:
//  * e2e/passkeys.ts (the extension in a test browser): `startRelyingParty()` with the defaults.
//  * e2e/windows-provider.ts (the Windows system passkey provider, assisted): `assisted: true`
//    and `userVerification: "required"` give a page with a user name field and an exclude-list
//    switch, require the UV flag, and check what a regular site would check (flags, counter,
//    user handle, public key algorithm). Every ceremony is recorded as an event, served at
//    /state and appended to a JSON-lines file.
//
// Standalone: `bun run e2e/rp.ts [--port N] [--log FILE]` starts the assisted variant and keeps
// running (Ctrl+C to stop).

import { appendFileSync } from "node:fs";
import {
  generateAuthenticationOptions,
  generateRegistrationOptions,
  verifyAuthenticationResponse,
  verifyRegistrationResponse,
  type WebAuthnCredential,
} from "@simplewebauthn/server";
import { cose, decodeAttestationObject, decodeCredentialPublicKey, isoBase64URL, parseAuthenticatorData } from "@simplewebauthn/server/helpers";

export const rpID = "localhost";

/** Not in Windows' Hyper-V reserved ranges (6885-6984 and others, `netsh int ipv4 show excludedportrange protocol=tcp`) and not the Vite port 1420. */
export const DEFAULT_ASSISTED_PORT = 8473;

export interface RelyingPartyOptions {
  /** TCP port on 127.0.0.1; 0 (the default) or a port that is taken means any free one. */
  port?: number;
  /** What the options ask for and what verification enforces. Default "preferred" (not enforced). */
  userVerification?: "preferred" | "required" | "discouraged";
  /** Appends one JSON object per event (ceremony result, page report) to this file. */
  logFile?: string;
  /** Page with a user name field, an exclude-list switch and a result panel; stricter checks are always recorded. */
  assisted?: boolean;
  /** The assisted page's default user name (the test passkey is easy to find and delete by it). */
  userName?: string;
  /** Also require the signature counter to be 0 (LocalPass passkeys never count). Off: only "not going backwards". */
  counterZero?: boolean;
  /** Do not print rejected ceremonies to the console (unit tests that reject on purpose). */
  quiet?: boolean;
}

export interface RpCheck {
  name: string;
  ok: boolean;
  detail?: string;
}

/** One line of the event log and of /state. */
export interface RpEvent {
  seq: number;
  time: string;
  kind: "register" | "signin" | "client";
  /** register / signin: SimpleWebAuthn accepted the response. */
  verified?: boolean;
  /** Milliseconds the server needed to verify. */
  verifyMs?: number;
  error?: string;
  userName?: string;
  credentialId?: string;
  aaguid?: string;
  fmt?: string;
  flags?: { up: boolean; uv: boolean; be: boolean; bs: boolean };
  counter?: number;
  userHandleMatches?: boolean | null;
  /** Everything an independent site would check, with the verdict of each. */
  checks?: RpCheck[];
  /** kind "client": what the page saw (navigator.credentials returned or threw). */
  action?: "create" | "get";
  outcome?: "ok" | "error";
  errorName?: string;
  errorMessage?: string;
  /** From the click to the answer, in the page. Includes the time the person needed. */
  pageMs?: number;
}

interface StoredCredential {
  id: string;
  userName: string;
  userID: Uint8Array<ArrayBuffer>;
  publicKey: Uint8Array<ArrayBuffer>;
  counter: number;
  transports?: WebAuthnCredential["transports"];
  aaguid: string;
  flags: { up: boolean; uv: boolean; be: boolean; bs: boolean };
}

const flagsOf = (f: { up: boolean; uv: boolean; be: boolean; bs: boolean }) => ({ up: f.up, uv: f.uv, be: f.be, bs: f.bs });

export function startRelyingParty(options: RelyingPartyOptions = {}) {
  const uvRequired = options.userVerification === "required";
  /** What the options ask for. "discouraged" lets an authenticator skip user verification (the real app then asks no Windows Hello when its setting is off). */
  const uvAsked = uvRequired ? "required" : options.userVerification === "discouraged" ? "discouraged" : "preferred";
  const credentials = new Map<string, StoredCredential>();
  const challenges = new Map<string, string>();
  const events: RpEvent[] = [];
  let origin = "";

  const record = (event: Omit<RpEvent, "seq" | "time">) => {
    const full: RpEvent = { seq: events.length + 1, time: new Date().toISOString(), ...event };
    events.push(full);
    if (options.logFile) {
      try {
        appendFileSync(options.logFile, JSON.stringify(full) + "\n");
      } catch {}
    }
    return full;
  };

  const assistedMarkup = options.assisted
    ? `<h1>LocalPass system passkey test</h1>
  <p><label>User name <input id="username" value="${(options.userName ?? "alice").replace(/[^\w.@-]/g, "")}" autocomplete="off"></label>
  <label><input type="checkbox" id="exclude" checked> send the exclude list (existing passkeys)</label></p>
  <button id="register">Register</button><button id="signin">Sign in</button>
  <pre id="result"></pre>`
    : `<input id="cond" autocomplete="username webauthn" placeholder="Username">
  <button id="register">Register</button><button id="signin">Sign in</button>
  <button id="register-key">Register security key</button><button id="wrong-rp">Wrong RP</button>`;

  const page = `<!doctype html><title>Passkey test</title>
  ${assistedMarkup}
  <div id="status">idle</div>
  <script type="module">
    const status = (text) => (document.getElementById('status').textContent = text);
    const post = async (url, body) => (await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: body ? JSON.stringify(body) : undefined })).json();
    const field = (id) => document.getElementById(id);
    const report = (action, started, error) => { void post('/api/client', { action, outcome: error ? 'error' : 'ok', errorName: error?.name, errorMessage: error?.message, pageMs: Math.round(performance.now() - started) }); };
    const show = (result) => { if (field('result')) field('result').textContent = JSON.stringify(result, null, 2); };
    async function register(attachment) {
      const started = performance.now();
      try {
        const options = await post('/api/register/options', { attachment, username: field('username')?.value, exclude: field('exclude') ? field('exclude').checked : undefined });
        const credential = await navigator.credentials.create({ publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options) });
        report('create', started);
        const result = await post('/api/register/verify', credential.toJSON());
        show(result);
        status(result.verified ? 'registered' + (attachment ? '-key' : '') : 'register-failed');
      } catch (e) { report('create', started, e); status('error:' + e.name + ':' + e.message); }
    }
    async function signIn(mediation) {
      const started = performance.now();
      try {
        const options = await post('/api/auth/options');
        const credential = await navigator.credentials.get({ publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options), mediation });
        report('get', started);
        const result = await post('/api/auth/verify', credential.toJSON());
        show(result);
        status(result.verified ? 'signed-in' + (mediation === 'conditional' ? '-conditional' : '') : 'signin-failed');
      } catch (e) { report('get', started, e); status('error:' + e.name + ':' + e.message); }
    }
    field('register').onclick = () => register();
    field('signin').onclick = () => signIn();
    if (field('register-key')) field('register-key').onclick = () => register('cross-platform');
    if (field('wrong-rp')) field('wrong-rp').onclick = async () => {
      try {
        const challenge = crypto.getRandomValues(new Uint8Array(32));
        await navigator.credentials.create({ publicKey: { challenge, rp: { id: 'example.com', name: 'Not this site' }, user: { id: new Uint8Array([1]), name: 'mallory', displayName: 'mallory' }, pubKeyCredParams: [{ type: 'public-key', alg: -7 }] } });
        status('created');
      } catch (e) { status('error:' + e.name); }
    };
    if (location.hash === '#conditional') signIn('conditional');
    if (location.hash === '#auto-register') register();
    if (location.hash === '#auto-signin') signIn();
  </script>`;

  const state = () => ({
    origin,
    rpId: rpID,
    userVerification: uvAsked,
    credentials: [...credentials.values()].map((c) => ({ id: c.id, userName: c.userName, counter: c.counter, aaguid: c.aaguid, flags: c.flags })),
    events,
  });

  const makeServer = (port: number) =>
    Bun.serve({
      hostname: "127.0.0.1",
      port,
      async fetch(request) {
        const url = new URL(request.url);
        if (request.method === "GET") {
          if (url.pathname === "/state") return Response.json(state());
          if (url.pathname === "/") return new Response(page, { headers: { "content-type": "text/html" } });
          return new Response("not found", { status: 404 });
        }
        const body = request.headers.get("content-type")?.includes("json") ? await request.json().catch(() => ({})) : {};
        const json = (value: unknown) => Response.json(value);
        switch (url.pathname) {
          case "/api/client": {
            record({ kind: "client", action: body.action, outcome: body.outcome, errorName: body.errorName, errorMessage: body.errorMessage, pageMs: body.pageMs });
            return json({ ok: true });
          }
          case "/api/register/options": {
            const userName: string = typeof body.username === "string" && body.username.trim() ? body.username.trim() : "alice";
            const options = await generateRegistrationOptions({
              rpName: "Passkey test",
              rpID,
              userName,
              userID: new TextEncoder().encode(`${userName}-1`),
              attestationType: "none",
              supportedAlgorithmIDs: body.attachment ? [-7, -257] : [-7],
              authenticatorSelection: { residentKey: "required", userVerification: uvAsked, ...(body.attachment ? { authenticatorAttachment: body.attachment } : {}) },
              excludeCredentials: body.exclude === false ? [] : [...credentials.values()].map((c) => ({ id: c.id })),
            });
            challenges.set("register", options.challenge);
            challenges.set("register-user", userName);
            return json(options);
          }
          case "/api/register/verify": {
            const started = performance.now();
            const checks: RpCheck[] = [];
            const check = (name: string, ok: boolean, detail?: string) => checks.push({ name, ok, ...(detail ? { detail } : {}) });
            try {
              const result = await verifyRegistrationResponse({ response: body, expectedChallenge: challenges.get("register")!, expectedOrigin: origin, expectedRPID: rpID, requireUserVerification: uvRequired });
              check("origin, RP id hash, challenge and type verified", result.verified);
              if (result.verified && result.registrationInfo) {
                const info = result.registrationInfo;
                const c = info.credential;
                const authData = parseAuthenticatorData(decodeAttestationObject(info.attestationObject).get("authData"));
                const flags = flagsOf(authData.flags);
                const alg = decodeCredentialPublicKey(c.publicKey).get(cose.COSEKEYS.alg);
                const userName = challenges.get("register-user") ?? "alice";
                credentials.set(c.id, { id: c.id, userName, userID: new TextEncoder().encode(`${userName}-1`), publicKey: c.publicKey, counter: c.counter, transports: c.transports, aaguid: info.aaguid, flags });
                check("user presence flag (UP)", flags.up);
                check("user verification flag (UV)", flags.uv, uvRequired ? "required by this site" : "not required by this site");
                check("backup flags BE=1 BS=1 (LocalPass design)", flags.be && flags.bs, `BE=${+flags.be} BS=${+flags.bs}`);
                check("attestation format none", info.fmt === "none", info.fmt);
                check("public key is ES256 (-7)", alg === -7, String(alg));
                if (options.counterZero) check("signature counter 0 at registration (LocalPass design)", authData.counter === 0, String(authData.counter));
                const event = record({ kind: "register", verified: true, verifyMs: Math.round(performance.now() - started), userName, credentialId: c.id, aaguid: info.aaguid, fmt: info.fmt, flags, counter: authData.counter, checks });
                return json({ verified: true, event });
              }
              record({ kind: "register", verified: false, verifyMs: Math.round(performance.now() - started), checks });
              return json({ verified: false });
            } catch (e) {
              if (!options.quiet) console.error("registration rejected:", e);
              check("origin, RP id hash, challenge and type verified", false, String(e instanceof Error ? e.message : e));
              record({ kind: "register", verified: false, verifyMs: Math.round(performance.now() - started), error: String(e instanceof Error ? e.message : e), checks });
              return json({ verified: false });
            }
          }
          case "/api/auth/options": {
            const options = await generateAuthenticationOptions({ rpID, userVerification: uvAsked });
            challenges.set("auth", options.challenge);
            return json(options);
          }
          case "/api/auth/verify": {
            const started = performance.now();
            const checks: RpCheck[] = [];
            const check = (name: string, ok: boolean, detail?: string) => checks.push({ name, ok, ...(detail ? { detail } : {}) });
            const stored = credentials.get(body.id);
            check("the credential was registered with this site", !!stored, stored ? undefined : `unknown credential id ${String(body.id).slice(0, 12)}...`);
            if (!stored) {
              record({ kind: "signin", verified: false, credentialId: body.id, error: "unknown credential", checks });
              return json({ verified: false });
            }
            try {
              const result = await verifyAuthenticationResponse({
                response: body,
                expectedChallenge: challenges.get("auth")!,
                expectedOrigin: origin,
                expectedRPID: rpID,
                credential: { id: stored.id, publicKey: stored.publicKey, counter: stored.counter, transports: stored.transports },
                requireUserVerification: uvRequired,
              });
              check("signature, origin, RP id hash and challenge verified", result.verified);
              const flags = flagsOf(parseAuthenticatorData(isoBase64URL.toBuffer(body.response.authenticatorData)).flags);
              const newCounter = result.authenticationInfo.newCounter;
              const handle: string | undefined = body.response.userHandle;
              const matches = handle ? handle === isoBase64URL.fromBuffer(stored.userID) : null;
              check("user presence flag (UP)", flags.up);
              check("user verification flag (UV)", flags.uv, uvRequired ? "required by this site" : "not required by this site");
              check("backup flags BE=1 BS=1 (LocalPass design)", flags.be && flags.bs, `BE=${+flags.be} BS=${+flags.bs}`);
              check(options.counterZero ? "signature counter stays 0 (LocalPass design)" : "signature counter not going backwards", options.counterZero ? newCounter === 0 : newCounter === 0 ? stored.counter === 0 : newCounter > stored.counter, `stored ${stored.counter}, now ${newCounter}`);
              check("user handle returned and equal to the registered user id", matches === true, matches === null ? "no userHandle in the response" : String(matches));
              if (result.verified) stored.counter = newCounter;
              const event = record({ kind: "signin", verified: result.verified, verifyMs: Math.round(performance.now() - started), userName: stored.userName, credentialId: stored.id, flags, counter: newCounter, userHandleMatches: matches, checks });
              return json({ verified: result.verified, event });
            } catch (e) {
              if (!options.quiet) console.error("sign-in rejected:", e);
              check("signature, origin, RP id hash and challenge verified", false, String(e instanceof Error ? e.message : e));
              record({ kind: "signin", verified: false, verifyMs: Math.round(performance.now() - started), credentialId: stored.id, error: String(e instanceof Error ? e.message : e), checks });
              return json({ verified: false });
            }
          }
        }
        return new Response("not found", { status: 404 });
      },
    });

  let server: ReturnType<typeof makeServer>;
  try {
    server = makeServer(options.port ?? 0);
  } catch {
    server = makeServer(0); // the wanted port is taken
  }
  origin = `http://localhost:${server.port}`;
  return {
    origin,
    port: server.port,
    verifiedCount: () => credentials.size,
    state,
    events: () => events,
    stop: () => server.stop(true),
  };
}

if (import.meta.main) {
  const arg = (name: string) => {
    const i = process.argv.indexOf(name);
    return i >= 0 ? process.argv[i + 1] : undefined;
  };
  const rp = startRelyingParty({ port: Number(arg("--port") ?? DEFAULT_ASSISTED_PORT), logFile: arg("--log"), userVerification: "required", assisted: true });
  console.log(`relying party for rpID "${rpID}" at ${rp.origin}  (state: ${rp.origin}/state)`);
  process.on("SIGINT", () => {
    rp.stop();
    process.exit(0);
  });
}
