// A fake LocalPass app on the other end of a fake native-messaging port: it does the handshake
// like the real broker (independent HMAC code, node:crypto) and answers requests.

import { createHmac, randomBytes } from "node:crypto";
import { toBase64Url } from "../src/shared/crypto";
import type { FakeChrome, FakePort, Json } from "./chrome";

export const SECRET = randomBytes(32);
export const CLIENT_ID = "client-1";

/** What the app computes for the handshake. */
export function proof(label: string, serverNonce: Buffer, clientNonce: Buffer, secret: Buffer = SECRET, id = CLIENT_ID): string {
  const input = Buffer.concat([Buffer.from(label), serverNonce, clientNonce, Buffer.from(id)]);
  return toBase64Url(new Uint8Array(createHmac("sha256", secret).update(input).digest()));
}

export function seedCredentials(fake: FakeChrome) {
  fake.local.data.clientId = CLIENT_ID;
  fake.local.data.secret = toBase64Url(new Uint8Array(SECRET));
}

/** An error answer, as the app sends it. */
export class AppError {
  constructor(
    public code: string,
    public message = code,
  ) {}
}

export interface AppOptions {
  /** The answer to a request: a result, an `AppError`, or a promise of either. */
  answer?: (request: Json) => unknown | Promise<unknown>;
  /** Called with the server proof of each completed handshake. */
  onProof?: (serverProof: string) => void;
}

/** The real app: challenge, check the client's proof, answer with its own, then serve requests. */
export function honestApp(options: AppOptions = {}) {
  return (port: FakePort) => {
    const serverNonce = randomBytes(32);
    port.onSend = (message) => {
      if (message.type === "hello") {
        if (message.clientId === CLIENT_ID) port.emit({ type: "challenge", nonce: toBase64Url(new Uint8Array(serverNonce)) });
        else port.emit({ type: "unpaired" });
      } else if (message.type === "auth") {
        const clientNonce = Buffer.from(message.clientNonce as string, "base64url");
        if (message.proof !== proof("lp-broker/client/v1", serverNonce, clientNonce)) {
          port.emit({ type: "error", code: "auth", message: "bad proof" });
          return;
        }
        const serverProof = proof("lp-broker/server/v1", serverNonce, clientNonce);
        options.onProof?.(serverProof);
        port.emit({ type: "ready", status: "unlocked", serverProof, appVersion: "0.1.1" });
      } else if (message.type === "pair") {
        port.emit({ type: "pairing", code: "123456" });
        port.emit({ type: "paired", clientId: "new-client", secret: toBase64Url(new Uint8Array(randomBytes(32))), status: "unlocked" });
      } else if ("op" in message) {
        void (async () => {
          const result = options.answer ? await options.answer(message) : { op: message.op };
          if (result instanceof AppError) port.emit({ id: message.id, ok: false, error: { code: result.code, message: result.message } });
          else port.emit({ id: message.id, ok: true, result });
        })();
      }
    };
  };
}
