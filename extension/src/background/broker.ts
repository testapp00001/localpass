// The connection to the LocalPass app: a native-messaging port to the helper, which relays to the
// app. Handles the handshake (pairing the first time, proving the secret afterwards, and checking
// the app proves it too) and then matches requests to responses.
//
// Everything the handshake needs lives in one `Connection` per port, so nothing from an earlier
// connection (nonces, "we asked to pair") can make a later one look trusted, and a service worker
// that was just restarted has exactly the same rules as one that has been running. A peer that
// does not follow the handshake is refused (the port is closed and nothing is sent to it).

import { BrokerError, type ResponseMessage, type ServerMessage, type VaultStatus } from "../shared/protocol";
import { constantTimeEqual, fromBase64Url, hmacSha256, proofInput, toBase64Url } from "../shared/crypto";
import { local } from "./storage";

const HOST = "app.localpass.host";
const PROTOCOL_VERSION = 1;
/** The app may need a few seconds to start when the helper launches it. */
const CONNECT_TIMEOUT_MS = 40_000;
const REQUEST_TIMEOUT_MS = 130_000;
/** The helper refuses frames over 1 MiB (and then exits): never send one. */
const MAX_REQUEST_BYTES = 1_000_000;

export type Phase =
  | "disconnected"
  | "connecting"
  | "unpaired" // the app doesn't know this browser yet (or any more)
  | "pairing" // waiting for the user to approve in the app
  | "ready"
  | "appNotRunning"
  | "denied"
  | "untrusted" // whatever answered could not prove it is the app
  | "error";

export interface BrokerState {
  phase: Phase;
  /** The confirmation code while pairing. */
  code?: string;
  vaultStatus?: VaultStatus;
  detail?: string;
  /** With `unpaired`: this browser was paired once, and the app no longer knows it. */
  forgotten?: boolean;
}

interface Credentials {
  clientId: string;
  secret: string;
}

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: ReturnType<typeof setTimeout>;
}

/** One port and what has happened on it so far. */
interface Connection {
  port: chrome.runtime.Port;
  /** The credentials this connection said hello with (none for an unknown browser). */
  creds: Credentials | null;
  serverNonce: Uint8Array | null;
  clientNonce: Uint8Array | null;
  authSent: boolean;
  pairSent: boolean;
  /** The app proved it knows the secret, or the user approved a new pairing. */
  trusted: boolean;
  /** Handshake messages are handled one at a time, in order. */
  queue: Promise<void>;
}

function browserName(): string {
  const ua = navigator.userAgent;
  if (ua.includes("Edg/")) return "Microsoft Edge extension";
  if (ua.includes("OPR/")) return "Opera extension";
  if (ua.includes("Brave")) return "Brave extension";
  return "Chrome extension";
}

export class BrokerClient {
  state: BrokerState = { phase: "disconnected" };
  private conn: Connection | null = null;
  private opening: Promise<void> | null = null;
  private pending = new Map<number, Pending>();
  private nextId = 1;
  private wantPair = false;
  private waiters: Array<() => void> = [];
  private onState: (state: BrokerState) => void;

  constructor(onState: (state: BrokerState) => void) {
    this.onState = onState;
  }

  private setState(next: BrokerState) {
    this.state = next;
    this.onState(next);
    // Wake anyone waiting for the connection to settle.
    if (next.phase !== "connecting") {
      const waiting = this.waiters;
      this.waiters = [];
      for (const wake of waiting) wake();
    }
  }

  private async loadCredentials(): Promise<Credentials | null> {
    const stored = await (await local()).get(["clientId", "secret"]);
    if (typeof stored.clientId === "string" && typeof stored.secret === "string") {
      return { clientId: stored.clientId, secret: stored.secret };
    }
    return null;
  }

  /** Opens the connection if it isn't open. `pair` asks to be paired if the app doesn't know us. */
  async connect(pair = false): Promise<void> {
    if (pair) this.wantPair = true;
    await (this.opening ??= this.open().finally(() => {
      this.opening = null;
    }));
    // Already connected: if we were waiting for permission to pair, ask now.
    if (pair && this.conn && this.state.phase === "unpaired") this.sendPair(this.conn);
  }

