(() => {
  // src/content/inject.ts
  (() => {
    const w = window;
    if (w.__localpassPasskeys || !navigator.credentials || !window.PublicKeyCredential)
      return;
    w.__localpassPasskeys = true;
    const nativeCreate = CredentialsContainer.prototype.create;
    const nativeGet = CredentialsContainer.prototype.get;
    const toB64u = (source) => {
      const bytes = source instanceof ArrayBuffer ? new Uint8Array(source) : new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
      let binary = "";
      for (const b of bytes)
        binary += String.fromCharCode(b);
      return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
    };
    const fromB64u = (text) => {
      const padded = text.replace(/-/g, "+").replace(/_/g, "/");
      const binary = atob(padded + "=".repeat((4 - padded.length % 4) % 4));
      const bytes = new Uint8Array(binary.length);
      for (let i = 0;i < binary.length; i++)
        bytes[i] = binary.charCodeAt(i);
      return bytes.buffer;
    };
    const waiting = new Map;
    window.addEventListener("message", (event) => {
      if (event.source !== window)
        return;
      const data = event.data;
      if (!data || data.__lp !== "res" || typeof data.id !== "string")
        return;
      const resolve = waiting.get(data.id);
      if (resolve) {
        waiting.delete(data.id);
        resolve(data);
      }
    });
    function call(op, payload, signal) {
      const id = crypto.randomUUID();
      const reply = new Promise((resolve, reject) => {
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
    function sameSiteAsTop() {
      try {
        return window.top === window || window.top.location.origin === window.location.origin;
      } catch {
        return false;
      }
    }
    function rpIdFits(rpId) {
      if (!rpId)
        return true;
      const host = window.location.hostname;
      return host === rpId || host.endsWith(`.${rpId}`);
    }
    const buffer = (b64) => fromB64u(b64);
    function define(target, props) {
      for (const [key, value] of Object.entries(props)) {
        Object.defineProperty(target, key, { value, enumerable: true, configurable: true, writable: typeof value !== "function" });
      }
    }
    function credentialObject(proto, id, response, json) {
      const credential = Object.create(proto);
      define(credential, {
        id,
        rawId: buffer(id),
        type: "public-key",
        authenticatorAttachment: "platform",
        response,
        getClientExtensionResults: () => ({}),
        toJSON: json
      });
      return credential;
    }
    function registrationCredential(r, credProps) {
      const response = Object.create(AuthenticatorAttestationResponse.prototype);
      define(response, {
        clientDataJSON: buffer(r.clientDataJSON),
        attestationObject: buffer(r.attestationObject),
        getTransports: () => ["internal", "hybrid"],
        getAuthenticatorData: () => buffer(r.authData),
        getPublicKey: () => buffer(r.publicKeySpki),
        getPublicKeyAlgorithm: () => r.algorithm
      });
      const id = r.credentialId;
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
          authenticatorData: r.authData
        }
      }));
      if (credProps)
        define(credential, { getClientExtensionResults: () => ({ credProps: { rk: true } }) });
      return credential;
    }
    function assertionCredential(r) {
      const response = Object.create(AuthenticatorAssertionResponse.prototype);
      define(response, {
        clientDataJSON: buffer(r.clientDataJSON),
        authenticatorData: buffer(r.authData),
        signature: buffer(r.signature),
        userHandle: buffer(r.userHandle)
      });
      const id = r.credentialId;
      return credentialObject(PublicKeyCredential.prototype, id, response, () => ({
        id,
        rawId: id,
        type: "public-key",
        authenticatorAttachment: "platform",
        clientExtensionResults: {},
        response: { clientDataJSON: r.clientDataJSON, authenticatorData: r.authData, signature: r.signature, userHandle: r.userHandle }
      }));
    }
    function reject(reply) {
      throw new DOMException(reply.message ?? "The request was not allowed.", reply.name ?? "NotAllowedError");
    }
    function wantsCreate(pk) {
      if (!sameSiteAsTop() || !rpIdFits(pk.rp?.id))
        return false;
      if (pk.authenticatorSelection?.authenticatorAttachment === "cross-platform")
        return false;
      const algs = (pk.pubKeyCredParams ?? []).map((p) => p.alg);
      return algs.length === 0 || algs.includes(-7);
    }
    async function create(options) {
      const pk = options?.publicKey;
      if (!pk || options?.signal?.aborted || !wantsCreate(pk))
        return nativeCreate.call(this, options);
      let reply;
      try {
        const { reply: pending } = call("create", {
          rpId: pk.rp?.id ?? "",
          rpName: pk.rp?.name ?? "",
          userHandle: toB64u(pk.user.id),
          userName: pk.user.name ?? "",
          userDisplayName: pk.user.displayName ?? "",
          challenge: toB64u(pk.challenge),
          algorithms: (pk.pubKeyCredParams ?? []).map((p) => p.alg),
          exclude: (pk.excludeCredentials ?? []).map((c) => toB64u(c.id)),
          uv: pk.authenticatorSelection?.userVerification ?? "preferred"
        }, options?.signal);
        reply = await pending;
      } catch (e) {
        if (e instanceof DOMException && e.name === "AbortError")
          throw e;
        return nativeCreate.call(this, options);
      }
      if (reply.fallback)
        return nativeCreate.call(this, options);
      if (!reply.ok)
        reject(reply);
      return registrationCredential(reply.result, !!pk.extensions?.credProps);
    }
    function wantsGet(options) {
      const pk = options.publicKey;
      if (!sameSiteAsTop() || !rpIdFits(pk.rpId))
        return false;
      return options.mediation !== "silent";
    }
    async function get(options) {
      const pk = options?.publicKey;
      if (!pk || options?.signal?.aborted || !wantsGet(options))
        return nativeGet.call(this, options);
      const conditional = options.mediation === "conditional";
      const payload = {
        rpId: pk.rpId ?? "",
        allow: (pk.allowCredentials ?? []).map((c) => toB64u(c.id)),
        challenge: toB64u(pk.challenge),
        uv: pk.userVerification ?? "preferred",
        conditional
      };
      if (conditional) {
        const nativeAbort = new AbortController;
        options.signal?.addEventListener("abort", () => nativeAbort.abort());
        const native = nativeGet.call(this, { ...options, signal: nativeAbort.signal });
        const { id, reply } = call("get", payload, options.signal);
        const ours = reply.then((r) => {
          if (r.fallback)
            return new Promise(() => {});
          if (!r.ok)
            reject(r);
          nativeAbort.abort();
          return assertionCredential(r.result);
        });
        try {
          return await Promise.race([ours, native]);
        } finally {
          window.postMessage({ __lp: "req", id: crypto.randomUUID(), op: "cancel", payload: { target: id } }, "*");
          waiting.delete(id);
        }
      }
      let reply;
      try {
        reply = await call("get", payload, options.signal).reply;
      } catch (e) {
        if (e instanceof DOMException && e.name === "AbortError")
          throw e;
        return nativeGet.call(this, options);
      }
      if (reply.fallback)
        return nativeGet.call(this, options);
      if (!reply.ok)
        reject(reply);
      return assertionCredential(reply.result);
    }
    Object.defineProperty(CredentialsContainer.prototype, "create", { value: create, configurable: true, writable: true, enumerable: true });
    Object.defineProperty(CredentialsContainer.prototype, "get", { value: get, configurable: true, writable: true, enumerable: true });
    const constructor = PublicKeyCredential;
    constructor.isUserVerifyingPlatformAuthenticatorAvailable = () => Promise.resolve(true);
    constructor.isConditionalMediationAvailable = () => Promise.resolve(true);
    const nativeCapabilities = constructor.getClientCapabilities;
    if (nativeCapabilities) {
      constructor.getClientCapabilities = async () => ({
        ...await nativeCapabilities.call(PublicKeyCredential),
        conditionalGet: true,
        passkeyPlatformAuthenticator: true,
        userVerifyingPlatformAuthenticator: true
      });
    }
  })();
})();
