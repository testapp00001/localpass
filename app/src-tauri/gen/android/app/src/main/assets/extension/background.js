// src/shared/crypto.ts
var encoder = new TextEncoder;
function toBase64Url(bytes) {
  let binary = "";
  for (const b of bytes)
    binary += String.fromCharCode(b);
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
function fromBase64Url(text) {
  const padded = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(padded + "=".repeat((4 - padded.length % 4) % 4));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
}
function concat(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let offset = 0;
  for (const p of parts) {
    out.set(p, offset);
    offset += p.length;
  }
  return out;
}
async function hmacSha256(secret, message) {
  const key = await crypto.subtle.importKey("raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, message));
}
function proofInput(label, serverNonce, clientNonce, clientId) {
  return concat(encoder.encode(label), serverNonce, clientNonce, encoder.encode(clientId));
}
function constantTimeEqual(a, b) {
  if (a.length !== b.length)
    return false;
  let diff = 0;
  for (let i = 0;i < a.length; i++)
    diff |= (a[i] ?? 0) ^ (b[i] ?? 0);
  return diff === 0;
}
async function sha256Hex(text) {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
  return Array.from(digest, (b) => b.toString(16).padStart(2, "0")).join("");
}

// src/shared/protocol.ts
class BrokerError extends Error {
  code;
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

// src/background/storage.ts
var gates = new WeakMap;
async function local() {
  const area = chrome.storage.local;
  let gate = gates.get(area);
  if (!gate) {
    gate = area.setAccessLevel({ accessLevel: "TRUSTED_CONTEXTS" });
    gates.set(area, gate);
  }
  await gate;
  return area;
}

// src/background/broker.ts
var HOST = "app.localpass.host";
var PROTOCOL_VERSION = 1;
var CONNECT_TIMEOUT_MS = 40000;
var REQUEST_TIMEOUT_MS = 130000;
var MAX_REQUEST_BYTES = 1e6;
function browserName() {
  const ua = navigator.userAgent;
  if (ua.includes("Edg/"))
    return "Microsoft Edge extension";
  if (ua.includes("OPR/"))
    return "Opera extension";
  if (ua.includes("Brave"))
    return "Brave extension";
  return "Chrome extension";
}

class BrokerClient {
  state = { phase: "disconnected" };
  conn = null;
  opening = null;
  pending = new Map;
  nextId = 1;
  wantPair = false;
  waiters = [];
  onState;
  constructor(onState) {
    this.onState = onState;
  }
  setState(next) {
    this.state = next;
    this.onState(next);
    if (next.phase !== "connecting") {
      const waiting = this.waiters;
      this.waiters = [];
      for (const wake of waiting)
        wake();
    }
  }
  async loadCredentials() {
    const stored = await (await local()).get(["clientId", "secret"]);
    if (typeof stored.clientId === "string" && typeof stored.secret === "string") {
      return { clientId: stored.clientId, secret: stored.secret };
    }
    return null;
  }
  async connect(pair = false) {
    if (pair)
      this.wantPair = true;
    await (this.opening ??= this.open().finally(() => {
      this.opening = null;
    }));
    if (pair && this.conn && this.state.phase === "unpaired")
      this.sendPair(this.conn);
  }
  async open() {
    if (this.conn)
      return;
    this.setState({ phase: "connecting" });
    let creds;
    try {
      creds = await this.loadCredentials();
    } catch (e) {
      this.setState({ phase: "error", detail: `LocalPass could not protect this extension's storage, so it does not connect (${String(e)})` });
      return;
    }
    let port;
    try {
      port = chrome.runtime.connectNative(HOST);
    } catch (e) {
      this.setState({ phase: "error", detail: String(e) });
      return;
    }
    const conn = { port, creds, serverNonce: null, clientNonce: null, authSent: false, pairSent: false, trusted: false, queue: Promise.resolve() };
    this.conn = conn;
    port.onMessage.addListener((message) => {
      if (this.conn !== conn)
        return;
      if (!("type" in message)) {
        this.onResponse(conn, message);
        return;
      }
      conn.queue = conn.queue.then(() => this.onMessage(conn, message)).catch((e) => {
        console.warn("broker message failed", e);
        if (!conn.trusted)
          this.refuse(conn);
      });
    });
    port.onDisconnect.addListener(() => {
      const reason = chrome.runtime.lastError?.message;
      if (this.conn !== conn)
        return;
      this.dropConnection(conn);
      const phase = this.state.phase;
      if (phase === "ready" || phase === "connecting" || phase === "pairing" || phase === "unpaired") {
        const detail = reason ?? "disconnected";
        this.setState({ phase: /not found|forbidden/i.test(detail) ? "appNotRunning" : "disconnected", detail });
      }
    });
    this.send(conn, {
      type: "hello",
      v: PROTOCOL_VERSION,
      name: browserName(),
      kind: "extension",
      ...creds ? { clientId: creds.clientId } : {}
    });
  }
  send(conn, message) {
    try {
      conn.port.postMessage(message);
      return true;
    } catch {
      if (this.conn === conn) {
        this.dropConnection(conn);
        this.setState({ phase: "disconnected", detail: "disconnected" });
      }
      return false;
    }
  }
  sendPair(conn) {
    if (conn.pairSent || conn.trusted)
      return;
    conn.pairSent = true;
    this.send(conn, { type: "pair" });
  }
  dropConnection(conn) {
    if (this.conn === conn)
      this.conn = null;
    this.wantPair = false;
    try {
      conn.port.disconnect();
    } catch {}
    for (const [, p] of this.pending) {
      clearTimeout(p.timer);
      p.reject(new BrokerError("disconnected", "LocalPass closed the connection"));
    }
    this.pending.clear();
  }
  refuse(conn, detail = "The program on the other end could not prove it is LocalPass.") {
    if (this.conn !== conn)
      return;
    this.dropConnection(conn);
    this.setState({ phase: "untrusted", detail });
  }
  disconnect() {
    if (this.conn)
      this.dropConnection(this.conn);
    this.setState({ phase: "disconnected" });
  }
  async onMessage(conn, message) {
    if (this.conn !== conn)
      return;
    if (conn.trusted && message.type !== "hostError")
      return;
    switch (message.type) {
      case "challenge": {
        const creds = conn.creds;
        if (!creds || conn.authSent || conn.pairSent)
          return this.refuse(conn);
        const serverNonce = fromBase64Url(message.nonce);
        const clientNonce = crypto.getRandomValues(new Uint8Array(32));
        conn.serverNonce = serverNonce;
        conn.clientNonce = clientNonce;
        conn.authSent = true;
        const proof = await hmacSha256(fromBase64Url(creds.secret), proofInput("lp-broker/client/v1", serverNonce, clientNonce, creds.clientId));
        if (this.conn !== conn)
          return;
        this.send(conn, { type: "auth", clientNonce: toBase64Url(clientNonce), proof: toBase64Url(proof) });
        break;
      }
      case "ready": {
        const { creds, serverNonce, clientNonce } = conn;
        if (!creds || !serverNonce || !clientNonce || !conn.authSent)
          return this.refuse(conn);
        const expected = await hmacSha256(fromBase64Url(creds.secret), proofInput("lp-broker/server/v1", serverNonce, clientNonce, creds.clientId));
        if (this.conn !== conn)
          return;
        if (!constantTimeEqual(expected, fromBase64Url(message.serverProof)))
          return this.refuse(conn);
        conn.trusted = true;
        this.setState({ phase: "ready", vaultStatus: message.status });
        break;
      }
      case "unpaired":
        if (conn.authSent || conn.pairSent)
          return this.refuse(conn);
        this.setState({
          phase: "unpaired",
          forgotten: !!conn.creds,
          detail: conn.creds ? "LocalPass no longer knows this browser. Connect it again." : undefined
        });
        if (this.wantPair)
          this.sendPair(conn);
        break;
      case "pairing":
        if (!conn.pairSent)
          return this.refuse(conn);
        this.setState({ phase: "pairing", code: message.code });
        break;
      case "paired": {
        if (!conn.pairSent || typeof message.clientId !== "string" || typeof message.secret !== "string")
          return this.refuse(conn);
        await (await local()).set({ clientId: message.clientId, secret: message.secret });
        if (this.conn !== conn)
          return;
        conn.creds = { clientId: message.clientId, secret: message.secret };
        conn.trusted = true;
        this.wantPair = false;
        this.setState({ phase: "ready", vaultStatus: message.status });
        break;
      }
      case "denied":
        if (!conn.pairSent)
          return this.refuse(conn);
        this.wantPair = false;
        this.setState({ phase: "denied" });
        break;
      case "error":
        if (message.code === "auth")
          this.setState({ phase: "unpaired", detail: message.message, forgotten: !!conn.creds });
        else
          this.setState({ phase: "error", detail: message.message });
        break;
      case "hostError": {
        const phase = message.code === "appClosed" ? "disconnected" : message.code === "forbidden" ? "error" : "appNotRunning";
        this.dropConnection(conn);
        this.setState({ phase, detail: message.message });
        break;
      }
    }
  }
  onResponse(conn, message) {
    if (!conn.trusted)
      return;
    const pending = this.pending.get(message.id);
    if (!pending)
      return;
    this.pending.delete(message.id);
    clearTimeout(pending.timer);
    if (message.ok) {
      pending.resolve(message.result);
      return;
    }
    const code = message.error?.code ?? "internal";
    pending.reject(new BrokerError(code, message.error?.message ?? "request failed"));
    if (code === "unpaired")
      this.forget(conn);
  }
  async forget(conn) {
    if (this.conn === conn) {
      this.dropConnection(conn);
      this.setState({ phase: "unpaired", forgotten: true, detail: "LocalPass no longer knows this browser. Connect it again." });
    }
    try {
      await (await local()).remove(["clientId", "secret"]);
    } catch (e) {
      console.warn("could not forget the credentials", e);
    }
  }
  async whenReady() {
    if (this.state.phase !== "ready" || !this.conn)
      await this.connect();
    const deadline = Date.now() + CONNECT_TIMEOUT_MS;
    while (this.state.phase === "connecting" || this.state.phase === "pairing") {
      const left = deadline - Date.now();
      if (left <= 0)
        break;
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, left);
        this.waiters.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
    if (this.state.phase !== "ready") {
      throw new BrokerError(this.state.phase, this.state.detail ?? `not connected (${this.state.phase})`);
    }
  }
  async request(op, params = {}) {
    await this.whenReady();
    const conn = this.conn;
    if (!conn || !conn.trusted)
      throw new BrokerError("disconnected", "LocalPass is not connected");
    const id = this.nextId++;
    const frame = { ...params, id, op };
    if (new TextEncoder().encode(JSON.stringify(frame)).length > MAX_REQUEST_BYTES) {
      throw new BrokerError("tooLarge", "That request is too large for LocalPass");
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new BrokerError("timeout", "LocalPass did not answer in time"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
      if (!this.send(conn, frame)) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(new BrokerError("disconnected", "LocalPass closed the connection"));
      }
    });
  }
}

// src/background/passkeys.ts
var encoder2 = new TextEncoder;
function defaultRpId(origin) {
  return new URL(origin).hostname;
}
function clientData(type, challenge, origin) {
  return encoder2.encode(JSON.stringify({ type, challenge, origin, crossOrigin: false }));
}
async function sha256(bytes) {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
}
async function candidates(ask, origin, rpId, allow) {
  return ask("passkeyCandidates", { origin, rpId: rpId || defaultRpId(origin), allow });
}
async function create(ask, origin, input) {
  const rpId = input.rpId || defaultRpId(origin);
  const json = clientData("webauthn.create", input.challenge, origin);
  const created = await ask("passkeyCreate", {
    origin,
    rpId,
    rpName: input.rpName,
    userHandle: input.userHandle,
    userName: input.userName,
    userDisplayName: input.userDisplayName,
    algorithms: input.algorithms,
    exclude: input.exclude,
    uv: input.uv
  });
  return { ...created, clientDataJSON: toBase64Url(json) };
}
async function assert(ask, origin, itemId, rpIdOrEmpty, challenge, uv) {
  const rpId = rpIdOrEmpty || defaultRpId(origin);
  const json = clientData("webauthn.get", challenge, origin);
  const hash = await sha256(json);
  const signed = await ask("passkeyAssert", {
    origin,
    rpId,
    itemId,
    clientDataHash: toBase64Url(hash),
    uv
  });
  return { ...signed, clientDataJSON: toBase64Url(json) };
}

// src/background/index.ts
var broker = new BrokerClient((state) => {
  chrome.runtime.sendMessage({ kind: "brokerState", state }).catch(() => {});
});
var PENDING_TTL_MS = 2 * 60000;
var LOGIN_LIST_TTL_MS = 2 * 60000;
var LOGIN_LIST_KEY = "logins:all";
var LOGIN_LIST_LIMIT = 200;
function originOf(url) {
  if (!url)
    return null;
  try {
    const origin = new URL(url).origin;
    return origin === "null" ? null : origin;
  } catch {
    return null;
  }
}
function frameOrigin(sender) {
  if (sender.id !== chrome.runtime.id)
    throw new BrokerError("forbidden", "unknown sender");
  const origin = originOf(sender.origin ?? sender.url);
  if (!origin)
    throw new BrokerError("invalid", "this page has no usable address");
  if (sender.frameId && sender.frameId !== 0) {
    const top = originOf(sender.tab?.url);
    if (top !== origin)
      throw new BrokerError("forbidden", "login forms in other sites' frames are not filled");
  }
  return origin;
}
function fromContentScript(sender) {
  return !!sender.tab && sender.id === chrome.runtime.id && !sender.url?.startsWith("chrome-extension://");
}
function describe(e) {
  if (e instanceof BrokerError)
    return { code: e.code, message: e.message };
  return { code: "internal", message: e instanceof Error ? e.message : String(e) };
}
async function request(op, params = {}) {
  return broker.request(op, params);
}
async function sessionGet(key) {
  const stored = await chrome.storage.session.get(key);
  return stored[key];
}
async function neverSave() {
  const stored = await (await local()).get("neverSave");
  return Array.isArray(stored.neverSave) ? stored.neverSave : [];
}
async function findLogins(sender, interactive) {
  const origin = frameOrigin(sender);
  if (broker.state.phase !== "ready" && !interactive)
    return { status: "offline", logins: [] };
  try {
    const logins = await request("findLogins", { origin });
    return { status: "ok", logins };
  } catch (e) {
    const { code } = describe(e);
    if (["locked", "noVault", "unpaired", "appNotRunning", "denied", "untrusted", "disconnected", "error", "tooMany", "tooLarge"].includes(code)) {
      return { status: code, logins: [] };
    }
    throw e;
  }
}
async function rememberFill(tabId, fill) {
  await chrome.storage.session.set({ [`filled:${tabId}`]: { username: fill.username, hash: await sha256Hex(fill.password) } });
}
async function fill(sender, itemId) {
  const origin = frameOrigin(sender);
  const fill = await request("fillLogin", { itemId, origin, force: false });
  if (sender.tab?.id !== undefined)
    await rememberFill(sender.tab.id, fill);
  return fill;
}
var capturing = new Map;
function capture(sender, username, password) {
  if (!fromContentScript(sender) || sender.tab.id === undefined || !password)
    return Promise.resolve();
  const tabId = sender.tab.id;
  const work = storeCapture(sender, tabId, username, password).finally(() => {
    if (capturing.get(tabId) === work)
      capturing.delete(tabId);
  });
  capturing.set(tabId, work);
  return work;
}
async function storeCapture(sender, tabId, username, password) {
  const origin = frameOrigin(sender);
  if ((await neverSave()).includes(origin))
    return;
  const filled = await sessionGet(`filled:${tabId}`);
  if (filled && filled.username === username && filled.hash === await sha256Hex(password))
    return;
  const pending = { id: crypto.randomUUID(), origin, username, password, tabId, at: Date.now() };
  await chrome.storage.session.set({ [`pending:${tabId}`]: pending });
}
async function pendingFor(tabId) {
  await capturing.get(tabId)?.catch(() => {});
  const pending = await sessionGet(`pending:${tabId}`);
  if (!pending)
    return null;
  if (Date.now() - pending.at > PENDING_TTL_MS) {
    await chrome.storage.session.remove(`pending:${tabId}`);
    return null;
  }
  return pending;
}
async function pendingSave(sender) {
  if (!fromContentScript(sender) || sender.tab.id === undefined || sender.frameId !== 0)
    return null;
  const pending = await pendingFor(sender.tab.id);
  return pending ? { id: pending.id, origin: pending.origin, username: pending.username } : null;
}
async function savePending(sender, id) {
  if (!fromContentScript(sender) || sender.tab.id === undefined)
    throw new BrokerError("forbidden", "not from a page");
  const pending = await pendingFor(sender.tab.id);
  if (!pending || pending.id !== id)
    throw new BrokerError("notFound", "there is nothing to save any more");
  let outcome;
  try {
    outcome = await request("saveLogin", { origin: pending.origin, username: pending.username, password: pending.password });
  } catch (e) {
    if (!(e instanceof BrokerError && e.code === "denied"))
      throw e;
    await chrome.storage.session.remove(`pending:${sender.tab.id}`);
    return { id: pending.id, status: "declined" };
  }
  await chrome.storage.session.remove(`pending:${sender.tab.id}`);
  return outcome;
}
async function dismissPending(sender, id, never) {
  if (!fromContentScript(sender) || sender.tab.id === undefined)
    return;
  const pending = await pendingFor(sender.tab.id);
  if (!pending || pending.id !== id)
    return;
  if (never)
    await (await local()).set({ neverSave: [...new Set([...await neverSave(), pending.origin])] });
  await chrome.storage.session.remove(`pending:${sender.tab.id}`);
}
async function popupState() {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const origin = originOf(tab?.url);
  const info = {
    broker: broker.state,
    tab: tab?.id !== undefined ? { id: tab.id, origin, host: origin ? new URL(origin).hostname : null, title: tab.title ?? "" } : null,
    logins: [],
    vault: null
  };
  if (broker.state.phase !== "ready") {
    await broker.connect().catch(() => {});
    await Promise.race([broker.whenReady().catch(() => {}), new Promise((r) => setTimeout(r, 4000))]);
    info.broker = broker.state;
  }
  if (broker.state.phase === "ready") {
    try {
      const status = await request("status");
      info.vault = status.status;
      if (status.status === "unlocked" && origin)
        info.logins = await request("findLogins", { origin });
    } catch (e) {
      console.warn("popup state failed", e);
    }
  }
  return info;
}
async function fillTab(tabId, itemId, force) {
  const tab = await chrome.tabs.get(tabId);
  const origin = originOf(tab.url);
  if (!origin)
    throw new BrokerError("invalid", "this page can't be filled");
  const fill = await request("fillLogin", { itemId, origin, force });
  const now = await chrome.tabs.get(tabId).catch(() => null);
  if (!now || originOf(now.url) !== origin)
    throw new BrokerError("changed", "The page changed while LocalPass was asking. Nothing was filled.");
  await rememberFill(tabId, fill);
  const message = { kind: "doFill", fill, origin };
  await chrome.tabs.sendMessage(tabId, message, { frameId: 0 });
}
async function loadLogins() {
  const items = await request("searchItems", { query: "", limit: LOGIN_LIST_LIMIT });
  await chrome.storage.session.set({ [LOGIN_LIST_KEY]: { at: Date.now(), items } });
  return items;
}
async function cachedLogins() {
  const cached = await sessionGet(LOGIN_LIST_KEY);
  if (!cached)
    return null;
  if (Date.now() - cached.at > LOGIN_LIST_TTL_MS) {
    await chrome.storage.session.remove(LOGIN_LIST_KEY);
    return null;
  }
  return cached.items;
}
async function passkeysEnabled() {
  try {
    return (await (await local()).get("passkeysEnabled")).passkeysEnabled !== false;
  } catch {
    return true;
  }
}
async function privacy() {
  const setting = await chrome.privacy.services.passwordSavingEnabled.get({});
  return { disabled: setting.value === false, control: setting.levelOfControl };
}
async function setPrivacy(disable) {
  if (disable)
    await chrome.privacy.services.passwordSavingEnabled.set({ value: false });
  else
    await chrome.privacy.services.passwordSavingEnabled.clear({});
  return privacy();
}
async function handle(message, sender) {
  if (sender.id !== chrome.runtime.id)
    throw new BrokerError("forbidden", "unknown sender");
  const fromPage = fromContentScript(sender);
  const popupOnly = () => {
    if (fromPage)
      throw new BrokerError("forbidden", "not available to pages");
  };
  switch (message.kind) {
    case "findLogins":
      return findLogins(sender, message.interactive);
    case "fill":
      return fill(sender, message.itemId);
    case "capture":
      return capture(sender, message.username, message.password);
    case "pendingSave":
      return pendingSave(sender);
    case "savePending":
      return savePending(sender, message.id);
    case "dismissPending":
      return dismissPending(sender, message.id, message.never);
    case "passkeyCandidates":
      return candidates(request, frameOrigin(sender), message.rpId, message.allow);
    case "passkeyCreate":
      if (!fromPage)
        throw new BrokerError("forbidden", "passkeys are only created for pages");
      return create(request, frameOrigin(sender), message.request);
    case "passkeyAssert":
      if (!fromPage)
        throw new BrokerError("forbidden", "passkeys are only used for pages");
      return assert(request, frameOrigin(sender), message.itemId, message.rpId, message.challenge, message.uv);
    case "unlock":
      return request("unlock");
    case "state":
      popupOnly();
      return popupState();
    case "connect":
      popupOnly();
      await broker.connect(true);
      return broker.state;
    case "loadLogins":
      popupOnly();
      return loadLogins();
    case "cachedLogins":
      popupOnly();
      return cachedLogins();
    case "passkeysEnabled":
      return passkeysEnabled();
    case "fillTab":
      popupOnly();
      return fillTab(message.tabId, message.itemId, message.force);
    case "generate":
      popupOnly();
      return request("generatePassword");
    case "getPrivacy":
      popupOnly();
      return privacy();
    case "setPrivacy":
      popupOnly();
      return setPrivacy(message.disableBrowserManager);
  }
}
chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message.kind === "brokerState")
    return false;
  handle(message, sender).then((data) => sendResponse({ ok: true, data }), (e) => sendResponse({ ok: false, ...describe(e) }));
  return true;
});
chrome.runtime.onStartup.addListener(() => void broker.connect().catch(() => {}));
chrome.runtime.onInstalled.addListener(() => void broker.connect().catch(() => {}));
chrome.tabs.onRemoved.addListener((tabId) => void chrome.storage.session.remove([`pending:${tabId}`, `filled:${tabId}`]));