  private async open(): Promise<void> {
    if (this.conn) return;
    this.setState({ phase: "connecting" });
    let creds: Credentials | null;
    try {
      creds = await this.loadCredentials();
    } catch (e) {
      this.setState({ phase: "error", detail: `LocalPass could not protect this extension's storage, so it does not connect (${String(e)})` });
      return;
    }
    let port: chrome.runtime.Port;
    try {
      port = chrome.runtime.connectNative(HOST);
    } catch (e) {
      this.setState({ phase: "error", detail: String(e) });
      return;
    }
    const conn: Connection = { port, creds, serverNonce: null, clientNonce: null, authSent: false, pairSent: false, trusted: false, queue: Promise.resolve() };
    this.conn = conn;
    port.onMessage.addListener((message: ServerMessage | ResponseMessage) => {
      if (this.conn !== conn) return;
      if (!("type" in message)) {
        this.onResponse(conn, message);
        return;
      }
      conn.queue = conn.queue
        .then(() => this.onMessage(conn, message))
        .catch((e) => {
          console.warn("broker message failed", e);
          // Something went wrong while still checking who is there: do not trust it.
          if (!conn.trusted) this.refuse(conn);
        });
    });
    port.onDisconnect.addListener(() => {
      const reason = chrome.runtime.lastError?.message;
      if (this.conn !== conn) return;
      this.dropConnection(conn);
      const phase = this.state.phase;
      if (phase === "ready" || phase === "connecting" || phase === "pairing" || phase === "unpaired") {
        // "Specified native messaging host not found" means the browser connection isn't set up.
        const detail = reason ?? "disconnected";
        this.setState({ phase: /not found|forbidden/i.test(detail) ? "appNotRunning" : "disconnected", detail });
      }
    });
    this.send(conn, {
      type: "hello",
      v: PROTOCOL_VERSION,
      name: browserName(),
      kind: "extension",
      ...(creds ? { clientId: creds.clientId } : {}),
    });
  }

  private send(conn: Connection, message: unknown): boolean {
    try {
      conn.port.postMessage(message);
      return true;
    } catch {
      // The port is gone (Chrome throws on a disconnected port): let the next request reconnect.
      if (this.conn === conn) {
        this.dropConnection(conn);
        this.setState({ phase: "disconnected", detail: "disconnected" });
      }
      return false;
    }
  }

  private sendPair(conn: Connection) {
    if (conn.pairSent || conn.trusted) return;
    conn.pairSent = true;
    this.send(conn, { type: "pair" });
  }

