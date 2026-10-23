// Runs inside the web page (the "main world") before the page's own scripts. It replaces
// navigator.credentials.create/get so passkey requests can be answered by LocalPass. It does
// nothing itself: it turns each call into a message to the extension's content script (see
// passkeys.ts) and builds the browser objects the page expects from the reply. Whatever LocalPass
// can't or shouldn't handle goes to the browser's own implementation.

import { MAX_CHALLENGE_BYTES, MAX_CREDENTIAL_ID_BYTES, MAX_LIST, MAX_RP_ID_CHARS, MAX_TEXT_CHARS, MAX_USER_HANDLE_BYTES, rpIdFits as rpIdFitsHost } from "../shared/limits";

(() => {
  const w = window as unknown as Record<string, unknown>;
  if (w.__localpassPasskeys || !navigator.credentials || !window.PublicKeyCredential) return;
  w.__localpassPasskeys = true;

  const nativeCreate = CredentialsContainer.prototype.create;
  const nativeGet = CredentialsContainer.prototype.get;

  // ---- bytes ----

  const toB64u = (source: BufferSource): string => {
    const bytes = source instanceof ArrayBuffer ? new Uint8Array(source) : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  };
  const fromB64u = (text: string): ArrayBuffer => {
    const padded = text.replace(/-/g, "+").replace(/_/g, "/");
    const binary = atob(padded + "=".repeat((4 - (padded.length % 4)) % 4));
    const bytes = new Uint8Array(binary.length);
    for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
    return bytes.buffer;
  };

  // ---- talking to the extension ----

  interface Reply {
    ok?: boolean;
    fallback?: boolean;
    result?: Record<string, unknown>;
    name?: string;
    message?: string;
  }
  const waiting = new Map<string, (reply: Reply) => void>();
  window.addEventListener("message", (event) => {
    if (event.source !== window) return;
    const data = event.data as (Reply & { __lp?: string; id?: string }) | undefined;
    if (!data || data.__lp !== "res" || typeof data.id !== "string") return;
    const resolve = waiting.get(data.id);
    if (resolve) {
      waiting.delete(data.id);
      resolve(data);
    }
  });

  function call(op: "create" | "get", payload: object, signal?: AbortSignal | null): { id: string; reply: Promise<Reply> } {
    const id = crypto.randomUUID();
    const reply = new Promise<Reply>((resolve, reject) => {
      waiting.set(id, resolve);
      signal?.addEventListener("abort", () => {
        waiting.delete(id);
        window.postMessage({ __lp: "req", id: crypto.randomUUID(), op: "cancel", payload: { target: id } }, "*");
        reject(new DOMException("The operation was aborted.", "AbortError"));
      });
      window.postMessage({ __lp: "req", id, op, payload }, "*");
    });
    return { id, reply };
  }

  // ---- where the call is allowed ----

  /** Passkeys are only offered in the top page or a frame of the same site. */
  function sameSiteAsTop(): boolean {
    try {
      return window.top === window || window.top!.location.origin === window.location.origin;
    } catch {
      return false;
    }
  }

  function rpIdFits(rpId: string | undefined): boolean {
    return rpIdFitsHost(rpId ?? "", window.location.hostname);
  }

  // ---- limits (the same numbers as the app; over any of them the browser handles the call) ----

  const shortText = (value: unknown, max = MAX_TEXT_CHARS) => value === undefined || value === null || (typeof value === "string" && value.length <= max);
  const bytesWithin = (value: unknown, max: number) => !!value && typeof (value as ArrayBuffer).byteLength === "number" && (value as ArrayBuffer).byteLength <= max;
  const idList = (list: PublicKeyCredentialDescriptor[] | undefined) =>
    list === undefined || (Array.isArray(list) && list.length <= MAX_LIST && list.every((c) => !!c && bytesWithin(c.id, MAX_CREDENTIAL_ID_BYTES)));

  // ---- browser objects built from the reply ----

  const buffer = (b64: string) => fromB64u(b64);

  function define(target: object, props: Record<string, unknown>) {
    for (const [key, value] of Object.entries(props)) {
      Object.defineProperty(target, key, { value, enumerable: true, configurable: true, writable: typeof value !== "function" });
    }
  }

  function credentialObject(proto: object, id: string, response: object, json: () => object): PublicKeyCredential {
    const credential = Object.create(proto) as PublicKeyCredential;
    define(credential, {
      id,
      rawId: buffer(id),
      type: "public-key",
      authenticatorAttachment: "platform",
      response,
      getClientExtensionResults: () => ({}),
      toJSON: json,
    });
    return credential;
  }

  function registrationCredential(r: Record<string, string | number>, credProps: boolean): PublicKeyCredential {
    const response = Object.create(AuthenticatorAttestationResponse.prototype);
    define(response, {
      clientDataJSON: buffer(r.clientDataJSON as string),
      attestationObject: buffer(r.attestationObject as string),
      getTransports: () => ["internal", "hybrid"],
      getAuthenticatorData: () => buffer(r.authData as string),
      getPublicKey: () => buffer(r.publicKeySpki as string),
      getPublicKeyAlgorithm: () => r.algorithm as number,
    });
    const id = r.credentialId as string;
    const credential = credentialObject(PublicKeyCredential.prototype, id, response, () => ({
      id,
      rawId: id,
      type: "public-key",
      authenticatorAttachment: "platform",
      clientExtensionResults: credProps ? { credProps: { rk: true } } : {},
      response: {
        clientDataJSON: r.clientDataJSON,
        attestationObject: r.attestationObject,
        transports: ["internal", "hybrid"],
        publicKeyAlgorithm: r.algorithm,
        publicKey: r.publicKeySpki,
        authenticatorData: r.authData,
      },
    }));
    if (credProps) define(credential, { getClientExtensionResults: () => ({ credProps: { rk: true } }) });
    return credential;
  }

  function assertionCredential(r: Record<string, string>): PublicKeyCredential {
    const response = Object.create(AuthenticatorAssertionResponse.prototype);
    define(response, {
      clientDataJSON: buffer(r.clientDataJSON!),
      authenticatorData: buffer(r.authData!),
      signature: buffer(r.signature!),
      userHandle: buffer(r.userHandle!),
    });
    const id = r.credentialId!;
    return credentialObject(PublicKeyCredential.prototype, id, response, () => ({
      id,
      rawId: id,
      type: "public-key",
      authenticatorAttachment: "platform",
      clientExtensionResults: {},
      response: { clientDataJSON: r.clientDataJSON, authenticatorData: r.authData, signature: r.signature, userHandle: r.userHandle },
    }));
  }

  function reject(reply: Reply): never {
    throw new DOMException(reply.message ?? "The request was not allowed.", reply.name ?? "NotAllowedError");
  }

  // ---- create ----

  function createWithinLimits(pk: PublicKeyCredentialCreationOptions): boolean {
    return (
      !!pk.user &&
      shortText(pk.rp?.id, MAX_RP_ID_CHARS) &&
      shortText(pk.rp?.name) &&
      shortText(pk.user.name) &&
      shortText(pk.user.displayName) &&
      bytesWithin(pk.user.id, MAX_USER_HANDLE_BYTES) &&
      bytesWithin(pk.challenge, MAX_CHALLENGE_BYTES) &&
      (pk.pubKeyCredParams === undefined || (Array.isArray(pk.pubKeyCredParams) && pk.pubKeyCredParams.length <= MAX_LIST)) &&
      idList(pk.excludeCredentials)
    );
  }

  function wantsCreate(pk: PublicKeyCredentialCreationOptions): boolean {
    if (!sameSiteAsTop() || !createWithinLimits(pk) || !rpIdFits(pk.rp?.id)) return false;
    if (pk.authenticatorSelection?.authenticatorAttachment === "cross-platform") return false;
    const algs = (pk.pubKeyCredParams ?? []).map((p) => p.alg);
    // An empty list means "anything"; otherwise ES256 (-7) has to be acceptable.
    return algs.length === 0 || algs.includes(-7);
  }

  async function create(this: CredentialsContainer, options?: CredentialCreationOptions): Promise<Credential | null> {
    const pk = options?.publicKey;
    if (!pk || options?.signal?.aborted || !wantsCreate(pk)) return nativeCreate.call(this, options);
    let reply: Reply;
    try {
      const { reply: pending } = call(
        "create",
        {
          rpId: pk.rp?.id ?? "",
          rpName: pk.rp?.name ?? "",
          userHandle: toB64u(pk.user.id),
          userName: pk.user.name ?? "",
          userDisplayName: pk.user.displayName ?? "",
          challenge: toB64u(pk.challenge),
          algorithms: (pk.pubKeyCredParams ?? []).map((p) => p.alg),
          exclude: (pk.excludeCredentials ?? []).map((c) => toB64u(c.id)),
          uv: pk.authenticatorSelection?.userVerification ?? "preferred",
        },
        options?.signal,
      );
      reply = await pending;
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      return nativeCreate.call(this, options);
    }
    if (reply.fallback) return nativeCreate.call(this, options);
    if (!reply.ok) reject(reply);
    return registrationCredential(reply.result as Record<string, string | number>, !!(pk.extensions as { credProps?: boolean } | undefined)?.credProps);
  }

  // ---- get ----

  function getWithinLimits(pk: PublicKeyCredentialRequestOptions): boolean {
    return shortText(pk.rpId, MAX_RP_ID_CHARS) && bytesWithin(pk.challenge, MAX_CHALLENGE_BYTES) && idList(pk.allowCredentials);
  }

  function wantsGet(options: CredentialRequestOptions): boolean {
    const pk = options.publicKey!;
    if (!sameSiteAsTop() || !getWithinLimits(pk) || !rpIdFits(pk.rpId)) return false;
    return options.mediation !== "silent";
  }

  async function get(this: CredentialsContainer, options?: CredentialRequestOptions): Promise<Credential | null> {
    const pk = options?.publicKey;
    if (!pk || options?.signal?.aborted || !wantsGet(options)) return nativeGet.call(this, options);
    const conditional = options.mediation === "conditional";
    const payload = {
      rpId: pk.rpId ?? "",
      allow: (pk.allowCredentials ?? []).map((c) => toB64u(c.id)),
      challenge: toB64u(pk.challenge),
      uv: pk.userVerification ?? "preferred",
      conditional,
    };

    if (conditional) {
      // Keep the browser's own passkey suggestions working: both run, the first answer wins.
      const nativeAbort = new AbortController();
      options.signal?.addEventListener("abort", () => nativeAbort.abort());
      const native = nativeGet.call(this, { ...options, signal: nativeAbort.signal }) as Promise<Credential | null>;
      const { id, reply } = call("get", payload, options.signal);
      const ours = reply.then((r) => {
        if (r.fallback) return new Promise<Credential | null>(() => {}); // the browser's suggestions keep waiting
        if (!r.ok) reject(r);
        nativeAbort.abort();
        return assertionCredential(r.result as Record<string, string>);
      });
      try {
        return await Promise.race([ours, native]);
      } finally {
        // Whoever answered, stop waiting for the other.
        window.postMessage({ __lp: "req", id: crypto.randomUUID(), op: "cancel", payload: { target: id } }, "*");
        waiting.delete(id);
      }
    }

    let reply: Reply;
    try {
      reply = await call("get", payload, options.signal).reply;
    } catch (e) {
      if (e instanceof DOMException && e.name === "AbortError") throw e;
      return nativeGet.call(this, options);
    }
    if (reply.fallback) return nativeGet.call(this, options);
    if (!reply.ok) reject(reply);
    return assertionCredential(reply.result as Record<string, string>);
  }

  // ---- install ----

  Object.defineProperty(CredentialsContainer.prototype, "create", { value: create, configurable: true, writable: true, enumerable: true });
  Object.defineProperty(CredentialsContainer.prototype, "get", { value: get, configurable: true, writable: true, enumerable: true });

  // Tell sites that passkeys are available here.
  const constructor = PublicKeyCredential as unknown as Record<string, unknown> & {
    getClientCapabilities?: () => Promise<Record<string, boolean>>;
  };
  constructor.isUserVerifyingPlatformAuthenticatorAvailable = () => Promise.resolve(true);
  constructor.isConditionalMediationAvailable = () => Promise.resolve(true);
  const nativeCapabilities = constructor.getClientCapabilities;
  if (nativeCapabilities) {
    constructor.getClientCapabilities = async () => ({
      ...(await nativeCapabilities.call(PublicKeyCredential)),
      conditionalGet: true,
      passkeyPlatformAuthenticator: true,
      userVerifyingPlatformAuthenticator: true,
    });
  }
})();