  /** Closes the port and forgets it; requests still waiting fail at once. */
  private dropConnection(conn: Connection) {
    if (this.conn === conn) this.conn = null;
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

  /** Whatever answered did not follow the handshake or could not prove it is the app. */
  private refuse(conn: Connection, detail = "The program on the other end could not prove it is LocalPass.") {
    if (this.conn !== conn) return;
    this.dropConnection(conn);
    this.setState({ phase: "untrusted", detail });
  }

  disconnect() {
    if (this.conn) this.dropConnection(this.conn);
    this.setState({ phase: "disconnected" });
  }

  private async onMessage(conn: Connection, message: ServerMessage) {
    if (this.conn !== conn) return;
    // Once the peer is trusted only the helper's own notices (below) are expected; a handshake
    // message now is either a replay or an attack.
    if (conn.trusted && message.type !== "hostError") return;
    switch (message.type) {
      case "challenge": {
        const creds = conn.creds;
        if (!creds || conn.authSent || conn.pairSent) return this.refuse(conn);
        const serverNonce = fromBase64Url(message.nonce);
        const clientNonce = crypto.getRandomValues(new Uint8Array(32));
        conn.serverNonce = serverNonce;
        conn.clientNonce = clientNonce;
        conn.authSent = true;
        const proof = await hmacSha256(fromBase64Url(creds.secret), proofInput("lp-broker/client/v1", serverNonce, clientNonce, creds.clientId));
        if (this.conn !== conn) return;
        this.send(conn, { type: "auth", clientNonce: toBase64Url(clientNonce), proof: toBase64Url(proof) });
        break;
      }
      case "ready": {
        // Only an app that proves it knows the secret is trusted: this connection must have
        // exchanged nonces, and the proof must match. There is no other way to become ready.
        const { creds, serverNonce, clientNonce } = conn;
        if (!creds || !serverNonce || !clientNonce || !conn.authSent) return this.refuse(conn);
        const expected = await hmacSha256(fromBase64Url(creds.secret), proofInput("lp-broker/server/v1", serverNonce, clientNonce, creds.clientId));
        if (this.conn !== conn) return;
        if (!constantTimeEqual(expected, fromBase64Url(message.serverProof))) return this.refuse(conn);
        conn.trusted = true;
        this.setState({ phase: "ready", vaultStatus: message.status });
        break;
      }
      case "unpaired":
        // The app doesn't know this browser. Nothing here proves who is saying so, so the stored
        // credentials stay (an impostor must not be able to erase them); the user's Connect pairs
        // again and replaces them.
        if (conn.authSent || conn.pairSent) return this.refuse(conn);
        this.setState({
          phase: "unpaired",
          forgotten: !!conn.creds,
          detail: conn.creds ? "LocalPass no longer knows this browser. Connect it again." : undefined,
        });
        if (this.wantPair) this.sendPair(conn);
        break;
      case "pairing":
        if (!conn.pairSent) return this.refuse(conn);
        this.setState({ phase: "pairing", code: message.code });
        break;
      case "paired": {
        if (!conn.pairSent || typeof message.clientId !== "string" || typeof message.secret !== "string") return this.refuse(conn);
        await (await local()).set({ clientId: message.clientId, secret: message.secret });
        if (this.conn !== conn) return;
        conn.creds = { clientId: message.clientId, secret: message.secret };
        conn.trusted = true;
        this.wantPair = false;
        this.setState({ phase: "ready", vaultStatus: message.status });
        break;
      }
      case "denied":
        if (!conn.pairSent) return this.refuse(conn);
        this.wantPair = false;
        this.setState({ phase: "denied" });
        break;
      case "error":
        if (message.code === "auth") this.setState({ phase: "unpaired", detail: message.message, forgotten: !!conn.creds });
        else this.setState({ phase: "error", detail: message.message });
        break;
      case "hostError": {
        const phase = message.code === "appClosed" ? "disconnected" : message.code === "forbidden" ? "error" : "appNotRunning";
        // The helper ends after this: close our side too so the next request reconnects.
        this.dropConnection(conn);
        this.setState({ phase, detail: message.message });
        break;
      }
    }
  }

  private onResponse(conn: Connection, message: ResponseMessage) {
    // Requests are only sent on a trusted connection; answers from any other are noise.
    if (!conn.trusted) return;
    const pending = this.pending.get(message.id);
    if (!pending) return;
    this.pending.delete(message.id);
    clearTimeout(pending.timer);
    if (message.ok) {
      pending.resolve(message.result);
      return;
    }
    const code = message.error?.code ?? "internal";
    pending.reject(new BrokerError(code, message.error?.message ?? "request failed"));
    // The app (which proved itself) says this browser was removed: forget the secret and ask for a
    // new pairing instead of trying again with a key the app no longer accepts.
    if (code === "unpaired") void this.forget(conn);
  }

  private async forget(conn: Connection) {
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

  /** Waits until the connection is ready, or fails with a reason the UI can show. */
  async whenReady(): Promise<void> {
    if (this.state.phase !== "ready" || !this.conn) await this.connect();
    const deadline = Date.now() + CONNECT_TIMEOUT_MS;
    while (this.state.phase === "connecting" || this.state.phase === "pairing") {
      const left = deadline - Date.now();
      if (left <= 0) break;
      await new Promise<void>((resolve) => {
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

  async request<T = unknown>(op: string, params: Record<string, unknown> = {}): Promise<T> {
    await this.whenReady();
    const conn = this.conn;
    if (!conn || !conn.trusted) throw new BrokerError("disconnected", "LocalPass is not connected");
    const id = this.nextId++;
    // The envelope goes last: a parameter named `id` or `op` must not replace it.
    const frame = { ...params, id, op };
    if (new TextEncoder().encode(JSON.stringify(frame)).length > MAX_REQUEST_BYTES) {
      throw new BrokerError("tooLarge", "That request is too large for LocalPass");
    }
    return new Promise<T>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new BrokerError("timeout", "LocalPass did not answer in time"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject, timer });
      if (!this.send(conn, frame)) {
        this.pending.delete(id);
        clearTimeout(timer);
        reject(new BrokerError("disconnected", "LocalPass closed the connection"));
      }
    });
  }
}
